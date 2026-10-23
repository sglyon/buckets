---
name: Buckets
description: A quiet Tokyo Night workspace for keyboard-driven file browsing.
colors:
  background: "#1a1b26"
  chrome: "#16161e"
  surface: "#24283b"
  line: "#343b58"
  text: "#c0caf5"
  muted: "#9aa5ce"
  accent: "#7aa2f7"
  directory: "#7dcfff"
  success: "#9ece6a"
  warning: "#e0af68"
  danger: "#f7768e"
  cursor: "#2c3654"
  marked: "#253746"
typography:
  body:
    fontFamily: "JetBrains Mono"
    fontSize: "12px"
  input:
    fontFamily: "JetBrains Mono"
    fontSize: "13px"
  title:
    fontFamily: "JetBrains Mono"
    fontSize: "17px"
    fontWeight: 600
  label:
    fontFamily: "JetBrains Mono"
    fontSize: "11px"
  metadata:
    fontFamily: "JetBrains Mono"
    fontSize: "10px"
  preview:
    fontFamily: "JetBrains Mono"
    fontSize: "12px"
    lineHeight: "20px"
  quick-look:
    fontFamily: "JetBrains Mono"
    fontSize: "14px"
    lineHeight: "23px"
spacing:
  icon: "16px"
  pane-header: "48px"
  row: "29px"
  column-header: "28px"
  toolbar: "58px"
  status: "32px"
  shortcuts: "40px"
components:
  file-row:
    backgroundColor: "{colors.background}"
    textColor: "{colors.text}"
    typography: "{typography.body}"
    height: "29px"
  file-row-cursor:
    backgroundColor: "{colors.cursor}"
    textColor: "{colors.text}"
    height: "29px"
  file-row-marked:
    backgroundColor: "{colors.marked}"
    textColor: "{colors.text}"
    height: "29px"
  pane-header-active:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    height: "48px"
  keycap:
    backgroundColor: "{colors.line}"
    textColor: "{colors.text}"
    typography: "{typography.label}"
  command-button:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.muted}"
---

# Design

## Source of truth

- Status: Active; refreshed 2026-10-21 after the first native implementation.
- Primary surface: the native macOS Buckets workspace in `crates/buckets-app/src/main.rs`. Configurable command metadata and validation live in `crates/buckets-app/src/keybindings.rs`; Settings rows and native menu dispatch live in `settings_view.rs` and `menus.rs`. Appearance preferences and live semantic resolution live in `appearance.rs` and `theme.rs`. The frontmatter preserves the Tokyo Night defaults at the 13-point base; saved appearance preferences may override them. Source controls behavior.
- Evidence: `PRODUCT.md`, the user-approved Omarchy / Tokyo Night direction, source, the established workspace captures, `target/icon-preview-256.png`, and the completed Settings / bookmarks / inline-filter / shortcuts captures: `.impeccable/review/settings-desktop.png`, `.impeccable/review/settings-plugins.png`, `.impeccable/review/settings-json-error.png`, `.impeccable/review/bookmarks-desktop.png`, `.impeccable/review/settings-compact24.png`, `.impeccable/review/settings-compact-json.png`, `.impeccable/review/inline-filter.png`, `.impeccable/review/inline-filter-compact24.png`, `.impeccable/review/shortcuts-desktop.png`, and `.impeccable/review/shortcuts-compact24.png`.
- The desktop and Quick Look captures are 2880×1866 Retina images of a 1440-point-wide window. The compact capture is 1920×1346 at 2×, a 960-point-wide window including native title chrome. Do not infer GPUI pixel values from the displayed screenshot size.
- Code-first implementation was approved. No external comp or generated reference board is claimed. The document refresh merges implementation facts into the existing design commitments.
- Recorder evidence adds `.impeccable/review/recording-desktop.png` (1440×900, 13pt), `.impeccable/review/recording-compact24.png` (960×640, 24pt), `.impeccable/review/recording-verdict.md` and `target/recording-private-qa/native-verification.md`. The bounded recorder presentation verdict is SHIP for frozen binary `DBE208BB-F167-3D5B-9462-E49E0CF54290`; it does not certify subsequent theme changes.
- The implemented Agent pane uses presentation in `agent_view.rs`, orchestration in `agent_session.rs` and context policy/workspace identity in `agent_context.rs`. Frozen AI build `A33E70B0-E63E-3F9B-919B-547CE9719C50` passed source-boundary review (`.impeccable/review/ai-boundary-verdict.md`); this is separate from presentation approval.
- The earlier AI build could not receive native review because app selection hung. The streaming successor `6DAD8497-D6AC-358B-940B-6F5D65D12D35` now has actual desktop/compact captures and keyboard confirmation: `.impeccable/review/streaming-verdict.md`. This proves the named transcript interactions, not uncaptured account sign-in or memory-editing flows.
- Current theme build `16BE6073-23E8-3598-AAB4-EEB4EA1E2C3A` is compiled, but its native presentation is **NOT REVIEWED**: the isolated app connection hung twice before any screenshot, accessibility tree or input evidence. No theme capture or native theme coverage exists. `.impeccable/review/theme-verdict.md` records this unavailable review; prior captures and automated checks do not substitute for it.
- These captures establish the workspace, Quick Look, Settings, bookmarks, inline-filter and Shortcuts composition in the captured states. The shortcut captures show Workspace scope and compact 24-point wrapping with the effective value and Return/footer access. Static images do not prove key handling, JSON guards, persistence, terminal ownership or assistive-technology behavior. Live remap, disable/reset, conflict retention and terminal-input ownership were checked separately by the native verifier against build `DAF11CDE-C063-3E15-B012-14C7DFF66A5C`.

## Overview

UI copy is functional: name actions, locations, state and recovery. Do not add slogans or promotional language.

**Creative North Star: "A quiet developer workstation"**

Buckets uses the user's chosen Omarchy-inspired Tokyo Night palette and a compact monospaced workspace. Files and locations occupy the main surface; repeated aligned rows and subdued chrome support quick scanning. The normal workspace keeps shortcuts visible; focused modes consolidate those hints into their own command line or page footer.

The visual vocabulary is tonal and flat. Blue identifies active focus, cyan identifies directories, and textual state cues supplement color. The broad Quick Look surface temporarily gives file contents the window while preserving the workspace beneath a dark scrim.

**Key Characteristics:**

- Dense, aligned file lists with restrained chrome.
- Distinct active-pane, cursor and marked-file states.
- Real paths and operation outcomes.
- Native rendering, authored geometric icons, and discoverable keyboard commands.

## Colors

The palette is Tokyo Night: cool dark surfaces, pale lavender text, and limited blue/cyan accents. The frontmatter records unchanged defaults; `appearance.rs` owns 13 semantic settings tokens and `theme.rs` resolves them across workspace, Settings, GPUI components and terminal panes. The documentation token `line` corresponds to the runtime settings key `border`. Saved overrides use `#RRGGBB`; omitted values inherit Tokyo Night. The optional terminal ANSI override is one complete 16-color array, with Tokyo Night defaults when absent.

### Primary

- **Focus blue (`accent`)** identifies the active pane separator, cursor edge, input border and small brand tile.
- **Directory cyan (`directory`)** identifies folders and marked-row edges. The live GPUI input caret now follows `accent`.

### Secondary

- **Success green (`success`)** identifies the active-pane dot, marked label and normal status feedback.
- **Warning amber (`warning`)** is the plugin decoration warning tone.
- **Error rose (`danger`)** identifies failed operations, pane errors, destructive prompt borders and danger decorations.

### Neutral

- **Workspace night (`background`)** fills file panes and Quick Look.
- **Deep chrome (`chrome`)** fills sidebar, toolbar, preview and footer areas.
- **Raised night (`surface`)** identifies active pane headers, command controls, inactive cursor rows and hover rows.
- **Slate line (`line`)** separates regions and fills shortcut keycaps.
- **Lavender text (`text`)** carries filenames and primary content; **muted lavender (`muted`)** carries paths, metadata and supporting labels.
- **Cursor blue (`cursor`)** and **marked blue-green (`marked`)** distinguish row states.

**The State Separation Rule.** Preserve cursor, mark and active-pane cues independently. A marked file is not synonymous with the current row.

## Typography

JetBrains Mono remains the default for both interface and terminal text. Appearance settings now store independent interface and terminal font-family preferences. The resolver matches installed names case-insensitively; an unavailable requested family falls back to bundled JetBrains Mono with a visible notice, without rewriting that preference. The regular face is bundled under the SIL Open Font License, with native fallback for glyphs outside its coverage. Terminal alignment works best with a monospaced face. Monospace serves paths, filenames, code, counts and shortcuts rather than decorative display lettering.

The workspace body and file names use `body`; native input components use `input`. The wordmark uses `title`, supporting labels and keycaps use `label`, and column headings/type labels use `metadata`. The side preview uses `preview`; Quick Look uses `quick-look`. Prompt headings are 18px, and the keyboard reference title is 20px. Keep these modest distinctions rather than introducing a marketing type scale.

## Layout

The native window contains a fixed-height toolbar, a flexible workspace, a status line and a shortcut line. The optional places sidebar is 184px wide. Independent panes use persisted proportions and horizontal or vertical arrangement; equal sizing remains an explicit command. The optional preview is 300px wide in the full layout and 260px in compact mode. There are no decorative gutters between panes; hairline borders divide them.

Pane headers, column headers and rows use the frontmatter dimensions. Paths sit in a 36px region; pane counts sit in a 30px footer. File rows align a 16px authored icon, flexible truncated filename, optional 22px plugin badge, 56px size and 64px type/mark field. Header alignment reserves 64px for size and 72px for type. Compact metadata retains alignment rather than turning into cards.

Below the scaled 1200px compact threshold, the places sidebar is suppressed and Rename / Copy / Move hints leave the bottom strip. The toolbar hides the secondary Split / Commands labels, retaining their keycaps. The complete command list remains available through `?` and the command palette. The minimum window size is 960×640 points; the initial default is approximately 1440×900. Long path and filename text clips or truncates inside its allocation. Preview and sidebar toggles recover working space.

While the inline filename filter is active, its bottom command line replaces the global status and shortcut strips. The line carries the query, visible/total result count and one line of mode-specific help, preserving pane height and a visible result at 960×640 with 24-point text.

Quick Look uses a 32px outer inset, 56px heading, 40px footer, and a scrolling content region. Images are contained within the available region. Prompts use a 680px panel; shortcut help uses a 780px panel. These are desktop-native dimensions, not web breakpoints or mobile layouts.

## Elevation & Depth

Tonal surfaces and one-pixel separators establish hierarchy; the workspace does not use decorative shadows. Quick Look, prompts and help use dark translucent scrims to protect their input context. The scrim is an interruption cue, not glass decoration. No authored entrance animation or reusable motion scale is present.

## Shapes

File panes, rows and separators remain rectangular. GPUI `rounded_sm`, `rounded_md` and `rounded_lg` soften keycaps, command controls, drag feedback and overlay containers respectively; retain the framework aliases rather than inventing independent radius values. Status dots are circular. The row's one-pixel colored leading edge communicates focus or marking.

Interface icons are authored 24-unit SVG paths, rendered at 16px with a consistent 1.6-unit round stroke. They include folders, files, chevrons, search, close and check. Avoid emoji or miscellaneous glyphs as replacement file icons. Shortcut symbols and text badges are intentional text.

## Components

### File pane and list

A pane has a title, location, metadata header, virtualized list and count footer. The active header combines a green dot, literal `ACTIVE`, a raised surface and blue lower separator. The current active row uses cursor fill and blue edge; the inactive current row uses the raised surface. Marked rows use a separate fill and cyan edge, plus a green `MARKED` label. Hover uses the raised surface.

Plugins may add one compact text badge and a tooltip before metadata. Tone maps to neutral, accent, success, warning or danger colors. Git status is a plugin implementation of this generic row decoration contract, not a host-specific Git column. Plugin commands and their shortcut hints enter the command palette through the same host boundary.

### Commands and shortcut keycaps

Search and Split are quiet inline controls. Commands uses a raised, gently rounded container. Small slate keycaps sit beside muted action labels. Configurable commands show the first effective binding plus `(+N)` when alternatives exist, keeping toolbar, footer, menu, palette and help hints bounded. An unbound command says `Unbound`; fixed navigation such as arrows and x keeps its literal hint. Do not replace these with large primary call-to-action buttons.

### Input and command palette

The GPUI input component uses the 13px default theme and the resolved accent caret. Prompt panels are raised, bordered and gently rounded; their border is blue normally and rose for delete. Palette rows highlight the current command and change surface on hover. Native text editing belongs to the input component rather than a painted text imitation.

### Settings and Places

Settings is a full native workspace surface with a chrome header, global search, a left section rail, one continuous scrolling form on the right and a persistent outcome/footer area. Rows reuse background, raised-surface, cyan-focus, green-value and muted-description tokens. Appearance, Browsing, Places, Plugins, Shortcuts and Advanced each form one contiguous group. Clicking a section aligns its heading to the form's top, clamped at the content bottom; ordinary scrolling updates the active highlight from measured layout. Keyboard focus has a separate border and does not follow wheel scrolling. Search filters the whole form; sections without matching rows are disabled. Text-size decrement and increment rows disable at 9pt and 24pt and show `Minimum · 9pt` or `Maximum · 24pt`.

The section rail is 184 physical pixels wide, reducing to 168 below a 1100-pixel viewport. It scrolls independently when enlarged text needs more height. Compact preference rows stack their title and value, while descriptions wrap in the remaining main-column width. This preserves the existing colors, type and row treatment rather than introducing a second visual system.

Shortcut rows use the same preference-row treatment. Their descriptions expose Files, Workspace or Global scope, the stable action ID, recording instructions and defaults. The value shows all effective alternatives inline or `Unbound`; Enter opens an attached recorder that keeps the saved value visible. One recorded chord replaces all alternatives only after Apply. Additional alternatives remain editable in JSON. A separate Reset row appears only for an override and removes it to restore defaults.

The recorder separates a glyph-form candidate and `Not saved · replaces all alternatives` from the saved row value. Capture, Apply, Clear and Cancel are explicit controls; Tab / Shift-Tab chooses a control and Enter activates it. Bare Escape cancels. Clear saves an empty alternatives array to disable the action. Named conflicts and persistence failures keep the candidate available and retain the last saved binding. App commands are intercepted while Capture owns input; recording does not itself save or dispatch them. At 960×640/24pt, the selected control scrolls into view and the persistent recovery message wraps; recorder instructions remain keyboard-scrollable. These display and interaction facts are bounded by the frozen recorder proof above.

Appearance rows now expose the two requested/effective font families and 13 semantic color values with swatches. Each override has a reset path; clearing a color restores its default. Reset fonts and colors restores bundled fonts, Tokyo Night colors and the terminal ANSI palette while preserving text size and unrelated preferences. Validation and durable save precede live application, including existing terminal panes. These are implemented code facts; their new native presentation and interactive behavior remain unreviewed because the theme UI connection was unavailable.

Places is one list: saved locations first, followed by enabled built-ins, with redundant built-in targets omitted. Saved locations retain cyan folder icons and their stored order; the extra Bookmarks subsection is removed. Settings → Places contains built-in visibility toggles and paired Rename / Remove rows for saved locations. Labels may truncate in the sidebar, while the full canonical location remains the navigation source and the Settings description. Launch directory replaces the ambiguous Workspace label.

### Inline filename filter

Slash opens an undimmed, in-context command line below the panes. The first row carries the slash prompt, native input and visible/total count; the second is a single line of navigation and search-mode help. Plain text projects fuzzy matches without changing canonical entries. `fd:`, `rg:` and `fff:` switch the count label to Search and dispatch the corresponding broader search on Enter.

### Preview and Quick Look

The optional side preview stays bounded. Space opens or closes Quick Look; Escape closes it. Its header names the file, its footer explains left/right file browsing and Option-arrow preview scrolling, and its text content scrolls. This is a protected preview interaction rather than a separate navigation destination.

### Agent answers, results and workspace notes

The generic Agent pane reuses the resolved workspace background, text, muted labels, borders, focus accent and scaled type. A quiet editor-native transcript shows submitted prompts and live Markdown answers in place. Activity occupies one collapsible disclosure; response/code copy, retry and validated references belong to each turn. The lower multiline composer provides Send / Stop and compact account/settings/memory access. Enter sends, Shift-Enter inserts a newline, and IME composition does not submit. Partial responses remain visibly provisional, stopped or failed until a final result is validated.

The transcript retains a bounded in-memory history separately from plugin-owned
durable model state. Stable turn/control identities preserve keyboard focus
while actions appear. Following pauses when the reader scrolls back or selects
text; Jump to latest resumes it. Completed Markdown blocks keep stable render
identities while the active block updates. Model links, images and HTML are
inert; final host-validated references own file navigation. Native evidence for
these changes is recorded separately from source and transport checks.

Each cited file has an explicit Preview and Open in file pane action. Results opens a real file-result pane beside the Agent pane; its selection preview retains the indexed line, retrieval reason and snippet. The first-party plugin performs hybrid keyword/vector retrieval over bounded text prefixes. Result explanations must preserve the prefix-only limitation and reported skips/truncation rather than implying complete-file or exhaustive search.

Memory opens editable persistent notes scoped to the workspace and plugin. Save memory is explicit; Reload memory refuses to overwrite a dirty draft, and Revert edits restores saved text. Unsaved changes use the warning color and a textual label. Closing with dirty notes offers Save and close / Save and quit, Discard edits and Keep editing. The editor has a bounded viewport with its own text scrolling; the outer body scrolls controls. Tab / Shift-Tab control navigation, Enter activation, Escape cancellation and selected-control scrolling are implemented and await native confirmation.

Busy, cancellation failure, stale file-pane context, unavailable plugin and invalid request states retain explicit status text. A failed durable cancellation must say it could not be confirmed. Results and citations are checked by the host before navigation. The host exposes typed read-only context/list/read/search callbacks; the AI feature has no arbitrary shell or file-operation tool. Persistent notes and conversations are plugin-owned state rather than writes to browsed files. The source-boundary approval does not certify real cloud providers or live SSH/S3 behavior.

Account-capable plugins extend the existing Settings rows with connection status,
browser sign-in/cancellation, saved registrations, model choices, sign-out and
usage links. New Workspace AI setups default to Continue with ChatGPT. Explicit
API-key and local-provider selections stay active until changed. A first plan
connection has one dismissible confirmation. The Agent composer offers Settings
recovery and a compact Using ChatGPT plan / Manage usage state when applicable.
These are source facts; native sign-in presentation and keyboard behavior still
require a captured interaction pass. The generic account protocol is documented
in `docs/plugin-accounts-protocol.md`; credentials never enter Settings JSON.

### Operation feedback and drag feedback

The persistent status line reports operation text and active-operation counts, with rose error feedback. A dragged file has a raised rounded label with a blue border. Render actual results and filenames; never substitute invented activity metrics.

### Application icon

The icon is the approved bucket-b: cyan curved handle and lip, tapered blue bucket, and dark lowercase b on a rounded Tokyo Night base. Its source is `crates/buckets-app/assets/icon/buckets.svg`. `scripts/render-icon.sh` invokes the checked-in Swift/CoreGraphics renderer, producing the checked-in `Buckets.icns`, a `target/Buckets.iconset` and 16/32/64/256px previews. Run the renderer from macOS and inspect those outputs after any geometry change. `scripts/bundle-macos` copies the ICNS into the app bundle. The small in-app blue b tile is a separate compact wordmark treatment.

## Do's and Don'ts

### Do:

- **Do** keep real filesystem content, aligned metadata and distinct keyboard states visible.
- **Do** retain the Tokyo Night tokens and compact native dimensions recorded here.
- **Do** show textual action outcomes and shortcut hints alongside color cues.
- **Do** derive configurable command hints from the effective keybinding map and keep them bounded to the first binding plus an alternatives count.
- **Do** use the existing native icon source and rasterizer when reproducing the app icon.

### Don't:

- **Don't** turn the workspace into a decorative dashboard or add invented data, gradients or excessive roundness.
- **Don't** make Space mark files; it opens Quick Look, while x marks files.
- **Don't** claim visual captures prove keyboard coverage, screen-reader support, or all contrast combinations.
- **Don't** move plugin-specific Git semantics into the host's generic decoration layout.
- **Don't** present native editor commands, raw terminal input or fixed file-navigation keys as configurable application shortcuts.

## Brand

A quiet, precise developer workstation. The user pinned Omarchy-inspired styling and Tokyo Night. Trust comes from actual locations, clear operation outcomes and visible focus. Embedded terminal panes are the default; iTerm2 remains the user-selected external option. Tool launching is plugin-owned.

## Product goals

The visual system supports equally navigable independent locations, keyboard discovery and understandable plugins. `PRODUCT.md` owns product scope, providers and future search extensions.

## Personas and jobs

Developers compare project trees, transfer files between working locations, inspect content and search paths or text. The dense terminal/editor-like rhythm supports repeated use.

## Information architecture

One workspace: compact command bar, optional places sidebar, independent panes, optional preview, persistent operation line and shortcut strip. The command palette exposes verbs; location entry accepts paths and provider URIs; help lists shortcuts. Plugins contribute providers, search, commands, shortcut bindings and generic decorations. A generic plugin host action can open an Agent pane for Ask, search/index or explicit workspace notes; model behavior stays in the plugin. Plugin-owned editor, Git-tool and shell launches receive the active context and open embedded panes by default. Explicit iTerm2 options remain available.

## Design principles

- File content owns the space; chrome stays quiet.
- Active pane, cursor and multi-selection remain separate states.
- Input modes own keystrokes until submitted or cancelled.
- Slow work and failures receive visible feedback.
- Information density takes priority over decorative whitespace.

## Visual language

The canonical Colors, Typography, Layout, Elevation & Depth, Shapes and Components sections above describe the implemented visual language. Frontmatter preserves default documentation tokens; `appearance.rs`, `theme.rs` and the rendering sources own resolved runtime values. `.impeccable/design.json` carries native evidence and panel-preview extensions, not a second primitive token set.

## Accessibility

Complete keyboard access is a product requirement. Active pane, row focus and marking use text or geometry in addition to color. Native input components supply text editing; actual input-method, clipboard and assistive-technology behavior requires platform testing. Body-text contrast target is at least 4.5:1, but this document does not certify every rendered combination. Screen-reader behavior remains unverified.

## Responsive behavior

The Layout section records the implemented desktop compact mode. The supplied 960-point-wide screenshot retains two panes and the bounded preview, with the places sidebar hidden and shorter shortcut strip. Linux, other scale factors and mobile layouts are unverified.

## Interaction states

- Loading: a pane reports `Loading…`; the status line includes running-operation counts.
- Empty: a pane reports an empty directory or no search results; its footer retains item counts.
- Error: pane or operation text explains the failure in rose; the view remains available.
- Success: operation outcome appears in the status line and affected locations refresh.
- Marked/current/active: use the distinct treatments defined under Components.
- Delete: use an explicit confirmation prompt; conflict and read-only failures must remain textual.
- Offline or unavailable provider: surface the actual error; do not imply a successful connection.
- Agent state: live Markdown, code/table layout, multiline send, stopped partial replies, keyboard Retry, copied code, compact secondary controls and final reference preview received native confirmation on the frozen streaming build. IME composition, account-switch isolation and follow-tail invariants have source/test coverage; an actual IME, live-account switching and every memory-editing guard were not driven in this pass.
- Verification boundary: screenshots cover the named workspace, Quick Look, Settings, bookmark, inline-filter, Shortcuts and frozen recorder states, including compact layouts and validation feedback. The subsequent live-theme controls have no native captures or input verification, and their presentation verdict is NOT REVIEWED. They show layout and displayed state, while shortcut execution, conflict handling and input ownership rely on the separate native matrix. Uncaptured states require their own evidence; source presence alone is not a completed interaction check. See `docs/verification.md` for the separately verified native workflows.

## Content voice

Concise verbs: Open, Copy, Move, Rename, Duplicate, New folder, Search, Connect. Use exact paths and engine names. Name the failed operation in errors. Do not claim unimplemented capabilities.

## Implementation constraints

Rust + GPUI, macOS first. GPUI owns native rendering; core and plugin crates own behavior independently of theme. Background I/O and search must not obstruct input. Existing CLI configuration supplies provider authentication. Plugin capability checks are dispatch controls, not an operating-system sandbox. Native screenshots and platform checks replace web-only computed-style and browser-default audits. Verify behavior separately through core tests, real subprocess protocol tests and native interaction checks. Agent code follows the user’s domain-driven separation: presentation, application orchestration and context admission remain distinct; the first-party TypeScript plugin pins pi-durable 1.3.0 behind its adapter. Its private state requires strict ownership, 0700 directories and no-follow checks. Workspace identity derives from the explicit initial root set, or restores the saved identity on a launch without locations; ordinary navigation does not change it. Protocol and architecture details live in `docs/plugin-agent-protocol.md` and `docs/architecture.md`.

## Open questions

- Native AI review remains pending for build `A33E70B0-E63E-3F9B-919B-547CE9719C50`: answer/citation readability, result reasons, keyboard ownership, cancellation, note editing/save/close guards and compact reachability. No AI visual ship verdict is claimed.
- Native theme review at 1440×900/13pt and 960×640/24pt: fonts, color values/swatches, focus, reset/error states and terminal palette propagation remain pending.
- Screen-reader and full contrast-state verification, including user-selected colors.
- Visual verification of newly added Git decorations and plugin shortcut states.
- Linux packaging and platform behavior.
- Live remote integration and credentials in user-provided environments.
- Future plugin-owned embedding model/vector-store selection.

## Embedded terminal panes

Terminal panes share the file-pane chrome and display the tool title and working directory. Their grid defaults to the same muted Tokyo Night palette and bundled monospace font, with an independently configurable terminal font family and optional complete 16-color ANSI palette. Semantic background, foreground, selection, cursor and status colors resolve through the live theme. Shells and TUIs own raw terminal input. Workspace-scoped pane focus and command-palette actions, plus global close/settings/quit/text-size actions, may be configured with Command chords or function keys and continue to work in terminal context. Tab, Shift-Tab, Escape and native editing/clipboard commands retain their component-owned behavior. File-scoped commands do not enter terminals. Exited terminals remain visible until closed. Sessions restore terminal working directories as file panes; commands are never restarted automatically.

## Text sizing

The default bindings use Command-minus to decrease text size, Command-plus or Command-equals to increase it, and Command-0 to restore the 13-point base. These command bindings can be changed or disabled in Shortcuts. The base remains bounded to 9–24 points and persists in settings.json. Menus, the command palette and shortcut hints show the effective bindings.

The typography and authored chrome dimensions above describe the default base. They scale together using rem units; physical window bounds and hairline borders remain fixed. Embedded terminals remeasure glyphs, cell height and the PTY grid when the base changes. Narrow layouts temporarily hide the optional preview when it would crowd file panes, preserving the user's preview preference. Help and command results scroll within the available window at large text sizes.

## Settings

Command-comma opens or focuses a full workspace settings page while preserving the underlying file panes and terminal entities. Global search sits above a left section rail and a continuous grouped form. Each row shows its value and a description; cyan borders identify keyboard focus separately from the section highlighted by scroll position. Tab / Shift-Tab moves between search, the rail and controls. Arrow keys choose an available section while the rail owns focus, Enter or Space jumps to it, and Tab enters its controls. Section navigation leaves the search query intact and does not hide other matching sections.

Common controls persist immediately in settings.json. Plugins declare optional typed settings fields; the host renders enabled state plus Boolean, select, number, string and path fields generically. Advanced JSON supports the complete configuration with Command-S validation/save and explicit reload. Errors retain the last applied settings and draft. Closing, quitting or reloading with a dirty draft presents the corresponding save/discard/keep-editing choices. Escape or Command-W returns to the previous workspace focus. Settings rows, JSON actions and recovery states scroll or wrap within 960×640 at the 24-point maximum.

The `keybindings` object in settings.json maps stable action IDs to arrays of alternatives. An empty array disables that action; removing its map entry restores defaults. Plugin command IDs use `plugin.<plugin-id>/<command-id>`, so an override can persist while its plugin is unavailable. Core defaults take priority over conflicting plugin defaults; skipped plugin bindings remain visible as warnings instead of silently replacing a core command.

Scopes preserve input ownership. Files shortcuts run only while browsing. Workspace shortcuts also cover terminal panes, and Global shortcuts apply across the app; Workspace and Global overrides require a Command chord or function key. Native editor undo/redo and clipboard actions, Command-S in the JSON editor, raw terminal input, Settings navigation, and fixed file-navigation keys remain outside this configurable registry.

Places uses the sidebar rows and command palette with the same effective list. Command-Shift-B names the current location; saved places retain the `bookmarks` persistence key and stable shortcut IDs while the complete provider URL remains the navigation source. Settings and the palette expose rename/remove actions and show the location. Long locations wrap or truncate without altering the stored host, path, endpoint or Git reference. Adding an existing canonical location updates its name.

## Native menus

The macOS menu bar exposes Buckets, File, Edit, View, Go, Window and Help. Menu items dispatch the same workspace commands as key bindings, and their equivalents rebuild from the effective shortcut map. Context state disables file operations in editors, Settings, terminals and other non-file modes; selection and write access further gate destructive actions. Native text inputs supply Undo/Redo, while Cut/Copy/Paste/Select All follow the focused text, terminal or file context. Settings lives in the app menu; navigation and bookmarks live in Go. The command palette, keyboard reference and workspace hint strips use the same effective bindings.

## Inline filename filter

Slash opens a bottom command line with the file panes visible and undimmed. Plain text fuzzy-filters current entry names as the user types, with a visible result count and short keyboard hints. Arrow keys or Control-N/Control-P move through results, Enter opens the highlighted entry, and Escape restores prior cursor and marks against the latest listing. Canonical file identities remain independent of displayed indices, including after watcher refreshes.

The same line accepts `fd:`, `rg:` and `fff:` for explicit broader searches on Enter and labels that state as Search instead of presenting it as a live current-directory filter. Terminal panes retain their own slash input. At the compact 24-point maximum, the toolbar removes secondary action labels, and the filter command line temporarily replaces the global status and shortcut strips so the panes retain a visible result row.

## Application command line

Colon opens a bottom command area while file panes remain visible and undimmed.
Use the existing chrome, accent border, native input, muted descriptions and
textual errors. A context line identifies the frozen submitting pane/location;
bounded suggestions expose names, usage and provider restrictions. The bottom
area replaces ordinary status/hints and must preserve usable file space at
960×640/24pt. Completion and history replace input without executing it.

Tab / Shift-Tab chooses completions, Up / Down recalls accepted submissions,
Enter submits and Escape returns or cancels preparation. The colon belongs to
file browsing; terminals, editors, Settings and shortcut capture retain input
ownership. Core verbs are split, cd, rename and settings; plugin tools use the
same registration and invocation boundary as existing shortcuts/palette actions.
Explicit `:!` shell text is distinct from literal application/plugin argv.
Preparation, errors and cancellation remain visible in context. The host must
reject stale/cancelled results before creating panes or changing focus.

## Operation history and workspace controls

`operations_session.rs` renders Operations as an on-demand full surface, with a
quiet status entry during normal browsing, and owns worker orchestration.
`browser_controls.rs` integrates pane and named-workspace commands; `pane_view.rs`
owns cached sort projections; `named_workspaces.rs` owns saved metadata. This follows the source boundary in
[workspace lifecycle](docs/workspace-lifecycle.md).

Operations show actual counts/bytes, state words, per-item source/destination
paths and errors. Available Stop, Retry or Undo controls come from the selected
record's capability. Long details scroll, per-item pages are bounded, and
unsupported recovery explains its reason. Tab / Shift-Tab and activation use
explicit Operations-scoped actions so ancestor GPUI focus bindings cannot steal
them. Escape or Command-W returns to browsing.

Sorting, optional columns, orientation, keyboard resizing and named-workspace
commands reuse the palette. Divider dragging and keyboard resizing share the
same persisted weights. Compact columns disclose progressively while retaining
filenames; requested metadata remains saved when temporarily hidden. Named
Save/Open outcomes use short functional status text. Restoring a saved workspace
never starts terminal commands or model requests.

The 2026-10-23 native review confirmed desktop 13pt and compact 24pt layout,
explicit operation-control focus/activation, Rename Undo with disk checks,
keyboard resize, sorting, orientation and named restore. One initial batch,
one consolidated repair and one confirmation were used. Evidence and exact
limits live in `.impeccable/review/lifecycle-verdict.md` and
`docs/verification.md`; pointer divider resizing and assistive technology were
not exercised in this pass. The vertical capture is from the initial batch;
remaining lifecycle captures show the final build.

The 2026-10-23 Places refinement received native desktop13pt / compact24pt
verification on UUID `DA9E4672-ABBB-3DF0-8B5D-0F6D754616A7`: one list, matching
palette targets, Settings toggle save, rename retaining location, restart and
compact keyboard reachability. See `.impeccable/review/places-verdict.md` and
`docs/verification.md` for exact evidence and limits. No visual identity or token
changes were introduced.

The Settings section refinement received one initial native desktop13pt /
compact24pt batch and one consolidated compact-padding repair/confirmation.
Final UUID `32C08689-9FF5-3EB0-8736-792E489D507D` preserves the existing visual
world while verifying single-line rail labels, keyboard section access,
manual scrollspy independent of focus, and desktop heading alignment. Initial
evidence separately covers global cross-section search, recorder cancellation
and dirty JSON recovery. See `docs/verification.md` for build attribution and
the limits of this bounded pass.

The command-line refinement received one initial desktop13pt/compact24pt native
batch and one compact-density repair/confirmation. Final source `201f2c0`, UUID
`3D323591-579B-398C-8438-64F2CDD6B511`, keeps one compact candidate and short
help so a complete file row remains visible. Initial event evidence covers
literal argv, operation dispatch, local shell output/interruption, terminal colon
ownership, cycling, history and mode recovery; final confirmation covers compact
visibility/cycling and persisted history after restart. Exact build attribution,
tool transport limits and untested scenarios are in `docs/verification.md` and
`.impeccable/review/command-line-verdict.md`.
