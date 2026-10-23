# Working on Buckets

## Start here

Buckets is a Rust + GPUI desktop file browser, macOS first. Keep the core
portable; other platforms need their own verification. Read [README](README.md),
[product scope](PRODUCT.md), [design rules](DESIGN.md) and
[architecture](docs/architecture.md) before changing their respective areas.

- You are not alone in the repository. Inspect the working tree, preserve other
  contributors' edits, and coordinate shared files, builds and Git index writes.
  Do not reset/revert unrelated work or change the user's Git identity.
- Keep changes within the requested scope. Prefer existing domain interfaces,
  small modules and dependencies already present; do not add frameworks or
  orchestration requirements to solve a local change.
- No external agent skill, personal hook, local orchestration directory or
  screenshot archive is required to contribute. `target/`, `.omx/` and ignored
  `.impeccable/review/` evidence may be absent in a fresh clone. Checked-in
  `.impeccable/surfaces/` briefs supplement DESIGN.md; source owns behavior.
- Do not sign commits. Use `git -c commit.gpgsign=false commit ...` when a commit
  is requested; preserve existing author configuration.

## Setup and commands

Use current stable Rust with rustfmt/Clippy and macOS developer tools with a
working SDK. GPUI uses Metal; this app enables `runtime_shaders` in
`crates/buckets-app/Cargo.toml`. Python 3.9+ is needed for bundled developer-tools
and Python-backed acceptance fixtures. Use Node.js 24 LTS (24.11+) and npm for
TypeScript development; Vite+ also supports Node 22.19+ on the 22.x line and
Node 26+. CI uses Node 24. Install `ripgrep` (`rg`) and `fd` for the full Rust
acceptance suite (`brew install ripgrep fd` on macOS); CI installs both explicitly.
Optional integrations additionally need their own tools (SSH, AWS CLI, Neovim,
Lazygit or FFF).

Run commands from the repository root. Keep one default Cargo `target/` cache
and use these settings for **every Cargo invocation**, including scripts:

```sh
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0

scripts/dev                       # Build and run, forwarding location arguments
scripts/bundle-macos              # Build/sign target/debug/Buckets.app on macOS
scripts/bundle-macos release      # Optimized local bundle
scripts/check                     # fmt check, workspace tests, app check, strict Clippy
```

Do not create per-agent `CARGO_TARGET_DIR` caches or run concurrent Cargo gates.
Use focused tests while editing, then the relevant wider gate once sources are
settled. Avoid unnecessary rebuilds and large benchmark fixtures. Remove only
owned disposable outputs; preserve shared dependency caches and user-adopted apps.

```sh
cargo test -p buckets-core
cargo test -p buckets-plugin --test commands_acceptance
cargo test -p buckets-app --test command_line_acceptance
cargo clippy --workspace --all-targets -- -D warnings

npm ci --ignore-scripts
npm run check
npm test
```

`scripts/check` covers Rust. Pinned project-local Vite+ owns TypeScript tooling;
no global `vp` installation is required. `npm run check` runs formatting, lint
and type checks; `npm test` runs Vitest through Vite+. Both build the workspaces
first. `npm run build` packages the SDK (including declarations) before the AI
plugin using Vite+ tasks and `vp pack`; production dependencies stay external.
TypeScript remains a development dependency for SDK declaration generation.
`npm run format` formats all TypeScript source, tests, examples and configs;
`npm run format:check` only verifies formatting, and `npm run typecheck` only
checks types. Root `vite.config.ts` owns formatting/lint policy: 100-column lines,
preserved import order, and no generated outputs or dependencies. Package Vite
configs own library output and test discovery. Keep real Node/SQLite behavior
and all existing assertions when editing tests.
After `npm run build`, use `npm test --workspace @buckets/plugin-sdk` for a focused
SDK run, or `npm test -- --project workspace-ai` for AI tests. See
[CI](.github/workflows/ci.yml) for installed-plugin checks. Do not blindly enable
ignored tests: optional tools and installed AI-plugin fixtures require explicit
setup described in [verification](docs/verification.md) and the relevant test.
For installed AI checks, `scripts/install-ai-plugin --config-dir <owned-config>`
builds/installs the standalone plugin and refuses an existing destination. Point
`BUCKETS_AI_E2E_PLUGIN_DIR` at that config's `plugins` directory; CI demonstrates
the installed-plugin test targets against local service fixtures.

## Architecture and interfaces

| Area | Responsibility |
| --- | --- |
| `crates/buckets-core` | Typed `Location`, provider/backend operations, search, pane state, bounded operation plans/journal/recovery; no GPUI dependency |
| `crates/buckets-plugin` | Versioned JSON-RPC, manifests/capabilities, bounded subprocesses, typed command/terminal/agent/account contracts |
| `crates/buckets-terminal` | Alacritty state, PTY process lifetime, terminal input and GPUI grid |
| `crates/buckets-app` | Views, focus, action routing and asynchronous orchestration; keep pure models separate from rendering |
| `sdk/typescript` | Reusable executable-plugin SDK, typed command handlers and duplex service framing |
| `examples/plugins` | Rust/Python/TypeScript examples; native-tools and Git decorations remain plugin behavior |
| `plugins/ai` | Model providers, pi-durable conversations/memory, retrieval/indexing; keep these dependencies and policies out of the Rust core |

In the app, settings persistence/model, Settings view and section geometry are
separate. `command_line.rs` is pure parsing/path/history logic;
`command_line_view.rs` adapts input and `command_line_jobs.rs` owns background
plugin/PTY delivery. Pane view, Places and named-workspace models retain their
own identities. Agent presentation, orchestration and context admission live in
`agent_view.rs`, `agent_session.rs` and `agent_context.rs` respectively. Follow
these boundaries instead of adding more policy to `main.rs`.

Preserve full provider URLs, opaque settings fields and stable action IDs.
Protocol additions must remain compatible with existing plugins. Commands pass
literal argv and frozen pane/selection context; only explicit `:!` is shell text.
Static completion metadata must not cause plugin IPC on every keystroke. See
[plugin authoring](docs/plugins.md) and [agent protocol](docs/plugin-agent-protocol.md).
Capability grants constrain host dispatch; executable plugins are not OS sandboxes.
Bundled-source upgrades must preserve user-edited plugins/configuration.

## Safety, responsiveness and input

- Application-managed file mutations use the existing operation-plan/journal path and provider
  capabilities. Preserve no-overwrite, symlink/descendant, cancellation, uncertain
  outcome and guarded recovery checks. Never represent unsupported remote Undo
  as available. Arbitrary shell effects are outside journal Undo.
- Keep provider I/O, copying/hashing, indexing, plugin calls and process startup
  off the UI thread. Bound work/memory and associate completion with the original
  pane/location/revision/request. Cancelled or stale results must not create a
  pane, retarget an operation or steal focus.
- Preserve Tokyo Night defaults and semantic tokens in `appearance.rs`/`theme.rs`.
  Use the existing native components and compact layout. All controls need a
  complete keyboard path, distinct focus/selection/active state and useful errors.
- Native text editing, clipboard/undo, Settings/recorder input, IME composition
  and raw terminal keys retain ownership. Space is Quick Look; `x` marks files.
  Slash filtering and colon commands must not intercept terminal/editor input.
- Settings/history/workspace writes validate before private atomic persistence
  and retain drafts/last-good values on failed or conflicting saves.
- Use `buckets_plugin::ExclusiveFileLock` for production file leases. It explicitly
  unlocks on return or shutdown; closing a descriptor alone can let forked children
  prolong a completed lease. Callers still open and validate the lock file.

## Isolated verification

For manual QA, use owned temporary files and explicit configuration/plugin roots;
do not launch a test against someone's normal configuration or accounts:

```sh
BUCKETS_QA_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/buckets-qa.XXXXXX")
mkdir -p "$BUCKETS_QA_ROOT/config" "$BUCKETS_QA_ROOT/plugins" "$BUCKETS_QA_ROOT/files"
BUCKETS_CONFIG_DIR="$BUCKETS_QA_ROOT/config" \
BUCKETS_PLUGIN_DIR="$BUCKETS_QA_ROOT/plugins" \
  scripts/dev "$BUCKETS_QA_ROOT/files"
```

An explicit empty plugin directory disables automatic bundled-plugin installation.
Add only required fixture plugins, with read roots limited to owned test files.
Use offline/local service fixtures; real authentication, cloud requests or user
file operations need explicit task authorization. Trash/recovery tests must use
isolated trash/HOME fixtures or an explicitly supplied test trash store, never
the real user's Trash. Do not replace the shell's HOME globally.

Run meaningful tests for changed behavior, and report exact commands/results and
remaining limits. UI work needs actual native interaction checks at desktop
size and 960×640 with 24-point text where relevant. Screenshots establish visible
states, not key routing, persistence or live-provider behavior. Offline adapter
fixtures do not certify a real SSH/S3 service; no blanket performance, IME or
accessibility claim follows from a passing Rust gate. Keep proof local/ignored
where appropriate and write a reproducible text account of what was verified.
