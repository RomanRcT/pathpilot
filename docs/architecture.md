# Architecture

The workspace is split into four crates:

- `pathpilot-core`: GTK-independent application commands and the normal-mode key parser.
- `pathpilot-fs-local`: asynchronous GIO enumeration and conversion into domain entries.
- `pathpilot-ui-gtk`: GTK widgets, the virtualized list model, and command dispatch into selection changes.
- `pathpilot`: process startup, logging configuration, and GTK application lifecycle.

Input is translated to an `AppCommand` before it changes the UI. This keeps physical keys out of the public command model and allows future mouse, menu, and command-palette actions to share the same commands.

The current directory uses `GListStore`, `GtkSortListModel`, `GtkSingleSelection`, and `GtkListView`. `SignalListItemFactory` creates row widgets only as GTK needs them and rebinds those rows while scrolling. GIO publishes directory entries in bounded batches; every event carries a generation ID so the UI can discard results from a directory that is no longer current.

Each of the parent, current, and preview columns owns a persistent `DirectoryPane`. Navigation changes only their list models and labels; it does not rebuild the window or row factories. `NavigationState` remains GTK-independent and stores the last cursor position for each visited URI.
