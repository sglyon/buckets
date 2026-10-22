// Executable JavaScript counterpart to plugin.ts. Node 18+; no package install.
import { createInterface } from 'node:readline';
const reader = createInterface({ input: process.stdin, terminal: false });
for await (const line of reader) {
  const request = JSON.parse(line);
  let result;
  let error;
  if (request.method === 'commands.list') {
    result = [{ id: 'hello', title: 'Say hello', description: 'Example plugin command' }];
  } else if (request.method === 'commands.invoke' && request.params.request.command === 'hello') {
    result = { message: 'Hello from a Buckets plugin', arguments: request.params.request.arguments };
  } else {
    error = { code: -32601, message: `Unknown method or command: ${request.method}` };
  }
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, ...(error ? { error } : { result }) }) + '\n');
  reader.close();
  break;
}
