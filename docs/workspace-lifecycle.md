# Operations, pane controls and named workspaces

This delivery extends the existing keyboard-driven workspace. It keeps the
Tokyo Night tokens, ordinary file rows, command palette and contextual controls.
It adds no permanent dashboard or promotional content.

## Delivery contract

- File mutations use typed core operation plans with stable IDs and a bounded
  durable journal. The desktop can inspect actual progress, cancel, retry
  unfinished work, and undo when the provider and recorded receipt permit it.
- Recovery refuses changed files and collisions. No inverse operation silently
  overwrites data. A restart marks uncertain work interrupted without replaying
  it. A callback or conversation ID alone is not filesystem idempotency.
- File panes offer name, size, modification time and type sorting, direction,
  directories-first, and useful metadata columns. Refresh, filtering and sorting
  preserve cursor and marks by canonical location identity.
- Pane orientation and proportions are adjustable with keyboard commands and
  pointer controls, and persisted with the workspace.
- Named workspaces save full provider locations, identity, active pane and view
  layout. Opening one restores file locations without replaying terminal
  commands or agent requests. Rename/remove change saved metadata only.
- Workspace transitions respect unsaved settings and agent notes, cancel stale
  agent work, and do not redirect ongoing file operations into a new root set.

## Boundaries

The core owns file-operation plans, receipts, progress and recovery. The desktop
owns presentation and application orchestration. Named-workspace persistence
owns only session metadata. Plugins can build future bulk rename, sync and AI
plans against the shared operation boundary; these additional workflows are not
part of this delivery.

Provider limitations must remain visible. An existing remote operation does not
gain byte-level cancellation or safe undo merely by appearing in the journal.
Only report progress and recovery guarantees that the backend can uphold.

## Verification and resource budget

Use real temporary files for mutation/recovery tests and preserve all user
accounts, settings and files. Verify changed-file refusal, collision refusal,
partial failure, cancellation, reopen, and interrupted work. Exercise named
workspace round trips, schema bounds and concurrent external-change handling.

Reuse one Cargo target directory with incremental compilation disabled and
debug information disabled for this disk-constrained verification run. Compile
the integrated app once sources are ready; avoid duplicate dependency trees.
Run focused checks during implementation and a final workspace gate after
freeze. Native verification has one batched initial desktop/compact pass and
at most one confirmation pass after a consolidated repair batch. Keep the
working app, installed plugin, user configuration and verification records;
remove disposable build artifacts after final delivery.
