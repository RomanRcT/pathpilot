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

File-operation commands are deliberately blocked during this slice. The next Phase 4 slice will connect `selected_entries()` to cancellable batch copy, move, trash, and delete operations. Until then, no command can silently act on only the cursor item while multiple rows appear selected.

## Verification

Core tests cover entering Visual mode, forward and backward ranges, inclusive counts, bounds clamping, and the `v` command mapping. Workspace format, strict Clippy, and test checks remain mandatory. A manual GTK smoke test verifies native multi-row highlighting and mouse behavior.
