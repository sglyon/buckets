import { test } from "node:test";
import assert from "node:assert/strict";
import { commandHandlers, type CommandRegistration, type RequestContext } from "../src/index.ts";

function context(controller = new AbortController()): RequestContext {
  return {
    id: 1,
    signal: controller.signal,
    host: { call: async <T>() => undefined as T },
    notify: () => {},
  };
}
const envelope = (arguments_: unknown, command = "open") => ({
  protocol_version: 1,
  config: {},
  request: { command, arguments: arguments_ },
});
test("command helpers retain static descriptors, frozen literal argv and legacy context", async () => {
  const descriptor: CommandRegistration["descriptor"] = {
    id: "open",
    title: "Open",
    command_line: {
      name: "open",
      aliases: ["o"],
      locations: ["local"],
      arguments: [{ name: "paths", kind: "path", variadic: true }],
    },
  };
  const handlers = commandHandlers([{ descriptor, run: (args) => args }]);
  assert.deepEqual(await handlers["commands.list"](null, context()), [descriptor]);
  const input = {
    location: "/tmp/space dir",
    selected: ["/tmp/selected"],
    user_args: ["a 'quote' $(touch NO)", "", "line\nbreak"],
  };
  const result = await handlers["commands.invoke"](envelope(input), context());
  assert.deepEqual(result, input);
  assert.notEqual((result as typeof input).user_args, input.user_args);
  assert.deepEqual(
    await handlers["commands.invoke"](envelope({ location: "/tmp", selected: [] }), context()),
    { location: "/tmp", selected: [] },
  );
});
test("command helpers reject malformed argv and counts/choices before plugin code", async () => {
  let called = false;
  const handlers = commandHandlers([
    {
      descriptor: {
        id: "open",
        title: "Open",
        command_line: {
          name: "open",
          arguments: [
            {
              name: "mode",
              kind: "choice",
              required: true,
              choices: ["fast", "full"],
            },
          ],
        },
      },
      run: () => {
        called = true;
      },
    },
  ]);
  for (const user_args of [
    [],
    ["wrong"],
    ["fast", "extra"],
    ["bad\0value"],
    "not-an-array",
    Array(257).fill("fast"),
  ])
    await assert.rejects(
      async () =>
        handlers["commands.invoke"](
          envelope({ location: "/tmp", selected: [], user_args }),
          context(),
        ),
      /Invalid/,
    );
  assert.equal(called, false);
  await assert.rejects(
    async () =>
      handlers["commands.invoke"](
        envelope({ location: "/tmp", selected: [] }, "missing"),
        context(),
      ),
    /Unknown/,
  );
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(
    async () =>
      handlers["commands.invoke"](
        envelope({ location: "/tmp", selected: [], user_args: ["fast"] }),
        context(controller),
      ),
    /cancelled/,
  );
  assert.equal(called, false);
});
test("duplicate stable IDs cannot silently replace a command", () => {
  const entry = { descriptor: { id: "open", title: "Open" }, run: () => null };
  assert.throws(() => commandHandlers([entry, entry]), /Duplicate/);
});
