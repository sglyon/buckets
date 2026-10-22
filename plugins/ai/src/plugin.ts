#!/usr/bin/env node
import { servePlugin, RpcError, type AgentRequest, type Invocation } from '@buckets/plugin-sdk';
import { WorkspaceAI } from './runtime.ts';
import { resolve, isAbsolute } from 'node:path';

const commands = [
  { id: 'ai.ask', title: 'Ask these panes', description: 'Ask a read-only agent about files in the current workspace' },
  { id: 'ai.search', title: 'Semantic search these panes', description: 'Search file prefixes with embeddings and keyword fusion' },
  { id: 'ai.memory', title: 'Workspace memory', description: 'Read or explicitly edit durable workspace notes' },
  { id: 'ai.reindex', title: 'Reindex semantic search', description: 'Rebuild bounded embedding indexes using configured roots and ignore policy' },
];
const runtimes = new Map<string, Promise<WorkspaceAI>>();
let active: { id: string; runtime: WorkspaceAI } | undefined;
let shuttingDown = false;
async function close(): Promise<void> {
  if (shuttingDown) return;
  shuttingDown = true;
  for (const runtime of runtimes.values()) await (await runtime).close();
}
function invocation(params: unknown): Invocation {
  const value = params as Invocation;
  if (!value || value.protocol_version !== 1 || !value.config || typeof value.config !== 'object' || Array.isArray(value.config)) throw new RpcError(-32602, 'Invalid plugin invocation');
  return value;
}
process.once('SIGTERM', () => { void close().finally(() => process.exit(0)); });
process.once('SIGINT', () => { void close().finally(() => process.exit(0)); });
await servePlugin({
  onClose: close,
  handlers: {
    'commands.list': () => commands,
    'commands.invoke': params => {
      const request = invocation(params).request as { command?: string };
      const modes: Record<string, string> = { 'ai.ask': 'ask', 'ai.search': 'search', 'ai.memory': 'memory', 'ai.reindex': 'index' };
      const mode = modes[request.command ?? ''];
      if (!mode) throw new RpcError(-32601, 'Unknown AI command');
      return { host_action: 'agent', request: { mode } };
    },
    'agent.invoke': async (params, context) => {
      const value = invocation(params); const request = value.request as AgentRequest;
      if (!request || typeof request.request_id !== 'string' || !request.request_id || request.request_id.length > 256 || !['ask', 'search', 'memory', 'index'].includes(request.mode) || typeof request.prompt !== 'string' || request.prompt.length > 32768 || !request.context || typeof request.context.workspace_id !== 'string' || !request.context.workspace_id || request.context.workspace_id.length > 256 || !Number.isSafeInteger(request.context.revision) || request.context.revision < 0 || !Array.isArray(request.context.panes) || request.context.panes.length > 64 || typeof request.storage_dir !== 'string' || !isAbsolute(request.storage_dir)) throw new RpcError(-32602, 'Invalid agent request');
      const key = resolve(request.storage_dir);
      let runtime = runtimes.get(key);
      if (!runtime) { runtime = WorkspaceAI.open(key); runtimes.set(key, runtime); runtime.catch(() => runtimes.delete(key)); }
      const instance = await runtime;
      active = { id: request.request_id, runtime: instance };
      try { return await instance.invoke(request, value.config, context); }
      finally { active = undefined; }
    },
    'agent.cancel': async params => {
      const requestId = (params as { request_id?: string })?.request_id;
      if (requestId && active?.id === requestId) await active.runtime.cancel(requestId);
    },
  },
});
