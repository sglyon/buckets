// Test-only process: die while a replay-safe host read is executing.
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { WorkspaceAI } from "../src/runtime.ts";
import { fakeModels, request, rpc } from "./helpers.ts";
const directory = process.argv[2]!;
const fake = fakeModels(() => [
  {
    type: "toolCall",
    id: "crash-read",
    name: "files_read",
    arguments: { location: "/fixture/a.txt" },
  },
]);
const runtime = await WorkspaceAI.open(directory, { models: fake.models });
setInterval(() => {}, 1000);
process.once("SIGTERM", () => {
  void runtime.close().finally(() => process.exit(0));
});
await runtime.invoke(
  request(directory),
  {},
  rpc(async () => {
    await writeFile(join(directory, "tool-entered"), "durable tool started");
    return new Promise(() => {});
  }),
);
