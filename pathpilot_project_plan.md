# Ranger-Style Graphical File Manager for Linux

## Project Status

**Working title:** `PathPilot`  
**Primary platform:** Fedora Linux / Wayland  
**Primary language:** Rust  
**UI toolkit:** GTK 4 and libadwaita via `gtk-rs`
**Desktop integration:** GIO / GLib / FreeDesktop specifications  
**License:** GPL-3.0-or-later

### Current implementation status (2026-08-06)

The v0.1.x codebase has completed the navigation, preview, file-operation, and
input foundations described below. It also includes packaged RPM/Flatpak
artifacts, libadwaita integration, configurable light/dark appearance,
show/hide hidden files, theme-aware previews with line numbers, embedded
Neovim, and an embedded shell terminal synchronized with browser navigation.
Tabs, general filtering, drag-and-drop, remote GIO workflows, and richer
document previews remain planned work.

---

## 1. Product Vision

Build a fast, keyboard-first graphical file manager for Linux that combines:

- ranger/Yazi-style column navigation;
- Vim-like modal keyboard controls;
- full mouse and touchpad support;
- rich file previews;
- standard graphical file operations;
- a modern but restrained native Linux interface;
- responsiveness in very large directories.

The application should feel as immediate as a terminal file manager while remaining comfortable for users who expect drag-and-drop, context menus, dialogs, thumbnails, and conventional desktop behaviour.

---

## 2. Core Product Principles

1. **Keyboard-first, not keyboard-only**  
   Every important operation must be available through Vim-style commands, while mouse operations remain fully supported.

2. **Responsiveness is a feature**  
   No filesystem access, preview generation, hashing, thumbnailing, or file operation may block the GTK main thread.

3. **Progressive rendering**  
   Directory contents should appear incrementally. The UI must not wait for all metadata or thumbnails before displaying entries.

4. **Cancelable background work**  
   Preview and metadata jobs must be canceled or ignored immediately when the selection or directory changes.

5. **Native Linux integration**  
   Prefer GIO, GLib, FreeDesktop Trash, MIME databases, portals, and desktop application associations over custom replacements.

6. **Safe file operations**  
   Destructive actions require clear behaviour, progress reporting, conflict handling, and recoverability where possible.

7. **Small initial scope**  
   The first release should be an excellent local filesystem manager, not a complete replacement for every feature in Nautilus, Dolphin, and Yazi.

---

## 3. Target User Experience

### 3.1 Default Layout

Use a three-column layout:

1. **Parent column** — siblings of the current directory.
2. **Current column** — contents of the active directory.
3. **Preview column** — directory contents or selected-file preview.

Optional later layouts:

- dual-pane commander mode;
- single-column compact mode;
- full-width preview mode;
- terminal-integrated mode.

### 3.2 Navigation

Default Vim-style bindings:

| Command | Default binding |
|---|---|
| Move down | `j` |
| Move up | `k` |
| Enter directory / open | `l`, `Enter` |
| Go to parent | `h`, `Backspace` |
| First item | `gg` |
| Last item | `G` |
| Page down | `Ctrl-d`, `PageDown` |
| Page up | `Ctrl-u`, `PageUp` |
| Toggle selection | `Space` |
| Visual selection mode | `v` |
| Select all | `Ctrl-a` |
| Rename | `r` or `F2` |
| Copy | `y` or `Ctrl-c` |
| Cut / move | `x` or `Ctrl-x` |
| Paste | `p` or `Ctrl-v` |
| Move to trash | `d d` or `Delete` |
| Permanent delete | configurable, never a simple single key |
| Create file | `a f` |
| Create directory | `a d` or `Shift-n` |
| Search/filter | `/` |
| Command palette | `:` |
| Show help | `?` |
| Quit window | `q` |

Bindings must be configurable through a human-readable TOML file.

### 3.3 Mouse Behaviour

- Single click selects.
- Double click opens.
- Right click opens a context menu.
- Drag-and-drop supports move, copy, and external applications.
- Middle click may open a directory in a new tab.
- Mouse wheel scrolls the focused column.
- Horizontal wheel or touchpad gesture may move between columns.

Keyboard focus must remain predictable after mouse interactions.

---

## 4. MVP Scope

### 4.1 Included in MVP

- Local filesystem navigation.
- Three-column ranger-style layout.
- Vim-style navigation and visual selection.
- Conventional keyboard and mouse controls.
- Tabs.
- File and directory creation.
- Rename.
- Copy and move.
- Move to trash.
- Permanent delete with confirmation.
- Conflict resolution for copy/move operations.
- Operation progress UI.
- Sorting by name, extension, size, and modified time.
- Show/hide hidden files.
- Text filtering inside the current directory.
- Preview for:
  - plain text;
  - source code with syntax highlighting;
  - Markdown source and rendered Markdown;
  - common image formats;
  - directories;
  - basic file metadata.
- Open with default application.
- Open-with menu.
- Configurable keybindings.
- Persistent window state and recent locations.
- Basic light/dark theme compatibility.

### 4.2 Explicitly Deferred

- SMB, SFTP, FTP, WebDAV, and cloud storage.
- Full filesystem indexing.
- Content search across the entire disk.
- Plugin API.
- Archive modification.
- Git status overlays.
- PDF preview.
- Office document preview.
- Video/audio playback.
- File synchronization.
- Advanced bulk rename.
- Root/admin mode.
- Windows and macOS support.

These features may be added after the local filesystem experience is stable and measurable.

---

## 5. Technology Stack

### 5.1 Primary Stack

- **Rust stable**
- **GTK 4** through `gtk4-rs`
- **GLib/GIO** for filesystem abstractions and desktop integration
- **libadwaita** for the application shell, system appearance, and standard GNOME dialogs
- **Tokio** only for non-GIO asynchronous work that benefits from its runtime
- **GLib main context** for GTK-safe task completion and UI updates
- **Serde + TOML** for configuration
- **Tracing** for structured logging
- **thiserror / anyhow** according to layer responsibility

### 5.2 Suggested Libraries

| Capability | Candidate |
|---|---|
| GTK bindings | `gtk4`, `gio`, `glib`, `gdk4` |
| GNOME styling | `libadwaita` |
| Configuration | `serde`, `toml` |
| Logging | `tracing`, `tracing-subscriber` |
| Filesystem notifications | GIO `FileMonitor` first; `notify` only if required |
| Syntax highlighting | `syntect` initially; consider GtkSourceView later |
| Markdown parsing | `pulldown-cmark` |
| Image decoding | GTK/GDK loaders first; `image` for unsupported processing |
| MIME detection | GIO content type APIs; avoid extension-only detection |
| Cancellation | `gio::Cancellable`, generation IDs, cancellation tokens |
| Unique IDs | `uuid` only if persistent operation IDs are needed |
| Testing temp files | `tempfile` |
| Benchmarking | `criterion` |

### 5.3 Why GTK 4 Is Suitable

GTK 4 is fast enough for this product when used correctly:

- `GtkListView` and `GtkGridView` create widgets for visible items rather than the entire model.
- List items are created through factories and reused as the viewport changes.
- GIO provides asynchronous directory enumeration and file operations.
- GTK rendering is not expected to be the bottleneck for a three-column file manager.

The main performance risks are application-level mistakes:

- creating one permanent widget per filesystem entry;
- loading thumbnails before showing a directory;
- reading file contents on the UI thread;
- rebuilding complete models after every change;
- allowing stale preview jobs to update the current selection;
- repeatedly querying the same metadata;
- using expensive custom CSS or deeply nested widget hierarchies.

---

## 6. Architecture

Use a workspace with clearly separated crates.

```text
pathpilot/
├── Cargo.toml
├── crates/
│   ├── app/                 # GTK application composition
│   ├── core/                # Pure domain logic and commands
│   ├── fs-local/            # Local filesystem implementation
│   ├── operations/          # Copy/move/trash/delete job engine
│   ├── preview/             # Preview orchestration and providers
│   ├── config/              # Settings and keymap loading
│   └── ui-gtk/              # GTK widgets, models, controllers
├── assets/
├── data/
│   ├── icons/
│   ├── default-keymap.toml
│   └── org.example.PathPilot.desktop
├── tests/
└── docs/
```

### 6.1 Layer Rules

- `core` must not depend on GTK.
- UI widgets must not directly perform filesystem operations.
- Filesystem implementations expose async operations and streams/events.
- Preview providers return typed preview results, not GTK widgets.
- GTK conversion/rendering occurs in `ui-gtk`.
- Commands are the common abstraction for keyboard, mouse, menus, and command palette actions.

---

## 7. Core Domain Model

Suggested initial concepts:

```rust
pub struct Location {
    pub uri: String,
}

pub struct FileEntry {
    pub id: EntryId,
    pub location: Location,
    pub display_name: String,
    pub kind: FileKind,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
    pub content_type: Option<String>,
    pub is_hidden: bool,
    pub is_symlink: bool,
}

pub enum FileKind {
    Directory,
    Regular,
    Symlink,
    Special,
    Unknown,
}

pub enum AppCommand {
    NavigateUp,
    NavigateDown,
    Enter,
    GoParent,
    GoFirst,
    GoLast,
    ToggleSelection,
    BeginVisualSelection,
    CopySelection,
    CutSelection,
    Paste,
    Rename,
    Trash,
    DeletePermanently,
    CreateFile,
    CreateDirectory,
    Open,
    OpenWith,
    Refresh,
    ToggleHidden,
    StartFilter,
    ShowCommandPalette,
}
```

Avoid exposing raw `std::path::PathBuf` throughout the entire application if future GIO URIs or remote backends are likely. Local filesystem code may still use `PathBuf` internally.

---

## 8. State Management

Use a unidirectional flow:

```text
Input event
  -> AppCommand
  -> state transition / async request
  -> result message
  -> state update
  -> minimal UI model update
```

Recommended state groups:

- active tab;
- current location;
- navigation history;
- per-column directory model;
- cursor position;
- selection set;
- visual selection anchor;
- sort/filter configuration;
- active preview request;
- operation queue;
- clipboard state;
- transient dialogs and notifications.

Do not make GTK widgets the source of truth for navigation or selection state.

---

## 9. Directory Loading Pipeline

### 9.1 Requirements

- Display cached or partial entries quickly.
- Enumerate asynchronously.
- Fetch only essential attributes initially.
- Sort without freezing the UI.
- Add metadata progressively where possible.
- Cancel enumeration when the user leaves the directory.
- Preserve selection during monitor updates.

### 9.2 Pipeline

```text
Navigate to directory
  -> increment directory generation ID
  -> cancel previous enumeration
  -> clear or replace model with loading state
  -> asynchronously enumerate basic attributes
  -> publish entries in batches
  -> update GTK ListModel incrementally
  -> perform optional secondary metadata work
  -> start FileMonitor
```

Initial GIO attributes should be kept minimal, for example:

- standard name;
- display name;
- file type;
- hidden flag;
- symbolic link flag;
- size;
- modified time;
- content type when inexpensive.

### 9.3 Large Directory Targets

Initial performance targets on a normal SSD:

- application window visible: under 300 ms after process start;
- cached home directory visible: under 150 ms after window creation;
- first batch from a 10,000-entry directory: under 250 ms;
- keyboard selection response: under one frame under normal load;
- no UI freeze longer than 50 ms;
- stable memory usage while repeatedly navigating large directories.

Targets are provisional and must be measured on the developer's Fedora workstation.

---

## 10. GTK UI Implementation Rules

1. Use `GtkListView` for file rows.
2. Use `GtkGridView` only for a future icon mode.
3. Use `SignalListItemFactory` or subclassed factories.
4. Bind only visible row widgets.
5. Keep each row widget shallow.
6. Do not place thousands of children in `GtkBox`, `GtkListBox`, or custom containers.
7. Avoid rebuilding the entire `gio::ListStore` when only a few files changed.
8. Use `GtkSelectionModel` compatible models for selection behaviour.
9. Keep expensive icons and thumbnails behind an asynchronous cache.
10. Update the UI only from the GTK/GLib main context.
11. Treat CSS as presentation, not as a layout engine.
12. Profile before implementing custom rendering.

---

## 11. Preview System

### 11.1 Provider Interface

```rust
#[async_trait::async_trait]
pub trait PreviewProvider: Send + Sync {
    fn supports(&self, file: &FileEntry) -> bool;

    async fn load(
        &self,
        file: &FileEntry,
        request: PreviewRequest,
        cancel: CancellationToken,
    ) -> Result<PreviewContent, PreviewError>;
}
```

Possible result model:

```rust
pub enum PreviewContent {
    Directory(DirectoryPreview),
    Text(TextPreview),
    Markdown(MarkdownPreview),
    Image(ImagePreview),
    Metadata(MetadataPreview),
    Unsupported,
}
```

### 11.2 Selection Debounce

When the user holds `j` or `k`, the application must not fully parse every briefly selected file.

```text
Selection changes
  -> immediately update filename and basic metadata
  -> cancel previous preview
  -> wait 50-100 ms
  -> start preview for the latest selection
  -> discard result if generation ID is stale
```

### 11.3 Safety Limits

- Do not read entire huge text files.
- Preview only a configurable maximum number of bytes/lines.
- Decode images with size limits.
- Treat malformed files as recoverable preview failures.
- Never execute embedded content.
- Run external converters with strict timeouts and isolated temporary directories.

### 11.4 MVP Providers

#### Text and source code

- Read an initial chunk asynchronously.
- Detect binary content.
- Detect encoding, with UTF-8 as the primary path.
- Highlight source using `syntect` or GtkSourceView.
- Show truncation status for large files.

#### Markdown

- Toggle between rendered and source view.
- Render headings, paragraphs, lists, code blocks, links, and tables.
- Do not load remote images automatically in the MVP.

#### Images

- Decode off the UI thread when necessary.
- Scale to the preview viewport.
- Cache by path/URI, mtime, and requested dimensions.
- Preserve aspect ratio.

#### Directories

- Show child count when known.
- Show a short list of contents.
- Optionally show aggregate size only on explicit request; never calculate recursively during normal navigation.

### 11.5 Later Preview Providers

- PDF via Poppler.
- Office via LibreOffice headless conversion to PDF/image.
- Archives via libarchive.
- Audio/video metadata and thumbnails via GStreamer.
- Fonts.
- SQLite metadata browser.

---

## 12. File Operation Engine

### 12.1 Operations

- create file;
- create directory;
- rename;
- copy;
- move;
- move to trash;
- restore from trash later;
- permanent delete;
- duplicate;
- create symbolic link later.

### 12.2 Operation Model

```rust
pub struct OperationJob {
    pub id: OperationId,
    pub kind: OperationKind,
    pub state: OperationState,
    pub progress: OperationProgress,
    pub conflicts: Vec<Conflict>,
}
```

Jobs must support:

- progress;
- cancellation where safe;
- queued and concurrent execution policies;
- conflict prompts;
- detailed error reporting;
- partial-failure summaries;
- optional undo for operations where implementation is reliable.

### 12.3 Conflict Resolution

Support:

- replace;
- skip;
- rename incoming item;
- apply choice to all conflicts;
- compare size and modified time;
- never silently overwrite by default.

### 12.4 Trash

Use GIO/FreeDesktop Trash behaviour rather than implementing a private trash directory.

Permanent deletion must be visually and behaviourally distinct from moving to trash.

---

## 13. Keyboard Mode Engine

Implement keyboard handling as a state machine rather than scattered GTK shortcuts.

```rust
pub enum InputMode {
    Normal,
    Visual,
    Filter,
    Command,
    Rename,
    Dialog,
}
```

### 13.1 Multi-Key Sequences

Support sequences such as:

- `gg`;
- `dd`;
- `af`;
- `ad`;
- `yy` if desired.

The key engine should:

- track pending sequences;
- use a configurable timeout;
- show pending keys in the status area;
- cancel sequences on `Esc`;
- respect focused text-entry widgets;
- allow conventional shortcuts alongside Vim bindings.

### 13.2 Keymap Configuration

Example TOML:

```toml
[normal]
"j" = "cursor-down"
"k" = "cursor-up"
"h" = "parent-directory"
"l" = "enter"
"g g" = "cursor-first"
"G" = "cursor-last"
"space" = "toggle-selection"
"v" = "visual-mode"
"d d" = "trash"
"colon" = "command-palette"

[visual]
"j" = "extend-down"
"k" = "extend-up"
"y" = "copy-selection"
"d" = "trash-selection"
"escape" = "normal-mode"
```

Commands, not physical keys, must be the public extension point.

---

## 14. Mouse, Drag-and-Drop, and Desktop Integration

MVP requirements:

- internal drag-and-drop between visible directories;
- drag files to external applications;
- accept dropped files into the current directory;
- modifier-aware copy/move behaviour;
- URI list support;
- native open-with integration;
- default application launching;
- clipboard interoperability with desktop file managers where practical;
- Wayland-first behaviour.

Use portals where required for sandboxed builds, but do not make Flatpak packaging a prerequisite for early development.

---

## 15. Visual Design

### 15.1 Direction

- Modern, dense, and functional.
- More polished than ranger, less spacious than Nautilus.
- Strong keyboard focus indication.
- Clear distinction between cursor and multi-selection.
- Minimal animation.
- File type icons, but no mandatory thumbnails in list rows.
- Compact status line showing mode, selection count, pending keys, and operation status.

### 15.2 Suggested Components

- header bar with location breadcrumbs and tab controls;
- central three-column `GtkPaned` structure;
- status bar at bottom;
- toast notifications for completed operations;
- popovers for sort and view options;
- command palette overlay;
- optional path entry activated by a shortcut.

### 15.3 Accessibility

- Standard GTK focus navigation must continue to work.
- Expose accessible labels and roles.
- Do not rely on colour alone for selection or errors.
- Support system font and scaling settings.

---

## 16. Configuration

Suggested location:

```text
~/.config/pathpilot/config.toml
~/.config/pathpilot/keymap.toml
~/.cache/pathpilot/
```

Initial settings:

```toml
[ui]
show_hidden = false
confirm_permanent_delete = true
preview_delay_ms = 75
compact_rows = true
restore_tabs = true

[preview]
max_text_bytes = 1048576
max_text_lines = 10000
image_cache_mb = 256
render_markdown = true

[navigation]
wrap_cursor = false
folders_first = true
case_sensitive_sort = false
```

Invalid configuration should produce a useful warning and fall back to safe defaults.

---

## 17. Error Handling

Define domain-specific errors and convert them at boundaries.

The UI must distinguish:

- permission denied;
- file disappeared;
- destination exists;
- storage full;
- unsupported preview;
- invalid filename;
- mount unavailable;
- operation partially completed;
- cancellation.

Errors should be actionable and should include the affected path where safe.

Do not crash the process because one preview provider failed.

---

## 18. Logging and Diagnostics

Use `tracing` with categories such as:

- `navigation`;
- `directory_load`;
- `preview`;
- `operation`;
- `file_monitor`;
- `input`;
- `ui_model`.

Support:

```bash
RUST_LOG=pathpilot=debug pathpilot
```

Debug logs should include generation IDs, durations, entry counts, cancellation events, and operation IDs, but should avoid logging file contents.

Add an optional diagnostics page later with:

- GTK version;
- application version;
- active backend (Wayland/X11);
- cache size;
- current background jobs;
- recent errors.

---

## 19. Testing Strategy

### 19.1 Unit Tests

- navigation history;
- command dispatch;
- selection behaviour;
- visual range selection;
- key sequence parsing;
- sorting and filtering;
- conflict resolution policies;
- preview provider selection;
- stale generation rejection;
- configuration parsing.

### 19.2 Integration Tests

Use temporary directory trees to test:

- large directories;
- Unicode filenames;
- hidden files;
- symbolic links;
- broken links;
- permission errors;
- files disappearing during enumeration;
- copy/move conflicts;
- cancellation;
- concurrent filesystem changes.

### 19.3 UI Tests

Test the most important flows:

- navigate using only `h/j/k/l`;
- select with keyboard and mouse;
- rename;
- copy and paste;
- move to trash;
- open command palette;
- switch tabs;
- preview rapid selection changes.

Use GTK test utilities where practical. Avoid making the entire test suite depend on pixel-perfect screenshots.

### 19.4 Benchmarks

Benchmark:

- sorting 1k, 10k, and 100k entries;
- filtering large models;
- batch conversion from filesystem entries to UI objects;
- preview cache lookup;
- key dispatch;
- incremental model updates.

---

## 20. Performance Validation

### 20.1 Required Scenarios

1. Home directory with normal mixed content.
2. Directory with 10,000 small files.
3. Directory with 100,000 generated entries.
4. Network-like slow filesystem simulated through delayed backend calls.
5. Directory continuously modified by another process.
6. Rapid `j/k` navigation through large source files and images.
7. Copy of a multi-gigabyte file with progress and cancellation.
8. Copy of thousands of small files.

### 20.2 Tools

- `cargo flamegraph`;
- `perf`;
- Sysprof;
- GTK Inspector;
- `heaptrack` if needed;
- Criterion benchmarks;
- structured timing spans with `tracing`.

### 20.3 Performance Gates

A feature should not be considered complete if it:

- performs blocking filesystem work on the GTK thread;
- creates UI widgets for all entries in a large directory;
- causes repeated full-directory rescans for individual file events;
- continues expensive preview work after navigation;
- grows memory without bound during ordinary navigation.

---

## 21. Packaging for Fedora

### Development

```bash
sudo dnf install \
  rust cargo \
  gtk4-devel \
  libadwaita-devel \
  gcc pkgconf-pkg-config
```

Exact package names should be verified against the Fedora release used for development.

### Initial Distribution

1. Local Cargo build for development.
2. RPM spec for Fedora COPR.
3. Flatpak manifest after portals and sandbox behaviour are validated.
4. Optional AppImage only if there is real user demand.

Install desktop metadata:

- `.desktop` file;
- application icon;
- AppStream metadata;
- MIME capability declarations only when implemented.

---

## 22. Development Phases

### Phase 0 — Technical Spike

Goal: prove that GTK 4 can deliver the required interaction speed.

Deliverables:

- Rust/GTK application shell;
- one `GtkListView` populated with 100,000 synthetic entries;
- smooth keyboard navigation;
- row factory recycling verified;
- simple three-column layout;
- async load of one real directory;
- basic text/image preview cancellation experiment;
- performance notes recorded in `docs/performance-spike.md`.

Exit criteria:

- no visible freeze while navigating the synthetic list;
- directory enumeration does not block the UI;
- stale previews never replace the current preview.

### Phase 1 — Navigation Foundation

Deliverables:

- workspace structure;
- domain models;
- three-column navigation;
- cursor and selection state;
- `h/j/k/l`, `gg`, `G`;
- path history;
- hidden-file toggle;
- sorting;
- directory monitoring;
- mouse selection and opening.

Exit criteria:

- application is usable for read-only local navigation;
- large-directory targets are measured.

### Phase 2 — Preview MVP

Deliverables:

- preview coordinator;
- cancellation/generation mechanism;
- text preview;
- source highlighting;
- Markdown preview;
- image preview;
- directory metadata preview;
- preview cache.

Exit criteria:

- rapid navigation remains responsive;
- malformed and large files do not crash or freeze the application.

### Phase 3 — File Operations

Deliverables:

- create file/directory;
- rename;
- copy;
- move;
- trash;
- permanent delete;
- progress UI;
- conflict handling;
- cancellation;
- automatic model reconciliation after operations.

Exit criteria:

- operation engine passes temporary-filesystem integration tests;
- no silent overwrites;
- errors identify affected items.

### Phase 4 — Input and Configuration

Deliverables:

- complete mode state machine;
- multi-key sequences;
- visual mode;
- command palette;
- configurable TOML keymap;
- status line;
- conventional shortcuts;
- settings persistence.

Exit criteria:

- all primary operations are usable without a mouse;
- ordinary mouse workflows remain intuitive.

### Phase 5 — Polish and Fedora Packaging

Deliverables:

- accessibility pass;
- drag-and-drop;
- open-with integration;
- AppStream metadata;
- icon and desktop file;
- RPM/COPR package;
- crash handling and diagnostics;
- user documentation;
- performance regression suite.

Exit criteria:

- suitable for daily personal use on Fedora;
- known limitations documented.

---

## 23. Initial Milestones for Codex

### Milestone 1: Bootstrap

Ask Codex to:

1. Create the Cargo workspace and crates.
2. Add GTK 4 and GLib/GIO dependencies.
3. Create a minimal application window.
4. Add CI commands for format, clippy, and tests.
5. Add `README.md`, `CONTRIBUTING.md`, and architecture notes.
6. Keep all code compiling after every step.

### Milestone 2: Performance Spike

Ask Codex to:

1. Implement a `GtkListView` with 100,000 synthetic entries.
2. Use a list item factory, not one widget per model entry.
3. Add keyboard navigation.
4. Measure startup, model population, and navigation latency.
5. Document results.

### Milestone 3: Real Directory Model

Ask Codex to:

1. Add `FileEntry` and `Location` types.
2. Enumerate a directory asynchronously using GIO.
3. Publish entries in batches.
4. Support cancellation using `gio::Cancellable` or an equivalent mechanism.
5. Ignore stale results using a generation ID.
6. Display filename, icon, type, size, and modified time.

### Milestone 4: Three Columns

Ask Codex to:

1. Implement parent/current/preview columns.
2. Add `h/j/k/l` navigation.
3. Preserve cursor positions per directory.
4. Update only affected models during navigation.
5. Add mouse selection and double-click opening.

### Milestone 5: Preview Prototype

Ask Codex to:

1. Add a preview coordinator.
2. Debounce selection by 75 ms.
3. Cancel previous preview requests.
4. Implement bounded text preview.
5. Implement image preview.
6. Add tests proving stale previews are discarded.

---

## 24. First Codex Prompt

Use the following prompt to begin implementation:

```text
We are starting a new Rust project named PathPilot: a fast, ranger-style graphical file manager for Fedora Linux using GTK 4 and gtk4-rs.

Read pathpilot_project_plan.md completely before making changes.

For the first iteration, implement only Phase 0: Technical Spike.

Requirements:
1. Create a clean Cargo workspace with separate crates for app, core, and ui-gtk.
2. Build a minimal GTK 4 application window.
3. Add a three-column layout using GTK widgets appropriate for later resizing.
4. In the centre column, implement GtkListView backed by a model containing 100,000 synthetic file entries.
5. Use a list item factory so row widgets are created/reused only for visible items. Do not create one GTK widget per entry.
6. Implement keyboard cursor navigation with j, k, gg, and G.
7. Add a small status area showing the selected index and total item count.
8. Add structured tracing around startup, model creation, and selection changes.
9. Add unit tests for the non-GTK key-sequence parser or command mapping.
10. Add README instructions for building on Fedora.
11. Run cargo fmt, cargo clippy, and cargo test, and fix all failures.

Architecture constraints:
- The core crate must not depend on GTK.
- GTK widgets must not be the source of truth for commands.
- Represent navigation actions using an AppCommand enum in the core crate.
- Do not add preview, filesystem operations, tabs, plugins, or remote filesystems yet.
- Keep the implementation simple and compiling.

At the end, summarize:
- files created or changed;
- architecture decisions;
- commands run;
- test results;
- performance observations;
- recommended next step.
```

---

## 25. Definition of Done for MVP

The MVP is complete when:

- the application can be used daily for local file navigation;
- keyboard and mouse workflows coexist without focus conflicts;
- large directories do not freeze the interface;
- previews are asynchronous, bounded, and cancelable;
- copy/move/trash operations report progress and failures;
- there are no known silent data-loss paths;
- default keybindings are documented and configurable;
- Fedora installation instructions work;
- automated tests cover core navigation and operation logic;
- performance measurements are documented and reproducible.

---

## 26. Main Risks

### Risk: GTK model complexity

Mitigation: complete the 100,000-entry technical spike before building product features.

### Risk: Mixing Tokio and GLib runtimes incorrectly

Mitigation: use GIO async APIs for GIO operations, GLib main context for UI completion, and introduce Tokio only for clearly isolated background work.

### Risk: Preview providers make the application unstable

Mitigation: bounded reads, cancellation, timeouts, generation IDs, and strict provider interfaces.

### Risk: File operation bugs cause data loss

Mitigation: trash by default, explicit overwrite policies, integration tests, no permanent delete shortcut that is easy to trigger accidentally.

### Risk: Scope expands into a full Nautilus replacement

Mitigation: enforce the deferred-feature list until MVP performance and safety gates are met.

### Risk: GObject boilerplate slows development

Mitigation: keep GTK-specific objects inside `ui-gtk`; keep domain logic as ordinary Rust types; introduce custom GObjects only where required by GTK models.

---

## 27. Recommended First Decision

Proceed with **Rust + GTK 4 + gtk4-rs + GIO**.

Do not commit to a large architecture before completing the Phase 0 performance spike. If the spike shows smooth navigation through 100,000 synthetic rows and responsive asynchronous directory loading, continue with GTK 4. Based on GTK 4's virtualized list model design, this is the expected outcome.
