# Milestone 4: three-column navigation

Milestone 4 turns the three visual columns into independently loaded filesystem views:

- **Parent** lists the active directory's siblings and selects the active directory.
- **Current** lists the active directory and owns the navigation cursor.
- **Preview** lists children of the selected directory or shows basic metadata for a selected file.

## Input

- `j` / `k`: move the current-column cursor.
- `l`: enter the selected directory or open a file with its default application.
- `h`: move to the parent directory.
- `gg` / `G`: select the first or last current entry.
- Single click changes selection and preview.
- Double click opens directories and files in any visible pane.

All inputs converge on `AppCommand` or the same entry-opening function. GTK widgets do not define navigation semantics.

## State and loading

`NavigationState` stores the active URI and the cursor position for every visited directory. Returning through `h` prefers the directory just left, while revisiting another directory restores its remembered cursor.

The three `DirectoryPane` instances keep their widgets and row factories alive. Each pane owns a generation tracker and `gio::Cancellable`; reloading a pane cancels its prior enumeration, clears only that pane's store, and rejects stale events.

## Runtime verification

On the developer Fedora workstation on 2026-08-03, the debug build presented the window in 110 ms. Initial concurrent loads completed for the 13-entry current directory and 98-entry parent directory in 164 ms; the initially selected directory preview loaded 10 entries in 309 ms.
