# Changelog

All notable changes to PathPilot are documented in this file.

## [Unreleased]

### Added

- Persistent Vim-style directory sorting by name, extension, size, or modification time through the `s` prefix.
- Debounced GIO monitoring for the current and parent directories, preserving selection across external filesystem changes.
- Native embedded shell terminal opened with `o t`, starting in the active directory and synchronizing shell directory changes back to the browser.
- Header-bar controls for hidden items and the terminal, plus a keyboard hidden-item toggle on `.`.
- Persistent `preview_line_numbers` and `color_scheme` UI settings.
- Theme-aware source and Markdown previews with optional line numbers.

### Changed

- Migrated the application shell, window, header bar, and About dialog to libadwaita.
- Preview syntax colors now follow the effective light or dark appearance.

### Fixed

- Terminal focus now bypasses PathPilot's modal key handler and remains in VTE while shell directory changes update the browser.
- Application appearance now follows GNOME's modern color-scheme preference instead of the legacy GTK theme name.

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
[Unreleased]: https://github.com/RomanRcT/pathpilot/compare/v0.1.0...HEAD
