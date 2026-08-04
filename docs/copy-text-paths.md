# Copy names and paths

The `y` command family separates PathPilot's operation clipboard from the
desktop text clipboard:

- `y y` copies the selected item or Visual range for a later `p` operation;
- `y n` copies filenames;
- `y d` copies containing-directory paths;
- `y p` copies full paths including filenames.

`Ctrl+C` remains an immediate shortcut for operation copy. Text variants write
newline-separated values in list display order, preserve spaces and Unicode
without shell quoting, and leave the operation clipboard unchanged. Local
`file://` locations are decoded to ordinary filesystem paths; other backends
remain URIs.

All four continuations appear in the pending hint panel, F1 reference, and
command palette. The corresponding configurable keymap command names are
`copy`, `copy_name`, `copy_directory_path`, and `copy_full_path`.
