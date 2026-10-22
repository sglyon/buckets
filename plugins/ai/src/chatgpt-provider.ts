import { createProvider, type Model, type MutableModels, type ProviderStreams, type AssistantMessage, type AssistantMessageEventStream, type StreamOptions } from '@earendil-works/pi-ai';
import { openAIResponsesApi } from '@earendil-works/pi-ai/api/openai-responses.lazy';
import { createAssistantMessageEventStream } from '@earendil-works/pi-ai/utils/event-stream';
import type { RequestContext } from '@buckets/plugin-sdk';
import { ChatGPTAccounts, type AccountOptions } from './accounts.ts';

/** Fixed public diagnostics also keep reflected HTTP response bodies out of checkpoints. */
export function chatGPTFailure(detail: unknown): string {
  const value = typeof detail === 'string' ? detail : JSON.stringify(detail ?? '');
  if (/subscription_sharing_usage_limit_exceeded/.test(value)) return 'ChatGPT plan usage limit reached (subscription_sharing_usage_limit_exceeded). Manage usage in ChatGPT settings.';
  if (/subscription_sharing_usage_unavailable/.test(value)) return 'ChatGPT plan usage unavailable (subscription_sharing_usage_unavailable). Manage usage in ChatGPT settings.';
  if (/subscription_sharing_user_not_eligible|not eligible/.test(value)) return 'This ChatGPT account is not eligible for plan usage in Buckets (subscription_sharing_user_not_eligible). Choose another account, an API key, or a local provider.';
  if (/unsupported_capability|route_not_supported|not supported by ChatGPT/.test(value)) return 'This request is not supported by ChatGPT plan usage. Check the model and plugin configuration.';
  if (/user_unavailable|usage_unavailable|temporarily unavailable/.test(value)) return 'ChatGPT plan usage is temporarily unavailable. Try again later; your connection is preserved.';
  if (/invalid_user|chatpass_v2/i.test(value)) return 'ChatGPT could not authorize this request. Review your account access and plan usage in ChatGPT settings.';
  if (/\b403\b|forbidden|region restriction|policy restriction|account policy or region/i.test(value)) return 'ChatGPT access is restricted by account policy or region. Review account access or choose another provider.';
  if (/permission|policy|could not authorize/i.test(value)) return 'ChatGPT could not authorize this request. Review your account access and plan usage in ChatGPT settings.';
  if (/\b503\b|temporarily unavailable/i.test(value)) return 'ChatGPT plan usage is temporarily unavailable. Try again later; your connection is preserved.';
  if (/\b401\b|unauthori[sz]ed|authentication|session expired|reconnect/i.test(value)) return 'ChatGPT session needs attention. Continue with ChatGPT in Settings to reconnect.';
  if (/account changed|disconnected/i.test(value)) return 'The ChatGPT account changed or disconnected. Submit a new question after connecting.';
  if (/incomplete|did not complete|terminal response|stream ended|interrupted/i.test(value)) return 'ChatGPT response did not complete. Submit a new question to try again.';
  if (/429|rate.limit/i.test(value)) return 'ChatGPT request limit reached. Manage usage in ChatGPT settings or try again later.';
  return 'ChatGPT request failed. Check your connection and account settings, then submit a new question.';
}
const UNSUPPORTED = ['background', 'conversation', 'max_output_tokens', 'max_tool_calls', 'metadata', 'moderation', 'multi_agent', 'prompt', 'prompt_cache_retention', 'prompt_cache_options', 'safety_identifier', 'temperature', 'top_logprobs', 'top_p', 'truncation', 'user', 'previous_response_id'];
/** Enforce the SIWC wire contract independently of upstream token heuristics. */
export function chatGPTPayload(payload: unknown): unknown {
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) throw new Error('Invalid ChatGPT request payload');
  const result = structuredClone(payload) as Record<string, unknown>;
  for (const key of UNSUPPORTED) delete result[key];
  result.store = false; result.stream = true;
  if (!Array.isArray(result.input)) throw new Error('ChatGPT requests require explicit context');
  result.input = result.input.map(item => {
    if (!item || typeof item !== 'object') return item;
    const value = item as Record<string, unknown>;
    return value.role === 'system' ? { ...value, role: 'developer' } : value;
  });
  if (Array.isArray(result.tools) && result.tools.length) {
    const tools = result.tools as Record<string, unknown>[];
    if (tools.some(tool => !['function', 'custom'].includes(String(tool.type)))) throw new Error('Unsupported ChatGPT tool type');
    result.tools = [{ type: 'namespace', name: 'buckets', description: 'Read-only Buckets workspace tools', tools }];
  }
  return result;
}
function safeStream(source: AssistantMessageEventStream, model: Model<'openai-responses'>, completed: () => boolean): AssistantMessageEventStream {
  const target = createAssistantMessageEventStream();
  let last: AssistantMessage = { role: 'assistant', api: model.api, provider: model.provider, model: model.id, content: [], stopReason: 'error', timestamp: Date.now(), usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
  let terminal = false;
  const fail = (detail: unknown) => { if (terminal) return; terminal = true; target.push({ type: 'error', reason: 'error', error: { ...last, content: [], stopReason: 'error', errorMessage: chatGPTFailure(detail) } }); };
  void (async () => {
    try {
      for await (const event of source) {
        if ('partial' in event) last = event.partial;
        if (event.type === 'error') { last = event.error; fail(event.error.errorMessage); }
        else if (event.type === 'done') { last = event.message; if (!completed()) fail('ChatGPT stream ended without response.completed'); else { terminal = true; target.push(event); } }
        else target.push(event);
      }
      if (!terminal) fail('ChatGPT stream ended without response.completed');
    } catch (error) { fail(error instanceof Error ? error.message : 'ChatGPT stream interrupted'); }
    finally { target.end(); }
  })();
  return target;
}
export async function configureChatGPTModels(modelId: string, models: MutableModels, rpc: RequestContext, options: AccountOptions = {}): Promise<{ model: Model<'openai-responses'>; accountId: string; accounts: ChatGPTAccounts }> {
  const accounts = new ChatGPTAccounts(rpc, options);
  const connection = await accounts.modelConnection(modelId);
  const baseUrl = options.api ?? 'https://api.openai.com/v1';
  const model: Model<'openai-responses'> = { id: connection.model.id, name: connection.model.name, provider: 'chatgpt', api: 'openai-responses', baseUrl, reasoning: false, input: ['text'], contextWindow: 128000, maxTokens: 8192, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, compat: { supportsDeveloperRole: true, supportsMaxOutputTokens: false } };
  const api = openAIResponsesApi();
  function streamOptions(onCompleted: () => void, optionsIn: StreamOptions = {}): StreamOptions {
    return { ...optionsIn, fetch: options.fetch, maxRetries: 0,
      onPayload: async (value, current) => chatGPTPayload(await optionsIn.onPayload?.(value, current) ?? value),
      onProviderStreamEvent: async (event, current) => { const value = event as { type?: string; response?: { status?: string; error?: { code?: string } } }; if (value.type === 'response.incomplete') throw new Error('ChatGPT response incomplete'); if (value.type === 'response.failed') throw new Error(chatGPTFailure(value.response?.error?.code)); if (value.type === 'response.completed') { if (value.response?.status !== 'completed') throw new Error('ChatGPT response incomplete'); onCompleted(); } await optionsIn.onProviderStreamEvent?.(event, current); },
    };
  }
  const streams: ProviderStreams = {
    stream: (current, transcript, optionsIn) => { let completed = false; return safeStream(api.stream(current as Model<'openai-responses'>, transcript, streamOptions(() => { completed = true; }, optionsIn)), current as Model<'openai-responses'>, () => completed); },
    streamSimple: (current, transcript, optionsIn) => { let completed = false; return safeStream(api.streamSimple(current as Model<'openai-responses'>, transcript, streamOptions(() => { completed = true; }, optionsIn)), current as Model<'openai-responses'>, () => completed); },
  };
  models.setProvider(createProvider({ id: 'chatgpt', name: 'ChatGPT plan', baseUrl, models: [model], auth: { apiKey: { name: 'ChatGPT plan', resolve: async () => ({ auth: { apiKey: await accounts.accessToken(connection.accountId) } }) } }, api: streams }));
  return { model, accountId: connection.accountId, accounts };
}
