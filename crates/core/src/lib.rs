//! GTK-independent domain commands and key-sequence handling.

use std::{
    cmp::Ordering,
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
    GoFirst,
    GoLast,
}

/// Result of feeding a key to the normal-mode parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyResult {
    Command(AppCommand),
    Pending,
    Ignored,
}

/// Minimal state machine for Phase 0 normal-mode navigation.
#[derive(Debug)]
pub struct KeySequenceParser {
    pending_g: Option<Instant>,
    sequence_timeout: Duration,
}

impl Default for KeySequenceParser {
    fn default() -> Self {
        Self::new(Duration::from_millis(750))
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
            && now.duration_since(started) <= self.sequence_timeout
            && key == 'g'
        {
            return KeyResult::Command(AppCommand::GoFirst);
        }

        match key {
            'j' => KeyResult::Command(AppCommand::NavigateDown),
            'k' => KeyResult::Command(AppCommand::NavigateUp),
            'G' => KeyResult::Command(AppCommand::GoLast),
            'g' => {
                self.pending_g = Some(now);
                KeyResult::Pending
            }
            _ => KeyResult::Ignored,
        }
    }

    pub fn reset(&mut self) {
        self.pending_g = None;
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
    }

    #[test]
    fn recognizes_gg() {
        let now = Instant::now();
        let mut parser = KeySequenceParser::default();

        assert_eq!(parser.feed('g', now), KeyResult::Pending);
        assert_eq!(
            parser.feed('g', now + Duration::from_millis(10)),
            KeyResult::Command(AppCommand::GoFirst)
        );
    }

    #[test]
    fn expires_pending_sequence() {
        let now = Instant::now();
        let mut parser = KeySequenceParser::new(Duration::from_millis(50));

        assert_eq!(parser.feed('g', now), KeyResult::Pending);
        assert_eq!(
            parser.feed('g', now + Duration::from_millis(51)),
            KeyResult::Pending
        );
    }

    #[test]
    fn unrelated_key_clears_pending_sequence() {
        let now = Instant::now();
        let mut parser = KeySequenceParser::default();

        assert_eq!(parser.feed('g', now), KeyResult::Pending);
        assert_eq!(parser.feed('x', now), KeyResult::Ignored);
        assert_eq!(parser.feed('g', now), KeyResult::Pending);
    }
}
