# PathPilot

PathPilot is an experimental keyboard-first graphical file manager for Linux. The current code implements the Phase 0 GTK 4 performance spike and Milestone 3 real-directory model described in [`pathpilot_project_plan.md`](pathpilot_project_plan.md). It requires GTK 4.12 or newer.

## Current features

- Three resizable columns.
- A virtualized `GtkListView` populated asynchronously from the working directory.
- Three live parent/current/preview columns with independently cancelable models.
- Batched GIO enumeration with cancellation and stale-generation rejection.
- Name, native content-type icon, kind, size, and modified-time columns.
- Case-insensitive name sorting with directories first.
- `h/j/k/l`, `gg`, and `G` navigation with cursor restoration per directory.
- Single-click selection and double-click directory/file opening.
- Normal-mode navigation with `j`, `k`, `gg`, and `G`.
- A status line with the selected index and entry count.
- Structured startup, model creation, and selection tracing.
- GTK-independent commands and key-sequence parsing.

The application opens the directory from which it is started. Use `l` or double click to enter a directory and `h` to return to its parent.

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
