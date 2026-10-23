import { test } from "vite-plus/test";
import assert from "node:assert/strict";
import { PassThrough } from "node:stream";
import { servePlugin, type Handler } from "../src/index.ts";
const tick = () => new Promise((resolve) => setImmediate(resolve));
function peer(handlers: Record<string, Handler>, extra: Record<string, unknown> = {}) {
  const input = new PassThrough();
  const output = new PassThrough();
  let text = "";
  output.on("data", (chunk) => {
    text += chunk.toString();
  });
  const done = servePlugin({ input, output, handlers, ...extra });
  return {
    input,
    done,
    send: (v: unknown) => input.write(JSON.stringify(v) + "\n"),
    frames: () =>
      text
        .trim()
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line)),
  };
}
test("legacy one-shot handler completes after stdin EOF", async () => {
  const p = peer({
    hello: async () => {
      await tick();
      return { message: "hello" };
    },
  });
  p.send({ jsonrpc: "2.0", id: 1, method: "hello", params: {} });
  p.input.end();
  await p.done;
  assert.deepEqual(p.frames(), [{ jsonrpc: "2.0", id: 1, result: { message: "hello" } }]);
});
test("duplex callbacks and progress resolve without blocking the input reader", async () => {
  const p = peer({
    "agent.invoke": async (_, ctx) => {
      ctx.notify("agent.progress", { message: "reading" });
      return ctx.host.call("files.read", { location: "ssh://host/file" });
    },
  });
  p.send({ jsonrpc: "2.0", id: 3, method: "agent.invoke", params: {} });
  await tick();
  const callback = p.frames().find((f) => f.method === "files.read");
  assert.match(callback.id, /^plugin-/);
  p.send({ jsonrpc: "2.0", id: callback.id, result: { text: "remote" } });
  await tick();
  assert.deepEqual(p.frames().find((f) => f.id === 3).result, {
    text: "remote",
  });
  p.input.end();
  await p.done;
});
test("service EOF aborts a pending request and runs teardown", async () => {
  let closed = false,
    aborted = false;
  const p = peer(
    {
      "agent.invoke": async (_, ctx) =>
        new Promise((resolve) => {
          ctx.signal.addEventListener("abort", () => {
            aborted = true;
            assert.equal((ctx.signal.reason as { code: number }).code, -32098);
            resolve({ cancelled: true });
          });
        }),
    },
    {
      onClose: () => {
        closed = true;
      },
    },
  );
  p.send({ jsonrpc: "2.0", id: 1, method: "agent.invoke" });
  await tick();
  p.input.end();
  await p.done;
  assert.equal(closed, true);
  assert.equal(aborted, true);
  assert.deepEqual(p.frames(), []);
});
test("account service EOF cancels browser authorization without waiting for its timeout", async () => {
  let cancelled = false;
  const p = peer({
    "accounts.invoke": async (_, ctx) =>
      new Promise((resolve) => {
        ctx.signal.addEventListener("abort", () => {
          cancelled = true;
          resolve({});
        });
      }),
  });
  p.send({ jsonrpc: "2.0", id: 1, method: "accounts.invoke" });
  await tick();
  p.input.end();
  await p.done;
  assert.equal(cancelled, true);
  assert.deepEqual(p.frames(), []);
});
test("service shutdown is bounded even if a handler ignores cancellation", async () => {
  const p = peer({ "agent.invoke": () => new Promise(() => {}) }, { shutdownTimeoutMs: 20 });
  p.send({ jsonrpc: "2.0", id: 1, method: "agent.invoke" });
  await tick();
  p.input.end();
  await p.done;
});
test("cancel notification aborts only the named request", async () => {
  const p = peer({
    wait: async (_, ctx) =>
      new Promise((resolve) => ctx.signal.addEventListener("abort", () => resolve("cancelled"))),
  });
  p.send({ jsonrpc: "2.0", id: 4, method: "wait" });
  await tick();
  p.send({ jsonrpc: "2.0", method: "$/cancelRequest", params: { id: 4 } });
  await tick();
  p.input.end();
  await p.done;
  assert.equal(p.frames()[0].result, "cancelled");
});
test("invalid versions, ambiguous frames and duplicate active IDs fail the connection", async () => {
  for (const frame of [
    { jsonrpc: "1.0", id: 1, method: "hello" },
    { jsonrpc: "2.0", id: 1, method: "hello", result: "ambiguous" },
  ]) {
    const p = peer({ hello: () => "hello" });
    const rejected = assert.rejects(p.done, /JSON-RPC/);
    p.send(frame);
    await rejected;
  }
  const p = peer({ "agent.invoke": () => new Promise(() => {}) }, { shutdownTimeoutMs: 20 });
  const rejected = assert.rejects(p.done, /Duplicate/);
  p.send({ jsonrpc: "2.0", id: 1, method: "agent.invoke" });
  await tick();
  p.send({ jsonrpc: "2.0", id: 1, method: "agent.invoke" });
  await rejected;
});
test("unterminated and oversized frames fail without an unbounded line buffer", async () => {
  const p = peer({}, { maxFrameBytes: 64 });
  const rejected = assert.rejects(p.done, /exceeds/);
  p.input.write("x".repeat(65));
  await rejected;
  const q = peer({});
  const bad = assert.rejects(q.done, /newline/);
  q.input.end("{}");
  await bad;
});
