# Buckets plugins

Plugins are ordinary executables speaking one line of JSON-RPC 2.0 on stdin and
stdout. Rust, Python, TypeScript, or any other language works. The host starts a
fresh process for each invocation, sends one request, closes stdin, reads one
response, then cleans up the process. Write logs to stderr. There is no compiler
ABI or Rust-version coupling between the application and a plugin.

The reusable TypeScript SDK is in [`sdk/typescript`](../sdk/typescript). It
provides typed requests/results, JSON-RPC dispatch, duplex host callbacks,
progress, cancellation and bounded shutdown. Its installed entry point is
compiled JavaScript with TypeScript declarations. See its
[README](../sdk/typescript/README.md) for authoring and build instructions.

Agent plugins may additionally declare the `agent` capability to use a persistent
service. This does not change ordinary command/provider/search invocations.
The host supervises the service and exposes bounded read-only workspace tools;
the plugin owns its durable runtime and model/index dependencies. See the
[agent protocol](plugin-agent-protocol.md) and the
[Workspace AI plugin](../plugins/ai/README.md). The latter uses pi-durable,
configurable embeddings and explicit workspace notes.

## Install and configure

Put each plugin in its own directory under the application's configured plugin
directory: `BUCKETS_PLUGIN_DIR` when supplied, otherwise
`$BUCKETS_CONFIG_DIR/plugins`, defaulting to `~/.config/buckets/plugins`.
A plugin directory contains `plugin.json` and its executable or source.
`PluginRegistry::discover` accepts either a directory containing plugin folders or
a single plugin folder. Discovery is deterministic by plugin ID; malformed
manifests, duplicate IDs, and unsupported protocol versions produce errors.

```json
{
  "id": "my-search",
  "name": "My search",
  "protocol_version": 1,
  "executable": "python3",
  "args": ["search.py"],
  "capabilities": ["search"],
  "permissions": {
    "read_roots": ["/home/me/projects"],
    "network": false,
    "write": false
  },
  "timeout_ms": 10000,
  "config": {}
}
```

Executables with path separators and permission roots are resolved relative to
the manifest directory. Bare executable names use `PATH`. Arguments are passed
directly to the executable, never interpolated into a shell. All declared read
roots must exist. The timeout must be between 1 ms and 300 s; default 10 s.

Install only trusted code. Plugins inherit your account's OS permissions. The
manifest is a **host dispatch policy, not an OS sandbox**: the host checks the
declared capability, validates requested search roots and returned paths, and
requires an explicit `write` grant for provider mutations. It cannot prevent a
malicious executable from opening unrelated files or making network requests.
Use an external sandbox/container when you need isolation. Changing plugin
files can change executable behavior on the next invocation.

### User settings and plugin defaults

Open Settings with Command-comma. Preferences live in
`$BUCKETS_CONFIG_DIR/settings.json`, defaulting to
`~/.config/buckets/settings.json`. Plugin configuration belongs in this file;
changing a preference does not rewrite a plugin manifest or executable.

```json
{
  "font_size": 14,
  "show_hidden": false,
  "sidebar": true,
  "preview": true,
  "plugins": {
    "native-tools": {
      "enabled": true,
      "config": { "terminal_target": "embedded", "nvim": "/opt/homebrew/bin/nvim" }
    },
    "git-status": { "config": { "show_ignored": true } },
    "python-search": { "enabled": false }
  }
}
```

An omitted `enabled` means enabled. Disabled plugins remain in the settings
catalog, but command discovery, command execution, search, provider requests,
decorations, and terminal launch requests cannot start their processes. The
backend reads the same saved overrides for plugin filesystem and search calls.

The host recursively merges a plugin's `config` object over its manifest defaults.
Nested object keys merge; arrays and other values replace their default value.
Remove an override key to restore its default. Empty optional executable fields
in the settings form remove that override, allowing the plugin's fallback.
Override configuration must be an object. Unrecognized configuration keys remain
available through Advanced JSON, including model and vector-store settings.

Settings use strict JSON without comments, bounded to 1 MiB. Unknown application
and plugin-setting keys survive saves. The editor validates application fields
and declared plugin fields, retains last-good preferences on invalid reloads,
and exposes malformed source for deliberate JSON repair. Saving uses an atomic
replacement and checks the last-read bytes to reject conflicting external edits.
Use **Reload from disk** after editing the file externally; dirty JSON drafts
require an explicit choice before being discarded. Settings files reached through
a symlink can be read, but writes are refused to preserve the link: edit the
target externally and reload. Preferences migrate from workspace state only
when `settings.json` is absent.

### Optional settings metadata

A manifest can declare generic top-level configuration fields. This metadata is
optional; existing plugins and arbitrary advanced configuration remain supported.

```json
{
  "config": { "terminal_target": "embedded", "show_details": true, "limit": 100 },
  "settings_fields": [
    { "key": "terminal_target", "title": "Terminal target", "kind": "select",
      "options": ["embedded", "iterm2", "external"] },
    { "key": "show_details", "title": "Show details", "kind": "bool" },
    { "key": "limit", "title": "Result limit", "kind": "number", "min": 1, "max": 1000 },
    { "key": "shell", "title": "Shell executable", "kind": "text",
      "description": "Optional; uses the plugin's default shell when unset." }
  ]
}
```

Kinds are `bool`, `text`, `number`, and `select`. `description`, numeric `min`/`max`,
and select `options` are optional metadata except that a select must have choices.
Missing optional config keys remain unset; metadata does not invent defaults.
The host validates known values and bounds labels, options, and field count;
it does not implement JSON Schema or interpret dotted keys as nested paths.

Rust hosts can use `discover_with_settings(directory, &overrides)` or
`discover_with_settings_file(directory, settings_path)`. `with_settings` reapplies
overrides from manifest defaults without launching a process. `plugins()` exposes
the complete catalog, `default_config` retains original defaults, `config` contains
effective values, and `is_enabled(id)` reports runtime availability. Plain
`discover(directory)` remains isolated from ambient settings.

## Wire protocol

The host sends one UTF-8 JSON line, at most 1 MiB:

```json
{"jsonrpc":"2.0","id":1,"method":"search","params":{"protocol_version":1,"config":{},"request":{"query":"router","roots":[{"path":"/home/me/projects/app","respect_gitignore":true,"follow_symlinks":false}],"limit":100,"mode":"filename","config":{}}}}
```

`params.config` is the effective manifest configuration after saved user overrides.
`params.request.config`
is optional invocation-specific configuration. The host preserves both; a plugin
must decide and document how they merge. Either can be JSON null. Configuration
is untrusted input and should be validated.

Respond with exactly one line, at most 8 MiB, and the matching request ID:

```json
{"jsonrpc":"2.0","id":1,"result":[{"path":"/home/me/projects/app/router.rs","score":0.92,"line":18,"snippet":"fn route(request: Request)","kind":"file"}]}
```

Or return an error:

```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"Embedding model is unavailable"}}
```

The host rejects malformed JSON, mismatched IDs, invalid versions, multiple
response lines, or responses containing both `result` and `error`. Nonzero exits
include up to 16 KiB of stderr in the error. Excess output is rejected. Timeouts
kill the plugin; on Unix the host kills its process group, including subprocesses.
Escaping process groups is outside the trust boundary. Host calls are synchronous
and must run on a worker thread in a graphical application.

## Search capability

`search` takes `SearchRequest` and returns an array of `SearchResult`. Modes are
`filename`, `content`, `semantic`, and `hybrid`. A plugin should reject modes it
does not support. Limits range from 1 to 10,000. Returned paths must exist and
remain within a requested root after symlink resolution. Scores must be finite,
and optional line numbers are one-based. The host sorts by descending score,
then path and line for deterministic ties.

Each root independently carries `respect_gitignore` and `follow_symlinks`.
Those are part of the search contract; a plugin must honor them or return a clear
error. The host validates paths but does not reimplement an indexer's traversal.
Honor `.gitignore` negation, nested files, `.git/info/exclude`, and global excludes
when your indexer supports Git semantics. Avoid silently indexing ignored files.

The dependency-free Python example uses `git ls-files --cached --others
--exclude-standard` when ignore rules are requested. It requires a Git worktree
for this mode and rejects symlink following. Its alternate `os.walk` traversal
can search outside a worktree with ignore rules disabled.

### Real FFF integration

FFF is a search SDK, rather than a shell `fff` command. The example at
`examples/plugins/fff-search` calls the upstream Python `FileFinder` SDK for
fuzzy filename scoring and indexed content search. Set up its dependencies:

```sh
uv venv .venv --python 3.13
uv pip install --python .venv/bin/python -r examples/plugins/fff-search/requirements.txt
```

Its shipped manifest uses the repository's `.venv` and grants only the repository
root. When copying it elsewhere, update `executable` and `permissions.read_roots`
to absolute paths. It follows FFF's ignore policy and rejects requests to include
ignored files. It supports `filename` and `content` modes. This invocation-scoped
protocol rebuilds FFF's index per request; persistent warm-index performance is
not claimed. A future protocol extension can add long-lived sessions without
changing the search request/result types.

Upstream documentation: [FFF SDK](https://github.com/dmtrKovalenko/fff) and
[Python API](https://github.com/dmtrKovalenko/fff/blob/main/_autodocs/api-reference/fff-python.md).

### Semantic and hybrid indexers

An embedding plugin can use the same capability and return `semantic` or
`hybrid` results. Keep the embedding model, storage, and indexing policy inside
that plugin. Suggested manifest configuration (a contract example, not an
implemented embedding service):

```json
{
  "model": {"provider":"local", "name":"my-embedding-model", "dimensions":768},
  "vector_store": {"type":"sqlite", "path":"/home/me/.cache/buckets/vectors.db"},
  "roots": [{"path":"/home/me/projects", "respect_gitignore":true, "follow_symlinks":false}],
  "chunking": {"max_tokens":512, "overlap_tokens":64},
  "ranking": {"keyword_weight":0.4, "semantic_weight":0.6}
}
```

Treat invocation roots as an upper bound: intersect configured indexed roots
with requested roots and read grants. Recheck ignore rules before returning
stored hits, remove deleted/ignored files from the index, and store hashes or
mtimes for incremental updates. Version indexes by model/dimensions/chunking
settings. Define deterministic ranking, cancellation and bounded query latency.
Use command capability actions such as `index.rebuild`, `index.status`, and
`index.remove` for explicit maintenance. Store secrets outside the manifest and
resolve them through your chosen credential mechanism. Neither the host nor the
example sends files to an embedding service.

## Commands capability

`commands.list` receives null and returns an array of
`{id,title,description,shortcut?,command_line?}`. Shortcut strings use the application's GPUI
key syntax, such as `ctrl-e` or `alt-enter`. Shortcuts are suggestions: the
application resolves collisions and reserves its essential navigation keys.
The host bounds descriptors (256 commands, 128-character titles, 512-character
descriptions) and rejects duplicate or invalid IDs. `commands.invoke` receives
`{command,arguments}` and returns any JSON value. The host requires the
`commands` capability. The TypeScript/JavaScript and Rust examples implement a
real `hello` command. The Rust SDK's `serve_once` function handles wire framing:

```rust
use buckets_plugin::{RpcError, serve_once};
use serde_json::json;

fn main() -> anyhow::Result<()> {
    serve_once(|method, _params| match method {
        "commands.list" => Ok(json!([])),
        _ => Err(RpcError { code: -32601, message: "Unknown method".into() }),
    })
}
```

Build the standalone Rust example with
`cargo build --release --manifest-path examples/plugins/rust-search/Cargo.toml`.
The JavaScript manifest runs the `.mjs` counterpart directly with Node 18+;
`plugin.ts` provides the typed version for a TypeScript build pipeline.

### Command-line registration

The optional `command_line` object registers static names and positional
arguments for Buckets' `:` command line. Existing descriptors, palette entries,
shortcuts and arbitrary legacy `arguments` remain compatible. Every installed
command has a qualified `plugin:<plugin-id>/<command-id>` name; registration
adds short names and aliases. Built-in names remain reserved. Ambiguous aliases
never silently choose a plugin: use the qualified name. Disabled plugins cannot
be invoked.

```json
{"id":"open_nvim","title":"Open in Neovim","command_line":{
  "name":"nvim","aliases":[],
  "arguments":[{"name":"paths","kind":"path","variadic":true}],
  "locations":["local"],"unavailable_reason":"Neovim requires a local directory"
}}
```

Argument kinds are `text`, `path`, `directory`, and `choice`. `required` defaults
to false; required arguments precede optional ones. Only the final argument may
be `variadic`. Choice arguments require nonempty unique `choices`; other kinds
omit them. Path/directory hints let the host complete cached current-directory
entries; nested directory prefixes are entered manually in this version.
Completion requires the caret at the input's end. Choices are static literal
candidates. No per-key plugin RPC is used. Static
`locations` restrict provider support to `local`, `ssh`, `s3`, `git` or `plugin`;
omitted/empty accepts every provider. This does not probe network reachability
or executable presence.

Names and aliases contain 1–64 ASCII letters, digits, `-`, `_` or `.`. There are
at most 16 aliases and 32 argument definitions per command. Repeated names
within one plugin are rejected. Each choice has at most 256 characters with no
control characters, and an argument has at most 128 choices. Unknown metadata
fields, invalid order and malformed descriptors fail discovery with an error.

Submission retains the normal `commands.invoke` request, adding literal argv:

```json
{"command":"open_nvim","arguments":{"location":"/home/me/project","selected":["/home/me/project/current.rs"],"user_args":["src/main.rs","a file.rs"]}}
```

`location` and `selected` are frozen from the submitting pane; later navigation
does not retarget the request. Palette/shortcut calls may omit `user_args`.
The host validates counts and choices before starting a command-line plugin.
At most 256 arguments, 32 KiB each and 128 KiB combined are accepted; NUL is
rejected. Do not join/reparse arguments as shell text. Return ordinary JSON such
as `{"message":"Index rebuilt"}` or an existing terminal/agent host action;
host actions retain their permission checks.

TypeScript authors can use the SDK helper:

```typescript
import { commandHandlers, servePlugin } from '@buckets/plugin-sdk';
await servePlugin({ handlers: commandHandlers([{
  descriptor: { id: 'index', title: 'Rebuild index', command_line: {
    name: 'reindex', arguments: [
      { name: 'mode', kind: 'choice', choices: ['changed', 'all'] },
    ],
  } },
  run: async ({ location, user_args = [] }, context) => {
    context.signal.throwIfAborted();
    return { message: `Requested ${user_args[0] ?? 'changed'} indexing at ${location}` };
  },
}]) });
```

Python reads `params["request"]["arguments"].get("user_args", [])` and passes
values separately to subprocess argv. Rust authors can deserialize `CommandContext`
and read `user_args` from the arguments object; the SDK exports
`CommandDescriptor`, `CommandLineSpec`, `CommandArgument` and their enums.
Rust `serve_once` and generic TypeScript handlers remain compatible.

`PluginRegistry::invoke_command_line_result` validates a cached descriptor,
frozen `CommandContext` and argv with an `AtomicBool` cancellation token.
`invoke_command_result_cancellable` provides process cancellation for legacy
commands. Cancellation stops/reaps the invocation process and group; the UI
rejects cancelled or stale results before creating a pane. It cannot undo effects
already performed, including an external app already launched. One-shot process
cancellation does not promise an SDK `AbortSignal`; persistent SDK handlers can
also receive `$/cancelRequest`. A terminal already handed to the host has its own
PTY lifetime.

### Developer tools in your terminal

`examples/plugins/native-tools` adds three embedded-terminal commands and three
explicit iTerm2 alternatives:

| Command | Suggested shortcut | Behavior |
| --- | --- | --- |
| Open in Neovim | Ctrl E | Open selected local files in an embedded Neovim pane |
| Open Lazygit | Ctrl G | Open an embedded Lazygit pane at the active local directory |
| Open terminal here | Alt Enter | Start an embedded shell at the active local directory |
| Open Neovim / Lazygit / terminal in iTerm2 | Palette | Explicit external alternatives |

The application passes command arguments as `{location,selected}`. `location`
is the active pane's location string; `selected` is an array of selected file
location strings. The example also accepts legacy `selection`. Remote URLs
are rejected with a useful message; mount the filesystem before opening local
developer tools against it.

Command-line registrations add `:nvim [paths…]`, `:lazygit [directory]` (alias
`:git`), and `:terminal [directory]` (alias `:term`). These tools remain plugin
commands; only `split`, `cd`, `rename` and `settings` are built-in verbs.
`:plugin:native-tools/open_terminal [directory]` explicitly selects this plugin.
`nvim-iterm2`, `lazygit-iterm2` and
`terminal-iterm2` retain the external option. Relative arguments resolve against
the frozen local directory. Neovim accepts new file paths and falls back to the
selection without arguments; Lazygit/shell accept one existing directory. These
are path positionals, not executable options. The plugin commands are local-only,
independently of the host's shell-command provider support.

The default target is the app's embedded terminal. The Python plugin chooses the
program, arguments and directory; the host owns the PTY and renders its output.
The plugin does not start the embedded child process. Neovim and Lazygit remain
plugin behavior rather than hardcoded application tools.

```json
{"terminal_target":"embedded","terminal_bundle_id":"com.googlecode.iterm2","nvim":"nvim","lazygit":"lazygit"}
```

Set `terminal_target` to `external` to make the three shortcut commands use the
configured external terminal. The explicit `_iterm2` command IDs always select
iTerm2, independently of that default. To use Apple's Terminal for configured
external launches, set `terminal_bundle_id` to
`com.apple.Terminal`. Alternatively omit that key and set `terminal_app` to a
macOS application name. Launch Services opens a private executable `.command`
file in the specified app. iTerm2 registers these scripts in its installed
application metadata. Paths and executable arguments are quoted using Python's
`shlex`; no path is concatenated into an unquoted shell command or AppleScript.
The script sets the directory, removes itself, then runs the tool. Application
launch happens through the OS and survives plugin-process cleanup. Neovim and
Lazygit must already be installed. The plugin searches the current PATH and, on
macOS, Homebrew's standard directories. Absolute paths may be configured.

For Linux or a terminal with its own CLI, set an argv template instead:

```json
{"terminal_target":"external","terminal_argv":["x-terminal-emulator","-e","{script}"],"shell":"/bin/bash"}
```

`{script}` is required and `{cwd}` is optional. Each template element remains
one argument, preserving spaces and quotes. The terminal launches in a new
process session. The plugin does not install tools or modify terminal profiles.

### Typed terminal host actions

Command results without a `host_action` field keep their existing arbitrary JSON
behavior (`CommandResult::Value`). A plugin can request an embedded terminal by
returning this explicit result envelope:

```json
{"host_action":"terminal","request":{"program":"/usr/bin/env","args":["python3","-i"],"cwd":"/home/me/projects","title":"Python","env":{"PYTHONUNBUFFERED":"1"}}}
```

Declare both `commands` and `terminal` capabilities and grant
`permissions.terminal:true`. The host's `invoke_command_result` returns
`CommandResult::Terminal(TerminalLaunchRequest)` after validation. The original
`invoke_command` JSON API remains available and applies the same validation.

`program` must be an existing absolute file path, and `cwd` an existing absolute
directory within a declared read root. Arguments remain an argv array; they are
never reparsed as a shell command. Environment overrides apply to the new child,
not the application. The host rejects NUL bytes, invalid environment names,
more than 1,024 arguments, more than 64 environment variables, titles longer
than 128 characters, and combined arguments/environment above 128 KiB. Individual
arguments and environment values are capped at 32 KiB. Unknown host actions
produce a clear error. The terminal engine performs process creation and owns
process lifetime after plugin exit.

These are additional dispatch grants, not a sandbox: installed plugin code
already has its account's OS permissions. An embedded shell is an intentional
interactive program. Its commands receive the user's ordinary shell privileges.

## Row decorations capability

Plugins with `capabilities:["decorations"]` implement `decorations` and receive:

```json
{"location":"/home/me/project","entries":["/home/me/project/src","/home/me/project/main.rs"]}
```

Return generic row annotations, independent of filesystem-provider types:

```json
[{"path":"/home/me/project/main.rs","badge":"M","tooltip":"Git: unstaged modified","tone":"warning"}]
```

`tone` is one of `neutral`, `accent`, `success`, `warning`, or `danger`.
The host rejects unrequested/duplicate paths, more annotations than requested
entries, badges longer than eight characters, tooltips longer than 512
characters, and invalid control characters. Requests contain at most 10,000
entries. Return paths in the exact string format you received. Empty arrays are
valid outside a plugin's applicable context. The app requests decorations on a
background worker and should discard results from stale directory generations.

`examples/plugins/git-status` is a complete Git implementation. It uses
`git status --porcelain=v1 -z` to preserve spaces, quotes, newlines, and renamed
paths; handles staged/unstaged changes, conflicts, renames, and untracked files;
and aggregates nested changes onto visible directory rows. Ignored files are
omitted by default, or shown with a neutral `!` when `config.show_ignored` is
true. It returns no annotations outside a local Git worktree. All Git execution
stays in the plugin. The UI knows only the generic annotation contract.

For a development run with the supplied trusted examples:

```sh
BUCKETS_PLUGIN_DIR="$PWD/examples/plugins" cargo run -p buckets-app
```

The desktop app bootstraps the first-party Git status and developer-tools
plugins into its default user plugin directory on first use, preserving existing
plugin files. A known unchanged first-party v1 native-tools source can migrate to
the embedded-terminal version while retaining user configuration. Edited source
files remain untouched; the app reports an actionable upgrade notice instead.
The immutable `legacy-v1.py` file is a migration fingerprint, not a runnable
registered plugin. These trusted copies grant `/` as their read root so commands can
follow user-selected directories and external drives. An explicit
`BUCKETS_PLUGIN_DIR` uses only that directory and bypasses bootstrap. FFF remains
an optional separately installed dependency. When manually copying examples,
change their repository-relative `permissions.read_roots` to existing absolute
directories. Use Reload plugins in the palette after editing manifests.
Installing a plugin authorizes its executable to run with your account
permissions, as described above.

## Account capability

The optional `accounts` capability adds generic account connection, selection,
model discovery and sign-out controls in Settings. Credentials use a private
Rust-owned store rather than plugin settings. Provider-specific authorization
stays in the executable plugin. See [the account protocol](plugin-accounts-protocol.md)
for callbacks, cancellation, public state and the credential lease contract.

## Provider capability

Provider requests use methods `provider.list`, `provider.stat`, `provider.read`,
`provider.write`, `provider.mkdir`, `provider.copy`, `provider.move`, and
`provider.remove`. Providers require `provider` capability plus either a declared
read root or `network:true`. Mutating methods additionally require `write:true`.

| Method | Request | Result |
| --- | --- | --- |
| list | `{path}` | `[{name,is_dir,is_symlink,size,modified}]` |
| stat | `{path}` | `{name,is_dir,is_symlink,size,modified}` |
| read | `{path,max_bytes}` | `{data_base64,truncated}` or `{text,truncated}` |
| write | `{path,data_base64,text?,create_new:true}` | any JSON success value |
| mkdir | `{path}` | any JSON success value |
| copy / move | `{source,destination,no_replace:true}` | any JSON success value |
| remove | `{path,trash:true}` | any JSON success value |

`data_base64` carries standard base64-encoded bytes and takes precedence over `text`. Use it for binary files and invalid UTF-8. The host includes `text` on write only when the bytes are valid UTF-8, for compatibility with text-only plugins. Reject malformed base64, unsupported encodings, or oversized requests; never replace file data with a display label. Return `truncated:true` whenever a read is incomplete. The host allows incomplete reads for previews and refuses them for transfers. Responses remain limited to 8 MiB and requests to 1 MiB, including base64 overhead; return a clear error when a transfer exceeds your protocol capacity. Streaming large plugin files requires a future protocol extension.

`modified` is optional Unix seconds. Provider paths belong to the provider's
namespace. Providers must validate their own paths, credentials, limits and
write policies. A write that requests `create_new` or `no_replace` must never
overwrite existing data. Reject trash requests when the backend cannot perform
recoverable deletion. The host does not silently downgrade to permanent removal.

The executable `examples/plugins/local-provider` implements an actual read-only
provider rooted at the repository. Open `plugin://local-provider/` after loading
the example registry. It denies traversal and escaping symlinks and supports
bounded text previews. It refuses all mutations. This is a complete runnable
reference for the provider protocol, without remote service credentials.

The example's read protocol is text-only: it rejects NUL-containing or invalid
UTF-8 data with an error. It never substitutes labels or replacement characters
for file bytes. A truncated preview may omit an incomplete final UTF-8 character
and always carries `truncated:true`; it must not be used as a complete file copy.
