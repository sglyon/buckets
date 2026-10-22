/** A bounded, provider-aware prefix index. Rust owns traversal, ignore policy and reads. */
import { createHash } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
import type { AgentFile, AgentRequest, FileRead, FileSearch, RequestContext } from '@buckets/plugin-sdk';
import { endpoint, type AIConfig } from './config.ts';

export interface Chunk { line: number; text: string; vector: number[] }
interface Indexed { location: string; name: string; chunks: Chunk[] }
export interface IndexReceipt { files: number; reused: number; skipped: number; truncated: boolean; note: string }
const hash = (text: string): string => createHash('sha256').update(text).digest('hex');
export function chunks(text: string): { line: number; text: string }[] {
  const result: { line: number; text: string }[] = [];
  for (let offset = 0; offset < text.length; offset += 1400) {
    const value = text.slice(offset, offset + 1600);
    if (value.trim()) result.push({ line: 1 + text.slice(0, offset).split('\n').length - 1, text: value });
    if (offset + 1600 >= text.length) break;
  }
  return result;
}
export function normalize(vector: number[]): number[] {
  const scale = vector.reduce((maximum, value) => Math.max(maximum, Math.abs(value)), 0);
  if (!scale || !Number.isFinite(scale)) throw new Error('Embedding vector must contain finite, nonzero values');
  const scaled = vector.map(value => value / scale);
  const norm = Math.hypot(...scaled);
  return scaled.map(value => value / norm);
}
export function cosine(a: number[], b: number[]): number {
  if (a.length !== b.length || !a.length) throw new Error('Embedding dimensions changed; reindex with one embedding model');
  const aa = normalize(a), bb = normalize(b);
  return Math.max(-1, Math.min(1, aa.reduce((sum, value, index) => sum + value * bb[index]!, 0)));
}
export class RetrievalIndex {
  private db: DatabaseSync;
  constructor(file: string) {
    this.db = new DatabaseSync(file);
    this.db.exec('PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS documents(scope TEXT NOT NULL,location TEXT NOT NULL,name TEXT NOT NULL,hash TEXT NOT NULL,chunks TEXT NOT NULL,PRIMARY KEY(scope,location));');
  }
  close(): void { this.db.close(); }
  private scope(request: AgentRequest, config: AIConfig): { id: string; roots: { location: string; respect_gitignore: boolean }[] } {
    const roots = config.index_roots.length ? config.index_roots : request.context.panes.map(p => ({ location: p.location, respect_gitignore: true }));
    const unique = [...new Map(roots.map(r => [r.location, r])).values()].sort((a, b) => a.location.localeCompare(b.location));
    if (!unique.length) throw new Error('Open a directory pane or configure index_roots before indexing');
    return { roots: unique, id: hash(JSON.stringify({ roots: unique, model: config.embedding_model, endpoint: config.embedding_endpoint, bytes: config.max_file_bytes })) };
  }
  async embed(input: string[], config: AIConfig, signal: AbortSignal): Promise<number[][]> {
    if (!config.embedding_endpoint || !config.embedding_model) throw new Error('Configure embedding_endpoint and embedding_model in the AI plugin settings to use semantic search');
    const url = endpoint(config.embedding_endpoint, 'embedding_endpoint');
    const key = config.embedding_api_key_env ? process.env[config.embedding_api_key_env] : undefined;
    if (config.embedding_api_key_env && !key) throw new Error(`Set ${config.embedding_api_key_env} in the Buckets process environment for embeddings`);
    let response: Response;
    try {
      response = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json', ...(key ? { Authorization: `Bearer ${key}` } : {}) },
        body: JSON.stringify({ model: config.embedding_model, input }), signal: AbortSignal.any([signal, AbortSignal.timeout(30000)]) });
    } catch (error) {
      if (signal.aborted) throw new Error('Semantic indexing cancelled');
      throw new Error('Embedding request failed; check the configured endpoint and its availability');
    }
    if (!response.ok) { await response.body?.cancel(); throw new Error(`Embedding endpoint returned HTTP ${response.status}`); }
    const reader = response.body?.getReader();
    if (!reader) throw new Error('Embedding endpoint returned no response body');
    const data: Uint8Array[] = []; let size = 0;
    while (true) { const chunk = await reader.read(); if (chunk.done) break; size += chunk.value.length; if (size > 8 * 1024 * 1024) { await reader.cancel(); throw new Error('Embedding response exceeds 8 MiB'); } data.push(chunk.value); }
    let value: { data?: { index: number; embedding: number[] }[] };
    try { value = JSON.parse(Buffer.concat(data).toString('utf8')); } catch { throw new Error('Embedding endpoint returned invalid JSON'); }
    if (!Array.isArray(value.data) || value.data.length !== input.length) throw new Error('Embedding endpoint returned the wrong vector count');
    const vectors = [...value.data].sort((a, b) => a.index - b.index);
    const dimensions = vectors[0]?.embedding?.length;
    if (!dimensions || dimensions > 16384 || vectors.some((v, i) => v.index !== i || !Array.isArray(v.embedding) || v.embedding.length !== dimensions || !v.embedding.every(n => typeof n === 'number' && Number.isFinite(n)) || !v.embedding.some(n => n !== 0))) throw new Error('Embedding endpoint returned invalid vectors');
    return vectors.map(v => normalize(v.embedding));
  }
  async index(request: AgentRequest, config: AIConfig, context: RequestContext, force = false): Promise<IndexReceipt> {
    if (!config.embedding_endpoint || !config.embedding_model) throw new Error('Configure embedding_endpoint and embedding_model in the AI plugin settings to use semantic search');
    const { id: scope, roots } = this.scope(request, config);
    const found = new Map<string, AgentFile>(); let truncated = false;
    for (const root of roots) {
      const response = await context.host.call<FileSearch>('files.search', { location: root.location, query: '', engine: 'builtin', files_only: true, limit: Math.min(1000, config.max_files + 1), respect_gitignore: root.respect_gitignore });
      truncated ||= response.truncated || response.results.length >= Math.min(1000, config.max_files + 1);
      for (const file of response.results) if (!file.is_dir) found.set(file.location, file);
    }
    const selected = [...found.values()].sort((a, b) => a.location.localeCompare(b.location)).slice(0, config.max_files);
    truncated ||= found.size > selected.length;
    const seen: string[] = []; let reused = 0, skipped = 0, prefix = false;
    for (const file of selected) {
      context.signal.throwIfAborted();
      let read: FileRead;
      try { read = await context.host.call<FileRead>('files.read', { location: file.location, max_bytes: config.max_file_bytes }); }
      catch { skipped++; continue; }
      if (read.binary || !read.text.trim()) { skipped++; continue; }
      prefix ||= read.truncated;
      const digest = hash(read.text);
      const previous = this.db.prepare('SELECT hash FROM documents WHERE scope=? AND location=?').get(scope, file.location) as { hash: string } | undefined;
      seen.push(file.location);
      if (!force && previous?.hash === digest) { reused++; continue; }
      const parts = chunks(read.text); const vectors: number[][] = [];
      for (let i = 0; i < parts.length; i += 16) vectors.push(...await this.embed(parts.slice(i, i + 16).map(p => p.text), config, context.signal));
      const saved: Chunk[] = parts.map((p, i) => ({ ...p, vector: vectors[i]! }));
      this.db.prepare('INSERT OR REPLACE INTO documents(scope,location,name,hash,chunks) VALUES(?,?,?,?,?)').run(scope, file.location, read.name ?? file.name, digest, JSON.stringify(saved));
      context.notify('agent.progress', { request_id: request.request_id, message: `Indexed ${seen.length}/${selected.length} files` });
    }
    // The completed enumeration is authoritative for this scope. Old ignored,
    // removed or unreadable entries cannot leak into the next result set.
    const current = new Set(seen);
    for (const row of this.db.prepare('SELECT location FROM documents WHERE scope=?').all(scope) as { location: string }[]) {
      if (!current.has(row.location)) this.db.prepare('DELETE FROM documents WHERE scope=? AND location=?').run(scope, row.location);
    }
    const note = `Indexed ${seen.length} text files (${reused} unchanged, ${skipped} skipped); ${config.max_file_bytes}-byte prefixes${prefix ? ' (some files truncated)' : ''}${truncated ? '; traversal capped, refine index_roots or raise max_files' : ''}. Ignore rules follow each root policy.`;
    return { files: seen.length, reused, skipped, truncated, note };
  }
  async search(request: AgentRequest, config: AIConfig, context: RequestContext): Promise<{ files: AgentFile[]; receipt: IndexReceipt }> {
    const receipt = await this.index(request, config, context);
    const { id: scope } = this.scope(request, config);
    const [query] = await this.embed([request.prompt], config, context.signal);
    const docs = (this.db.prepare('SELECT location,name,chunks FROM documents WHERE scope=?').all(scope) as { location: string; name: string; chunks: string }[]).map(row => ({ ...row, chunks: JSON.parse(row.chunks) as Chunk[] }));
    const terms = request.prompt.toLocaleLowerCase().match(/[\p{L}\p{N}_-]+/gu) ?? [];
    const ranked = docs.map(doc => {
      const best = doc.chunks.map(chunk => ({ chunk, value: cosine(query!, chunk.vector) })).sort((a, b) => b.value - a.value)[0];
      const corpus = (doc.name + '\n' + doc.chunks.map(c => c.text).join('\n')).toLocaleLowerCase();
      const keyword = terms.reduce((score, term) => score + (corpus.includes(term) ? 1 : 0), 0);
      return { doc, best, keyword };
    }).filter(row => row.best);
    const semantic = [...ranked].sort((a, b) => b.best!.value - a.best!.value || a.doc.location.localeCompare(b.doc.location));
    const lexical = ranked.filter(row => row.keyword > 0).sort((a, b) => b.keyword - a.keyword || a.doc.location.localeCompare(b.doc.location));
    const vectorRanks = new Map(semantic.map((row, i) => [row.doc.location, i + 1]));
    const keywordRanks = new Map(lexical.map((row, i) => [row.doc.location, i + 1]));
    const files = ranked.map(row => {
      const semanticRank = vectorRanks.get(row.doc.location)!; const keywordRank = keywordRanks.get(row.doc.location);
      return { location: row.doc.location, name: row.doc.name, is_dir: false, line: row.best!.chunk.line,
        snippet: row.best!.chunk.text.slice(0, 500), reason: `Embedding similarity ${row.best!.value.toFixed(3)}${keywordRank ? ` · keyword rank ${keywordRank}` : ''} · indexed prefix`,
        score: 1 / (60 + semanticRank) + (keywordRank ? 1 / (60 + keywordRank) : 0) };
    }).sort((a, b) => b.score - a.score || a.location.localeCompare(b.location)).slice(0, config.search_limit);
    return { files, receipt };
  }
}
