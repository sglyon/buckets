import { test } from "node:test";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { access } from "node:fs/promises";
import assert from "node:assert/strict";
import { mkdtemp, rm, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { WorkspaceAI, configureModels } from "../src/runtime.ts";
import { readConfig } from "../src/config.ts";
import { fakeModels, request, rpc } from "./helpers.ts";

async function fixture(run: (directory: string) => Promise<void>) {
  const directory = await mkdtemp(join(tmpdir(), "buckets-ai-"));
  try {
    await run(directory);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
test("workspace memory works without credentials, persists and explicitly clears", async () =>
  fixture(async (directory) => {
    let runtime = await WorkspaceAI.open(directory);
    const context = rpc(async () => {
      throw new Error("No host or model call expected");
    });
    assert.equal((await runtime.invoke(request(directory, "memory"), {}, context)).memory, "");
    const save = request(directory, "memory");
    save.memory_update = "Investigate the importer\nDo not forget tests";
    await runtime.invoke(save, {}, context);
    await runtime.close();
    runtime = await WorkspaceAI.open(directory);
    assert.equal(
      (await runtime.invoke(request(directory, "memory"), {}, context)).memory,
      save.memory_update,
    );
    save.memory_update = "";
    await runtime.invoke(save, {}, context);
    assert.equal((await runtime.invoke(request(directory, "memory"), {}, context)).memory, "");
    await runtime.close();
  }));
test("real pi harness executes host read tools and deduplicates completed requests after restart", async () =>
  fixture(async (directory) => {
    const fake = fakeModels((_, call) =>
      call === 1
        ? [
            {
              type: "toolCall",
              id: "read-1",
              name: "files_read",
              arguments: { location: "/fixture/a.txt" },
            },
          ]
        : [
            {
              type: "text",
              text: JSON.stringify({
                answer: "A useful file",
                citations: [{ location: "/fixture/a.txt", line: 2 }],
                results: [],
              }),
            },
          ],
    );
    let reads = 0;
    const context = rpc(async (method, params) => {
      assert.equal(method, "files.read");
      assert.equal(params.location, "/fixture/a.txt");
      reads++;
      return {
        location: params.location,
        text: "one\ntwo\n",
        truncated: false,
        binary: false,
      };
    });
    let runtime = await WorkspaceAI.open(directory, { models: fake.models });
    const result = await runtime.invoke(request(directory), {}, context);
    assert.equal(result.answer, "A useful file");
    assert.equal(result.citations[0]?.line, 2);
    assert.equal(fake.count(), 2);
    assert.equal(reads, 2);
    await runtime.close();
    runtime = await WorkspaceAI.open(directory, { models: fake.models });
    assert.equal((await runtime.invoke(request(directory), {}, context)).answer, "A useful file");
    assert.equal(fake.count(), 2);
    const changed = request(directory);
    changed.context.revision = 2;
    await assert.rejects(runtime.invoke(changed, {}, context), /older workspace snapshot/);
    await runtime.close();
  }));
test("host cancellation receipt blocks retry after restart without replaying model work", async () =>
  fixture(async (directory) => {
    const fake = fakeModels(() => [
      {
        type: "text",
        text: JSON.stringify({ answer: "done", citations: [], results: [] }),
      },
    ]);
    let runtime = await WorkspaceAI.open(directory, { models: fake.models });
    await runtime.invoke(
      request(directory),
      {},
      rpc(async () => ({})),
    );
    await runtime.close();
    await mkdir(join(directory, "host-cancelled-requests"));
    await writeFile(
      join(directory, "host-cancelled-requests", "host-marker.json"),
      JSON.stringify({ request_id: "request-1" }),
    );
    runtime = await WorkspaceAI.open(directory, { models: fake.models });
    await assert.rejects(
      runtime.invoke(
        request(directory),
        {},
        rpc(async () => ({})),
      ),
      /cancelled/,
    );
    assert.equal(fake.count(), 1);
    await runtime.close();
  }));
test("unverified file references and fabricated line numbers are rejected", async () =>
  fixture(async (directory) => {
    const fake = fakeModels(() => [
      {
        type: "text",
        text: JSON.stringify({
          answer: "bad line",
          citations: [{ location: "/fixture/a.txt", line: 99 }],
          results: [],
        }),
      },
    ]);
    const runtime = await WorkspaceAI.open(directory, { models: fake.models });
    await assert.rejects(
      runtime.invoke(
        request(directory),
        {},
        rpc(async () => ({
          location: "/fixture/a.txt",
          text: "one",
          binary: false,
          truncated: false,
        })),
      ),
      /outside the verified/,
    );
    await runtime.close();
  }));
test("a live state owner is never stolen", async () =>
  fixture(async (directory) => {
    const runtime = await WorkspaceAI.open(directory);
    await assert.rejects(WorkspaceAI.open(directory), /already open/);
    await runtime.close();
    const reopened = await WorkspaceAI.open(directory);
    await reopened.close();
  }));
test("configuration errors are explicit and never require saving credentials", () => {
  assert.throws(() => readConfig({ api_key: "secret" }), /environment variables/);
  assert.equal(readConfig({}).provider, "chatgpt");
  assert.throws(() => configureModels(readConfig({})), /Continue with ChatGPT/);
  assert.throws(
    () =>
      configureModels(
        readConfig({
          provider: "openai",
          model: "test",
          api_key_env: "BUCKETS_TEST_MISSING_KEY",
        }),
      ),
    /BUCKETS_TEST_MISSING_KEY/,
  );
  assert.throws(() => readConfig({ index_roots: [{ location: "/fixture" }] }), /respect_gitignore/);
});
test("SIGKILL during a replay-safe tool resumes the real SQLite harness on retry", async () =>
  fixture(async (directory) => {
    const child = spawn(
      process.execPath,
      [new URL("./crash-child.ts", import.meta.url).pathname, directory],
      { stdio: ["ignore", "ignore", "pipe"] },
    );
    let errors = "";
    child.stderr.on("data", (data) => {
      errors += data.toString();
    });
    try {
      let entered = false;
      for (let i = 0; i < 200; i++) {
        try {
          await access(join(directory, "tool-entered"));
          entered = true;
          break;
        } catch {}
        if (child.exitCode !== null) break;
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
      assert.equal(entered, true, errors);
      const exited = once(child, "exit");
      child.kill("SIGKILL");
      await exited;
      const fake = fakeModels(() => [
        {
          type: "text",
          text: JSON.stringify({
            answer: "Resumed after process crash",
            citations: [{ location: "/fixture/a.txt", line: 1 }],
            results: [],
          }),
        },
      ]);
      const runtime = await WorkspaceAI.open(directory, {
        models: fake.models,
      });
      let reads = 0;
      const result = await runtime.invoke(
        request(directory),
        {},
        rpc(async (_, params) => {
          reads++;
          return {
            location: params.location,
            text: "real replayed content",
            truncated: false,
            binary: false,
          };
        }),
      );
      assert.equal(result.answer, "Resumed after process crash");
      assert.ok(reads >= 2);
      assert.equal(fake.count(), 1);
      await runtime.close();
    } finally {
      if (child.exitCode === null && child.signalCode === null) {
        const exited = once(child, "exit");
        child.kill("SIGKILL");
        await exited;
      }
    }
  }));
test("a host cancellation marker aborts an interrupted tool before any hidden replay", async () =>
  fixture(async (directory) => {
    const child = spawn(
      process.execPath,
      [new URL("./crash-child.ts", import.meta.url).pathname, directory],
      { stdio: ["ignore", "ignore", "pipe"] },
    );
    let errors = "";
    child.stderr.on("data", (chunk) => {
      errors += chunk.toString();
    });
    try {
      let entered = false;
      for (let i = 0; i < 200; i++) {
        try {
          await access(join(directory, "tool-entered"));
          entered = true;
          break;
        } catch {}
        if (child.exitCode !== null) break;
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
      assert.equal(entered, true, errors);
      const exited = once(child, "exit");
      child.kill("SIGKILL");
      await exited;
      await mkdir(join(directory, "host-cancelled-requests"));
      await writeFile(
        join(directory, "host-cancelled-requests", "cancelled.json"),
        JSON.stringify({ request_id: "request-1" }),
      );
      const fake = fakeModels(() => [
        {
          type: "text",
          text: JSON.stringify({
            answer: "New question only",
            citations: [],
            results: [],
          }),
        },
      ]);
      const runtime = await WorkspaceAI.open(directory, {
        models: fake.models,
      });
      let callbacks = 0;
      const host = rpc(async () => {
        callbacks++;
        throw new Error("Cancelled tool must not replay");
      });
      await assert.rejects(runtime.invoke(request(directory), {}, host), /cancelled/);
      assert.equal(fake.count(), 0);
      assert.equal(callbacks, 0);
      assert.equal(
        (await runtime.invoke(request(directory, "ask", "new-question"), {}, host)).answer,
        "New question only",
      );
      assert.equal(fake.count(), 1);
      assert.equal(callbacks, 0);
      await runtime.close();
    } finally {
      if (child.exitCode === null && child.signalCode === null) {
        const exited = once(child, "exit");
        child.kill("SIGKILL");
        await exited;
      }
    }
  }));
test("one state directory cannot mix separate workspace identities", async () =>
  fixture(async (directory) => {
    const runtime = await WorkspaceAI.open(directory);
    await runtime.invoke(
      request(directory, "memory"),
      {},
      rpc(async () => ({})),
    );
    const other = request(directory, "memory");
    other.context.workspace_id = "another-workspace";
    await assert.rejects(
      runtime.invoke(
        other,
        {},
        rpc(async () => ({})),
      ),
      /different workspace/,
    );
    await runtime.close();
  }));
test("graceful service shutdown checkpoints pending work for a same-ID reopen", async () =>
  fixture(async (directory) => {
    const child = spawn(
      process.execPath,
      [new URL("./crash-child.ts", import.meta.url).pathname, directory],
      { stdio: ["ignore", "ignore", "pipe"] },
    );
    let errors = "";
    child.stderr.on("data", (data) => {
      errors += data.toString();
    });
    try {
      let entered = false;
      for (let i = 0; i < 200; i++) {
        try {
          await access(join(directory, "tool-entered"));
          entered = true;
          break;
        } catch {}
        if (child.exitCode !== null) break;
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
      assert.equal(entered, true, errors);
      const exited = once(child, "exit");
      child.kill("SIGTERM");
      await exited;
      const fake = fakeModels(() => [
        {
          type: "text",
          text: JSON.stringify({
            answer: "Resumed after graceful close",
            citations: [],
            results: [],
          }),
        },
      ]);
      const runtime = await WorkspaceAI.open(directory, {
        models: fake.models,
      });
      const result = await runtime.invoke(
        request(directory),
        {},
        rpc(async (_, params) => ({
          location: params.location,
          text: "replayed",
          binary: false,
          truncated: false,
        })),
      );
      assert.equal(result.answer, "Resumed after graceful close");
      await runtime.close();
    } finally {
      if (child.exitCode === null && child.signalCode === null) {
        const exited = once(child, "exit");
        child.kill("SIGKILL");
        await exited;
      }
    }
  }));
test("historical admission IDs retain snapshot bindings after newer requests and restart", async () =>
  fixture(async (directory) => {
    const fake = fakeModels(() => [
      {
        type: "text",
        text: JSON.stringify({
          answer: "Snapshot-bound answer",
          citations: [],
          results: [],
        }),
      },
    ]);
    let runtime = await WorkspaceAI.open(directory, { models: fake.models });
    const a = request(directory, "ask", "A");
    await runtime.invoke(
      a,
      {},
      rpc(async () => ({})),
    );
    const b = request(directory, "ask", "B");
    b.context.revision = 2;
    await runtime.invoke(
      b,
      {},
      rpc(async () => ({})),
    );
    assert.equal(fake.count(), 2);
    const changedA = request(directory, "ask", "A");
    changedA.context.revision = 3;
    await assert.rejects(
      runtime.invoke(
        changedA,
        {},
        rpc(async () => ({})),
      ),
      /older workspace snapshot/,
    );
    assert.equal(fake.count(), 2);
    await runtime.close();
    runtime = await WorkspaceAI.open(directory, { models: fake.models });
    await assert.rejects(
      runtime.invoke(
        changedA,
        {},
        rpc(async () => ({})),
      ),
      /older workspace snapshot/,
    );
    assert.equal(
      (
        await runtime.invoke(
          a,
          {},
          rpc(async () => ({})),
        )
      ).answer,
      "Snapshot-bound answer",
    );
    assert.equal(fake.count(), 2);
    await runtime.close();
  }));
test("result display names come from the host and cannot disguise the target", async () =>
  fixture(async (directory) => {
    const fake = fakeModels(() => [
      {
        type: "text",
        text: JSON.stringify({
          answer: "Result",
          citations: [],
          results: [{ location: "/fixture/a.txt", name: "harmless-looking.txt" }],
        }),
      },
    ]);
    const runtime = await WorkspaceAI.open(directory, { models: fake.models });
    const result = await runtime.invoke(
      request(directory),
      {},
      rpc(async (_, params) => ({
        location: params.location,
        name: "a.txt",
        text: "verified content",
        binary: false,
        truncated: false,
      })),
    );
    assert.equal(result.results[0]?.name, "a.txt");
    assert.equal(result.results[0]?.location, "/fixture/a.txt");
    await runtime.close();
  }));
