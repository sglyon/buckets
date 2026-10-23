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

For typed commands, use `commandHandlers([{ descriptor, run }])` with
`servePlugin`. `CommandDescriptor.command_line` optionally declares a name,
aliases, positional arguments, static choice/path completion hints and provider
availability. `run` receives the submitting pane's frozen
`{location, selected, user_args?}` and the normal request context. Arguments stay
separate strings; never join them into shell text. Legacy palette/shortcut calls
may omit `user_args`. See [the command contract](../../docs/plugins.md#command-line-registration)
for examples, bounds and cancellation guarantees.

For duplex services, `ctx.host.call('files.read', {location})` returns the host's
validated result. `ctx.notify('agent.progress', {request_id, message})` reports
legacy status. Add an optional `update` with an increasing `sequence`, full
Markdown `text`, a `phase`, and bounded `activities` for live transcript snapshots.
Snapshots are provisional: only the final validated result can supply citations
and file actions. See [the agent protocol](../../docs/plugin-agent-protocol.md)
for types, limits, coalescing, and cancellation. Only declared host APIs are available; executable plugins are trusted
code and are not OS sandboxed. Do not print logs to stdout; use stderr.

`servePlugin` frames messages, validates IDs/versions and bounds frame sizes.
It keeps reading while handlers await host callbacks. Legacy stdin EOF permits
the one handler to finish. Agent-service EOF aborts request signals, runs
`onClose`, and bounds teardown to two seconds. `$/cancelRequest` aborts the
matching request signal; durable plugins must also record cancellation in their
own state. See `docs/plugin-agent-protocol.md` and the AI plugin for the complete
host protocol and crash/cancel distinction.

From the repository root: `npm ci --ignore-scripts`, `npm run check` and `npm test`.
Pinned project-local Vite+ handles formatting, linting, typechecking, tests and
library packaging. `npm run build` emits Node ESM and SDK declarations;
`npm run format` formats all TypeScript, and `npm run format:check` checks it.
No global Vite+ installation is required. The example manifest remains a normal executable
manifest. `npm pack` packages only compiled SDK files and this README.

The macOS CI job runs the SDK/AI tests and TypeScript checks on Node 24, then
installs the standalone plugin into a fresh runner-temporary configuration and
runs the three opt-in Rust integration tests against that installed package.
Those tests use local model/embedding HTTP fixtures and require no cloud secrets.
