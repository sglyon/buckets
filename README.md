# Buckets

A native Rust + GPUI file browser for developers. Independent panes, keyboard commands, previews, and a versioned plugin protocol connect local files and remote locations in one workspace.

The initial desktop target is macOS. The core and plugin host are independent of the renderer, so future platform work can reuse them.

## Run

Install a current stable Rust toolchain and Xcode command-line tools. Then:

```sh
scripts/dev
```

On macOS, this builds and runs the app bundle so the Dock and Command-Tab use its name and icon. The launcher forwards directory arguments and environment settings. To create a local macOS app bundle without launching it:

```sh
scripts/bundle-macos
open target/debug/Buckets.app
```

Pass `release` to the bundle script for an optimized build. The local bundle is ad-hoc signed; notarized distribution is separate work.

## Keyboard

These are the defaults. Change application and plugin shortcuts in **Settings → Shortcuts**; native editor and terminal input keys remain independent.

| Shortcut | Action |
| --- | --- |
| Space | Quick Look; Space or Escape closes it |
| ↑ / ↓ or j / k | Navigate files |
| Enter / l; ← / h | Open; go to parent |
| Tab / Shift-Tab | Focus next / previous pane |
| ⌘Option-→ / ← | Focus next / previous pane, including terminals |
| ⌘T / ⌘W | Split / close pane |
| ⌘Option-] / [ | Grow / shrink the active pane |
| ⌘Shift-J | Operations and recovery |
| ⌘L | Enter a path or connect to a location |
| ⌘Shift-B | Save the current location to Places |
| ⌘K | Search and run commands |
| ⌘, | Open Settings |
| ⌘− / ⌘+; ⌘0 | Decrease / increase text size; reset to default |
| Ctrl-E / Ctrl-G | Open Neovim / Lazygit through the developer-tools plugin |
| Option-Enter | Open an embedded terminal at the current directory |
| / | Live filename filter in the bottom command line |
| x; Shift-↑ / ↓ | Mark and advance; select a range |
| ⌘C / ⌘X / ⌘V | Copy / cut / paste files |
| F2; ⌘D | Rename; duplicate |
| F5 / F6 | Copy / move to a directory |
| ⌘N / ⌘Shift-N | New file / folder |
| ⌘Backspace | Move to recoverable trash after confirmation |
| ⌘P / ⌘B | Toggle side preview / places sidebar |
| .; ? | Toggle hidden files; open keyboard reference |

Quick Look supports text and local images. Alt-↑ / ↓ scrolls previews. The command palette exposes provider and plugin commands as well as built-in actions.

Press `/` to fuzzy-filter filenames in the active pane as you type. The file list stays visible. Use ↑ / ↓ or Ctrl-P / Ctrl-N to choose a match, Enter to open it, and Escape to restore browsing. For broader searches, submit `fd: query`, `rg: query`, or `fff: query` from the same bottom line.

The native macOS menu bar groups file operations, text editing, view preferences, location navigation, pane management and keyboard help. File actions disable when the current context cannot perform them. Edit actions follow the focused text input, terminal or file pane.

Text sizing applies to the file browser, Quick Look, and embedded terminals, and is saved between launches. Command-= also increases the size. Command-0 resets the 13-point base; the supported range is 9–24 points.

Neovim, Lazygit, and shells run in native terminal panes backed by Alacritty and a real PTY. Terminal panes send ordinary keys directly to the running program. The command palette also includes explicit iTerm2 alternatives; set `terminal_target` to `external` in the developer-tools plugin configuration to use an external terminal for its shortcuts.

In a terminal, ⌘A selects the scrollback, ⌘C copies the selection, and ⌘V pastes. Shift-PageUp / PageDown scroll history outside full-screen TUIs. Closing a terminal stops its process and jobs. Restoring a workspace opens its terminal working directories as file panes; it does not rerun commands.

## Pane controls and saved workspaces

Use the command palette to sort the active pane by name, size, modified date or type, reverse direction, and toggle directories-first. Size, Modified and Type columns are optional; compact panes temporarily hide metadata before crowding filenames. Sorting and refresh preserve the current file and marks.

Drag a divider or use **⌘Option-] / [** to resize the active pane. Palette commands arrange panes side by side or top to bottom and equalize sizes. Orientation, proportions and each pane's view survive relaunch.

Choose **Save named workspace** in the palette, then **Open workspace: NAME** to restore its locations, active pane, layout and view preferences. Rename, Remove and Reload named workspaces are also available there. Removing a saved workspace removes metadata only. Names live in the private `workspaces.json` beside `settings.json`; provider URLs retain SSH hosts, S3 endpoints and Git refs without storing credentials.

Each new saved name receives a separate workspace identity. Saving does not switch context; opening the name selects its own workspace notes and agent context. Existing notes remain with their original workspace. Saved terminals and agents restore as file locations, without rerunning programs or requests. Close running terminal panes and resolve unsaved settings or notes before switching.

## Operations and recovery

Press **⌘Shift-J** or choose **Show operations and recovery** to inspect the bounded operation history. Select a record to see source/destination paths, per-item outcomes, errors and any interrupted staging location. Tab / Shift-Tab selects the available action; Enter activates it. Escape or ⌘W returns to files. Use ↑ / ↓ to select an operation, `[` / `]` to page its item details, and PageUp / PageDown to scroll long details.

Rename, copy, move, duplicate, creation and trash use typed plans recorded before their effects. Local transfers report real byte counts and can stop between chunks; remote actions can stop between items. Retry processes only known unfinished work. Restart marks uncertain work interrupted and never silently replays it.

Undo is guarded local recovery: it refuses changed files or destination collisions and keeps removed outputs recoverable. Remote and cross-filesystem Undo are explicitly unavailable. History retains at most 128 operations and one plan accepts at most 1,024 top-level actions. Journal files live in the private `operations` configuration directory. Interrupted staging files are identified for inspection rather than blindly deleted.

## Workspace AI

The optional TypeScript AI plugin adds **Ask these panes**, **Semantic search these panes**, **Workspace memory**, and **Reindex semantic search** to the command palette. Install it explicitly from the checkout with `scripts/install-ai-plugin` (Node.js 22.19+ and npm required), then reload plugins from the palette. Startup never downloads packages. In **Settings → Plugins → Workspace AI**, choose **Continue with ChatGPT** to connect an eligible ChatGPT plan. New setups use ChatGPT by default; OpenAI/Anthropic API keys and compatible local endpoints remain available through the provider setting. Embeddings have their own endpoint and credentials. See [plugin setup and limits](plugins/ai/README.md).

A command opens or focuses one agent pane for that plugin and workspace. Enter sends a question; Shift-Enter inserts a newline. Real answer text streams as Markdown into a bounded conversation transcript. Tool activity stays in one collapsible disclosure. Stop or Escape preserves partial text, and Retry starts a fresh request. Copy actions belong to individual replies and code blocks. Scroll back to read earlier content without being pulled to the bottom; Jump to latest resumes following. Tab / Shift-Tab reaches the available actions, memory and validated references. Cited files can be previewed or opened in an ordinary file pane. Results opens a normal file list with reasons and snippets. Existing pane-switch, split, close, text-size and theme settings continue to apply.

Workspace memory is an explicit notes editor: inspect it offline, choose Save memory to replace it (including an empty value to clear it), or Revert edits to discard a draft. Closing or quitting asks about unsaved notes. State lives under `$BUCKETS_CONFIG_DIR/agents/<plugin>/<workspace-id>` (the default configuration directory is `~/.config/buckets`). An explicit launch root set defines its stable workspace identity; reopening a different root set keeps separate notes and conversations. Ordinary navigation retains that identity, and launching without paths restores the saved workspace. Reopening a workspace restores agent locations as file panes; it does not resume a cancelled request or rerun a model.

ChatGPT registrations and tokens live separately under the configuration
directory's private `accounts/<plugin>` vault. They are never settings values or
workspace notes. Settings supports saved accounts, model selection, sign-out and
Manage usage. Switching accounts resets model context while retaining explicit
workspace notes. Usage limits stop a request; Buckets does not silently charge a
configured API key instead.

Host tools are read-only and bound to explicitly open pane roots plus configured `index_roots`, intersected with the installed plugin's permissions. Configured index roots control search scope and their `.gitignore` policy; they do not silently widen to every open pane. File-pane changes reject stale answers; workspace-memory acknowledgments remain independent of file navigation. Real answer snapshots stream into the pane while the harness runs; final citations and file actions require host validation. Installed plugin code is trusted executable code, not an OS sandbox; model and embedding requests can send admitted content to the configured provider.

## Locations

```text
/Users/me/projects
ssh://my-host/home/me/projects
s3://my-bucket/prefix
s3://my-bucket/prefix?endpoint=https%3A%2F%2Fs3.example.com
git:///Users/me/projects/repository?ref=HEAD&path=src
plugin://my-provider/path
```

SSH uses your SSH configuration/agent and requires Python 3 on the host. Remote search uses `fd` or `rg` on the host. S3 uses the AWS CLI and its existing credential configuration; compatible endpoints must support the conditional operations Buckets uses to prevent overwrites. Git revision locations are read-only; open the repository's local path to edit its working tree. Read the verification notes for transfer size limits and the boundary between tested adapters and live remote services.

Git row badges come from the Git status plugin, including changed descendants on directory rows. Plugin commands receive the current location and selected files, and may declare their own shortcuts. Settings exposes terminal destinations and tool executables under Developer tools.

## Settings

Press **⌘,** to open Settings. Filter preferences, then use Tab / Shift-Tab and Enter to change text size, hidden files, sidebar, preview, and plugin options. The category control beside search cycles through Appearance, Browsing, Places, Plugins, Shortcuts and Advanced; use Left / Right when it is focused. Changes save and apply immediately. Escape or ⌘W returns to the workspace; existing terminal processes keep running. Unsaved JSON prompts before closing Settings or quitting the app.

Preferences live in `~/.config/buckets/settings.json`, or `$BUCKETS_CONFIG_DIR/settings.json` when configured. Choose **Edit settings.json** for the built-in JSON editor, then **⌘S** to validate and save. The file uses strict JSON, without comments. After editing it in another program, choose **Reload from disk**. Unsaved drafts and external-edit conflicts require an explicit save/discard choice.

For example, these overrides increase text size and use iTerm2 for the developer-tools shortcuts:

```json
{
  "font_size": 15,
  "show_hidden": false,
  "sidebar": true,
  "preview": true,
  "plugins": {
    "native-tools": {
      "enabled": true,
      "config": { "terminal_target": "iterm2" }
    }
  }
}
```

In **Shortcuts**, filter by action name or stable ID, then press Enter on a binding to record it. Press the desired key combination, Tab to **Apply**, and press Enter to save. **Clear** disables the action; its Reset row restores defaults. Escape cancels without changing the saved binding. Capture, Apply, Clear and Cancel are keyboard reachable, and app commands are intercepted while the recorder owns focus, so recording Command-Q or Command-W does not execute them. Modifier-only presses and held-key repeats are ignored.

A recording replaces the action's alternatives with one chord. Use the advanced JSON editor for multiple alternatives. Changes apply immediately after a successful save; invalid, conflicting, or failed writes keep the candidate open and the saved settings unchanged. Native editor clipboard/undo, terminal program input, file navigation keys, Tab / Shift-Tab, and Escape remain fixed.

Shortcut overrides are stored under `keybindings`. An empty array disables an action; removing its key restores its default. Plugin action IDs use `plugin.<plugin-id>/<command-id>` and can be saved before the plugin is installed. Core commands take priority over conflicting plugin defaults; skipped bindings show a warning and no active shortcut in the palette.

```json
{
  "keybindings": {
    "pane.split": ["cmd-\\"],
    "file.quick_look": ["space", "cmd-alt-p"],
    "plugin.native-tools/open_nvim": ["ctrl-e"],
    "view.sidebar": []
  }
}
```

Files-only shortcuts apply while browsing. Pane focus, split and command palette shortcuts also work in terminals. Settings, Quit, Close, and text sizing apply across the app; their bindings require Command or a function key, and native editing / JSON-save shortcuts are protected. Full alternatives remain visible in the Shortcuts category and JSON.

Appearance changes use the same immediate save/validation flow. Edit interface and terminal font families separately, or a semantic color's hex value beside its swatch. Clear an edit or use its Reset row to restore that default. **Reset fonts and colors** restores Tokyo Night and the ANSI palette while keeping text size and other preferences. A requested font that is not installed stays in the file, uses bundled JetBrains Mono at runtime, and shows a fallback notice.

```json
{
  "appearance": {
    "font_family": "Helvetica",
    "terminal_font_family": "JetBrains Mono",
    "colors": { "accent": "#82AAFF", "directory": "#86E1FC", "background": "#1A1B26" }
  }
}
```

Color keys are `background`, `chrome`, `surface`, `border`, `text`, `muted`, `accent`, `directory`, `success`, `warning`, `danger`, `cursor` (active file row), and `marked` (marked file rows). The terminal caret uses `accent`; terminal selection uses `border`. Existing terminals update in place. Advanced JSON can set `appearance.terminal_ansi` to an array of exactly 16 `#RRGGBB` strings, ordered normal black–white then bright black–white. Missing values inherit defaults; invalid colors or palette lengths are rejected without changing the saved settings. Use a monospaced terminal font for consistent TUI alignment.

Plugin configuration merges with each manifest's defaults. Settings offers controls declared by the plugin; arbitrary nested configuration remains available in JSON. Workspace locations are stored separately in `workspace.json`. Existing appearance preferences migrate on the first launch without a settings file.

Places combines your saved locations with optional built-in shortcuts in one list. Saved locations appear first, in their stored order; a duplicate built-in target is omitted. Press **⌘Shift-B** to save the current location, or **⌘K → Open Places** to navigate by keyboard. In **Settings → Places**, rename/remove saved locations and toggle Home, Desktop, Downloads, Launch directory and Root. Launch directory is the directory the app started in; named workspaces are separate. **⌘B** toggles the sidebar, which hides automatically in compact layouts.

Saved locations continue to use the `bookmarks` array in settings.json, preserving existing entries and full SSH hosts/paths, S3-compatible endpoints and Git references. Edit locations and ordering through **Edit settings.json**, or reload after an external edit. For example, this keeps only Home and Downloads among the built-in shortcuts:

```json
{
  "places": { "builtins": ["home", "downloads"] },
  "bookmarks": [
    { "name": "Remote projects", "location": "ssh://my-host/home/me/projects" },
    { "name": "Build artifacts", "location": "s3://my-bucket/builds?endpoint=https%3A%2F%2Fs3.example.com" }
  ]
}
```

The built-in IDs are `home`, `desktop`, `downloads`, `launch_directory` and `root`. An empty `builtins` array shows only saved locations; omitting `places` retains the default shortcuts. Hiding a shortcut does not disable its direct Go command. Existing `bookmark.add` / `bookmark.open` shortcut IDs remain compatible.

SSH authentication continues to use your SSH configuration and agent. Saved places store locations, not passwords or private keys.

## Development

```sh
scripts/check
```

This checks formatting, runs workspace tests (including real PTY acceptance), checks the native application, and runs Clippy. macOS CI runs the same checks. Build dependencies are pinned in `Cargo.lock`. Real Neovim and Lazygit tests run explicitly as described in the verification notes.

| Crate | Responsibility |
| --- | --- |
| `buckets-core` | Locations, filesystem providers, safe operations, search, pane state |
| `buckets-plugin` | Plugin manifests, protocol, capability dispatch, bounded subprocesses |
| `buckets-terminal` | Alacritty terminal model, PTY processes, GPUI terminal rendering and input |
| `buckets-app` | GPUI workspace, keyboard actions, inputs, clipboard and drag/drop |

Read [architecture](docs/architecture.md), [plugin authoring](docs/plugins.md), [product requirements](PRODUCT.md), and [design rules](DESIGN.md). Implementation and verification status is recorded in [verification](docs/verification.md).

## Plugins

Git status decorations and Neovim, Lazygit, and terminal launch commands are supplied by first-party plugins. The default terminal is embedded; iTerm2 remains available. The app installs these plugin sources in its configuration directory on first launch. Use the command palette to discover the commands and their shortcuts.

Plugins communicate over newline-delimited JSON-RPC, with a manifest declaring search, provider, command, decoration, or terminal capabilities. A terminal command returns a typed launch request containing the executable, arguments, working directory, and environment; Buckets owns the resulting terminal process. Rust, Python and TypeScript examples live in `examples/plugins`. Plugin executables run with your user account's OS permissions; install code you trust.

A real FFF SDK example is included. It requires its Python dependency and installation in the configured plugin directory; see the plugin guide for setup. To try the repository examples after installing dependencies, launch with `BUCKETS_PLUGIN_DIR="$PWD/examples/plugins" scripts/dev`. The examples initially grant access only to this repository; edit their declared roots for your own projects. The semantic/hybrid search protocol includes indexing roots, ignore behavior, arbitrary model/store configuration, and ranked results. The embedding/indexing implementation is intentionally left to a future plugin.
