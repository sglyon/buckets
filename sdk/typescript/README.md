# Buckets TypeScript plugin SDK

The SDK supports ordinary one-shot plugins and persistent agent services on the
same versioned JSON-RPC wire. It compiles to JavaScript for installed consumers;
Node does not need to strip TypeScript inside `node_modules`.

```ts
import { servePlugin } from '@buckets/plugin-sdk';

await servePlugin({ handlers: {
  'commands.list': () => [{ id: 'hello', title: 'Say hello' }],
  'commands.invoke': (params, ctx) => ({ message: 'Hello' }),
} });
```

For duplex services, `ctx.host.call('files.read', {location})` returns the host's
validated result. `ctx.notify('agent.progress', {request_id, message})` reports
progress. Only declared host APIs are available; executable plugins are trusted
code and are not OS sandboxed. Do not print logs to stdout; use stderr.

`servePlugin` frames messages, validates IDs/versions and bounds frame sizes.
It keeps reading while handlers await host callbacks. Legacy stdin EOF permits
the one handler to finish. Agent-service EOF aborts request signals, runs
`onClose`, and bounds teardown to two seconds. `$/cancelRequest` aborts the
matching request signal; durable plugins must also record cancellation in their
own state. See `docs/plugin-agent-protocol.md` and the AI plugin for the complete
host protocol and crash/cancel distinction.

From the repository root: `npm ci --ignore-scripts`, `npm run build`, `npm test`,
and `npm run typecheck`. The example manifest remains a normal executable
manifest. `npm pack` packages only compiled SDK files and this README.

The macOS CI job runs the SDK/AI tests and TypeScript checks on Node 24, then
installs the standalone plugin into a fresh runner-temporary configuration and
runs the three opt-in Rust integration tests against that installed package.
Those tests use local model/embedding HTTP fixtures and require no cloud secrets.
