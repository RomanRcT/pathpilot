# Phase 4: explicit input state

The first Phase 4 slice replaces implicit GTK keyboard branches with a GTK-independent `AppMode` state machine.

## Modes

- `Normal` dispatches navigation and operation commands.
- `Find` owns incremental filename matching until Enter, Escape, or the idle commit delay.
- `Filter` owns incremental name filtering until Enter accepts it, Escape clears it, or the idle commit delay applies it.
- `TextInput` owns text entry for create-file, create-directory, and rename actions.
- `Visual` owns an anchored, inclusive selection range while keeping one active cursor.

Both query modes also commit themselves after `[ui] query_commit_delay_ms` without a keystroke, so focus is back with navigation without any extra key. Transitions are explicit and mutually exclusive. A new mode cannot start while another mode is active. Enter completes the current mode only after validation; Escape returns to Normal without performing the pending action. Navigation also resets transient input state.

Command mode will extend the same enum in a later Phase 4 slice.

## Integrated text input

Commands that require text no longer open a separate GTK dialog. `a f`, `a d`, `r`, and `F2` display an input bar above the main window using the same visual language as filename find. The bar embeds a real `GtkEntry`, while `AppMode` remains the source of truth for mode transitions and validation:

- typed Unicode characters and clipboard actions update the value;
- Left/Right, Home/End, selection, Delete, and Backspace use standard GTK editing behavior;
- `Ctrl+C`, `Ctrl+X`, and `Ctrl+V` operate on the entry while it owns focus instead of dispatching file operations;
- Enter validates and submits;
- Escape cancels;
- invalid empty or path-like names remain in the mode with an inline explanation.

Rename starts with the current name selected in the entry. Typing replaces the selection, or the user can reposition the cursor and edit individual characters. The source URI is captured when rename begins, so a mouse selection change cannot redirect the pending operation to another item.

Binary confirmations remain modal yes/no dialogs. Trash, permanent deletion, and paste-conflict decisions therefore keep the stronger interruption appropriate for destructive or safety-sensitive choices.

## Verification

Core tests cover mutually exclusive transitions, cancellation, input validation, replacement of the initial rename value, and the explicit Filter mode transitions. Workspace formatting, strict Clippy, and tests remain required for every slice.
