import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { WorkspaceAI } from '../src/runtime.ts';
import { request, rpc } from './helpers.ts';

test('actual Pi compatible adapter reaches a keyless HTTP endpoint without Authorization', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'buckets-real-adapter-'));
  const observed: { path?: string; authorization?: string; model?: string }[] = [];
  const server = createServer(async (req, res) => {
    const data: Buffer[] = []; for await (const part of req) data.push(part);
    const body = JSON.parse(Buffer.concat(data).toString());
    observed.push({ path: req.url, authorization: req.headers.authorization, model: body.model });
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    const answer = JSON.stringify({ answer: 'Actual compatible adapter response', citations: [], results: [] });
    res.write(`data: ${JSON.stringify({ id: 'fixture-chat', object: 'chat.completion.chunk', created: 1, model: 'fixture', choices: [{ index: 0, delta: { role: 'assistant', content: answer }, finish_reason: null }] })}\n\n`);
    res.write(`data: ${JSON.stringify({ id: 'fixture-chat', object: 'chat.completion.chunk', created: 1, model: 'fixture', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] })}\n\n`);
    res.end('data: [DONE]\n\n');
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening'); const address = server.address(); assert.ok(address && typeof address !== 'string');
  let runtime: WorkspaceAI | undefined;
  try {
    runtime = await WorkspaceAI.open(directory);
    const result = await runtime.invoke(request(directory), { provider: 'compatible', model: 'fixture', base_url: `http://127.0.0.1:${address.port}/v1`, api_key_env: '' }, rpc(async () => { throw new Error('No host tool expected'); }));
    assert.equal(result.answer, 'Actual compatible adapter response');
    assert.deepEqual(observed, [{ path: '/v1/chat/completions', model: 'fixture', authorization: undefined }]);
  } finally { await runtime?.close(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); await rm(directory, { recursive: true, force: true }); }
});
