//! GTK-independent domain commands and key-sequence handling.

use std::{
    cmp::Ordering,
    collections::HashMap,
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Location {
    uri: String,
}

impl Location {
    pub fn new(uri: impl Into<String>) -> Self {
        Self { uri: uri.into() }
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocationPresentation {
    pub compact: String,
    pub full: String,
}

pub fn present_location(
    location: &Location,
    home: Option<&std::path::Path>,
) -> LocationPresentation {
    let Some(encoded_path) = location.uri().strip_prefix("file://") else {
        return LocationPresentation {
            compact: location.uri().to_owned(),
            full: location.uri().to_owned(),
        };
    };
    let full = percent_decode(encoded_path).unwrap_or_else(|| encoded_path.to_owned());
    let compact = home
        .and_then(std::path::Path::to_str)
        .and_then(|home| {
            (full == home).then(|| "~".to_owned()).or_else(|| {
                full.strip_prefix(home)
                    .filter(|rest| rest.starts_with('/'))
                    .map(|rest| format!("~{rest}"))
            })
        })
        .unwrap_or_else(|| full.clone());
    LocationPresentation { compact, full }
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            decoded.push(u8::from_str_radix(text, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    Directory,
    Regular,
    Symlink,
    Special,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub location: Location,
    pub display_name: String,
    pub kind: FileKind,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub unix_mode: Option<u32>,
    pub content_type: Option<String>,
    pub is_hidden: bool,
    pub is_symlink: bool,
    /// Archive format detected from the file signature, never from its name.
    pub archive_format: Option<String>,
}

impl FileEntry {
    pub fn compare_name(&self, other: &Self) -> Ordering {
        self.compare(other, SortMode::default())
    }

    pub fn compare(&self, other: &Self, mode: SortMode) -> Ordering {
        match (
            self.kind == FileKind::Directory,
            other.kind == FileKind::Directory,
        ) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        let ordering = match mode.key {
            SortKey::Name => compare_text(&self.display_name, &other.display_name),
            SortKey::Extension => compare_optional_text(
                file_extension(&self.display_name),
                file_extension(&other.display_name),
                mode.descending,
            ),
            SortKey::Size => compare_optional(self.size, other.size, mode.descending),
            SortKey::Modified => compare_optional(self.modified, other.modified, mode.descending),
        };
        let ordering = if mode.key == SortKey::Name && mode.descending {
            ordering.reverse()
        } else {
            ordering
        };
        ordering.then_with(|| {
            let names = compare_text(&self.display_name, &other.display_name);
            if mode.descending {
                names.reverse()
            } else {
                names
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortKey {
    #[default]
    Name,
    Extension,
    Size,
    Modified,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SortMode {
    pub key: SortKey,
    pub descending: bool,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match (self.key, self.descending) {
            (SortKey::Name, false) => "name ↑",
            (SortKey::Name, true) => "name ↓",
            (SortKey::Extension, false) => "extension ↑",
            (SortKey::Extension, true) => "extension ↓",
            (SortKey::Size, false) => "size ↑",
            (SortKey::Size, true) => "size ↓",
            (SortKey::Modified, false) => "modified ↑",
            (SortKey::Modified, true) => "modified ↓",
        }
    }
}

fn compare_text(left: &str, right: &str) -> Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

fn file_extension(name: &str) -> Option<&str> {
    std::path::Path::new(name)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
}

fn compare_optional<T: Ord>(left: Option<T>, right: Option<T>, descending: bool) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            let ordering = left.cmp(&right);
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_optional_text(left: Option<&str>, right: Option<&str>, descending: bool) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            let ordering = compare_text(left, right);
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Generation(u64);

impl Generation {
    pub fn value(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Default)]
pub struct GenerationTracker {
    current: Generation,
}

#[derive(Debug)]
pub struct NavigationState {
    current: Location,
    cursor_by_location: HashMap<Location, u32>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OperationId(u64);

impl OperationId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationKind {
    CreateFile {
        parent: Location,
        name: String,
    },
    CreateDirectory {
        parent: Location,
        name: String,
    },
    Rename {
        source: Location,
        new_name: String,
    },
    Trash {
        target: Location,
    },
    Delete {
        target: Location,
    },
    Copy {
        source: Location,
        destination: Location,
    },
    Move {
        source: Location,
        destination: Location,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OperationProgress {
    pub completed_items: u64,
    pub total_items: Option<u64>,
    pub completed_bytes: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationJob {
    pub id: OperationId,
    pub kind: OperationKind,
    pub state: OperationState,
    pub progress: OperationProgress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardAction {
    Copy,
    Move,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardItem {
    pub source: Location,
    pub display_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationClipboard {
    pub action: ClipboardAction,
    pub items: Vec<ClipboardItem>,
}

impl OperationClipboard {
    pub fn should_clear_after_move(&self, succeeded: bool) -> bool {
        succeeded && self.action == ClipboardAction::Move
    }

    pub fn status_label(&self) -> String {
        let action = match self.action {
            ClipboardAction::Copy => "COPY",
            ClipboardAction::Move => "CUT",
        };
        match self.items.as_slice() {
            [item] => format!("{action}: {}", item.display_name),
            items => format!("{action}: {} items", items.len()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputModeKind {
    CreateFile,
    CreateDirectory,
    Rename,
}

impl InputModeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::CreateFile => "Create file",
            Self::CreateDirectory => "Create directory",
            Self::Rename => "Rename",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextInputMode {
    kind: InputModeKind,
    value: String,
    replace_on_type: bool,
    error: Option<&'static str>,
}

impl TextInputMode {
    fn new(kind: InputModeKind, initial: impl Into<String>) -> Self {
        let value = initial.into();
        Self {
            kind,
            replace_on_type: !value.is_empty(),
            value,
            error: None,
        }
    }

    pub fn kind(&self) -> InputModeKind {
        self.kind
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn error(&self) -> Option<&'static str> {
        self.error
    }

    pub fn will_replace_on_type(&self) -> bool {
        self.replace_on_type
    }

    pub fn push(&mut self, character: char) {
        if self.replace_on_type {
            self.value.clear();
            self.replace_on_type = false;
        }
        self.value.push(character);
        self.error = None;
    }

    pub fn pop(&mut self) {
        if self.replace_on_type {
            self.value.clear();
            self.replace_on_type = false;
        } else {
            self.value.pop();
        }
        self.error = None;
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.replace_on_type = false;
        self.error = None;
    }

    fn validate(&mut self) -> bool {
        self.error = if self.value.is_empty() {
            Some("Name cannot be empty")
        } else if self.value == "." || self.value == ".." || self.value.contains(['/', '\0']) {
            Some("Name cannot contain a path separator or be . or ..")
        } else {
            None
        };
        self.error.is_none()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisualSelection {
    anchor: u32,
    cursor: u32,
}

impl VisualSelection {
    pub fn new(position: u32) -> Self {
        Self {
            anchor: position,
            cursor: position,
        }
    }

    pub fn anchor(self) -> u32 {
        self.anchor
    }

    pub fn cursor(self) -> u32 {
        self.cursor
    }

    pub fn set_cursor(&mut self, position: u32, item_count: u32) {
        if item_count > 0 {
            self.cursor = position.min(item_count - 1);
        }
    }

    pub fn range(self) -> std::ops::RangeInclusive<u32> {
        self.anchor.min(self.cursor)..=self.anchor.max(self.cursor)
    }

    pub fn len(self) -> u32 {
        self.anchor.abs_diff(self.cursor) + 1
    }

    pub fn is_empty(self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum AppMode {
    #[default]
    Normal,
    Find,
    Command,
    TextInput(TextInputMode),
    Visual(VisualSelection),
}

impl AppMode {
    pub fn begin_find(&mut self) -> bool {
        if *self != Self::Normal {
            return false;
        }
        *self = Self::Find;
        true
    }

    pub fn begin_command(&mut self) -> bool {
        if *self != Self::Normal {
            return false;
        }
        *self = Self::Command;
        true
    }

    pub fn begin_text_input(&mut self, kind: InputModeKind, initial: impl Into<String>) -> bool {
        if *self != Self::Normal {
            return false;
        }
        *self = Self::TextInput(TextInputMode::new(kind, initial));
        true
    }

    pub fn begin_visual(&mut self, position: u32) -> bool {
        if *self != Self::Normal {
            return false;
        }
        *self = Self::Visual(VisualSelection::new(position));
        true
    }

    pub fn visual(&self) -> Option<VisualSelection> {
        match self {
            Self::Visual(selection) => Some(*selection),
            _ => None,
        }
    }

    pub fn visual_mut(&mut self) -> Option<&mut VisualSelection> {
        match self {
            Self::Visual(selection) => Some(selection),
            _ => None,
        }
    }

    pub fn text_input(&self) -> Option<&TextInputMode> {
        match self {
            Self::TextInput(input) => Some(input),
            _ => None,
        }
    }

    pub fn text_input_mut(&mut self) -> Option<&mut TextInputMode> {
        match self {
            Self::TextInput(input) => Some(input),
            _ => None,
        }
    }

    pub fn submit_text_input(&mut self) -> Option<(InputModeKind, String)> {
        let input = self.text_input_mut()?;
        if !input.validate() {
            return None;
        }
        let submission = (input.kind(), input.value().to_owned());
        *self = Self::Normal;
        Some(submission)
    }

    pub fn finish_find(&mut self) -> bool {
        if *self != Self::Find {
            return false;
        }
        *self = Self::Normal;
        true
    }

    pub fn cancel(&mut self) -> bool {
        if *self == Self::Normal {
            return false;
        }
        *self = Self::Normal;
        true
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FilenameFind {
    active: bool,
    query: String,
    original_position: u32,
    current_position: Option<u32>,
}

impl FilenameFind {
    pub fn start(&mut self, position: u32) {
        self.active = true;
        self.query.clear();
        self.original_position = position;
        self.current_position = Some(position);
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn push(&mut self, character: char, names: &[String]) -> Option<u32> {
        self.query.push(character);
        self.select_first(names)
    }

    pub fn pop(&mut self, names: &[String]) -> Option<u32> {
        self.query.pop();
        if self.query.is_empty() {
            self.current_position = Some(self.original_position);
            self.current_position
        } else {
            self.select_first(names)
        }
    }

    pub fn accept(&mut self) {
        self.active = false;
    }

    pub fn cancel(&mut self) -> u32 {
        self.active = false;
        self.query.clear();
        self.current_position = Some(self.original_position);
        self.original_position
    }

    pub fn repeat(&mut self, names: &[String], forward: bool) -> Option<u32> {
        if self.query.is_empty() || names.is_empty() {
            return None;
        }
        let current = self.current_position.unwrap_or(self.original_position) as usize;
        let positions = matching_positions(names, &self.query);
        let position = if forward {
            positions
                .iter()
                .copied()
                .find(|position| *position > current)
                .or_else(|| positions.first().copied())
        } else {
            positions
                .iter()
                .rev()
                .copied()
                .find(|position| *position < current)
                .or_else(|| positions.last().copied())
        }? as u32;
        self.current_position = Some(position);
        Some(position)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn select_first(&mut self, names: &[String]) -> Option<u32> {
        if self.query.is_empty() {
            return self.current_position;
        }
        let start = self.original_position as usize;
        let positions = matching_positions(names, &self.query);
        let position = positions
            .iter()
            .copied()
            .find(|position| *position >= start)
            .or_else(|| positions.first().copied())? as u32;
        self.current_position = Some(position);
        Some(position)
    }
}

fn matching_positions(names: &[String], query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    names
        .iter()
        .enumerate()
        .filter_map(|(position, name)| name.to_lowercase().contains(&query).then_some(position))
        .collect()
}

impl NavigationState {
    pub fn new(current: Location) -> Self {
        Self {
            current,
            cursor_by_location: HashMap::new(),
        }
    }

    pub fn current(&self) -> &Location {
        &self.current
    }

    pub fn remember_cursor(&mut self, position: u32) {
        self.cursor_by_location
            .insert(self.current.clone(), position);
    }

    pub fn navigate_to(&mut self, location: Location) -> Option<u32> {
        self.current = location;
        self.cursor_by_location.get(&self.current).copied()
    }
}

impl GenerationTracker {
    pub fn advance(&mut self) -> Generation {
        self.current.0 = self.current.0.wrapping_add(1);
        self.current
    }

    pub fn accepts(&self, generation: Generation) -> bool {
        self.current == generation
    }
}

/// An action understood by the application independently of its input source.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AppCommand {
    NavigateUp,
    NavigateDown,
    Enter,
    GoParent,
    GoFirst,
    GoLast,
    Quit,
    CreateFile,
    CreateDirectory,
    Rename,
    Trash,
    PermanentDelete,
    Copy,
    CopyName,
    CopyDirectoryPath,
    CopyFullPath,
    Cut,
    Paste,
    ToggleVisual,
    CycleLayout,
    FullPreview,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PaneLayout {
    #[default]
    Browse,
    FocusPreview,
    PreviewOnly,
}

impl PaneLayout {
    pub fn next(self) -> Self {
        match self {
            Self::Browse => Self::FocusPreview,
            Self::FocusPreview => Self::PreviewOnly,
            Self::PreviewOnly => Self::Browse,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Browse => "Browse",
            Self::FocusPreview => "Focus preview",
            Self::PreviewOnly => "Preview only",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaletteCommand {
    pub command: AppCommand,
    pub title: &'static str,
    pub keys: &'static str,
}

pub const PALETTE_COMMANDS: &[PaletteCommand] = &[
    PaletteCommand {
        command: AppCommand::CreateFile,
        title: "Create file",
        keys: "a f",
    },
    PaletteCommand {
        command: AppCommand::CreateDirectory,
        title: "Create directory",
        keys: "a d",
    },
    PaletteCommand {
        command: AppCommand::Rename,
        title: "Rename selected item",
        keys: "r / F2",
    },
    PaletteCommand {
        command: AppCommand::Copy,
        title: "Copy selection for paste",
        keys: "y y / Ctrl+C",
    },
    PaletteCommand {
        command: AppCommand::CopyName,
        title: "Copy filename as text",
        keys: "y n",
    },
    PaletteCommand {
        command: AppCommand::CopyDirectoryPath,
        title: "Copy containing directory path",
        keys: "y d",
    },
    PaletteCommand {
        command: AppCommand::CopyFullPath,
        title: "Copy full path as text",
        keys: "y p",
    },
    PaletteCommand {
        command: AppCommand::Cut,
        title: "Cut selection",
        keys: "x / Ctrl+X",
    },
    PaletteCommand {
        command: AppCommand::Paste,
        title: "Paste",
        keys: "p / Ctrl+V",
    },
    PaletteCommand {
        command: AppCommand::Trash,
        title: "Move selection to Trash",
        keys: "d d / Delete",
    },
    PaletteCommand {
        command: AppCommand::PermanentDelete,
        title: "Delete selection permanently",
        keys: "d D / Shift+Delete",
    },
    PaletteCommand {
        command: AppCommand::ToggleVisual,
        title: "Toggle Visual selection",
        keys: "v",
    },
    PaletteCommand {
        command: AppCommand::CycleLayout,
        title: "Cycle pane layout",
        keys: "z",
    },
    PaletteCommand {
        command: AppCommand::FullPreview,
        title: "Load full syntax preview",
        keys: "F",
    },
    PaletteCommand {
        command: AppCommand::GoParent,
        title: "Go to parent directory",
        keys: "h",
    },
    PaletteCommand {
        command: AppCommand::GoFirst,
        title: "Go to first item",
        keys: "g g",
    },
    PaletteCommand {
        command: AppCommand::GoLast,
        title: "Go to last item",
        keys: "G",
    },
    PaletteCommand {
        command: AppCommand::Quit,
        title: "Quit PathPilot",
        keys: "q",
    },
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandPalette {
    query: String,
    selected: usize,
    key_labels: std::collections::HashMap<AppCommand, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaletteMatch {
    pub command: AppCommand,
    pub title: &'static str,
    pub keys: String,
}

impl CommandPalette {
    pub fn reset(&mut self) {
        self.query.clear();
        self.selected = 0;
    }

    pub fn set_key_labels(&mut self, labels: impl IntoIterator<Item = (AppCommand, String)>) {
        self.key_labels = labels.into_iter().collect();
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.selected = 0;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn matches(&self) -> Vec<PaletteMatch> {
        let terms = self.query.to_lowercase();
        PALETTE_COMMANDS
            .iter()
            .filter(|item| {
                let keys = self
                    .key_labels
                    .get(&item.command)
                    .map_or(item.keys, String::as_str);
                terms.split_whitespace().all(|term| {
                    item.title.to_lowercase().contains(term) || keys.to_lowercase().contains(term)
                })
            })
            .map(|item| PaletteMatch {
                command: item.command,
                title: item.title,
                keys: self
                    .key_labels
                    .get(&item.command)
                    .cloned()
                    .unwrap_or_else(|| item.keys.to_owned()),
            })
            .collect()
    }

    pub fn move_selection(&mut self, offset: i32) {
        let count = self.matches().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        self.selected = if offset < 0 {
            self.selected.saturating_sub(offset.unsigned_abs() as usize)
        } else {
            self.selected.saturating_add(offset as usize).min(count - 1)
        };
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_command(&self) -> Option<AppCommand> {
        self.matches().get(self.selected).map(|item| item.command)
    }
}

/// Result of feeding a key to the normal-mode parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyResult {
    Command(AppCommand),
    Pending(PendingKeySequence),
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyHint {
    pub key: char,
    pub label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandHint {
    pub keys: &'static str,
    pub label: &'static str,
}

pub const COMMAND_REFERENCE: &[CommandHint] = &[
    CommandHint {
        keys: "j",
        label: "Down",
    },
    CommandHint {
        keys: "k",
        label: "Up",
    },
    CommandHint {
        keys: "h",
        label: "Parent",
    },
    CommandHint {
        keys: "l",
        label: "Open",
    },
    CommandHint {
        keys: "g",
        label: "Go to",
    },
    CommandHint {
        keys: "G",
        label: "Last item",
    },
    CommandHint {
        keys: "a",
        label: "Add (create)",
    },
    CommandHint {
        keys: "f",
        label: "Find by name",
    },
    CommandHint {
        keys: "r / F2",
        label: "Rename",
    },
    CommandHint {
        keys: "y / Ctrl+C",
        label: "Copy",
    },
    CommandHint {
        keys: "x / Ctrl+X",
        label: "Cut (move)",
    },
    CommandHint {
        keys: "p / Ctrl+V",
        label: "Paste",
    },
    CommandHint {
        keys: "v",
        label: "Visual selection",
    },
    CommandHint {
        keys: "d / Delete",
        label: "Delete (trash)",
    },
    CommandHint {
        keys: "d D / Shift+Delete",
        label: "Delete permanently",
    },
    CommandHint {
        keys: "q",
        label: "Quit",
    },
    CommandHint {
        keys: "F1",
        label: "Hide hints",
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingKeySequence {
    pub prefix: char,
    pub hints: Vec<KeyHint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyBinding {
    pub sequence: String,
    pub command: AppCommand,
    pub label: &'static str,
}

#[derive(Debug)]
pub struct KeySequenceParser {
    pending: Option<(Instant, char)>,
    sequence_timeout: Duration,
    bindings: Vec<KeyBinding>,
}

impl Default for KeySequenceParser {
    fn default() -> Self {
        Self::new(Duration::MAX)
    }
}

impl KeySequenceParser {
    pub fn new(sequence_timeout: Duration) -> Self {
        Self::with_bindings(sequence_timeout, default_key_bindings())
    }

    pub fn with_bindings(sequence_timeout: Duration, bindings: Vec<KeyBinding>) -> Self {
        Self {
            pending: None,
            sequence_timeout,
            bindings,
        }
    }

    pub fn feed(&mut self, key: char, now: Instant) -> KeyResult {
        if let Some(started) = self.pending.take()
            && now.duration_since(started.0) <= self.sequence_timeout
        {
            let sequence = format!("{}{key}", started.1);
            if let Some(binding) = self.bindings.iter().find(|item| item.sequence == sequence) {
                return KeyResult::Command(binding.command);
            }
        }
        if let Some(binding) = self
            .bindings
            .iter()
            .find(|item| item.sequence == key.to_string())
        {
            return KeyResult::Command(binding.command);
        }
        self.begin_sequence(key, now)
    }

    pub fn reset(&mut self) {
        self.pending = None;
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    fn begin_sequence(&mut self, prefix: char, now: Instant) -> KeyResult {
        let hints: Vec<_> = self
            .bindings
            .iter()
            .filter_map(|binding| {
                let mut chars = binding.sequence.chars();
                (chars.next() == Some(prefix))
                    .then(|| chars.next())
                    .flatten()
                    .map(|key| KeyHint {
                        key,
                        label: binding.label,
                    })
            })
            .collect();
        if hints.is_empty() {
            return KeyResult::Ignored;
        }
        self.pending = Some((now, prefix));
        KeyResult::Pending(PendingKeySequence { prefix, hints })
    }
}

fn default_key_bindings() -> Vec<KeyBinding> {
    [
        ("j", AppCommand::NavigateDown, "down"),
        ("k", AppCommand::NavigateUp, "up"),
        ("l", AppCommand::Enter, "open"),
        ("h", AppCommand::GoParent, "parent"),
        ("q", AppCommand::Quit, "quit"),
        ("r", AppCommand::Rename, "rename"),
        ("yy", AppCommand::Copy, "copy for paste"),
        ("yn", AppCommand::CopyName, "copy filename"),
        ("yd", AppCommand::CopyDirectoryPath, "copy directory path"),
        ("yp", AppCommand::CopyFullPath, "copy full path"),
        ("x", AppCommand::Cut, "cut"),
        ("p", AppCommand::Paste, "paste"),
        ("v", AppCommand::ToggleVisual, "visual selection"),
        ("z", AppCommand::CycleLayout, "cycle pane layout"),
        ("F", AppCommand::FullPreview, "full preview"),
        ("G", AppCommand::GoLast, "last item"),
        ("gg", AppCommand::GoFirst, "first item"),
        ("af", AppCommand::CreateFile, "create file"),
        ("ad", AppCommand::CreateDirectory, "create directory"),
        ("dd", AppCommand::Trash, "move to trash"),
        ("dD", AppCommand::PermanentDelete, "delete permanently"),
    ]
    .into_iter()
    .map(|(sequence, command, label)| KeyBinding {
        sequence: sequence.to_owned(),
        command,
        label,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: FileKind) -> FileEntry {
        FileEntry {
            location: Location::new(format!("file:///tmp/{name}")),
            display_name: name.to_owned(),
            kind,
            size: None,
            modified: None,
            unix_mode: None,
            content_type: None,
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        }
    }

    #[test]
    fn locations_keep_backend_neutral_uris() {
        let location = Location::new("file:///tmp/example");
        assert_eq!(location.uri(), "file:///tmp/example");
    }

    #[test]
    fn presents_local_paths_without_file_scheme_and_preserves_remote_uris() {
        let local = present_location(
            &Location::new("file:///home/roman/My%20Files"),
            Some(std::path::Path::new("/home/roman")),
        );
        assert_eq!(local.compact, "~/My Files");
        assert_eq!(local.full, "/home/roman/My Files");
        let remote = present_location(&Location::new("sftp://host/home/roman"), None);
        assert_eq!(remote.compact, "sftp://host/home/roman");
        assert_eq!(remote.full, remote.compact);
    }

    #[test]
    fn name_sort_puts_directories_first_and_ignores_case() {
        let mut entries = [
            entry("zeta.txt", FileKind::Regular),
            entry("beta", FileKind::Directory),
            entry("Alpha.txt", FileKind::Regular),
        ];
        entries.sort_by(FileEntry::compare_name);

        assert_eq!(entries[0].display_name, "beta");
        assert_eq!(entries[1].display_name, "Alpha.txt");
        assert_eq!(entries[2].display_name, "zeta.txt");
    }

    #[test]
    fn metadata_sort_is_deterministic_and_keeps_directories_first() {
        let directory = entry("folder", FileKind::Directory);
        let mut small = entry("small.rs", FileKind::Regular);
        small.size = Some(10);
        let mut large = entry("large.txt", FileKind::Regular);
        large.size = Some(20);
        let unknown = entry("unknown", FileKind::Regular);
        let mut entries = [unknown, small, directory, large];
        let descending = SortMode {
            key: SortKey::Size,
            descending: true,
        };
        entries.sort_by(|left, right| left.compare(right, descending));

        assert_eq!(entries[0].display_name, "folder");
        assert_eq!(entries[1].display_name, "large.txt");
        assert_eq!(entries[2].display_name, "small.rs");
        assert_eq!(entries[3].display_name, "unknown");
    }

    #[test]
    fn extension_sort_puts_extensionless_entries_last() {
        let mut entries = [
            entry("README", FileKind::Regular),
            entry("main.rs", FileKind::Regular),
            entry("notes.md", FileKind::Regular),
        ];
        entries.sort_by(|left, right| {
            left.compare(
                right,
                SortMode {
                    key: SortKey::Extension,
                    descending: false,
                },
            )
        });
        assert_eq!(entries[0].display_name, "notes.md");
        assert_eq!(entries[1].display_name, "main.rs");
        assert_eq!(entries[2].display_name, "README");
    }

    #[test]
    fn generation_tracker_rejects_stale_results() {
        let mut tracker = GenerationTracker::default();
        let stale = tracker.advance();
        let current = tracker.advance();

        assert!(!tracker.accepts(stale));
        assert!(tracker.accepts(current));
    }

    #[test]
    fn navigation_restores_cursor_for_visited_directory() {
        let first = Location::new("file:///first");
        let second = Location::new("file:///second");
        let mut navigation = NavigationState::new(first.clone());

        navigation.remember_cursor(42);
        assert_eq!(navigation.navigate_to(second), None);
        navigation.remember_cursor(7);
        assert_eq!(navigation.navigate_to(first), Some(42));
        assert_eq!(navigation.current().uri(), "file:///first");
    }

    #[test]
    fn maps_single_key_commands() {
        let now = Instant::now();
        let mut parser = KeySequenceParser::default();

        assert_eq!(
            parser.feed('j', now),
            KeyResult::Command(AppCommand::NavigateDown)
        );
        assert_eq!(
            parser.feed('k', now),
            KeyResult::Command(AppCommand::NavigateUp)
        );
        assert_eq!(
            parser.feed('G', now),
            KeyResult::Command(AppCommand::GoLast)
        );
        assert_eq!(
            parser.feed('h', now),
            KeyResult::Command(AppCommand::GoParent)
        );
        assert_eq!(parser.feed('l', now), KeyResult::Command(AppCommand::Enter));
        assert_eq!(parser.feed('q', now), KeyResult::Command(AppCommand::Quit));
        assert_eq!(parser.feed('x', now), KeyResult::Command(AppCommand::Cut));
        assert_eq!(parser.feed('p', now), KeyResult::Command(AppCommand::Paste));
        assert_eq!(
            parser.feed('v', now),
            KeyResult::Command(AppCommand::ToggleVisual)
        );
    }

    #[test]
    fn recognizes_gg() {
        let now = Instant::now();
        let mut parser = KeySequenceParser::default();

        assert!(matches!(parser.feed('g', now), KeyResult::Pending(_)));
        assert_eq!(
            parser.feed('g', now + Duration::from_millis(10)),
            KeyResult::Command(AppCommand::GoFirst)
        );
    }

    #[test]
    fn expires_pending_sequence() {
        let now = Instant::now();
        let mut parser = KeySequenceParser::new(Duration::from_millis(50));

        assert!(matches!(parser.feed('g', now), KeyResult::Pending(_)));
        assert!(matches!(
            parser.feed('g', now + Duration::from_millis(51)),
            KeyResult::Pending(_)
        ));
    }

    #[test]
    fn unrelated_key_clears_pending_sequence() {
        let now = Instant::now();
        let mut parser = KeySequenceParser::default();

        assert!(matches!(parser.feed('g', now), KeyResult::Pending(_)));
        assert_eq!(parser.feed('x', now), KeyResult::Command(AppCommand::Cut));
        assert!(matches!(parser.feed('g', now), KeyResult::Pending(_)));
    }

    #[test]
    fn exposes_create_command_continuations() {
        let KeyResult::Pending(pending) = KeySequenceParser::default().feed('a', Instant::now())
        else {
            panic!("expected pending sequence");
        };
        assert_eq!(pending.prefix, 'a');
        assert_eq!(pending.hints.len(), 2);
        assert_eq!(pending.hints[0].key, 'f');
        assert_eq!(pending.hints[1].key, 'd');
    }

    #[test]
    fn command_reference_uses_progressive_disclosure() {
        let add = COMMAND_REFERENCE
            .iter()
            .find(|hint| hint.keys == "a")
            .expect("top-level add command");
        assert_eq!(add.label, "Add (create)");
        assert!(
            !COMMAND_REFERENCE
                .iter()
                .any(|hint| hint.keys == "a f" || hint.keys == "a d")
        );
    }

    #[test]
    fn filename_find_matches_unicode_case_insensitively_and_wraps() {
        let names = vec![
            "alpha.txt".to_owned(),
            "Notes.md".to_owned(),
            "ПРОЕКТ.yaml".to_owned(),
            "other-notes.txt".to_owned(),
        ];
        let mut find = FilenameFind::default();
        find.start(2);
        for character in "проект".chars() {
            find.push(character, &names);
        }
        assert_eq!(find.current_position, Some(2));

        find.start(2);
        for character in "notes".chars() {
            find.push(character, &names);
        }
        assert_eq!(find.current_position, Some(3));
        find.accept();
        assert_eq!(find.repeat(&names, true), Some(1));
        assert_eq!(find.repeat(&names, false), Some(3));
    }

    #[test]
    fn cancelling_filename_find_restores_original_position() {
        let names = vec!["alpha".to_owned(), "beta".to_owned()];
        let mut find = FilenameFind::default();
        find.start(1);
        find.push('a', &names);
        assert_eq!(find.cancel(), 1);
        assert!(!find.is_active());
        assert!(find.query().is_empty());
    }

    #[test]
    fn maps_operation_key_sequences() {
        let now = Instant::now();
        for (prefix, continuation, command) in [
            ('a', 'f', AppCommand::CreateFile),
            ('a', 'd', AppCommand::CreateDirectory),
            ('d', 'd', AppCommand::Trash),
            ('d', 'D', AppCommand::PermanentDelete),
            ('y', 'y', AppCommand::Copy),
            ('y', 'n', AppCommand::CopyName),
            ('y', 'd', AppCommand::CopyDirectoryPath),
            ('y', 'p', AppCommand::CopyFullPath),
        ] {
            let mut parser = KeySequenceParser::default();
            assert!(matches!(parser.feed(prefix, now), KeyResult::Pending(_)));
            assert_eq!(
                parser.feed(continuation, now + Duration::from_millis(10)),
                KeyResult::Command(command)
            );
        }
    }

    #[test]
    fn clipboard_is_retained_after_copy_and_cleared_only_after_successful_move() {
        let source = Location::new("file:///tmp/source.txt");
        let copy_clipboard = OperationClipboard {
            action: ClipboardAction::Copy,
            items: vec![ClipboardItem {
                source: source.clone(),
                display_name: "source.txt".to_owned(),
            }],
        };
        assert!(!copy_clipboard.should_clear_after_move(true));
        assert_eq!(copy_clipboard.status_label(), "COPY: source.txt");

        let move_clipboard = OperationClipboard {
            action: ClipboardAction::Move,
            items: vec![ClipboardItem {
                source: source.clone(),
                display_name: "source.txt".to_owned(),
            }],
        };
        assert!(!move_clipboard.should_clear_after_move(false));
        assert!(move_clipboard.should_clear_after_move(true));
        assert_eq!(move_clipboard.status_label(), "CUT: source.txt");
    }

    #[test]
    fn command_palette_filters_by_title_and_key_and_clamps_selection() {
        let mut palette = CommandPalette::default();
        assert_eq!(palette.selected_command(), Some(AppCommand::CreateFile));
        palette.set_query("permanent");
        assert_eq!(
            palette.selected_command(),
            Some(AppCommand::PermanentDelete)
        );
        palette.set_query("ctrl+c");
        assert_eq!(palette.selected_command(), Some(AppCommand::Copy));
        palette.set_query("create");
        palette.move_selection(10);
        assert_eq!(
            palette.selected_command(),
            Some(AppCommand::CreateDirectory)
        );
        palette.set_query("not a command");
        assert_eq!(palette.selected_command(), None);
        palette.set_key_labels([(AppCommand::Copy, "zz".to_owned())]);
        palette.set_query("ctrl+c");
        assert_eq!(palette.selected_command(), None);
        palette.set_query("zz");
        assert_eq!(palette.selected_command(), Some(AppCommand::Copy));
    }

    #[test]
    fn key_parser_accepts_runtime_bindings() {
        let mut parser = KeySequenceParser::with_bindings(
            Duration::MAX,
            vec![KeyBinding {
                sequence: "z".to_owned(),
                command: AppCommand::Copy,
                label: "copy",
            }],
        );
        assert_eq!(
            parser.feed('z', Instant::now()),
            KeyResult::Command(AppCommand::Copy)
        );
        assert_eq!(parser.feed('y', Instant::now()), KeyResult::Ignored);
    }

    #[test]
    fn command_mode_has_explicit_entry_and_cancel_transitions() {
        let mut mode = AppMode::default();
        assert!(mode.begin_command());
        assert_eq!(mode, AppMode::Command);
        assert!(!mode.begin_find());
        assert!(mode.cancel());
        assert_eq!(mode, AppMode::Normal);
    }

    #[test]
    fn pane_layout_cycles_through_all_views() {
        let layout = PaneLayout::default();
        assert_eq!(layout.next(), PaneLayout::FocusPreview);
        assert_eq!(layout.next().next(), PaneLayout::PreviewOnly);
        assert_eq!(layout.next().next().next(), PaneLayout::Browse);
    }

    #[test]
    fn mode_state_machine_allows_only_explicit_transitions() {
        let mut mode = AppMode::default();
        assert!(mode.begin_find());
        assert!(!mode.begin_text_input(InputModeKind::CreateFile, ""));
        assert!(mode.finish_find());
        assert!(mode.begin_text_input(InputModeKind::CreateDirectory, ""));
        assert!(!mode.begin_find());
        assert!(mode.cancel());
        assert_eq!(mode, AppMode::Normal);
        assert!(!mode.cancel());
    }

    #[test]
    fn visual_selection_tracks_an_inclusive_range_in_both_directions() {
        let mut mode = AppMode::default();
        assert!(mode.begin_visual(4));
        let selection = mode.visual().expect("visual selection");
        assert_eq!(selection.anchor(), 4);
        assert_eq!(selection.range(), 4..=4);

        mode.visual_mut()
            .expect("visual selection")
            .set_cursor(1, 10);
        let selection = mode.visual().expect("visual selection");
        assert_eq!(selection.cursor(), 1);
        assert_eq!(selection.range(), 1..=4);
        assert_eq!(selection.len(), 4);

        mode.visual_mut()
            .expect("visual selection")
            .set_cursor(99, 10);
        let selection = mode.visual().expect("visual selection");
        assert_eq!(selection.cursor(), 9);
        assert_eq!(selection.range(), 4..=9);
        assert_eq!(selection.len(), 6);
    }

    #[test]
    fn text_input_validates_names_and_rename_replaces_the_initial_selection() {
        let mut mode = AppMode::default();
        mode.begin_text_input(InputModeKind::CreateFile, "");
        assert_eq!(mode.submit_text_input(), None);
        assert_eq!(
            mode.text_input().and_then(TextInputMode::error),
            Some("Name cannot be empty")
        );
        for character in "../bad".chars() {
            mode.text_input_mut().expect("text input").push(character);
        }
        assert_eq!(mode.submit_text_input(), None);
        mode.cancel();

        mode.begin_text_input(InputModeKind::Rename, "old-name.txt");
        mode.text_input_mut().expect("text input").push('n');
        for character in "ew-name.txt".chars() {
            mode.text_input_mut().expect("text input").push(character);
        }
        assert_eq!(
            mode.submit_text_input(),
            Some((InputModeKind::Rename, "new-name.txt".to_owned()))
        );
        assert_eq!(mode, AppMode::Normal);
    }
}
