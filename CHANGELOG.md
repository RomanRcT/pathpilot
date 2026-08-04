# Changelog

All notable changes to PathPilot are documented in this file.

## [0.1.0] - 2026-08-04

First preview release.

### Added

- Asynchronous three-column GTK4 file navigation with mouse and Vim-style keyboard controls.
- Cancelable, bounded previews for text, syntax-highlighted source, Markdown, images, metadata, and directories.
- Create, rename, trash, permanent delete, recursive copy, move, paste, conflict handling, and operation progress.
- Visual multi-selection and cancellable batch file operations.
- Integrated text input, filename search, progressive key hints, command reference, and searchable command palette.
- Configurable TOML keymap and persistent UI state, including window geometry, last directory, and per-layout dividers.
- Browse, focused-preview, and preview-only pane layouts.
- Compact mode-colored status line with file metadata and Git status.
- Human-readable local path headings, frequent places, and configurable bookmarks.
- Desktop text clipboard export for filenames, containing directories, and full paths.
- MIME-specific Open With application history and native GTK/GNOME chooser.
- Embedded Neovim editing for local text files using GTK4 VTE.
- GitHub Actions checks for formatting, strict Clippy, and the full test workspace.

### Known limitations

- Fedora/Wayland is the primary validated environment; no packaged binary is provided yet.
- Browsing and embedded editing are currently local-filesystem focused.
- Tabs, filtering, hidden-file controls, richer previews, drag-and-drop, and plugin APIs are not implemented.
- This is a preview release and configuration formats may evolve before a stable release.

[0.1.0]: https://github.com/RomanRcT/pathpilot/releases/tag/v0.1.0
