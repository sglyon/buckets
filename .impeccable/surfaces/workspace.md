# Workspace controls and operations

Mode: Operate. Extend the existing file-browsing surface and command palette.
The established Tokyo Night tokens, configurable fonts, active-pane treatment,
file selection and focus rules remain authoritative.

## Everyday flow

Most of the window belongs to file panes. Sorting and metadata controls belong
to the pane and palette. Named-workspace save/open/rename/remove are palette
commands. Operations have a compact status entry and an on-demand history/detail
surface, without a permanently expanded dashboard.

Use existing controls for pending, complete, failed, cancelled, interrupted and
undone states. State words and actual file counts/bytes carry meaning; color
supports the words. Do not manufacture speed, percentage or ETA when a provider
does not expose the required information. Cancellation that can only happen
between provider actions must say so.

Cancel, Retry and Undo appear only when relevant to the selected operation.
Unsupported recovery explains its limitation. Partial success names the files
that completed, and Retry does not silently repeat them. Recovery never implies
that a changed file will be overwritten.

## Keyboard and layout

All actions remain reachable through the palette and ordinary control focus.
Enter activates the selected action; Escape returns to browsing. Native editor
and terminal bindings retain ownership. Keyboard pane resizing must be possible
without dragging a divider. Pointer and keyboard resizing update the same
persisted proportions. Layout changes preserve pane IDs and running content.

At desktop and 960-point compact widths, controls wrap or disclose progressively
while preserving the file list and primary action. At 24-point text, details
scroll and the active control remains reachable. Selected rows, marked files
and keyboard control focus retain distinct treatments.

## Performance and persistence

Render cached operation snapshots; never await a journal lock held by running
I/O. Coalesce progress and bound history. Sort and filter projections update
when their inputs change rather than allocating keys per rendered row.

Named workspaces preserve full provider addresses and view geometry. Opening a
workspace respects dirty editors and invalidates prior agent work. Restored
locations do not execute saved terminal commands or model requests. Credentials,
theme and global keybindings retain their existing separate storage.

## Evidence

Mutation and metadata invariants require real-file tests. The native check is
one batched initial desktop/compact inspection and one confirmation after a
single repair batch. Screenshots demonstrate only their displayed states;
keyboard, cancellation and recovery need their own observed results.
