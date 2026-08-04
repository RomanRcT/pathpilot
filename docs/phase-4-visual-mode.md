# Phase 4: visual selection

Visual mode adds contiguous multi-selection without conflating selection with the active cursor.

## Interaction

- `v` enters Visual mode with the current item as both anchor and cursor.
- `j` / `k` move the cursor and extend or shrink the inclusive range.
- `gg` / `G` extend the range to the first or last item.
- `v` or Escape returns to Normal mode and keeps only the cursor item selected.
- Clicking an item while Visual mode is active extends the anchored range to that item.

The status line reports the number of selected items. The preview continues to follow the cursor item rather than an arbitrary member of the range.

## GTK model

`DirectoryPane` uses `GtkMultiSelection` for native list-row selection rendering. It separately stores a cursor position and a Visual anchor:

- Normal mode normalizes mouse and keyboard changes to one selected item.
- Visual mode normalizes them to one contiguous anchor-to-cursor range.
- programmatic selection changes are guarded against recursive signal handling.

This keeps GTK responsible for accessible selection presentation while the GTK-independent `VisualSelection` defines range semantics.

## Operation boundary

The batch-operations slice connects `selected_entries()` to copy, move, trash, and permanent delete. `y` and `x` capture the complete Visual range in the operation clipboard; destructive commands retain their yes/no confirmation and describe the selected group. Other commands remain blocked while Visual mode is active, so no command can silently act on only the cursor item while multiple rows appear selected.

Each batch has one operation ID and cancellable handle, reports aggregate top-level progress, continues after per-item failures, and returns successful destinations plus structured failures. Paste computes every destination before starting and asks once when one or more names conflict. A CUT clipboard is cleared only after every move succeeds.

## Verification

Core tests cover entering Visual mode, forward and backward ranges, inclusive counts, bounds clamping, and the `v` command mapping. Operations tests also verify that a batch continues after an item failure without overwriting existing data. Workspace format, strict Clippy, and test checks remain mandatory. A manual GTK smoke test verifies native multi-row highlighting, group operations, cancellation, and mouse behavior.
