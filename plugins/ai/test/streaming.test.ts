import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createModels, createProvider, type AssistantMessage, type Model } from '@earendil-works/pi-ai';
import { createAssistantMessageEventStream } from '@earendil-works/pi-ai/utils/event-stream';
import type { AgentProgress } from '@buckets/plugin-sdk';
import type { SnapshotEvent } from '@earendil-works/pi-durable';
import { answerPrefix, assertAnswerText, StreamingProgress } from '../src/streaming.ts';
import { WorkspaceAI } from '../src/runtime.ts';
import { request, rpc } from './helpers.ts';

test('every JSON fragmentation boundary decodes only the top-level answer without escape or surrogate fragments', () => {
  const answer = '# Useful files\n\n**Unicode** 😀 λ 漢字\n\n```ts\nconst path = "a\\b";\n```\n\tquoted: "yes"';
  for (const object of [
    { answer, citations: [], results: [] },
    { citations: [{ label: '"answer":"wrong"', nested: { answer: 'wrong too' } }], results: [{ reason: '}, ["answer"]' }], answer },
  ]) {
    for (const source of [JSON.stringify(object), '```json\n' + JSON.stringify(object) + '\n```', JSON.stringify(object).replace('😀', '\\ud83d\\ude00')]) {
      for (let boundary = 0; boundary <= source.length; boundary++) {
        const preview = answerPrefix(source.slice(0, boundary));
        assert.ok(answer.startsWith(preview), JSON.stringify({ boundary, preview }));
        assert.doesNotMatch(preview, /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/);
      }
      assert.equal(answerPrefix(source), answer);
    }
  }
  assert.equal(answerPrefix('{"nested":{"answer":"private"},"answer":"public'), 'public');
  assert.equal(answerPrefix('raw model commentary or reasoning'), '');
  assert.throws(() => answerPrefix('{"answer":"bad\\q"}'), /invalid answer format/);
  assert.equal(Buffer.byteLength(answerPrefix(JSON.stringify({ answer: '😀'.repeat(16384) }))), 65536);
  assert.throws(() => answerPrefix(JSON.stringify({ answer: '😀'.repeat(16385) })), /64 KiB/);
  assert.throws(() => assertAnswerText('\ud800'), /invalid Unicode/);
});

test('stream snapshots replace text and remain bounded, private and cancellable', async () => {
  const controller = new AbortController(); const sent: AgentProgress[] = [];
  const context = rpc(async () => ({}), controller.signal);
  context.notify = (_, event) => sent.push(event as AgentProgress);
  const progress = new StreamingProgress('r', context, () => assert.fail('valid stream aborted'));
  progress.accept([{ type: 'message_start', message: { role: 'assistant', content: [{ type: 'thinking', thinking: 'PRIVATE REASONING' }, { type: 'text', text: '{"answer":"First' }] } as AssistantMessage }]);
  await new Promise(resolve => setTimeout(resolve, 50));
  for (let i = 0; i < 40; i++) progress.accept([{ type: 'tool_execution_start', toolCallId: `t${i}`, toolName: 'files_read', args: { location: '/files/\u0085' + '漢'.repeat(300) + '\nline' } }]);
  progress.accept([{ type: 'message_start', message: { role: 'assistant', content: [{ type: 'text', text: '{"answer":"Replacement' }] } as AssistantMessage }]);
  await new Promise(resolve => setTimeout(resolve, 50));
  const latest = sent.at(-1)!.update!;
  assert.equal(latest.text, 'Replacement');
  assert.equal(latest.activities.length, 32);
  assert.ok(latest.activities.every(activity => Buffer.byteLength(activity.title) <= 512 && !/[\r\n]/.test(activity.title)));
  assert.ok(sent.every((event, index) => event.update!.sequence === index + 1));
  assert.doesNotMatch(JSON.stringify(sent), /PRIVATE REASONING|"citations"/);
  const count = sent.length; controller.abort();
  progress.verifying('{"answer":"After cancel"}');
  await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(sent.length, count); progress.close();
});

test('an authoritative Pi overflow snapshot removes stale text and ghost-running tools', () => {
  const sent: AgentProgress[] = [];
  const context = rpc(async () => ({})); context.notify = (_, event) => sent.push(event as AgentProgress);
  const progress = new StreamingProgress('overflow', context, () => assert.fail('valid snapshot aborted'));
  progress.accept([{ type: 'message_start', message: { role: 'assistant', content: [{ type: 'text', text: '{"answer":"Old provisional text' }] } as AssistantMessage }, { type: 'tool_execution_start', toolCallId: 'old-running', toolName: 'files_read', args: { location: '/fixture/a' } }]);
  assert.equal(sent.at(-1)?.update?.text, 'Old provisional text');
  const snapshot = { type: 'snapshot', entries: [], tools: [], nestedTools: [{ callId: 'nested-failed', name: 'files_read', status: 'done', summary: { isError: true } }], compactions: [], inbox: [], agent: {}, usage: {} } as unknown as SnapshotEvent;
  progress.accept([snapshot]);
  const current = sent.at(-1)!.update!;
  assert.equal(current.text, '');
  assert.equal(current.phase, 'waiting');
  assert.ok(current.activities.every(activity => activity.state === 'error'));
  assert.equal(current.activities.length, 2);
  progress.verifying('{"answer":"Final provisional text"}');
  progress.accept([{ type: 'message_start', message: { role: 'assistant', content: [{ type: 'text', text: '{"answer":"Late stale batch' }] } as AssistantMessage }]);
  assert.equal(sent.at(-1)?.update?.text, 'Final provisional text');
  assert.equal(sent.at(-1)?.update?.phase, 'verifying');
  progress.close();
});

test('the real durable harness publishes decoded text before model completion and final validation', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'buckets-stream-'));
  const model: Model<'openai-completions'> = { id: 'stream-test', name: 'Stream test', provider: 'stream-test', api: 'openai-completions', baseUrl: 'http://fixture.invalid', reasoning: false, input: ['text'], contextWindow: 10000, maxTokens: 1000, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } };
  let sawText!: () => void;
  const textObserved = new Promise<void>(resolve => { sawText = resolve; });
  let providerCompleted = false; let timedOut = false;
  const stream = () => {
    const events = createAssistantMessageEventStream();
    const message: AssistantMessage = { role: 'assistant', provider: model.provider, api: model.api, model: model.id, content: [], stopReason: 'stop', timestamp: Date.now(), usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
    void (async () => {
      events.push({ type: 'start', partial: structuredClone(message) });
      message.content = [{ type: 'text', text: '' }];
      events.push({ type: 'text_start', contentIndex: 0, partial: structuredClone(message) });
      for (const delta of ['{"answer":"# Hello ', '\\ud83d', '\\ude00', '\\n\\n**Actual streaming**', '","citations":[],"results":[]}']) {
        (message.content[0] as { text: string }).text += delta;
        events.push({ type: 'text_delta', contentIndex: 0, delta, partial: structuredClone(message) });
        await new Promise(resolve => setTimeout(resolve, 25));
      }
      let timer: ReturnType<typeof setTimeout> | undefined;
      await Promise.race([textObserved, new Promise<void>(resolve => { timer = setTimeout(() => { timedOut = true; resolve(); }, 3000); })]);
      if (timer) clearTimeout(timer);
      providerCompleted = true;
      events.push({ type: 'text_end', contentIndex: 0, content: (message.content[0] as { text: string }).text, partial: structuredClone(message) });
      events.push({ type: 'done', reason: 'stop', message }); events.end();
    })();
    return events;
  };
  const models = createModels(); models.setProvider(createProvider({ id: model.provider, models: [model], auth: { apiKey: { name: 'fixture', resolve: async () => ({ auth: {} }) } }, api: { stream, streamSimple: stream } }));
  const runtime = await WorkspaceAI.open(directory, { models });
  const updates: AgentProgress[] = [];
  const context = rpc(async () => assert.fail('No file references expected'));
  context.notify = (_, value) => {
    const event = value as AgentProgress; updates.push(event);
    if (event.update?.text.includes('Hello') && !providerCompleted) sawText();
  };
  try {
    const result = await runtime.invoke(request(directory), {}, context);
    assert.equal(timedOut, false, 'text must become observable while the provider is held open');
    assert.equal(result.answer, '# Hello 😀\n\n**Actual streaming**');
    assert.ok(updates.some(event => event.update?.phase === 'verifying'));
    assert.equal(updates.at(-1)?.update?.text, result.answer);
    assert.ok(updates.every(event => !event.update?.text.startsWith('{')));
  } finally { await runtime.close(); await rm(directory, { recursive: true, force: true }); }
});
