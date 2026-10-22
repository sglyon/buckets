# Agent workspace

Mode: Operate. Extend the native multi-pane workspace and configured runtime
theme. Preserve all file, terminal, Settings and shortcut workflows.

## Purpose

Ask questions about the current panes, turn grounded search results into normal
file navigation, and retain explicit workspace notes across restarts. Model and
durable state belong to a plugin; Rust owns provider reads and workspace state.

## Interaction contract

Expose plugin commands in the existing command palette and configurable shortcut
catalog. The agent surface has a keyboard-accessible prompt, request progress,
cancel and recovery, and clearly labeled answer, citations and workspace memory.
Do not invent model responses while configuration is missing. Explain the needed
setup and retain the prompt after a failed request.

For ChatGPT-authenticated requests, show Using ChatGPT plan with Manage usage
near the composer. Keep that state distinct from merely having a saved account
while an API-key or local provider is selected. Missing authorization offers a
direct path to Settings. Plan-limit failures preserve the prompt and provide
usage recovery; they never silently change the billing provider. Account changes
cancel or invalidate requests that started under the previous connection.

Citations navigate to actual provider locations. Search results retain source
locations, match explanations and optional lines, and can become a normal file
pane without moving files. Terminal processes and unrelated panes survive agent
requests. User-edited memory is inspectable and explicitly saved; a failed save
retains the draft. Conversation persistence and active request state remain
distinct from authoritative filesystem state.

All controls are reachable by keyboard, including during progress and errors.
Use the configured fonts and semantic colors. At 960x640 and 24-point text,
prompt, focused controls, long errors and notes remain visible or keyboard
scrollable. Keep the established quiet Tokyo Night composition as the default.

## Evidence boundary

Transport, broker policy, SDK and durable state require deterministic integration
tests. Actual provider calls require configured credentials; fixture responses
must never be presented as live model coverage. Native presentation and keyboard
checks are separate from source and headless tests. Capture desktop and compact
states in one bounded pass if the native automation connection is available.
