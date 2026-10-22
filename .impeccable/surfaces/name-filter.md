# Inline name filter

Mode: Operate. Extend the existing Tokyo Night GPUI workspace. The user requested an fzf-inspired name filter on `/`, using a Vim-style command line at the bottom instead of a modal.

Keep file panes visible and undimmed. A compact bottom input shows the query, match count and keyboard hints. Plain text filters names in the active pane immediately with fuzzy matching; empty text restores its current entries. Arrow keys and Control-N/Control-P navigate results, Enter opens the selected entry, and Escape clears the filter and restores browsing state. Ordinary `/` in a terminal belongs to its program.

Explicit `fd:`, `rg:` and `fff:` prefixes retain existing broader search operations when submitted. Distinguish those searches from the live current-directory filter in the input hint. Do not change locations in other panes or lose marked files when projecting the visible list. Retain keyboard focus and usable input at the 24-point maximum in a 960×640 window.

Verification covers Unicode-safe matching, stable ordering, clear/restore, opening a filtered result, search-prefix dispatch and terminal key ownership. Native captures show the bottom input in context, including compact large text.
