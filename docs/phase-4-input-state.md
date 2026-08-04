# Phase 4: explicit input state

The first Phase 4 slice replaces implicit GTK keyboard branches with a GTK-independent `AppMode` state machine.

## Modes

- `Normal` dispatches navigation and operation commands.
- `Find` owns incremental filename matching until Enter or Escape.
- `TextInput` owns text entry for create-file, create-directory, and rename actions.

Transitions are explicit and mutually exclusive. A new mode cannot start while another mode is active. Enter completes the current mode only after validation; Escape returns to Normal without performing the pending action. Navigation also resets transient input state.

Visual and command modes will extend the same enum in later Phase 4 slices.

## Integrated text input

Commands that require text no longer open a separate GTK dialog. `a f`, `a d`, `r`, and `F2` display an input overlay above the main window using the same visual language as filename find:

- typed Unicode characters update the value;
- Backspace edits the value;
- Enter validates and submits;
- Escape cancels;
- invalid empty or path-like names remain in the mode with an inline explanation.

Rename starts with the current name selected semantically: typing replaces it, while Backspace clears it. The source URI is captured when rename begins, so a mouse selection change cannot redirect the pending operation to another item.

Binary confirmations remain modal yes/no dialogs. Trash, permanent deletion, and paste-conflict decisions therefore keep the stronger interruption appropriate for destructive or safety-sensitive choices.

## Verification

Core tests cover mutually exclusive transitions, cancellation, input validation, and replacement of the initial rename value. Workspace formatting, strict Clippy, and tests remain required for every slice.
