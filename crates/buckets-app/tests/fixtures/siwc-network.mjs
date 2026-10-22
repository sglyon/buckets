// Installed-process fixture only. Production has no endpoint override setting.
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { appendFileSync, readFileSync } from 'node:fs';

const config = globalThis.__bucketsSiwcFixture;
if (!config?.log || !config?.control) throw new Error('Missing isolated SIWC fixture configuration');
process.env.OPENAI_API_KEY = 'ambient-api-key-must-never-be-used';
const { privateKey, publicKey } = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
const key = { ...publicKey.export({ format: 'jwk' }), kid: 'siwc-fixture', alg: 'ES256', use: 'sig' };
const calls = new Map();
const json = (value, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } });
function control() { try { return JSON.parse(readFileSync(config.control, 'utf8')); } catch { return {}; } }
function log(value) { appendFileSync(config.log, JSON.stringify(value) + '\n'); }
function require(value, reason) { if (!value) { log({ failure: reason }); throw new Error(`SIWC fixture contract: ${reason}`); } }
function jwt(claims, invalidSignature = false) {
  const header = Buffer.from(JSON.stringify({ alg: 'ES256', kid: 'siwc-fixture' })).toString('base64url');
  const body = Buffer.from(JSON.stringify(claims)).toString('base64url');
  const signature = sign('sha256', Buffer.from(`${header}.${body}`), { key: privateKey, dsaEncoding: 'ieee-p1363' });
  if (invalidSignature) signature[0] ^= 1;
  return `${header}.${body}.${signature.toString('base64url')}`;
}
function sse(events) {
  return new Response(events.map(event => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join(''), { headers: { 'content-type': 'text/event-stream' } });
}
globalThis.fetch = async (input, init) => {
  const request = new Request(input, init);
  const url = new URL(request.url);
  const options = control();
  const raw = request.method === 'POST' ? await request.text() : '';
  const form = new URLSearchParams(raw);
  require(['https://auth.openai.com', 'https://api.openai.com'].includes(url.origin), 'unexpected network destination');
  require(!request.headers.get('authorization')?.includes('ambient-api-key'), 'ambient API credential leaked');
  if (url.origin === 'https://auth.openai.com') {
    if (url.pathname === '/.well-known/openid-configuration') return json({ issuer: url.origin, jwks_uri: `${url.origin}/fixture-jwks`, revocation_endpoint: `${url.origin}/fixture-revoke` });
    if (url.pathname === '/fixture-jwks') return json({ keys: [key] });
    if (url.pathname === '/fixture-revoke') {
      log({ kind: 'revoke', client: form.get('client_id'), token_is_refresh: form.get('token')?.startsWith('fixture-refresh-') });
      return options.revocationFailure ? json({ error: 'fixture' }, 503) : new Response('', { status: 200 });
    }
    require(url.pathname === '/api/accounts/oauth/token', 'unexpected authorization route');
    const client = form.get('client_id');
    require(client?.startsWith('oaiapp_fixture_'), 'token exchange lacks issued client ID');
    require(form.get('resource') === 'https://api.openai.com/v1', 'wrong token resource');
    const grant = form.get('grant_type');
    log({ kind: 'token', client, grant });
    if (grant === 'refresh_token' && options.refreshFailure) return json({ error: options.refreshFailure, error_description: 'RAW-FIXTURE-SECRET-MUST-NOT-ESCAPE' }, 400);
    let nonce = 'refresh-nonce';
    if (grant === 'authorization_code') {
      const code = JSON.parse(form.get('code'));
      require(code.client === client && code.redirect === form.get('redirect_uri'), 'exchange registration/callback mismatch');
      require(createHash('sha256').update(form.get('code_verifier')).digest('base64url') === code.challenge, 'invalid PKCE verifier');
      nonce = options.badNonce ? 'wrong-fixture-nonce' : code.nonce;
    } else require(grant === 'refresh_token' && form.get('refresh_token')?.startsWith(`fixture-refresh-${client}`), 'invalid refresh grant');
    const now = Math.floor(Date.now() / 1000);
    const scopes = 'openid profile email offline_access resource.invoke' + (options.noPlan ? '' : ' chatgpt.tokens.use.direct');
    const tokens = { token_type: 'Bearer', expires_in: 3600, scope: scopes, access_token: `fixture-access-${client}${grant === 'refresh_token' ? '-rotated' : ''}`, refresh_token: `fixture-refresh-${client}${grant === 'refresh_token' ? '-rotated' : ''}`, id_token: jwt({ iss: 'https://auth.openai.com', sub: `subject-${client}`, aud: options.badAudience ? 'wrong-client' : client, nonce, email: 'same-email@example.test', exp: now + 3600, iat: now }, options.badSignature) };
    if (grant === 'refresh_token' && options.missingRefresh) delete tokens.refresh_token;
    return json(tokens);
  }
  const bearer = request.headers.get('authorization');
  require(bearer?.startsWith('Bearer fixture-access-oaiapp_fixture_'), 'request lacks ChatGPT OAuth bearer');
  const client = bearer.replace(/^Bearer fixture-access-/, '').replace(/-rotated$/, '');
  if (url.pathname === '/v1/models') {
    log({ kind: 'models', client });
    return json({ models: [{ slug: 'fixture-model-a', display_name: 'Fixture A', visibility: 'list' }, { slug: 'hidden-model', display_name: 'Hidden', visibility: 'hide' }, { slug: 'fixture-model-b', display_name: 'Fixture B', visibility: 'list' }], data: [{ id: 'wrong-standard-api-shape' }] });
  }
  require(url.pathname === '/v1/responses' && request.method === 'POST', 'unexpected inference route');
  const body = JSON.parse(raw);
  log({ kind: 'responses', client, body });
  require(body.store === false && body.stream === true && Array.isArray(body.input), 'invalid stateless streaming contract');
  for (const field of ['background', 'conversation', 'max_output_tokens', 'max_tool_calls', 'metadata', 'moderation', 'multi_agent', 'prompt', 'prompt_cache_retention', 'safety_identifier', 'temperature', 'top_logprobs', 'top_p', 'truncation', 'user', 'previous_response_id']) require(!(field in body), `unsupported field ${field}`);
  require(body.input.every(item => item.role !== 'system'), 'system role reached public plan route');
  require(body.tools?.length === 1 && body.tools[0].type === 'namespace' && body.tools[0].name === 'buckets', 'tools are not grouped');
  require(body.tools[0].tools.some(tool => tool.name === 'workspace_snapshot'), 'workspace tool absent');
  if (options.responsesFailure) return sse([{ type: 'response.failed', response: { id: 'fixture-failed', status: 'failed', error: { code: options.responsesFailure, message: 'RAW-FIXTURE-SECRET-MUST-NOT-ESCAPE' } } }]);
  const turn = (calls.get(client) ?? 0) + 1; calls.set(client, turn);
  const id = `fixture-${client}-${turn}`;
  const item = turn === 1 ? { type: 'function_call', id: `fc-${id}`, call_id: `call-${id}`, name: 'workspace_snapshot', namespace: 'buckets', arguments: '{}' } : { type: 'message', id: `msg-${id}`, role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: JSON.stringify({ answer: `private-answer-${client}`, citations: [], results: [] }), annotations: [] }] };
  const events = [{ type: 'response.created', response: { id, status: 'in_progress' } }, { type: 'response.output_item.added', output_index: 0, item }, { type: 'response.output_item.done', output_index: 0, item }];
  if (!options.endWithoutCompleted) events.push({ type: 'response.completed', response: { id, status: 'completed', output: [item], usage: { input_tokens: 20, output_tokens: 10, total_tokens: 30 } } });
  return sse(events);
};
