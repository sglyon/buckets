# Agent service protocol (version 1)

Agent plugins opt in with `"capabilities":["commands","agent"]`. Existing
one-shot plugins remain unchanged. Their ordinary command invocation may return
`{"host_action":"agent","request":{"mode":"ask","prompt":""}}` to open
an agent pane. Modes are `ask`, `search`, `memory`, `index` (explicit reindex).

The host uses `PluginRegistry::start_agent(id)` to launch the same executable as
a persistent duplex JSONL service. Keep one service per plugin/workspace (open or reuse its existing pane);
serialize invocations and keep its database owned by this process alone. The Rust host holds an advisory lock on the stable `.host-service.lock` inode in storage; it is never unlinked. A second live owner fails fast.
On Unix the storage leaf must be a real directory owned by the current user;
the host opens it without following symlinks and enforces mode 0700 before
the plugin creates databases, indexes, or workspace notes.
Cancellation, timeout, fatal protocol errors, disabling a plugin, or dropping a
pane terminate and reap the service process group. A later request starts a new
service, reopening durable state. Ordinary command discovery still closes stdin
and expects one response. EOF must stop the TypeScript service.

Host request (numeric host IDs; callback IDs are plugin-owned strings):

```json
{"jsonrpc":"2.0","id":1,"method":"agent.invoke","params":{"protocol_version":1,"config":{},"request":{"request_id":"unique-admission-id","mode":"ask","prompt":"Explain these files","storage_dir":"/absolute/private/plugin-state","context":{"workspace_id":"stable-id","revision":1,"panes":[{"id":"1","location":"/work/project","selected":[],"cursor":null,"entries":[],"capabilities":["list","read","search"]}]}}}}
```

All `location` values are opaque provider locations (absolute local paths or full
SSH/S3/Git/plugin URLs). Entries have `location`, `name`, `is_dir`. The bounded
snapshot carries no terminal output or credentials. For memory mode,
`memory_update` omitted means read; a present string explicitly replaces memory,
including an empty string to clear it. `prompt` is not implicit memory.

While invoking, the plugin can issue these host requests:

- `workspace.snapshot`, params `{}` returns the invocation's revisioned snapshot.
- `files.list`, params `{location,limit?}` returns `{entries,truncated}` with
  entries `{location,name,is_dir,size}`.
- `files.read`, params `{location,max_bytes?}` returns
  `{location,name,text,truncated,binary}`. Location and name are authoritative
  canonical provider values. Explicit reads may open a known ignored
  file; indexing must enumerate through `files.search` with ignore policy.
- `files.search`, params `{location,query,engine?,limit?,respect_gitignore?,files_only?,plugin?,mode?}`
  returns `{results,truncated}` with results `{location,name,is_dir,line?,snippet?,score?}`. Engines `auto`, `builtin`, `fd`, `rg`, `fff`;
  empty query enumerates candidates under the root's ignore rules. `plugin`
  optionally selects an installed search plugin; `mode` then selects
  `filename`, `content`, `semantic`, `hybrid`. Existing search plugins currently
  require local roots. Provider features unsupported by the backend return an
  explicit error. Root ignore policy cannot be relaxed by a plugin request; overlapping grants use the strictest matching ignore policy.
  `files_only:true` filters regular files before the result limit for builtin,
  auto, fd and rg searches on local/SSH roots. Use it for index enumeration so
  directory entries cannot consume the file budget. FFF and generic search
  plugins do not currently support this option and return an explicit error.

Callbacks use e.g. `{"jsonrpc":"2.0","id":"plugin-1","method":"files.read","params":{"location":"/work/project/a.txt"}}`.
The host replies with matching string ID and exactly one result/error. Errors
are JSON-RPC `{code,message}`. Only the four host methods above are dispatched.
There is no arbitrary shell or mutation tool. Host grants are explicit roots;
local references undergo canonical containment, remote references retain exact
host/bucket/endpoint/revision identity and component-boundary containment. SSH content tools and returned references also receive server-side realpath checks; credential-bearing S3 endpoints are rejected before entering context.
Remote directory listing may include symlink metadata; that does not grant
access to the target. A later read or final reference still requires validation.

Optional progress notifications are
`{"jsonrpc":"2.0","method":"agent.progress","params":{"request_id":"unique-admission-id","message":"Reading files"}}`.
These are progress messages, not a token-streaming claim.

Final response result:

```json
{"answer":"Answer with citations","citations":[{"location":"/work/project/a.txt","line":1,"label":"a.txt:1"}],"results":[{"location":"/work/project/a.txt","name":"a.txt","is_dir":false,"line":1,"snippet":"Matched text","reason":"Why it matches","score":0.8}],"memory":"Explicit workspace notes"}
```

All fields have empty defaults. Line numbers are positive, scores
finite, references must pass the Rust broker before the app accepts them. The
result pane accepts files only. Every result name must match its authorized
canonical location; local and SSH file kinds are checked by the host rather
than accepted from a model. Other providers retain their provider read semantics.
The
host rejects malformed/mismatched frames and bounds data, frame count, and total
invocation duration. A watchdog durably records explicit cancellation in
`storage_dir/host-cancelled-requests/*.json` (each file contains `{request_id}`),
then sends `agent.cancel` and gives the process group two seconds to stop before
SIGKILL. The plugin must read these retained markers before resuming durable
work and abort the matching admissions. A crash or timeout does not create a
user-cancellation marker. The watchdog runs independently of host callbacks;
an already running read-only provider call may finish later under its existing
timeout, while the model service stops promptly. The host never reaps the child
until its watchdog has joined, preventing process-ID reuse during teardown.
Run service and provider work off the UI thread.

The manifest permission policy is dispatch control, not an OS sandbox. Trusted
plugins still inherit the user's account permissions. Read-only host tools do
not make a malicious executable safe. Durable admission IDs deduplicate agent
submissions, not external filesystem effects.
