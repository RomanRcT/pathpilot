//! GTK-independent domain commands and key-sequence handling.

use std::time::{Duration, Instant};

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
