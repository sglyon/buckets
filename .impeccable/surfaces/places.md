# Places refinement

Mode: Operate. Preserve the established Tokyo Night file-browser geometry,
tokens and Settings controls. This consolidates navigation terminology, without
adding a new dashboard or replacing the visual identity.

## Contract

One Places list combines user-saved locations with optional built-in shortcuts.
Saved locations come first in their stored order. A saved location suppresses an
identical built-in target. Existing bookmark data, provider URLs, opaque metadata
and stable shortcut IDs survive without migration. Settings owns built-in
visibility and saved-place rename/remove; JSON remains available for location
and order edits. The launch directory must not be called Workspace, because
named workspaces are a separate concept.

The palette exposes the same effective list for keyboard navigation. Direct Go
commands remain available even if a shortcut is hidden from Places. Save,
Open, Rename and Remove use consistent Places terminology. Compact layouts keep
the existing auto-hidden sidebar rule; palette and Settings stay reachable.

## Verification budget

Real-file tests cover legacy data, full provider addresses, list order/dedup,
validation, toggle persistence and external-change refusal. Use one shared
Cargo target with incremental/debug info disabled. Native verification uses
owned offline fixtures at desktop13pt and compact24pt in one initial batched
pass; one consolidated repair/confirmation is the ceiling. Preserve the human
session/configuration and remove disposable build caches only after delivery.
