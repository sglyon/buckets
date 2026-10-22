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

## Streaming conversation

Use a quiet editor-native transcript within the existing theme. Submitted
questions form compact prompt blocks; assistant Markdown grows in place as real
text arrives. Preserve a bounded history of turns in the current pane. Do not
imply that this visible transcript restores across launches; the plugin's
durable model state has a separate lifecycle.

Paragraphs, headings, lists, inline code and fenced code have a clear reading
hierarchy. Keep completed blocks stable while the active block changes. Text is
selectable; response and code copy preserve exact source text. Model links,
images and raw HTML cannot trigger external navigation or network loads. Only
host-validated final references enable file actions.

One collapsible Activity disclosure holds bounded tool updates. Show useful
reading/responding/verifying phases; do not display raw protocol, JSON envelopes,
internal reasoning or a growing status dump. Partial text is provisional until
the final result is validated. Stop and failure retain the partial reply with
an explicit state and retry action.

The bottom composer supports multiple lines, Enter to send and Shift-Enter for
a newline; composing an IME candidate must not submit. Keep draft and focus
stable during updates. Stop is the primary active-request action. Copy, Retry,
Results and reference actions appear in the relevant turn rather than a
permanent row of controls that cannot act.

Follow the tail only while the reader remains at the bottom. Scrolling back
pauses following and reveals a keyboard-reachable Jump to latest. Streaming
must not steal focus, retarget a selected control or yank the reader away from
history. Control identities belong to stable turn/item IDs. Account/provider
changes clear visible conversation context and invalidate pending work;
explicit workspace notes retain their own lifecycle.

At 960×640 with 24-point text, transcript, composer and recovery remain visible
or keyboard-scrollable. Narrow panes stack actions and references; long code
scrolls horizontally. The composer grows to a bounded height and then scrolls
internally, preserving space for the transcript.

## Evidence boundary

Transport, broker policy, SDK and durable state require deterministic integration
tests. Actual provider calls require configured credentials; fixture responses
must never be presented as live model coverage. Native presentation and keyboard
checks are separate from source and headless tests. Capture desktop and compact
states in one bounded pass if the native automation connection is available.
