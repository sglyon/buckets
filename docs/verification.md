# Verification

Date: 2026-10-21. Initial target: macOS on Apple Silicon.

## Automated behavior checks

The independent verification lane ran:

```sh
CARGO_TARGET_DIR=target/verification cargo test -p buckets-core -p buckets-plugin
CARGO_TARGET_DIR=target/verification cargo test -p buckets-plugin --test runtime real_fff_sdk -- --ignored
```

After settings, native-menu, bookmark and inline-filter integration, `scripts/check` passed **100 tests**: 33 app tests, 29 core tests, 25 plugin tests, and 13 terminal tests. Formatting, application compilation, and strict workspace Clippy passed. The app checks include saved font sizes, bounds/reset behavior, shortcut normalization and plugin-shortcut reservation. The terminal total includes 4 unit tests and 9 independent real-PTY acceptance tests.

Three tests are explicitly ignored in the default run because they need installed optional tools. All three were run here and passed: real FFF search, real Neovim editing/saving, and real Lazygit interaction. This yields **103 unique executed tests** across the integrated and explicit runs. The FFF check used the project's `.venv` and `fff-search 0.11.0`; editor checks used `/opt/homebrew/bin/nvim` and `/opt/homebrew/bin/lazygit`.

Current logs:

- `target/acceptance-final-check.log`: integrated formatting, 100 regular tests, app compilation, strict Clippy.
- `target/bookmarks-optional-tools.log`: explicit FFF, Neovim and Lazygit tests.
- `target/settings-independent-acceptance.log`: 16 independent settings/plugin/backend acceptance checks.
- `target/bookmarks-independent-acceptance.log`: independent bookmark acceptance checks.
- `target/settings-tab-tests.log` and `target/settings-tab-clippy.log`: 18 app unit tests and strict app all-target Clippy after the final native layout, draft-quit and JSON focus corrections.
- `target/settings-tab-fmt.log`: formatting after the earlier Settings corrections.
- `target/delivery-bundle-verification.log`: current signed-bundle verification and matching executable/bundle UUIDs.
- `target/native-final-qa/native-verification.md`: final native matrix, capture list, individual limits and isolated process cleanup.
- `target/font-zoom-tests.log` and `target/font-zoom-clippy.log`: latest focused text-size regressions and strict app/terminal Clippy.
- `target/embedded-final-clippy.log`: final strict workspace Clippy repeat after native UI fixes; formatting also passed.
- `target/terminal-acceptance-final.log`: 9 default PTY acceptance tests passed, including cleanup leaving an unrelated process alive.
- `target/plugin-terminal-acceptance.log`: 4 independent plugin host-action boundary tests passed.
- `target/terminal-real-tools-final.log`: real Neovim and Lazygit tests passed.
- `target/embedded-final-fff.log`: explicit real FFF test passed.

The later configurable-shortcut change passed the full app package: **62 test
executions covering 47 unique tests** in
`target/keybindings-fix-tests.log`. Five shared keymap unit tests are
included in four targets, accounting for 15 duplicate executions. The 14 new
unique checks comprise five catalog/validation tests, six independent settings
persistence tests, and three integration checks for tagged binding replacement,
dispatcher completeness, and native literal-plus matching with alias removal.
Invalid/conflicting writes preserve the saved map;
remap, disable, reset, external changes, inactive plugins and unambiguous plugin
namespaces are covered. Final formatting and strict all-target app Clippy passed
in `target/keybindings-fix-fmt.log` and `target/keybindings-fix-clippy.log`.

The targeted terminal tests open actual PTYs. Cleanup assertions inspect processes and required an approved run outside the command sandbox on this machine. PTY or executable failures are test failures; they are not silently skipped. Optional-tool tests remain explicitly ignored by default and can be reproduced with:

```sh
cargo test -p buckets-terminal --test acceptance -- --ignored --test-threads=1
```


Coverage includes:

- Local copying, atomic no-replace renaming, duplicate naming, recursive directory transfers, descendant rejection, dangling symlink collisions, and preserved symlink loops.
- Recoverable trash and protections against trashing the home/trash directory, exercised in a child process with an isolated HOME.
- Unicode, spaces, literal percent signs, reserved URI characters, and newline names round-tripping through remote locations.
- Correct local path resolution through a symlink followed by `..`.
- Bounded previews and binary detection.
- Independent pane history and selection behavior in the renderer-independent model.
- Real Git repository snapshot listing and preview, separate from modified working files.
- Actual `fd` and `rg` subprocess searches, literal queries, Git ignore handling, result limits and filename integrity.
- Plugin subprocess framing, error propagation, bounded response sizes, timeouts, capability grants, search-root/result containment, duplicate IDs, and protocol version validation.
- Actual Python and FFF search results, FFF content line numbers/snippets and ignore behavior.
- A real read-only plugin provider used through the core Backend; traversal and symlink escapes and denied writes.
- Rejection of incomplete plugin reads during cross-provider copying.
- Optional binary plugin transfers preserve arbitrary bytes through `data_base64`; the text-only example refuses binary or invalid UTF-8 data.
- Production SSH provider scripts exercised through a transport-only fixture: quoting, Unicode/metacharacter names, no-overwrite operations, recursive directory transfer, preserved dangling symlinks, remote `fd`/`rg` searches and ignores, recoverable source removal, and rejection of a destination reached through a symlink into the source.
- A stateful S3 CLI fixture exercises pagination, empty objects, an object and directory sharing a name, directory copy/move, overwrite refusal, read-back verification, conditional source deletion, and concurrent source changes before and during deletion. These tests run the production Rust adapter unchanged; they do not certify a live service.

The independent lane caught and obtained regression fixes for three data-integrity defects: percent-encoded path changes on round trip, incorrect symlink-parent resolution, and treating a truncated plugin read as a complete copy.

The ninth independent acceptance check exposed the example text-only provider rewriting binary data as display text. The provider now refuses binary and invalid UTF-8 reads, and the independent regression rerun passes without creating output files. Strict Clippy for core/plugin crates and all their test targets passed after the streaming-transfer expansion, with warnings denied. The final integrated workspace run is recorded above; the app’s final build and strict Clippy rerun also passed after the last native drag adjustment.

## Embedded terminal checks

The terminal engine uses an actual OS PTY and Alacritty's terminal-state parser. Independent tests verify:

- PTY resize is visible to the child through `stty size`; process exit and nonzero status are observable, and writes to an exited session fail.
- ANSI output with true-color sequences and wide Unicode produces terminal screen text without leaking escape bytes. This checks parsing and characters, not pixel-perfect color rendering.
- Bracketed paste preserves Unicode and multiline text while removing embedded ESC bytes that could terminate a paste envelope.
- Closing or dropping a pane terminates foreground processes, separate background process groups, and jobs whose session leader already exited. An unrelated child process remains alive.
- Control-C, Control-Space, Escape, Tab, Shift-Tab, application-cursor arrows, modified arrows, Option-as-Meta and Unicode key encoding; Command shortcuts are not sent as terminal bytes.
- A typed plugin launch request reaches a real PTY with literal argv, exact working directory and environment, including spaces, Unicode, quotes and shell metacharacters.
- Host dispatch rejects unknown host actions, invalid or oversized arguments/environment, and cwd symlinks escaping a declared root. Legacy JSON results remain ordinary values.
- Real Neovim enters insert mode, accepts multiline Unicode text, writes the exact expected bytes to a Unicode filename with spaces, and quits. The harness waits for acknowledged insert mode before pasting and accounts for Vim's final-EOL behavior.
- Real Lazygit renders its TUI in an isolated Git repository, enters an alternate screen, accepts first-run dialog dismissal and `q`, and restores the prior main-screen content.

These are functional PTY/TUI checks. They do not establish smooth rendering under every workload, comprehensive IME behavior, screen-reader support, or native pixel fidelity.

## Native desktop checks

The desktop agent exercised the running native application with CUA against isolated fixture files. Confirmed behavior includes:

- Keyboard location entry, file creation, Tab pane switching, and copy/paste between panes with file bytes checked on disk.
- Finder clipboard interoperability in both directions, including Unicode filenames.
- Space opens the text Quick Look overlay; Space/Escape dismiss it. The interface explicitly shows x for marking files.
- Native content search through rg and relative search-result labels.
- Internal drag/drop copied a 3-byte binary file into another pane without changing the bytes.
- Closing the application window exits the application cleanly.
- Git status badges, plugin command filtering, palette arrows/Escape, and plugin shortcut dispatch are visible and functional. The first-party bootstrap test proves installation preserves user configuration.
- An earlier explicit external-terminal check launched Neovim through iTerm2 with the exact selected file in argv and the active directory as its process working directory. External terminal UI interaction was not inspected.
- The new embedded Neovim pane was exercised natively through Ctrl-E: text entry, Escape, `:w` and Return saved the fixture file, with bytes checked on disk. The capture is `.impeccable/review/nvim.png`. The shell reported a resize from 33×51 to 33×78 after a neighboring pane closed; Cmd-W removed its child process, checked independently by PID. A return to file Quick Look was also checked by the desktop agent.
- Native Ctrl-G opened embedded Lazygit; Space staged the fixture file and `git status` confirmed the staged change. Tab/Escape remained inside the TUI without opening workspace overlays. The capture is `.impeccable/review/lazygit.png`.
- A raw native input probe exposed inherited GPUI Component Tab interception. Dedicated terminal Tab/backtab actions fixed it. The repeated native probe captured exactly `[9,27,91,90,27,32,5]`: Tab, Shift-Tab (`ESC [ Z`), Escape, Space and Control-E. The byte evidence is `target/ui-fixture/raw-input-final.json`; no workspace pane navigation or Quick Look was triggered.
- Native terminal Command-A/Command-C copied scrollback into a safe application text prompt; native paste reached the terminal and executed the isolated clipboard marker. Captures are `.impeccable/review/terminal-clipboard.png` and `.impeccable/review/terminal-raw-input.png`.
- An exited shell retained its visible exit-7 footer, and the terminal command palette exposed all three explicit iTerm2 alternatives. Captures are `.impeccable/review/terminal-exited.png` and `.impeccable/review/terminal-iterm-options.png`.

Native inspection caught and fixed a Tab keymap conflict, list-width misalignment, a missing font, stale mouse selections, absolute result labels, and native-versus-internal drag ordering. JetBrains Mono is now bundled.

The independent visual reviewer inspected `desktop.png`, `quick-look.png`, and an actual 960×640 content window (`small.png`, 1920×1346 Retina including titlebar). It found no material visual defect in those states. It also inspected the actual bucket-b icon at Dock sizes and separately passed the added Git-badge and developer-command-palette presentation. This is a bounded presentation verdict, not a claim of exhaustive interaction or accessibility coverage.

## Font size and macOS launch checks

Native keyboard checks confirmed Command-minus, Command-equals, Command-Shift-equals/plus and Command-0 in file panes, terminal panes and an input prompt. Text sizes clamp at 9 and 24 points; an 18-point setting survived application relaunch. Quick Look and terminal text scale with the same setting. Actual child `stty size` changed from 33×51 at the default size to 21×42 in the larger-text test layout.

At 24 points in a 960×640 window, location entry retained visible input, Escape and Enter/Confirm controls; Enter confirmed the existing fixture directory. Help scrolled with End/Home, and command-palette arrow navigation scrolled the selected result into view. Captures: `.impeccable/review/zoom-terminal-18.png`, `zoom-quick-look-18.png`, `zoom-persisted-18-small.png`, `zoom-prompt-24-small.png`, `zoom-help-24-small.png`, and `zoom-palette-24-small.png`.

The missing switcher icon was traced to an unbundled development executable with no bundle identifier. The verified macOS development launcher now runs `Buckets.app/Contents/MacOS/buckets`. A native probe confirmed its `dev.sglyon.buckets` identity, correct bundle URL, regular activation policy and correct bucket pixels from `NSRunningApplication.icon`; the capture is `.impeccable/review/icon-process.png`. A bare executable also installs an AppKit Dock icon, but proper switcher identity relies on the app bundle. The switcher overlay itself was not captured. Existing user sessions were preserved; isolated QA instances were closed.

The latest focused font tests and final strict workspace Clippy are recorded in `target/zoom-final-focused.log` and `target/zoom-final-clippy.log`. Bundle signature verification passed, and `target/zoom-bundle-verification.log` records matching executable/bundle Mach-O UUIDs.

## Settings checks

Independent acceptance tests exercise first-run migration, restart persistence, unknown-key preservation, invalid fields, malformed-file repair, concurrent stores, external replacement/deletion, invalid reload recovery, symlink preservation and killed-lock-holder recovery. Plugin tests verify nested overrides reach a real child process, removal restores manifest defaults, invalid declared values fail before execution, and disabled plugins cannot dispatch any capability. A core acceptance test calls actual backend provider and FFF dispatch to confirm both honor saved preferences on every operation. These 16 independent checks passed in `target/settings-independent-acceptance.log`.

Five bookmark acceptance tests cover offline full-location persistence/restart for local, SSH, S3-compatible, Git and plugin URLs; rename/remove; unknown metadata; malformed/oversized values; external-edit conflicts; and rejecting credential-bearing endpoints without changing ordinary endpoint paths.

The inline filter adds five Unicode/ranking matcher tests and four tests of the actual pane helpers: canonical result identity, live-reload cursor/mark restoration, selection of projected ranges without hidden rows, and empty-result/engine-prefix behavior. These are included in the 100-test workspace run.

The settings implementation also adds bootstrap coverage for additive metadata migration without replacing user configuration, custom schema or edited plugin sources. Settings files use atomic writes, optimistic conflict detection and a cooperative advisory lock; failed validation does not change effective preferences or overwrite source bytes.

Final native acceptance used the isolated `target/native-final-qa` configuration and fixtures. It confirmed keyboard category/form navigation, scrolling to the final control, JSON Tab/Shift-Tab navigation without changing the draft, and return to the workspace. Invalid JSON blocked Save-and-quit without changing disk contents; Keep editing restored the draft. The native window-close control also entered the draft guard, and Escape returned to editing. This caught and fixed a quit-dispatch timing issue in addition to inherited JSON Tab interception.

A raw terminal probe received exactly `[47, 32, 9, 14, 16, 13]` (slash, Space, Tab, Ctrl-N, Ctrl-P, Return) while retaining the same child PID across Command-comma → Settings → Escape. Evidence is `target/native-final-qa/raw-input.json`. Native bookmark checks created, renamed with Unicode text, and removed a bookmark while preserving another bookmark's full location. A valid Save-and-quit preserved an unknown setting containing Unicode text, plugin configuration and the remaining bookmark; relaunch and opening that bookmark confirmed its full path survived.

## Inline filter checks

The inline filename filter keeps canonical entries separate from its cached visible-index projection. Five matcher tests cover Unicode-safe lowercase matching, token conjunction, ranking and stable ties; four pane-helper tests cover result identity, cursor/mark restoration after reordered listings, projected range selection without hidden entries, and empty/prefixed-query handling.

Native acceptance at 960×640 confirmed the bottom command line, live filtering and visible results at both 13 and 24 points. Inspection caught a zero-height result list at the maximum size; redundant global footer strips now consolidate into the filter line while filtering. Compact toolbar labels and hints also adapt to preserve the viewport. Captures are `.impeccable/review/inline-filter.png` and `inline-filter-compact24.png`.

Native checks also confirmed `fd:` and `rg:` submission from the command line, Ctrl-N/P match navigation, Enter opening the selected canonical directory, and restoration of the prior cursor and marked selection on Escape.

The independent presentation reviewer marked all six listed material fixes resolved and returned **ship** after inspecting eight refreshed desktop/compact Settings, bookmark and filter states. No material regression was found in that bounded confirmation. The scoped verdict is `.impeccable/review/settings-filter-verdict.md`; keyboard and persistence conclusions rely on the separate native checks above.

## Configurable shortcut checks

The isolated native pass changed Split to Command-Y, confirmed the former
Command-T did nothing, and confirmed Command-Y created exactly one pane. Empty
bindings disabled the command; Reset restored Command-T without restarting.
Conflicting Command-K showed the named conflict and preserved exact saved bytes.
JSON Tab/Shift-Tab, contextual Close and a live shell survived repeated map
replacement. The raw receiver still obtained `[47, 32, 9, 14, 16, 13]`.

Native testing caught a platform plus-key mismatch. Tagged runtime aliases now
match the native literal-plus event while retaining canonical conflict checks.
Command-plus increases size exactly once, disabling it removes the alias, and
Reset restores one increment. The full matrix and limits are in
`target/keybindings-qa/native-verification.md`.

The independent reviewer returned **ship** after a single correction batch and
confirmation of `shortcuts-desktop.png` and `shortcuts-compact24.png`. Scope labels
are visible at 13pt desktop and 24pt compact sizes, with no material regression.
Native menu actions and toolbar hints were checked; displayed native menu
shortcut glyphs were not independently verified because AX omitted key
equivalents and menu-open screenshots were unavailable. Alternate layouts,
exhaustive custom combinations, IME and accessibility were not certified.

## Shortcut recording checks

Build `DBE208BB-F167-3D5B-9462-E49E0CF54290` passed 73 app test
executions covering 53 unique tests; five shared backend tests are repeated
across five targets. Six independent recorder tests also passed. Logs are
`target/recorder-app-tests.log` and
`target/shortcut-recording-independent-tests.log`.

Native checks captured Quit, Close, Settings and font-size shortcuts without
dispatching them, cancelled without changing saved bytes, applied Command-Y,
verified the former binding stopped working, retained a candidate after a
named conflict, and exercised Clear and Reset. Normal input and shortcuts
resumed afterwards. The private Recorder Test instance exited cleanly and its
installation was removed. The adopted Acceptance QA installation and the
user's other instances were preserved. Details and scope limits are in
`target/recording-private-qa/native-verification.md`.

The bounded presentation review returned **ship** for desktop and compact
24-point captures, with all recorder actions and conflict recovery visible.
Receipt: `.impeccable/review/recording-verdict.md`. Those images establish
presentation only; native interception and persistence rely on separate tests.

## Appearance checks

Build `16BE6073-23E8-3598-AAB4-EEB4EA1E2C3A` passed all 90 app test
executions covering 60 unique tests. Seven targets include the same five
keymap unit tests, accounting for 30 repeated executions. Seven appearance
cases cover partial defaults, validation, exact-byte protection on rejected
saves, restart/reset behavior, unknown metadata, independent font fallback and
host-to-terminal palette semantics. Evidence: `target/theme-app-tests.log` and
`target/theme-independent-tests.log`. Formatting and strict app all-targets
Clippy passed (`target/theme-app-fmt.log`, `target/theme-app-clippy.log`).

The terminal package passed 16 tests; two optional TUI tests were skipped.
Coverage includes configured ANSI colors, default foreground/background/cursor,
extended colors, program truecolor and dynamic palette precedence, and OSC
color query responses. Strict terminal all-targets Clippy passed. Evidence:
`target/terminal-theme-tests.log`, `target/terminal-theme-clippy.log`.

Native appearance verification remains pending. The private Theme Test app
launched and was visible in CUA inventory, but both exact-path and observed-name
app selections stalled without returning AX state or a screenshot. No UI input
was sent. A process sample showed the normal macOS event loop, rather than a
startup deadlock. The bounded attempts were stopped; the untouched private
process was terminated and its installation removed, preserving all human apps.
Consequently live editing, compact layout, terminal geometry after font changes,
and native relaunch behavior are **not verified** by this pass. Details:
`target/theme-private-qa/native-verification.md`; presentation disposition:
`.impeccable/review/theme-verdict.md` (**not reviewed**, no captures available).

## TypeScript SDK and agent release checks

Build `A33E70B0-E63E-3F9B-919B-547CE9719C50` passed the full frozen
`scripts/check` sequence: formatting, workspace tests, app compilation and strict
workspace all-targets Clippy. The workspace log reports 184 top-level passes
and six ignored tests, plus one nested permission-test child. Accounting for
30 repeated keymap units and two repeated context-policy units gives 152 unique
regular Rust tests. Three explicitly installed AI integration tests passed
separately; the remaining optional FFF/Neovim/Lazygit cases were not rerun.
Evidence: `target/ai-final-workspace-check.log`,
`target/ai-installed-end-to-end.log`.

The TypeScript SDK and AI plugin passed 25 tests and TypeScript checks. Coverage
includes legacy and duplex framing, bounded shutdown, actual pi-durable SQLite
restart/crash recovery, historical admission fingerprints, cancellation,
workspace identity, verified result names, real embedding HTTP requests and
cache refresh. Evidence: `target/ai-typescript-tests.log`,
`target/ai-typescript-typecheck.log`.

Independent tests installed a standalone compiled plugin with vendored SDK and
production dependencies, then invoked it through Rust's real `AgentService` and
`AgentBroker`. Offline memory survived reopening and explicit clearing. The
actual compatible provider adapter used a local HTTP fixture without an
Authorization header, searched an open pane despite separate configured index
roots, read a Unicode filename and returned a verified citation. Hybrid search
used an HTTP embedding fixture, SQLite cache and rank fusion; ignore rules,
cache reuse and stale-file pruning passed. These are fixture-backed integration
results, not live cloud-provider, embedding-model or remote-host certification.
Installer evidence: `target/ai-install.log` and the installed integration log.

A focused independent boundary/lifecycle review approved the corrected source
after one initial pass and one confirmation. Corrections covered workspace
identity, Ask/index scope, private state permissions, durable cancellation error
visibility, file-only enumeration, authoritative result metadata and historical
request-ID reuse. Receipt: `.impeccable/review/ai-boundary-verdict.md`.

CI now includes Node 24 SDK/plugin tests and type checking, a fresh standalone
installation and the three explicit installed integration tests alongside the
Rust gates. YAML and command wiring were checked locally; remote CI was not run.
Evidence: `target/ai-ci-yaml-review.log`.

Native AI verification remains pending. Inventory found the private test app,
but selection first reported duplicate test-bundle identities and then stalled
after the unused staged registration was removed. No window state, screenshot
or native input was obtained. The bounded attempts were stopped. Agent keyboard
routing, visual presentation, memory editing and compact layout are unverified.
Evidence: `target/ai-private-qa/native-verification.md`; presentation disposition:
`.impeccable/review/ai-presentation-verdict.md` (**not reviewed**).

Cleanup confirmed unchanged private preferences, no saved workspace, no native
agent state and no child process. Only the private AI QA process was terminated
and its installed/staged copies removed. Production, human-adopted Acceptance,
Settings QA and other user apps were preserved. Test fixtures, installed-plugin
packages and verification logs remain under `target/ai-private-qa`.

## Limits of this evidence

- Outgoing cross-application dragging is implemented using native AppKit file dragging, but remains unverified by automation. Bounded CUA attempts did not deliver an outside-window movement that triggered native export; no conclusion about the native export session follows from those attempts. Internal pane dragging and Finder clipboard interoperability were verified. Inbound cross-application drops also require a direct manual pass. These gaps must not be described as verified drag/drop coverage.

- Live SSH and S3-compatible services have not been tested without endpoint/credential fixtures. Stateful CLI/transport fixtures validate adapter behavior, but do not establish live authentication, host compatibility, or service certification. Remote SSH search requires the selected tools on the host. S3 servers must support the conditional operations used by safe transfers.
- Git revision locations are read-only. Local working trees use the local provider.
- Recursive cross-provider transfers and recoverable SSH/S3 trash are implemented and exercised through the adapters above. Partial directory transfers preserve completed destination entries and report their location. Local cross-filesystem move verification is covered at the helper level; moving across actual mounted volumes is unverified.
- Native-provider file transfers use private disk spools and streaming I/O, with a 15-minute command timeout and capacity constrained by available disk space and the backing service. An independent 129 MiB + 17 byte local-to-SSH-to-local transfer, with markers and SHA-256 verification, passed and exceeds the removed 128 MiB and 32 MiB limits.
- The process-plugin wire protocol permits optional base64 binary payloads and remains bounded to 1 MiB requests and 8 MiB responses; larger plugin transfers fail explicitly. The example local-provider intentionally remains a bounded text-only reference.
- FFF's example host invocation rebuilds the index per call; no persistent warm-index performance claim is made.
- Linux, complete native IME composition, screen-reader output, code signing for distribution, and notarization have not been verified. Option is treated as Meta inside terminal panes; normal composed text uses the native input handler.
- The settings editor refuses writes through a symbolic link to preserve the link. Edit its target externally and reload instead. Settings uses strict JSON and explicit reload, without a file watcher.

## Reproducible native acceptance checklist

Use the isolated `target/ui-fixture` directories when performing mutations:

- Navigate between independent panes with Tab/Shift-Tab; split and close panes with the keyboard.
- Enter a local location; attempt an invalid one and confirm usable recovery.
- Open and cancel the command palette and help; execute palette commands by keyboard.
- Create and rename files, including names with spaces and Unicode; verify actual bytes on disk.
- Select a range, copy/cut/paste between panes, duplicate, and exercise a destination conflict.
- Search names with `fd`, contents with `rg`, and fuzzy names with the installed FFF plugin; Escape returns to browsing.
- Toggle preview/sidebar/hidden files and scroll preview by keyboard.
- Confirm local file-URL clipboard interoperability with Finder in both directions.
- Exercise internal and external drops and verify the resulting files and destination.
- Test delete cancellation; automated trash checks already isolate HOME and confirm recoverable bytes.
- Open Settings with Command-comma from files, a terminal and a text prompt; close and confirm the previous focus and terminal process survive.
- Filter preferences, change a form value using the keyboard, and check settings.json and relaunch persistence.
- Edit/save valid JSON with Command-S; try malformed JSON and external edits, then verify draft preservation and explicit reload/discard recovery.
- Disable a plugin and verify its commands, search and decorations stop dispatching; edit a declared plugin field and confirm effective configuration.
- Use Settings at 960×640 with 24-point text, including plugin field entry and dirty-draft confirmation.

## Current delivery

`target/debug/Buckets.app` is the delivered locally ad-hoc-signed macOS bundle, including the embedded PTY terminal, JSON-backed Settings, live appearance tokens and font choices, configurable application/plugin shortcuts with key recording, native menus, saved full-location bookmarks and inline filename filtering. Split defaults to Command-T. The developer-tools plugin requests embedded shell, Neovim and Lazygit panes by default and retains explicit iTerm2 alternatives. `scripts/dev` starts the source build; `scripts/bundle-macos` reproduces the bundle.

The final bundle passed `codesign --verify --deep --strict`. Its arm64 Mach-O UUID matches `target/debug/buckets`, as recorded in `target/delivery-bundle-verification.log`. Native checks above cover the named fixture workflows and captures; they do not certify every TUI, workload, IME or accessibility scenario.

The agent host and pane are included in this bundle. Workspace AI is an explicit
standalone plugin installation: run `scripts/install-ai-plugin`, restart the
app, and configure its model and embedding endpoint in Settings. The installer
does not overwrite an existing plugin. No model is selected by default; offline
workspace memory is available without provider credentials. The release tests
installed the plugin only into the isolated QA configuration, preserving human
configuration directories.

The earlier isolated keybindings Acceptance QA instance quit cleanly after verification; its app and raw-probe child PIDs were confirmed absent and that temporary installation was removed. During later recorder work, an Acceptance QA installation became user-adopted and was preserved. Subsequent checks used the separate private Recorder Test identity, which was cleanly closed and removed. Target fixtures, logs and captures remain available.
