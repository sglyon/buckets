import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, sign } from 'node:crypto';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { RequestContext } from '@buckets/plugin-sdk';
import { ChatGPTAccounts, type AccountOptions } from '../src/accounts.ts';
import { chatGPTFailure, chatGPTPayload } from '../src/chatgpt-provider.ts';
import { WorkspaceAI } from '../src/runtime.ts';
import { readConfig } from '../src/config.ts';
import { request } from './helpers.ts';

interface FixtureSettings { deny?: boolean; wrongState?: boolean; badNonce?: boolean; badSignature?: boolean; wrongAudience?: boolean; subject?: string; scopes?: string; tokenFailure?: string; refreshFailure?: string; omitRefreshToken?: boolean; revocationFailure?: boolean; callbackClient?: string; responseFailure?: string; httpStatus?: number; incomplete?: boolean; eof?: boolean; toolFirst?: boolean; beforeResponse?: () => Promise<void> }
async function fixture(run: (f: { manager: ChatGPTAccounts; rpc: RequestContext; options: AccountOptions; state: () => any; settings: FixtureSettings; urls: URL[]; bodies: Record<string, unknown>[]; advances: (ms: number) => void; calls: string[]; leaseCount: () => number }) => Promise<void>) {
  const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const key = { ...publicKey.export({ format: 'jwk' }), kid: 'fixture-key', alg: 'RS256', use: 'sig' };
  let value: any; let lease: string | undefined; let clock = Date.now(); let latestNonce = ''; let issued = 0; let issuer = '';
  const settings: FixtureSettings = {}; const urls: URL[] = []; const bodies: Record<string, unknown>[] = []; const calls: string[] = [];
  const server = createServer(async (req, res) => {
    const chunks: Buffer[] = []; for await (const part of req) chunks.push(part); const raw = Buffer.concat(chunks).toString();
    calls.push(req.url!);
    const json = (data: unknown, status = 200) => { res.writeHead(status, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(data)); };
    if (req.url === '/.well-known/openid-configuration') { json({ issuer, jwks_uri: `${issuer}/jwks`, revocation_endpoint: `${issuer}/revoke` }); return; }
    if (req.url === '/jwks') { json({ keys: [key] }); return; }
    if (req.url === '/revoke') { json({}, settings.revocationFailure ? 503 : 200); return; }
    if (req.url === '/api/accounts/oauth/token') {
      const form = new URLSearchParams(raw); const refresh = form.get('grant_type') === 'refresh_token';
      if (refresh ? settings.refreshFailure : settings.tokenFailure) { json({ error: refresh ? settings.refreshFailure : settings.tokenFailure, description: 'SECRET-reflected-body' }, 400); return; }
      assert.equal(form.get('resource'), 'https://api.openai.com/v1'); assert.notEqual(form.get('client_id'), 'dynamic_agent_client');
      const payload = { iss: issuer, aud: settings.wrongAudience ? 'wrong-client' : form.get('client_id'), exp: Math.floor(clock / 1000) + 3600, iat: Math.floor(clock / 1000), nonce: settings.badNonce ? 'wrong-nonce' : latestNonce, sub: settings.subject ?? 'subject-1', email: 'same@example.test' };
      const header = Buffer.from(JSON.stringify({ alg: 'RS256', kid: 'fixture-key' })).toString('base64url');
      const claims = Buffer.from(JSON.stringify(payload)).toString('base64url');
      const signature = sign('sha256', Buffer.from(`${header}.${claims}`), privateKey).toString('base64url');
      json({ access_token: refresh ? 'fixture-refreshed-access' : 'fixture-access', ...(!settings.omitRefreshToken ? { refresh_token: refresh ? 'fixture-rotated-refresh' : 'fixture-refresh' } : {}), id_token: `${header}.${claims}.${settings.badSignature ? 'invalid-signature' : signature}`, token_type: 'Bearer', scope: settings.scopes ?? 'openid profile email offline_access resource.invoke chatgpt.tokens.use.direct', expires_in: 3600 }); return;
    }
    if (req.url === '/v1/models') { assert.match(req.headers.authorization ?? '', /^Bearer fixture-/); json({ models: [{ slug: 'hidden', display_name: 'Hidden', visibility: 'hide' }, { slug: 'fixture-model', display_name: 'Fixture model', visibility: 'list' }] }); return; }
    if (req.url === '/v1/responses') {
      assert.match(req.headers.authorization ?? '', /^Bearer fixture-/); const body = JSON.parse(raw); bodies.push(body);
      await settings.beforeResponse?.();
      if (settings.httpStatus) { json({ error: { message: 'SECRET-reflected-body' } }, settings.httpStatus); return; }
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      const event = (value: unknown) => res.write(`data: ${JSON.stringify(value)}\n\n`);
      event({ type: 'response.created', response: { id: 'fixture-response', status: 'in_progress', output: [] } });
      if (settings.responseFailure) { event({ type: 'response.failed', response: { status: 'failed', error: { code: settings.responseFailure, message: 'SECRET-reflected-body' } } }); res.end(); return; }
      if (settings.toolFirst && !(body.input as any[]).some(item => item.type === 'function_call_output')) {
        event({ type: 'response.output_item.added', output_index: 0, item: { type: 'function_call', id: 'fc_fixture', call_id: 'call_fixture', name: 'files_read', namespace: 'buckets', arguments: '' } });
        event({ type: 'response.function_call_arguments.delta', output_index: 0, item_id: 'fc_fixture', delta: '{"location":"/fixture/a.txt"}' });
        event({ type: 'response.function_call_arguments.done', output_index: 0, item_id: 'fc_fixture', arguments: '{"location":"/fixture/a.txt"}' });
        event({ type: 'response.output_item.done', output_index: 0, item: { type: 'function_call', id: 'fc_fixture', call_id: 'call_fixture', name: 'files_read', namespace: 'buckets', arguments: '{"location":"/fixture/a.txt"}' } });
        event({ type: 'response.completed', response: { id: 'fixture-tool', status: 'completed', output: [], usage: { input_tokens: 12, output_tokens: 20, total_tokens: 32 } } }); res.end(); return;
      }
      const answer = JSON.stringify({ answer: 'From the ChatGPT plan fixture', citations: [], results: [] });
      event({ type: 'response.output_item.added', output_index: 0, item: { type: 'message', id: 'msg_1', role: 'assistant', status: 'in_progress', content: [] } });
      event({ type: 'response.content_part.added', output_index: 0, content_index: 0, part: { type: 'output_text', text: '', annotations: [] } });
      event({ type: 'response.output_text.delta', output_index: 0, content_index: 0, delta: answer });
      event({ type: 'response.output_text.done', output_index: 0, content_index: 0, text: answer });
      event({ type: 'response.output_item.done', output_index: 0, item: { type: 'message', id: 'msg_1', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: answer, annotations: [] }] } });
      if (!settings.eof) event({ type: settings.incomplete ? 'response.incomplete' : 'response.completed', response: { id: 'fixture-response', status: settings.incomplete ? 'incomplete' : 'completed', output: [], usage: { input_tokens: 12, output_tokens: 20, total_tokens: 32 }, ...(settings.incomplete ? { incomplete_details: { reason: 'max_output_tokens' } } : {}) } });
      res.end(); return;
    }
    json({}, 404);
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening'); const address = server.address(); assert.ok(address && typeof address !== 'string'); issuer = `http://127.0.0.1:${address.port}`;
  const rpc: RequestContext = { id: 1, signal: new AbortController().signal, notify() {}, host: { call: async <T>(method: string, paramsRaw: unknown) => {
    const params = paramsRaw as any;
    if (method === 'credentials.read') return { value: structuredClone(value) } as T;
    if (method === 'credentials.acquire') { if (lease) throw new Error('Credential store is busy; retry'); lease = `lease-${Math.random()}`; return { lease, value: structuredClone(value) } as T; }
    if (method === 'credentials.commit') { assert.equal(params.lease, lease); value = structuredClone(params.value); lease = undefined; return {} as T; }
    if (method === 'credentials.release') { assert.equal(params.lease, lease); lease = undefined; return {} as T; }
    if (method === 'files.read') return { location: params.location, text: 'Fixture contents', binary: false, truncated: false } as T;
    if (method === 'browser.open') {
      const url = new URL(params.url); urls.push(url); latestNonce = url.searchParams.get('nonce')!;
      const callback = new URL(url.searchParams.get('redirect_uri')!); callback.searchParams.set('state', settings.wrongState ? 'wrong-state' : url.searchParams.get('state')!);
      if (settings.deny) callback.searchParams.set('error', 'access_denied');
      else { callback.searchParams.set('code', 'fixture-code'); if (url.searchParams.get('client_id') === 'dynamic_agent_client') callback.searchParams.set('client_id', settings.callbackClient ?? `oaiapp_fixture_${++issued}`); }
      const response = await fetch(callback); if (settings.wrongState) assert.equal(response.status, 400); return {} as T;
    }
    throw new Error('Unexpected host operation');
  } } };
  const options: AccountOptions = { issuer, api: `${issuer}/v1`, now: () => clock, signInTimeoutMs: 1000 };
  const manager = new ChatGPTAccounts(rpc, options);
  try { await run({ manager, rpc, options, state: () => structuredClone(value), settings, urls, bodies, advances: ms => { clock += ms; }, calls, leaseCount: () => lease ? 1 : 0 }); }
  finally { server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); }
}

test('ChatGPT is the default; existing explicit API and local provider choices remain', () => {
  assert.equal(readConfig({}).provider, 'chatgpt');
  for (const provider of ['openai', 'anthropic', 'compatible']) assert.equal(readConfig({ provider }).provider, provider);
  for (const field of ['access_token', 'refresh_token', 'id_token']) assert.throws(() => readConfig({ [field]: 'fixture-secret' }), /environment variables/);
});
test('first sign-in verifies identity, discovers account models and returns no credentials', async () => fixture(async f => {
  assert.equal((await f.manager.invoke({ action: 'status' })).sign_in_required, true); assert.equal(f.calls.length, 0);
  const connected = await f.manager.invoke({ action: 'sign_in' });
  assert.equal(connected.first_connection, true); assert.equal(connected.using_plan, true); assert.equal(connected.sign_in_required, false);
  assert.deepEqual(connected.models, [{ id: 'fixture-model', name: 'Fixture model' }]); assert.match(connected.accounts[0].label, /same@example.test/);
  assert.doesNotMatch(JSON.stringify(connected), /fixture-access|fixture-refresh|id_token/);
  const authorization = f.urls[0]; assert.equal(authorization.searchParams.get('agent_name_hint'), 'Buckets'); assert.equal(authorization.searchParams.get('client_id'), 'dynamic_agent_client'); assert.equal(new URL(authorization.searchParams.get('redirect_uri')!).hostname, '127.0.0.1');
  assert.equal(f.state().accounts[0].subject, 'subject-1'); assert.equal(f.leaseCount(), 0);
}));
test('returning sign-in reuses client and host IDs; same-email registrations remain distinct', async () => fixture(async f => {
  const first = await f.manager.invoke({ action: 'sign_in' }); const id = first.active_account_id!;
  const returning = await f.manager.invoke({ action: 'sign_in', account_id: id });
  assert.equal(returning.first_connection, false); assert.equal(returning.accounts.length, 1);
  assert.equal(f.urls[1].searchParams.get('client_id'), f.state().accounts[0].client_id); assert.equal(f.urls[1].searchParams.get('ext_agent_host_id'), f.urls[0].searchParams.get('ext_agent_host_id')); assert.equal(f.urls[1].searchParams.has('agent_name_hint'), false); assert.ok(f.urls[1].searchParams.get('id_token_hint'));
  const second = await f.manager.invoke({ action: 'sign_in' }); assert.equal(second.accounts.length, 2); assert.notEqual(second.accounts[0].label, second.accounts[1].label);
  await f.manager.invoke({ action: 'select', account_id: id }); assert.equal((await f.manager.modelConnection('')).accountId, id);
}));
test('denial, mismatched state, nonce, audience and signature never replace active credentials', async () => {
  for (const failure of ['deny', 'wrongState', 'badNonce', 'wrongAudience', 'badSignature'] as const) await fixture(async f => {
    const signed = await f.manager.invoke({ action: 'sign_in' }); const before = f.state().accounts[0].tokens; f.settings[failure] = true;
    await assert.rejects(f.manager.invoke({ action: 'sign_in', account_id: signed.active_account_id }), /declined|cancelled|timed out|verified/);
    assert.deepEqual(f.state().accounts[0].tokens, before); assert.equal(f.leaseCount(), 0);
  });
});
test('issued registration is retained after failed exchange and reused on retry', async () => fixture(async f => {
  f.settings.tokenFailure = 'invalid_grant'; await assert.rejects(f.manager.invoke({ action: 'sign_in' }), /expired|reconnect/);
  const saved = f.state(); assert.equal(saved.accounts.length, 1); assert.equal(saved.accounts[0].tokens, undefined); assert.notEqual(saved.accounts[0].client_id, 'dynamic_agent_client');
  delete f.settings.tokenFailure; await f.manager.invoke({ action: 'sign_in', account_id: saved.accounts[0].id });
  assert.equal(f.urls[1].searchParams.get('client_id'), saved.accounts[0].client_id); assert.equal(f.state().accounts.length, 1);
}));
test('identity sign-in without plan grant stays connected and requests consent to enable plan', async () => fixture(async f => {
  f.settings.scopes = 'openid profile email offline_access'; const result = await f.manager.invoke({ action: 'sign_in' });
  assert.equal(result.accounts[0].connected, true); assert.equal(result.plan_enabled, false); assert.equal(result.sign_in_required, true); assert.equal(result.first_connection, false);
  await assert.rejects(f.manager.modelConnection(''), /Continue with ChatGPT/);
  delete f.settings.scopes; const enabled = await f.manager.invoke({ action: 'sign_in', account_id: result.active_account_id }); assert.equal(f.urls[1].searchParams.get('prompt'), 'consent'); assert.equal(enabled.first_connection, true);
}));
test('connecting ChatGPT while an API provider is selected preserves billing choice and defers plan-use confirmation', async () => fixture(async f => {
  const connected = await f.manager.invoke({ action: 'sign_in' }, 'openai'); assert.equal(connected.using_plan, false); assert.equal(connected.first_connection, false); assert.match(connected.message!, /unchanged/);
  const selected = await f.manager.invoke({ action: 'status' }, 'chatgpt'); assert.equal(selected.first_connection, true); assert.equal(selected.using_plan, true); assert.equal((await f.manager.invoke({ action: 'status' }, 'chatgpt')).first_connection, false);
}));
test('refresh rotates credentials once under a lease; terminal failures clear tokens, transient failures preserve them', async () => fixture(async f => {
  await f.manager.invoke({ action: 'sign_in' }); f.advances(3600000);
  const tokens = await Promise.all([f.manager.modelConnection(''), f.manager.modelConnection('')]); assert.equal(tokens[0].token, 'fixture-refreshed-access'); assert.equal(tokens[1].token, 'fixture-refreshed-access'); assert.equal(f.state().accounts[0].tokens.refresh, 'fixture-rotated-refresh');
  assert.equal(f.calls.filter(path => path === '/api/accounts/oauth/token').length, 2);
  f.advances(3600000); f.settings.refreshFailure = 'server_error'; await assert.rejects(f.manager.modelConnection(''), /renew/); assert.ok(f.state().accounts[0].tokens);
  f.settings.refreshFailure = 'invalid_grant'; await assert.rejects(f.manager.modelConnection(''), /expired/); assert.equal(f.state().accounts[0].tokens, undefined); assert.ok(f.state().accounts[0].client_id); assert.equal(f.leaseCount(), 0);
}));
test('a successful refresh without a replacement token preserves the old record and rejects the malformed exchange', async () => fixture(async f => {
  await f.manager.invoke({ action: 'sign_in' }); const original = f.state().accounts[0].tokens; f.advances(3600000); f.settings.omitRefreshToken = true;
  await assert.rejects(f.manager.modelConnection(''), /could not renew/); assert.deepEqual(f.state().accounts[0].tokens, original); assert.equal(f.leaseCount(), 0);
  delete f.settings.omitRefreshToken; assert.equal((await f.manager.modelConnection('')).token, 'fixture-refreshed-access'); assert.equal(f.state().accounts[0].tokens.refresh, 'fixture-rotated-refresh');
}));
test('sign-out revokes then clears locally, retaining host and registration; offline revocation is explicit', async () => fixture(async f => {
  const connected = await f.manager.invoke({ action: 'sign_in' }); const host = f.state().host_id;
  f.settings.revocationFailure = true; const signedOut = await f.manager.invoke({ action: 'sign_out', account_id: connected.active_account_id });
  assert.match(signedOut.message!, /Remote revocation could not be confirmed/); assert.equal(signedOut.active_account_id, undefined); assert.equal(f.state().accounts[0].tokens, undefined); assert.equal(f.state().host_id, host); assert.ok(f.state().accounts[0].client_id);
}));
test('SIWC payload excludes unsupported fields and groups local tools without changing names', () => {
  const payload = chatGPTPayload({ model: 'fixture', input: [{ type: 'message', role: 'system', content: 'rules' }], store: true, stream: false, previous_response_id: 'old', max_output_tokens: 3, temperature: 1, tools: [{ type: 'function', name: 'files_read', parameters: { type: 'object' } }] }) as any;
  assert.equal(payload.store, false); assert.equal(payload.stream, true); assert.equal(payload.input[0].role, 'developer'); assert.equal(payload.max_output_tokens, undefined); assert.equal(payload.previous_response_id, undefined); assert.equal(payload.temperature, undefined); assert.equal(payload.tools[0].type, 'namespace'); assert.equal(payload.tools[0].tools[0].name, 'files_read');
});
test('HTTP policy and availability failures do not recommend repeated OAuth sign-in', () => {
  const policy = chatGPTFailure('OpenAI API error 403'); assert.match(policy, /policy or region/); assert.doesNotMatch(policy, /reconnect|Continue with ChatGPT/);
  const unavailable = chatGPTFailure('OpenAI API error 503'); assert.match(unavailable, /temporarily unavailable/); assert.match(unavailable, /connection is preserved/); assert.doesNotMatch(unavailable, /reconnect/);
  assert.match(chatGPTFailure('OpenAI API error 401'), /Continue with ChatGPT/);
});
test('actual Responses admission distinguishes 401 credentials, 403 policy and 503 availability without exposing HTTP bodies', async () => {
  for (const status of [401, 403, 503]) await fixture(async f => {
    await f.manager.invoke({ action: 'sign_in' }); f.settings.httpStatus = status;
    const directory = await mkdtemp(join(tmpdir(), 'buckets-siwc-http-status-')); let runtime: WorkspaceAI | undefined;
    try {
      runtime = await WorkspaceAI.open(directory, { accountOptions: f.options });
      await assert.rejects(runtime.invoke(request(directory), {}, f.rpc), error => {
        const message = String(error); assert.doesNotMatch(message, /SECRET-reflected-body/);
        if (status === 401) assert.match(message, /Continue with ChatGPT/);
        else { assert.doesNotMatch(message, /reconnect|Continue with ChatGPT/); assert.match(message, status === 403 ? /policy or region/ : /temporarily unavailable/); }
        return true;
      });
      assert.ok(f.state().accounts[0].tokens); await runtime.close(); runtime = undefined;
      assert.equal((await readFile(join(directory, 'conversation.sqlite'))).includes(Buffer.from('SECRET-reflected-body')), false);
    } finally { await runtime?.close(); await rm(directory, { recursive: true, force: true }); }
  });
});
test('real durable harness uses signed-in account, namespaces tools and keeps secrets out of SQLite', async () => fixture(async f => {
  await f.manager.invoke({ action: 'sign_in' }); const directory = await mkdtemp(join(tmpdir(), 'buckets-siwc-provider-')); let runtime: WorkspaceAI | undefined;
  try {
    runtime = await WorkspaceAI.open(directory, { accountOptions: f.options });
    const answer = await runtime.invoke(request(directory), {}, f.rpc); assert.equal(answer.answer, 'From the ChatGPT plan fixture'); assert.equal(f.bodies.length, 1);
    const body = f.bodies[0] as any; assert.equal(body.model, 'fixture-model'); assert.equal(body.store, false); assert.equal(body.stream, true); assert.equal(body.tools[0].type, 'namespace'); assert.equal(body.max_output_tokens, undefined); assert.equal(body.temperature, undefined); assert.equal(body.previous_response_id, undefined);
    await runtime.close(); runtime = undefined;
    const sqlite = await readFile(join(directory, 'conversation.sqlite')); assert.equal(sqlite.includes(Buffer.from('fixture-access')), false); assert.equal(sqlite.includes(Buffer.from('fixture-refresh')), false);
  } finally { await runtime?.close(); await rm(directory, { recursive: true, force: true }); }
}));
test('namespaced model tool calls execute through Rust RPC and replay tool results in the next Responses request', async () => fixture(async f => {
  await f.manager.invoke({ action: 'sign_in' }); f.settings.toolFirst = true;
  const directory = await mkdtemp(join(tmpdir(), 'buckets-siwc-tools-')); let runtime: WorkspaceAI | undefined;
  try {
    runtime = await WorkspaceAI.open(directory, { accountOptions: f.options }); const result = await runtime.invoke(request(directory), {}, f.rpc); assert.match(result.answer, /ChatGPT plan fixture/); assert.equal(f.bodies.length, 2);
    const second = f.bodies[1] as any; assert.ok(second.input.some((item: any) => item.type === 'function_call' && item.namespace === 'buckets' && item.name === 'files_read')); assert.ok(second.input.some((item: any) => item.type === 'function_call_output' && String(item.output).includes('Fixture contents')));
  } finally { await runtime?.close(); await rm(directory, { recursive: true, force: true }); }
}));
test('switching ChatGPT accounts resets model history and rejects a changed-account in-flight result', async () => fixture(async f => {
  const first = await f.manager.invoke({ action: 'sign_in' }); const second = await f.manager.invoke({ action: 'sign_in' }); await f.manager.invoke({ action: 'select', account_id: first.active_account_id });
  const directory = await mkdtemp(join(tmpdir(), 'buckets-siwc-isolation-')); let runtime: WorkspaceAI | undefined;
  try {
    runtime = await WorkspaceAI.open(directory, { accountOptions: f.options }); const original = request(directory); original.prompt = 'FIRST-ACCOUNT-PRIVATE-PROMPT'; await runtime.invoke(original, {}, f.rpc);
    await f.manager.invoke({ action: 'select', account_id: second.active_account_id }); const next = request(directory, 'ask', 'second-request'); next.prompt = 'Second account question'; await runtime.invoke(next, {}, f.rpc);
    assert.doesNotMatch(JSON.stringify((f.bodies[1] as any).input), /FIRST-ACCOUNT-PRIVATE-PROMPT/);
    f.settings.beforeResponse = async () => { delete f.settings.beforeResponse; await f.manager.invoke({ action: 'select', account_id: first.active_account_id }); };
    await assert.rejects(runtime.invoke(request(directory, 'ask', 'third-request'), {}, f.rpc), /account changed/);
  } finally { await runtime?.close(); await rm(directory, { recursive: true, force: true }); }
}));
test('plan-limit failures after streaming and incomplete responses never become successful answers', async () => {
  for (const failure of ['subscription_sharing_usage_limit_exceeded', 'subscription_sharing_usage_unavailable', 'incomplete', 'eof']) await fixture(async f => {
    await f.manager.invoke({ action: 'sign_in' }); const directory = await mkdtemp(join(tmpdir(), 'buckets-siwc-failure-')); let runtime: WorkspaceAI | undefined;
    try {
      if (failure === 'incomplete') f.settings.incomplete = true; else if (failure === 'eof') f.settings.eof = true; else f.settings.responseFailure = failure;
      runtime = await WorkspaceAI.open(directory, { accountOptions: f.options });
      await assert.rejects(runtime.invoke(request(directory), {}, f.rpc), ['incomplete', 'eof'].includes(failure) ? /did not complete/ : /ChatGPT plan usage/);
      await runtime.close(); runtime = undefined; const sqlite = await readFile(join(directory, 'conversation.sqlite')); assert.equal(sqlite.includes(Buffer.from('SECRET-reflected-body')), false);
    } finally { await runtime?.close(); await rm(directory, { recursive: true, force: true }); }
  });
  assert.doesNotMatch(chatGPTFailure('401 SECRET-reflected-body'), /SECRET/);
});
