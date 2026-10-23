# Architecture

Buckets separates the native application from storage and extension logic:

- `buckets-core` models locations, entries, panes, filesystem operations and search. It does not depend on GPUI.
- `buckets-plugin` runs versioned JSON-RPC extensions as child processes. Any language that reads and writes newline-delimited JSON can implement the protocol. Search, filesystem providers, row decorations, and commands use the same capability-dispatch boundary. Command descriptors may contribute validated shortcuts; the host retains its core shortcuts.
- `buckets-terminal` owns the Alacritty terminal model, portable PTY process lifecycle, keyboard encoding and GPUI terminal grid.
- `buckets-app` owns GPUI views, keyboard focus, commands, background tasks, clipboard and drag/drop.

`settings.json` is the preference source of truth, separate from workspace paths,
selection and pane restoration. It lives in `BUCKETS_CONFIG_DIR`, default
`~/.config/buckets`, and owns text size, hidden files, sidebar/preview visibility,
and plugin enable/config overrides. The application migrates legacy preferences
only when this file is absent. Settings use bounded strict JSON, preserve unknown
keys, validate before commit, and retain last-good applied values on malformed
reloads. The Advanced JSON editor can repair the original invalid source.
Same-directory atomic writes use a persistent advisory-lock inode and an
optimistic byte comparison; crashes release the lock, and stale writers cannot
replace acknowledged external edits. External edits require explicit Reload.
Symlinked settings files are readable but not rewritten through the link.

Appearance preferences use semantic color overrides rather than a complete
replacement palette. Missing values inherit Tokyo Night; app and terminal font
families are independent. The host resolves installed fonts and applies one
runtime theme to GPUI components, file views, Settings, previews and terminal
panes. An unavailable family keeps its saved preference while resolving to the
bundled JetBrains Mono face. Existing terminal processes survive appearance
updates; their font metrics and PTY dimensions are recomputed by rendering.
The terminal crate accepts a plain host-supplied appearance structure, without
depending on the app's settings schema. Configured ANSI colors are fallbacks:
program-supplied truecolor and dynamic terminal palette values take precedence.

Application shortcuts use a shared scoped action catalog in
`buckets-app/src/keybindings.rs`. Settings stores `keybindings` as a map from
stable action IDs to arrays of alternative single keystrokes. Omission inherits
defaults, an empty array disables a command shortcut, and removing an override
restores the default. Plugin command IDs use
`plugin.<plugin-id>/<command-id>`, keeping dotted namespaces unambiguous and
allowing overrides to survive while a plugin is unavailable. Validation checks
effective defaults and overrides together before persistence.

Live changes replace only GPUI bindings tagged as application-owned. Input and
terminal bindings are preserved; old core and plugin aliases are removed.
Files-only actions are scoped to browsing, workspace actions also reach terminal
panes, and Settings, Quit, contextual Close and text sizing work globally.
Native editing, terminal control input and fixed navigation/recovery keys retain
their component behavior. The same effective map supplies native menu
accelerators and shortcut hints. Conflicting plugin defaults are skipped with
core commands taking priority and a visible notice.
Canonical shortcut validation treats literal plus as shifted equals. Runtime
bindings also include the literal-plus events emitted by the native platform;
these aliases share the same ownership tag and disappear together on remap or
disable.

The Settings shortcut recorder intercepts keystrokes before GPUI action
resolution, scoped to its window and dedicated focus handle. Capturing Quit,
Close or another active shortcut therefore previews the chord rather than
executing it. Recording does not remove normal keybindings. Key-up latches
ignore held activation keys and repeats; focus loss or cancellation releases
capture. Explicit Apply validates and persists one replacement chord, Clear
persists an empty list, and failed persistence retains the candidate. JSON
continues to support multiple alternatives.

Plugin manifests remain distribution defaults. Optional `settings_fields` metadata
describes generic bool/text/number/select controls; advanced nested configuration
remains arbitrary JSON. Saved object overrides merge recursively over defaults.
Disabled plugins remain visible in the catalog while every runtime dispatch
rejects them before process creation. Both the UI registry and core filesystem/
search dispatch apply the same settings. Reconfiguration starts from manifest
defaults so deleting an override restores the default without editing manifests.
Bundled unchanged sources receive missing first-party settings metadata;
user-edited source, custom metadata and configuration remain intact.

Locations identify provider and path. Local, SSH, S3 and Git use the same application vocabulary while preserving provider-specific capability limits. Git revision browsing is read-only. Existing SSH/AWS/Git configuration supplies credentials; credentials must never be stored in workspace state.

Long-running work belongs off the UI thread. Completion must be associated with the pane/location/request that started it; stale responses cannot redirect a pane. Paths must be passed as process arguments, with remote shell quoting where required. Search and previews are bounded; large file contents must not be loaded wholesale.

File mutation must refuse collisions, self-copies and copies into descendants. Symlinks must not make recursive transfers escape into unrelated trees. Cross-provider transfers require an explicit implementation; unsupported transfers must fail honestly.

Plugins are executable programs installed by the user. The host validates protocol and capability declarations, limits response sizes, and terminates failed/timed-out calls. This isolates crashes and dependency environments; it does not sandbox filesystem/network access. A plugin runs with the user's OS permissions.

A future hybrid search extension can own index scheduling, embeddings and vector storage. Its search request receives configured roots and ignore policy. The extension returns ranked paths/snippets/scores through the existing result shape; GPUI does not need embedding-specific state.

The app distributes Git status and developer tools as first-party executable plugins. On first launch it materializes their sources in the default configuration directory without replacing user-owned plugin directories. An explicit plugin-directory override uses that directory as supplied. The UI renders generic badges returned for visible entries; the Git plugin alone invokes Git and aggregates changes for directory rows. The developer-tools plugin returns a validated typed terminal launch request by default. The host spawns a PTY off the UI thread and inserts a terminal pane adjacent to the originating pane, retaining its working directory even if focus changed while the request ran. Explicit iTerm2 commands use private self-removing scripts. Terminal panes retain exit status, and closing a pane or quitting closes its owned process. Workspace persistence records working directories, never commands or terminal environment; restored terminals become file panes. Bundled plugin upgrades replace only exact known managed sources and merge capabilities while preserving user settings. Edited sources remain untouched with an upgrade notice.

## Agent workspace and plugin boundaries

The TypeScript SDK lives in `sdk/typescript`; the first-party AI plugin lives in
`plugins/ai`. Pi-durable is a pinned dependency of that plugin, rather than the
Rust application. Existing one-shot plugins retain their protocol. Agent plugins
opt into a persistent duplex service with typed host callbacks, progress and
cancellation. See [the service protocol](plugin-agent-protocol.md).

The implemented feature slice comprises Ask these panes, navigable semantic and
keyword results, and explicitly edited workspace notes. Rust supplies bounded,
revisioned pane snapshots and provider-aware list/read/search tools. It validates
all returned file references. The plugin owns model configuration, conversations,
versioned notes, embeddings and its local SQLite index. Its hybrid retrieval
uses keyword/vector rank fusion over bounded file prefixes; traversal and ignore
policy remain enforced by the host. Unsupported provider searches fail
explicitly. The plugin can also delegate retrieval to another search plugin.

The app separates agent presentation (`agent_view.rs`), application orchestration
(`agent_session.rs`) and context admission (`agent_context.rs`). The generic
host-action response opens an agent pane without embedding model logic in GPUI.
Read grants intersect open/configured roots with manifest permissions. Explicit
index roots govern search policy separately from reads of open panes.

The service is the sole owner of its workspace database. Stable workspace IDs,
unique admission IDs, generation guards and context fingerprints prevent stale
responses from navigating changed panes. Rust records explicit cancellation
before stopping the model service; the plugin reconciles that record before
resuming durable work. A read-only provider call already in progress may finish
later, while the watchdog stops the model service independently.

Streaming is an observation of real pi-durable generation and tool events.
The plugin extracts provisional Markdown answer text and sends bounded,
sequenced full snapshots through the existing progress channel. The Rust host
validates sequence, size and activity limits; the UI may coalesce intermediate
snapshots without corrupting text. Final answers and file references still
pass the original admission and broker validation before becoming actionable.
Reasoning blocks, raw tool output and provider diagnostics do not enter public
streaming updates.

`agent_transcript.rs` owns bounded presentation history, turn lifecycle,
following intent and inert Markdown preparation. The view renders this state
and dispatches explicit turn actions. Account/provider changes clear visible
conversation context; ordinary cancellation retains a marked partial reply.
Visible pane history is not a second durable conversation store.

Workspace identity is distinct from a configuration profile: an explicit
initial set of provider locations defines a stable workspace anchor, while a
launch without locations restores the saved workspace identity. Navigation does
not change that identity. Admission IDs include a launch epoch; the plugin keeps
per-admission fingerprint receipts so an old ID cannot retrieve an answer for a
different snapshot after intervening requests. The host verifies private state
directory ownership and permissions before allowing the service to write there.

Conversations and notes survive restart. Ordinary agent close/cancel aborts the
request; an uncancelled service interruption can resume through the same
admission ID. The UI does not automatically restore pending agent requests.
A new question supersedes an older pending question. Jobs require a live service;
persistence does not provide background execution. See the upstream
[durable harness README](https://github.com/earendil-works/pi/blob/main/packages/durable/README.md)
and [specification](https://github.com/earendil-works/pi/blob/main/packages/durable/docs/spec.md).

The Rust operation journal now supplies stable IDs, typed plans, durable intent,
per-item outcomes and guarded local recovery for desktop file mutations. Agent
host tools still expose no arbitrary shell or file mutation. A future agent
mutation flow must add explicit plan review and admission against current file
versions; conversation deduplication alone cannot make filesystem writes atomic.

Neovim bulk rename, collections, directory comparison/sync, source/artifact
lineage and editable workflow recipes remain future features.

## Plugin accounts

Account management is a separate application service from workspace tools.
Plugins declare the `accounts` capability and implement the generic account
protocol; Rust owns private credential persistence and supervised execution,
and GPUI renders public connection state. Provider-specific OAuth, model
discovery and subscription errors remain in the plugin. The desktop does not
depend on pi-durable or an OpenAI authentication SDK.

Credential leases serialize token refresh across processes. Credentials live
outside workspace databases and settings JSON. Browser authorization starts
only from an explicit sign-in action, never from model tools. Account changes
invalidate pending presentation and model requests. See
[the account protocol](plugin-accounts-protocol.md) for the wire contract and
storage boundaries.

## Operation and workspace lifecycle

`buckets-core::journal` owns typed `OperationPlan` / `OperationAction` values,
receipts and the inverse-operation policy. The journal uses private versioned
JSON, durable write-ahead intent, nonblocking OS locking and bounded history
(128 records, 1,024 top-level actions per plan, 8 MiB JSON). Loading marks running
records interrupted. Completed and uncertain effects are excluded from Retry;
capability reasons come from the core rather than duplicate UI heuristics.

Local writes use private sibling staging and atomic no-replace publication.
Receipts bind operation-owned content before publication; copy hashing reuses
the streamed write. Undo checks the receipt after persisting inverse intent,
immediately before the effect. It refuses edits/collisions and preserves removed
outputs in recoverable trash. This is guarded recovery, not a filesystem CAS.
Remote and cross-filesystem Undo are unavailable. Local cancellation is checked
per I/O chunk/hash; remote providers stop at item boundaries. Interrupted staging
paths remain explicit rather than being blindly cleaned up.

`operations_session.rs` orchestrates one desktop mutation job at a time and
coalesces worker progress into cached snapshots. Journal locks/I/O never block
rendering. It also owns the on-demand history/detail surface and its scoped
control actions. `browser_controls.rs` integrates pane and named-workspace
commands. Browsing and independent terminal jobs remain usable
while an operation runs; quit and workspace switching require resolving the
active mutation first.

`pane_view.rs` caches sort projections when entries or preferences change.
Canonical location identities preserve cursor/marks through sorting/filtering.
The pinned GPUI resizable group supplies pointer geometry; keyboard controls
update the same weights, and persistence runs on completed resize events rather
than every mouse movement. Optional columns hide responsively without changing
the requested preference.

`named_workspaces.rs` owns metadata only: up to 64 named snapshots containing
1–6 full provider locations, stable workspace identity, pane views, proportions,
orientation and active index. Private atomic saves, locking and external-edit
checks prevent silent concurrent replacement. New saved names receive their own
identity; updates and renames preserve it. Save does not switch the current note
namespace. Open selects that identity after validating preferences and dirty
state, and cancels/reaps stale agent contexts. Terminal/agent locations restore
as file panes; commands, environments and model requests are never replayed.
Live terminal panes require explicit closure before switching. Credentials,
accounts, theme and keybindings retain their separate stores.
