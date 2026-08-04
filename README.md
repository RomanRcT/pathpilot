# PathPilot

PathPilot is an experimental keyboard-first graphical file manager for Linux, built with Rust, GTK 4, and GIO. It combines ranger-style three-column navigation, Vim-inspired controls, conventional mouse interaction, and responsive file previews.

The project is in early development. The initial milestones, Phase 2 preview pipeline, and Phase 3 file-operation foundation are implemented. See [`pathpilot_project_plan.md`](pathpilot_project_plan.md) for the complete product plan. PathPilot currently requires GTK 4.12 or newer.

## Current features

- Three live parent/current/preview columns with independently cancelable models.
- Virtualized `GtkListView` instances populated asynchronously through GIO.
- Batched GIO enumeration with cancellation and stale-generation rejection.
- Name, native content-type icon, kind, size, and modified-time columns.
- Case-insensitive name sorting with directories first.
- `h/j/k/l`, `gg`, and `G` navigation with cursor restoration per directory.
- `q` closes the current window.
- Single-click selection and double-click directory/file opening.
- Preview requests are debounced by 75 ms, cancelable, and generation-checked.
- Bounded text previews and asynchronously decoded, viewport-scaled image previews.
- Empty files have an explicit empty state; YAML MIME variants are treated as text.
- Source previews are syntax-highlighted off the GTK thread with `syntect`.
- Markdown headings, emphasis, lists, tables, links, and code are rendered without executing HTML or loading remote content.
- A 24-entry LRU preview cache is invalidated by URI, size, and modification time.
- A status line with the selected index and entry count.
- Structured startup, model creation, and selection tracing.
- GTK-independent commands and key-sequence parsing.
- Keyboard-driven create, rename, trash, copy, move, and paste operations.
- Recursive directory copy with progress, cancellation, partial-result cleanup, and no silent overwrites.
- Explicit “Keep Both” conflict handling with unique destination names.
- Separately confirmed permanent deletion of files and non-empty directory trees.
- Explicit Normal, Find, and integrated Text Input modes.
- In-window create and rename input with validation instead of text-entry dialogs.

The application opens the directory from which it is started. Use `l` or double click to enter a directory and `h` to return to its parent.

## Current limitations

- Local filesystem browsing only.
- Single-item operations only; multi-selection will follow the Phase 4 visual mode.
- No tabs, configurable keymap, filtering, or hidden-file toggle yet.
- No PDF, archive, office document, audio, or video previews.
- No remote filesystem backends or plugin API.
- Fedora and Wayland are the primary development targets; other Linux environments are not yet validated.

## Roadmap

The next major steps are:

1. Complete modal input handling, visual selection, command palette, and TOML keymap configuration.
2. Extend visual selection into safe multi-item operation batches.
3. Add drag-and-drop, open-with integration, accessibility, diagnostics, and Fedora packaging.

Detailed phases, performance targets, and exit criteria are maintained in [`pathpilot_project_plan.md`](pathpilot_project_plan.md).

## Fedora development setup

Install the native dependencies:

```bash
sudo dnf install rust cargo gtk4-devel gcc pkgconf-pkg-config
```

Build and run:

```bash
cargo run -p pathpilot
```

Enable detailed logs with:

```bash
RUST_LOG=pathpilot=debug cargo run -p pathpilot
```

Run project checks:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

GTK 4 applications require a graphical session. PathPilot targets Wayland first but can also use another GDK backend supported by the local GTK installation.

## License

PathPilot is free software distributed under the [GNU General Public License v3.0 or later](LICENSE).
