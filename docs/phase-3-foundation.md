# Phase 3: file operations foundation

The first Phase 3 slice establishes a GTK-independent operation model, cancelable GIO primitives, and keyboard-driven GTK entry points for the initial operations.

## Implemented primitives

- create an empty file;
- create a directory;
- rename a file or directory;
- move an item to the FreeDesktop trash through GIO.

Every operation has an `OperationId`, typed `OperationKind`, state, progress fields, a `gio::Cancellable`, and a structured result. Common GIO failures are classified as already exists, permission denied, not found, cancelled, or other.

## Safety properties

- File creation uses `FileCreateFlags::NONE` and never overwrites an existing item.
- Names are validated before I/O; empty names, path separators, `.` and `..` are rejected.
- Trash uses the desktop's GIO implementation rather than a private trash directory.
- The operation layer contains no GTK widgets and reports completion through typed callbacks.
- Moving an item to trash always requires confirmation.

## Keyboard interface

- `a f` creates a file;
- `a d` creates a directory;
- `r` or `F2` renames the selected item;
- `d d` or `Delete` moves the selected item to trash after confirmation.
- `f` finds a filename without filtering the directory; `n` and `N` repeat the accepted search.
- `y` or `Ctrl+C` stores the selected item for copying;
- `x` or `Ctrl+X` stores the selected item for moving;
- `p` or `Ctrl+V` pastes into the current directory.

`F1` toggles a persistent, column-aligned reference containing the top-level commands. Pressing a sequence prefix replaces it with the valid continuations for that prefix, and completing or cancelling the sequence restores the main reference. The semi-transparent foreground hint does not consume layout space or disappear on a timer.

The `/` binding remains reserved for a future directory-filtering mode.

Copy and move use asynchronous GIO operations, expose byte progress in the status line, reject recursive destinations, and never silently overwrite an existing destination. A copied item remains in the operation clipboard for repeated pastes; a moved item is cleared only after success. `Escape` cancels an active transfer. The initial copy primitive handles files; recursive directory copy remains part of the next operation-engine slice, while GIO move already supports complete directory trees.

## Verification

Temporary-filesystem integration tests create and rename a file, verify that conflicts do not overwrite existing data, and reject path traversal before filesystem access.

## Next slice

Recursive directory copy, multi-selection batches, and interactive conflict policies will extend the operation engine next.
