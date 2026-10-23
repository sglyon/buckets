// Use with `node --experimental-strip-types plugin.ts` on Node 22+, or compile
// with TypeScript. The manifest uses plugin.mjs to avoid build prerequisites.
import { createInterface } from "node:readline";
type Request = {
  jsonrpc: "2.0";
  id: number;
  method: string;
  params: {
    request: { command?: string; arguments?: unknown };
    config: unknown;
  };
};
const reader = createInterface({ input: process.stdin, terminal: false });
for await (const line of reader) {
  const request: Request = JSON.parse(line);
  const response =
    request.method === "commands.list"
      ? {
          result: [
            {
              id: "hello",
              title: "Say hello",
              description: "Example plugin command",
            },
          ],
        }
      : request.method === "commands.invoke" && request.params.request.command === "hello"
        ? {
            result: {
              message: "Hello from a Buckets plugin",
              arguments: request.params.request.arguments,
            },
          }
        : {
            error: {
              code: -32601,
              message: `Unknown method or command: ${request.method}`,
            },
          };
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, ...response }) + "\n");
  reader.close();
  break;
}
