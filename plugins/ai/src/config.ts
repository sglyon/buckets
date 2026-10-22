export interface IndexRoot { location: string; respect_gitignore: boolean }
export interface AIConfig {
  provider: 'openai' | 'anthropic' | 'compatible';
  model: string;
  base_url: string;
  api_key_env: string;
  embedding_endpoint: string;
  embedding_model: string;
  embedding_api_key_env: string;
  index_roots: IndexRoot[];
  max_files: number;
  max_file_bytes: number;
  search_limit: number;
  search_plugin: string;
}
function string(value: unknown, fallback = ''): string {
  if (value === undefined) return fallback;
  if (typeof value !== 'string' || value.length > 8192 || /[\x00-\x1f]/.test(value)) throw new Error('AI configuration text is invalid');
  return value.trim();
}
function integer(value: unknown, fallback: number, min: number, max: number): number {
  if (value === undefined) return fallback;
  if (!Number.isInteger(value) || (value as number) < min || (value as number) > max) throw new Error(`AI configuration number must be ${min}..${max}`);
  return value as number;
}
export function readConfig(raw: Record<string, unknown>): AIConfig {
  for (const forbidden of ['api_key', 'embedding_api_key', 'token']) {
    if (raw[forbidden] !== undefined) throw new Error('Keep API keys in environment variables; configure an environment variable name instead');
  }
  const provider = string(raw.provider, 'openai');
  if (!['openai', 'anthropic', 'compatible'].includes(provider)) throw new Error('AI provider must be openai, anthropic or compatible');
  const apiKeyEnv = string(raw.api_key_env, provider === 'anthropic' ? 'ANTHROPIC_API_KEY' : 'OPENAI_API_KEY');
  const embeddingKeyEnv = string(raw.embedding_api_key_env);
  for (const name of [apiKeyEnv, embeddingKeyEnv]) if (name && !/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) throw new Error('API key environment variable name is invalid');
  const roots = raw.index_roots ?? [];
  if (!Array.isArray(roots) || roots.length > 32) throw new Error('index_roots must be an array of at most 32 location policies');
  return {
    provider: provider as AIConfig['provider'], model: string(raw.model), base_url: string(raw.base_url), api_key_env: apiKeyEnv,
    embedding_endpoint: string(raw.embedding_endpoint), embedding_model: string(raw.embedding_model), embedding_api_key_env: embeddingKeyEnv,
    index_roots: roots.map(root => {
      if (!root || typeof root !== 'object' || typeof root.location !== 'string' || typeof root.respect_gitignore !== 'boolean') throw new Error('Each index root needs location and respect_gitignore');
      return { location: string(root.location), respect_gitignore: root.respect_gitignore };
    }),
    max_files: integer(raw.max_files, 128, 1, 1000), max_file_bytes: integer(raw.max_file_bytes, 16384, 1024, 65536),
    search_limit: integer(raw.search_limit, 30, 1, 100), search_plugin: string(raw.search_plugin),
  };
}
export function endpoint(value: string, label: string): URL {
  let url: URL;
  try { url = new URL(value); } catch { throw new Error(`Configure ${label} with a valid HTTP(S) URL`); }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash || url.search) throw new Error(`${label} must be HTTP(S) without credentials, query parameters or fragments`);
  return url;
}
