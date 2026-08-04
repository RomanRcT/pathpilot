# Phase 4: command palette

The command palette reuses the bottom interaction panel rather than adding another overlay or modal dialog. Open it with `:` or `Ctrl+Shift+P`, type to filter by command name or displayed shortcut, move through matches with the arrow keys, and use `Enter` to dispatch the selected `AppCommand`. `Escape` returns to Normal mode without running anything.

The GTK-independent `CommandPalette` owns the query and selected result. Its catalogue maps readable titles and shortcut labels to the same `AppCommand` values used by keyboard input, so palette actions do not bypass existing mode checks or confirmation dialogs. GTK only renders the current matches and forwards editing and selection events.

The panel displays at most six matches at once and keeps the active item visible as selection moves. Commands that require further input transition into the existing integrated filename editor; destructive commands still open their yes/no confirmation.

Core tests cover filtering by title and shortcut, empty results, selection bounds, and explicit Command-mode transitions. Manual smoke tests cover both launch shortcuts, filtering, selection, dispatch into input and confirmation flows, and cancellation.
