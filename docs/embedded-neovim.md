# Embedded Neovim

Press `e` on a selected local text file to replace the preview with a GTK4 VTE
terminal running the user's `nvim`. PathPilot temporarily switches to the
Preview Only layout, shows `EDIT` in the status line, and lets the terminal
receive all keyboard input, including Escape.

Exit with ordinary Neovim commands such as `:q`, `:wq`, or `ZZ`. When the child
process exits, PathPilot restores the previous pane layout, focuses the current
directory list, and reloads the selected file preview.

The initial implementation intentionally supports local text files only.
Directories, binary content, non-UTF-8 paths, and remote GIO locations are
rejected with a status message. A failed `nvim` spawn also returns safely to the
previous layout.

Building this feature requires the GTK4 VTE development library. On Fedora:

```sh
sudo dnf install vte291-gtk4-devel
```
