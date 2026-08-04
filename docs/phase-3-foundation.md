# Phase 3: file operations foundation

The first Phase 3 slice establishes a GTK-independent operation model, cancelable GIO primitives, and keyboard-driven GTK entry points for the initial operations.

## Implemented primitives

- create an empty file;
- create a directory;
- rename a file or directory;
- move an item to the FreeDesktop trash through GIO;
- recursively copy files, directories, and symbolic links;
- move files and complete directory trees;
- permanently delete files or non-empty directory trees after a separate warning.

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

Copy and move use asynchronous GIO operations, expose item and byte progress in the status line, reject recursive destinations, and never silently overwrite an existing destination. Directory copy performs an asynchronous cancellable preflight, enumerates children in batches, creates directories in parent-first order, copies metadata without following symbolic links, and asynchronously removes the newly created partial destination after failure or cancellation. A copied item remains in the operation clipboard for repeated pastes; a moved item is cleared only after success. The status line keeps the current `COPY` or `CUT` clipboard visible. `Escape` cancels an active transfer and the operation remains active until its completion callback arrives.

When a paste destination exists, the UI asks whether to cancel or keep both items. “Keep Both” chooses the first available numbered name, preserving the existing item. A race that creates the same destination after the prompt is still reported as a conflict rather than overwritten.

`d D` or `Shift+Delete` invokes permanent recursive deletion behind a distinct irreversible-action warning. Ordinary `d d` and `Delete` continue to use the desktop trash. Errors include the URI of the affected item.

## Verification

Temporary-filesystem integration tests create and rename a file, verify that conflicts do not overwrite existing data, recursively copy nested directories, move files and directories, cancel a copy without leaving a destination, permanently delete a non-empty tree, classify structured GIO errors, and reject path traversal and recursive transfer destinations. Core tests verify copy-clipboard retention and successful move-clipboard clearing.

## Next slice

Multi-selection batches will follow the visual-selection work in Phase 4. Richer conflict policies such as replacing or merging trees remain intentionally deferred; Phase 3 always preserves existing data.
