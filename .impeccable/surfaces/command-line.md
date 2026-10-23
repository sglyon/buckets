# Application command line

Mode: Operate. Preserve the Tokyo Night native workspace, existing bottom
filename-filter treatment and native text-input behavior. The user requested
Vim-style `:` application commands and explicit `:!` shell execution, with a
typed plugin contract rather than tool-specific host logic.

## Task path and composition

Open from a file pane, see the frozen pane/location, type or complete an action,
submit, and see its outcome or recovery in the same context. Keep panes visible
and undimmed. Reuse accent borders, quiet chrome, keyboard-focus cues, muted
usage hints and textual error/preparation state. Bound candidate height so
960×640/24pt retains usable file content. Do not add a modal or decorative cards.

Core verbs are split/cd/rename/settings. Other stable host action IDs and qualified
plugin commands remain available. Plugin metadata owns aliases, argument kinds,
static choices and supported providers. Command completion never executes an
action. App/plugin arguments stay literal; only explicit `!` enters a shell.

## Interaction and ownership

Tab/Shift-Tab completes at the input's end, Up/Down recalls history matching the
draft anywhere without case sensitivity, Enter submits, Escape returns
or cancels preparation. Repeated completion/history keys must retain their cycle;
manual edits reset it. Paths use cached current-directory entries without
nested-prefix fetching or per-key plugin IPC; static choices remain literal.
Colon in terminals, inputs, Settings, recorder capture,
Quick Look and other existing focused modes retains that mode's ownership.
File actions must not silently retarget changed cursor/marks under a frozen
context label. Cancelled/stale requests never attach a terminal or steal focus.

Shell effects run outside operation-history Undo; process cancellation does not
roll back changes already made. This is behavior documentation, not a new modal.
History persists at most 100 accepted submissions in a private bounded file,
with no draft capture or automatic execution on restart. A running PTY has its
own lifecycle. Existing operation guards and confirmations remain authoritative.

## Verification boundary

Pure parser/path/history tests, plugin subprocess/metadata/argv/cancellation
contracts and SDK tests establish their respective boundaries. Actual native
entry, repeated completion/history, provider errors, literal typed invocation,
shell output, cancellation, input ownership and compact layout need one bounded
desktop/compact observation batch. Static captures do not prove key routing or
remote connectivity. Keep fixture commands local/offline and user data intact.

The bounded 2026-10-23 pass completed on final source `201f2c0`, UUID
`3D323591-579B-398C-8438-64F2CDD6B511`. The initial functional batch used
`61f43f0`/`701FEC72`; one compact-density repair and confirmation preserved one
complete file row, selected completion visibility and readable input/help at
24pt. A desktop restart restored accepted history. The verdict and six selected
initial/final proofs are in `.impeccable/review/command-line-verdict.md`;
`docs/verification.md` records actual interactions, tool-input/transport caveats,
offline scope and exclusions. No further polish round is required.
