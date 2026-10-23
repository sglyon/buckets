# Settings

Mode: Operate. Extend the existing native workspace; retain DESIGN.md's Tokyo Night palette, bundled monospace type, rem sizing and quiet chrome. The user pinned a Zed / VS Code style settings experience, opened with Command-comma and backed by JSON. Build directly in GPUI.

## Purpose

Change real application and plugin preferences without leaving the keyboard. Common controls and advanced JSON share one persisted source of truth. Existing file panes and terminal processes survive visiting Settings.

## Composition

A workspace settings page with a clear title, global search, a left section rail and one continuous grouped form on the right. Group Appearance, Browsing, Places, Plugins, Shortcuts and Advanced configuration in that order. Section clicks jump to headings; manual scrolling updates the active section from actual layout without moving keyboard focus. Search filters across all sections and disables navigation to groups with no matches. Values, descriptions, focus and save outcomes carry the hierarchy; avoid decorative cards or dashboard metrics. Show the settings file location beside an explicit JSON editing action.

Expose working preferences only: base text size, application and terminal font families, semantic theme colors, hidden files, places sidebar, file preview, plugin enabled state, and configuration declared by each plugin. Terminal destination and tool paths belong to the developer-tools plugin. Keep arbitrary nested plugin configuration available through JSON.

Appearance inherits Tokyo Night defaults and permits named color overrides in settings.json. Show token names, hex values and swatches with keyboard editing and reset actions. Apply successfully persisted changes across workspace, inputs, previews and existing terminals. Invalid values retain the working theme; unavailable font families retain the preference while using bundled JetBrains Mono with a visible explanation. Advanced JSON permits the terminal ANSI palette. Theme changes preserve terminal processes and workspace state.

Saved bookmarks extend the existing Places surface. Command-Shift-B names the current location, retaining the complete provider URL. Places and the command palette open saved locations; Settings and palette actions rename or remove them. Persist the name and full location in settings.json, including SSH host/path, S3 endpoint/prefix and Git reference/path. Keep credentials in the existing provider authentication mechanisms.

Shortcuts reuse preference rows with an inline key recorder. Show each application's or plugin command's effective binding and its scope. Opening a row focuses Capture; pressing a combination previews it without running the command. Tab / Shift-Tab reaches Capture, Apply, Clear and Cancel. Enter or Space can be recorded while Capture is focused; those keys activate the selected action control otherwise. Escape cancels without saving. Apply replaces the bindings with the captured combination, Clear disables the action, and Reset restores defaults. Multiple alternatives remain available in advanced JSON.

Persist overrides in the same settings file and apply successful changes live. Menu accelerators, command palette hints and workspace hints follow the effective map. Invalid keys, conflicts and write failures retain the last working configuration and captured candidate. Modifier-only presses and held-key repeats do not create bindings. Recording intercepts keys before application shortcuts, including Quit and Close; leaving the recorder restores normal routing. Native input editing, fixed navigation/recovery keys and raw terminal keys remain separate from application-command bindings.

## Interaction contract

Account-capable plugins expose connection status and account actions in Plugins.
Workspace AI defaults new setups to Continue with ChatGPT, with API-key and local
providers retained in the provider selector. Sign-in opens the system browser;
the Settings page remains usable, shows progress and offers cancellation. Saved
accounts, account-specific models, sign-out and Manage usage are reachable with
the same keyboard row navigation. A first successful plan connection explains
plan usage once. Credentials are private runtime state, never settings JSON.
Disabled plugins and stale requests cannot reconnect or replace current state.

Command-comma opens or reuses Settings from files, terminals and inputs. Tab/Shift-Tab reaches search, the section rail and controls. Arrow keys choose a section while the rail owns focus; Enter jumps and Tab enters that section's controls. Sections remain part of one continuous form. Section jumps preserve the query and refuse to abandon shortcut capture or invalid/pending inline edits. Command-S saves the JSON draft after validation. Form changes apply live when successfully saved. Close returns to the prior workspace focus; dirty JSON needs a clear save/discard path. Invalid JSON, write failures and external-edit conflicts preserve the last applied configuration and explain recovery.

## Responsive and state contract

Use the existing 13-point base and support its full 9–24-point range. At 960×640, navigation, inputs, confirmation and error recovery remain visible or keyboard-scrollable. Long paths and plugin descriptions wrap or truncate intentionally. Loading, no matching settings, disabled plugins, invalid configuration and successful save receive textual states. Motion conveys focus and state changes only.

The rail uses 184 physical pixels, reducing to 168 below 1100 pixels, and scrolls independently when large text needs more height. Compact rows stack their title and value. Selected editors and recorder controls remain scrollable within the main column. A query with no matches keeps search usable and never activates a stale control index. Scrollspy uses measured content geometry, including the final short section and dynamic row heights; scrolling performs no provider call or preference write.

## Verification

Independent persistence/plugin tests establish data behavior. Native keyboard checks cover entry/return from files and terminals, preference changes, JSON validation/save/reload, plugin configuration, persistence and compact large-text operation. Capture the standard page, plugins, JSON editing/error and compact large-text states in `.impeccable/review/`; visual review applies only to captured states.

Section navigation requires actual native click/jump, manual scrolling and keyboard observations at desktop 13pt and compact 960×640/24pt. Static captures and pure geometry tests alone cannot prove scrollspy event behavior. Verify global search, missing/empty matches, dynamic editors, recorder cancellation and dirty JSON guards within one bounded batch, followed by at most one consolidated repair/confirmation. Keep the normal app, user configuration and credentials intact.

The 2026-10-23 bounded pass completed on final source `73911c7`, UUID `32C08689-9FF5-3EB0-8736-792E489D507D`. The initial batch established cross-section search, wheel updates through the bottom, recorder cancellation and dirty JSON recovery; one compact physical-padding repair and confirmation established readable one-line rail labels, reverse scrolling independent of focus and final desktop heading alignment. `.impeccable/review/settings-scrollspy-verdict.md` and `docs/verification.md` record initial/final build attribution and exact limits. Coverage excludes a complete inline-editor, theme/font and JSON matrix.
