# PathPilot

PathPilot is a keyboard-first graphical file manager for Linux, built with Rust,
GTK 4, libadwaita, and GIO. It combines ranger-style three-column navigation,
Vim-inspired controls, conventional mouse interaction, responsive previews,
an embedded shell, and an embedded Neovim editor.

`v0.1.0` is the first preview release. The core local-file workflow is usable,
but remote filesystems, accessibility review, and broader desktop validation
are still in progress. See
[`pathpilot_project_plan.md`](pathpilot_project_plan.md) for the long-term plan.
PathPilot requires GTK 4.12 or newer, libadwaita 1.5 or newer, and GTK4 VTE.

## Current features

- Three live parent/current/preview columns with independently cancelable models.
- Virtualized `GtkListView` instances populated asynchronously through GIO.
- Batched GIO enumeration with cancellation and stale-generation rejection.
- Live current/parent directory monitoring with debounced automatic refresh.
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
- Mode-colored status line with compact metadata and separate Git repository status.
- Structured startup, model creation, and selection tracing.
- GTK-independent commands and key-sequence parsing.
- Keyboard-driven create, rename, trash, copy, move, and paste operations.
- Recursive directory copy with progress, cancellation, partial-result cleanup, and no silent overwrites.
- Explicit “Keep Both” conflict handling with unique destination names.
- Separately confirmed permanent deletion of files and non-empty directory trees.
- Explicit Normal, Find, and integrated Text Input modes.
- In-window create and rename editor with cursor movement, selection, clipboard support, and validation.
- Visual mode with anchored range selection and a separate preview cursor.
- Visual selections can be copied, moved, trashed, or permanently deleted as one cancellable batch.
- Searchable command palette opened with `:` or `Ctrl+Shift+P`.
- Configurable one- and two-key command bindings loaded from TOML with safe fallback.
- Persistent window geometry, pane layouts, divider positions, last directory, and hint preference.
- Browse, focused-preview, and preview-only layouts cycled with `z`.
- Filename search with `f`, Enter, `n`, and `N`.
- `g` places for Home, Downloads, root, and configurable bookmarks.
- Text clipboard commands for filenames, directory paths, and full paths.
- MIME-aware Open With history backed by the native GTK/GNOME application chooser.
- Embedded Neovim editing for local text files through a GTK4 VTE terminal.
- Toggleable hidden items and an embedded shell terminal synchronized with directory navigation.
- Theme-aware source and Markdown previews with configurable line numbers.
- Native libadwaita application window, header bar, About dialog, and system color-scheme support.

The first launch opens the directory from which PathPilot is started. Later
launches restore the last visited directory. Use `l` or double click to enter a
directory and `h` to return to its parent.

## Essential keyboard commands

| Keys | Action |
| --- | --- |
| `h` / `j` / `k` / `l` | Parent / down / up / open |
| `gg` / `G` | First / last item |
| `f`, Enter, `n`, `N` | Find and repeat filename matches |
| `a f` / `a d` / `r` | Create file / create directory / rename |
| `y y` / `x` / `p` | Copy / cut / paste filesystem items |
| `y n` / `y d` / `y p` | Copy name / directory path / full path as text |
| `d d` / `d D` | Trash / permanently delete |
| `v` | Toggle Visual selection |
| `g h` / `g d` / `g r` | Home / Downloads / filesystem root |
| `o e` / `o 1`…`o 9` | System Open With chooser / saved application |
| `o t` | Toggle the embedded terminal |
| `.` | Toggle hidden items |
| `s n/e/z/m` / `s r` | Sort by name/extension/size/modified time / reverse |
| `z` | Cycle pane layout |
| `e` | Edit a local text file in embedded Neovim |
| `:` / `F1` | Command palette / command reference |

## Current limitations

- Local filesystem browsing and embedded editing only; non-local GIO backends are not complete.
- No tabs or general-purpose filtering yet.
- No PDF, archive, office document, audio, or video previews.
- No remote filesystem backends or plugin API.
- Embedded editing requires `nvim` and currently accepts local text files only.
- Fedora and Wayland are the primary development targets; other Linux environments are not yet validated.

## Roadmap

The next major steps are:

1. Improve batch conflict policies and surface a richer operation summary.
2. Add drag-and-drop, accessibility, diagnostics, and remote GIO workflows.
3. Complete Fedora packaging and performance regression coverage.

Detailed phases, performance targets, and exit criteria are maintained in [`pathpilot_project_plan.md`](pathpilot_project_plan.md).

## Install release packages

Download release assets from the GitHub Releases page. On Fedora, install the
RPM with dependency resolution:

```bash
sudo dnf install ./pathpilot-0.1.0-1.fc44.x86_64.rpm
```

Install the single-file Flatpak bundle with:

```bash
flatpak install --user ./PathPilot-0.1.0-x86_64.flatpak
flatpak run io.github.RomanRcT.PathPilot
```

The Flatpak has host-filesystem access because PathPilot is a file manager. Its
embedded editor uses `flatpak-spawn --host nvim`, so Neovim and the user's
normal configuration remain on the host. The manifest therefore also grants
access to the Flatpak portal service.

## Fedora development setup

Install the native dependencies:

```bash
sudo dnf install rust cargo gtk4-devel libadwaita-devel vte291-gtk4-devel gcc pkgconf-pkg-config neovim
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

User settings are stored under `${XDG_CONFIG_HOME:-~/.config}/pathpilot/`.
`config.toml` contains UI state, bookmarks, and Open With history;
`keymap.toml` can override the default command bindings. See the focused guides
in [`docs/`](docs/) for configuration examples and implemented behavior.

Preview line numbers and the application color scheme can be configured under
`[ui]` (close PathPilot before editing the file):

```toml
[ui]
preview_line_numbers = true
color_scheme = "system" # system, light, or dark
sort_key = "name"       # name, extension, size, or modified
sort_descending = false
```

## License

PathPilot is free software distributed under the [GNU General Public License v3.0 or later](LICENSE).
