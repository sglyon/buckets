import { createModels, createProvider, type AssistantMessage, type Model, type TranscriptContext } from '@earendil-works/pi-ai';
import { createAssistantMessageEventStream } from '@earendil-works/pi-ai/utils/event-stream';
import type { AgentRequest, RequestContext } from '@buckets/plugin-sdk';
export function fakeModels(reply: (context: TranscriptContext, call: number) => AssistantMessage['content'], onCall?: () => void) {
  let count = 0;
  const model: Model<'openai-completions'> = { id: 'test', provider: 'test', name: 'Test only', api: 'openai-completions', baseUrl: 'http://test.invalid', reasoning: false, input: ['text'], contextWindow: 128000, maxTokens: 4096, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } };
  const stream = (_: unknown, input: TranscriptContext) => {
    const events = createAssistantMessageEventStream(); onCall?.();
    const content = reply(input, ++count);
    const message: AssistantMessage = { role: 'assistant', api: model.api, provider: model.provider, model: model.id, content, stopReason: content.some(c => c.type === 'toolCall') ? 'toolUse' : 'stop', timestamp: Date.now(), usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
    queueMicrotask(() => { events.push({ type: 'start', partial: message }); events.push({ type: 'done', reason: message.stopReason as 'stop' | 'toolUse', message }); events.end(); });
    return events;
  };
  const models = createModels(); models.setProvider(createProvider({ id: 'test', auth: { apiKey: { name: 'test only', resolve: async () => ({ auth: {} }) } }, models: [model], api: { stream, streamSimple: stream } }));
  return { models, count: () => count };
}
export function request(directory: string, mode: AgentRequest['mode'] = 'ask', id = 'request-1'): AgentRequest {
  return { request_id: id, mode, prompt: 'Explain this file', storage_dir: directory, context: { workspace_id: 'test-workspace', revision: 1, panes: [{ id: 'p1', location: '/fixture', selected: ['/fixture/a.txt'], cursor: '/fixture/a.txt', entries: [{ location: '/fixture/a.txt', name: 'a.txt', is_dir: false }], capabilities: ['read', 'list', 'search'] }] } };
}
export function rpc(call: (method: string, params: any) => Promise<any>, signal = new AbortController().signal): RequestContext {
  return { id: 1, signal, host: { call }, notify() {} };
}
