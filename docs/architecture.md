# Architecture

The Phase 0 workspace is split into three crates:

- `pathpilot-core`: GTK-independent application commands and the normal-mode key parser.
- `pathpilot-ui-gtk`: GTK widgets, the virtualized list model, and command dispatch into selection changes.
- `pathpilot`: process startup, logging configuration, and GTK application lifecycle.

Input is translated to an `AppCommand` before it changes the UI. This keeps physical keys out of the public command model and allows future mouse, menu, and command-palette actions to share the same commands.

The synthetic directory uses `GtkStringList`, `GtkSingleSelection`, and `GtkListView`. `SignalListItemFactory` creates row widgets only as GTK needs them and rebinds those rows while scrolling.
