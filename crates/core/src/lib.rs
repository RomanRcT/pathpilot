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
    pub content_type: Option<String>,
    pub is_hidden: bool,
    pub is_symlink: bool,
}

impl FileEntry {
    pub fn compare_name(&self, other: &Self) -> Ordering {
        match (
            self.kind == FileKind::Directory,
            other.kind == FileKind::Directory,
        ) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => self
                .display_name
                .to_lowercase()
                .cmp(&other.display_name.to_lowercase())
                .then_with(|| self.display_name.cmp(&other.display_name)),
        }
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
pub struct OperationClipboard {
    pub action: ClipboardAction,
    pub source: Location,
    pub display_name: String,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    Cut,
    Paste,
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

/// Minimal state machine for Phase 0 normal-mode navigation.
#[derive(Debug)]
pub struct KeySequenceParser {
    pending_g: Option<(Instant, char)>,
    sequence_timeout: Duration,
}

impl Default for KeySequenceParser {
    fn default() -> Self {
        Self::new(Duration::MAX)
    }
}

impl KeySequenceParser {
    pub fn new(sequence_timeout: Duration) -> Self {
        Self {
            pending_g: None,
            sequence_timeout,
        }
    }

    pub fn feed(&mut self, key: char, now: Instant) -> KeyResult {
        if let Some(started) = self.pending_g.take()
            && now.duration_since(started.0) <= self.sequence_timeout
        {
            let command = match (started.1, key) {
                ('g', 'g') => Some(AppCommand::GoFirst),
                ('a', 'f') => Some(AppCommand::CreateFile),
                ('a', 'd') => Some(AppCommand::CreateDirectory),
                ('d', 'd') => Some(AppCommand::Trash),
                ('d', 'D') => Some(AppCommand::PermanentDelete),
                _ => None,
            };
            if let Some(command) = command {
                return KeyResult::Command(command);
            }
        }

        match key {
            'j' => KeyResult::Command(AppCommand::NavigateDown),
            'k' => KeyResult::Command(AppCommand::NavigateUp),
            'l' => KeyResult::Command(AppCommand::Enter),
            'h' => KeyResult::Command(AppCommand::GoParent),
            'q' => KeyResult::Command(AppCommand::Quit),
            'r' => KeyResult::Command(AppCommand::Rename),
            'y' => KeyResult::Command(AppCommand::Copy),
            'x' => KeyResult::Command(AppCommand::Cut),
            'p' => KeyResult::Command(AppCommand::Paste),
            'G' => KeyResult::Command(AppCommand::GoLast),
            'g' | 'a' | 'd' => self.begin_sequence(key, now),
            _ => KeyResult::Ignored,
        }
    }

    pub fn reset(&mut self) {
        self.pending_g = None;
    }

    pub fn is_pending(&self) -> bool {
        self.pending_g.is_some()
    }

    fn begin_sequence(&mut self, prefix: char, now: Instant) -> KeyResult {
        self.pending_g = Some((now, prefix));
        let hints = match prefix {
            'g' => vec![KeyHint {
                key: 'g',
                label: "first item",
            }],
            'a' => vec![
                KeyHint {
                    key: 'f',
                    label: "create file",
                },
                KeyHint {
                    key: 'd',
                    label: "create directory",
                },
            ],
            'd' => vec![
                KeyHint {
                    key: 'd',
                    label: "move to trash",
                },
                KeyHint {
                    key: 'D',
                    label: "delete permanently",
                },
            ],
            _ => Vec::new(),
        };
        KeyResult::Pending(PendingKeySequence { prefix, hints })
    }
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
            content_type: None,
            is_hidden: false,
            is_symlink: false,
        }
    }

    #[test]
    fn locations_keep_backend_neutral_uris() {
        let location = Location::new("file:///tmp/example");
        assert_eq!(location.uri(), "file:///tmp/example");
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
        assert_eq!(parser.feed('y', now), KeyResult::Command(AppCommand::Copy));
        assert_eq!(parser.feed('x', now), KeyResult::Command(AppCommand::Cut));
        assert_eq!(parser.feed('p', now), KeyResult::Command(AppCommand::Paste));
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
        ] {
            let mut parser = KeySequenceParser::default();
            assert!(matches!(parser.feed(prefix, now), KeyResult::Pending(_)));
            assert_eq!(
                parser.feed(continuation, now + Duration::from_millis(10)),
                KeyResult::Command(command)
            );
        }
    }
}
