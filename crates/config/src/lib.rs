//! Loading and validation for user-configurable keyboard bindings.

use pathpilot_core::{AppCommand, KeyBinding};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

pub const DEFAULT_KEYMAP: &str = include_str!("../../../data/default-keymap.toml");

#[derive(Debug, Deserialize)]
struct KeymapFile {
    bindings: BTreeMap<String, BindingValue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BindingValue {
    One(String),
    Many(Vec<String>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
}

impl Keymap {
    pub fn defaults() -> Self {
        parse(DEFAULT_KEYMAP).expect("built-in keymap is valid")
    }
    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }
}

pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join("pathpilot/keymap.toml"))
}

pub fn load(path: &Path) -> Result<Keymap, String> {
    parse(&fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?)
}

pub fn load_or_default(path: Option<&Path>) -> (Keymap, Option<String>) {
    let Some(path) = path else {
        return (Keymap::defaults(), None);
    };
    if !path.exists() {
        return (Keymap::defaults(), None);
    }
    match load(path) {
        Ok(keymap) => (keymap, None),
        Err(error) => (Keymap::defaults(), Some(error)),
    }
}

fn parse(source: &str) -> Result<Keymap, String> {
    let file: KeymapFile = toml::from_str(source).map_err(|error| error.to_string())?;
    let mut seen = HashSet::new();
    let mut bindings = Vec::new();
    for (name, values) in file.bindings {
        let command = command(&name).ok_or_else(|| format!("unknown command `{name}`"))?;
        let values = match values {
            BindingValue::One(value) => vec![value],
            BindingValue::Many(values) => values,
        };
        if values.is_empty() {
            return Err(format!("command `{name}` has no bindings"));
        }
        for sequence in values {
            if sequence.is_empty() || sequence.chars().count() > 2 {
                return Err(format!("invalid sequence `{sequence}`"));
            }
            if !seen.insert(sequence.clone()) {
                return Err(format!("duplicate sequence `{sequence}`"));
            }
            bindings.push(KeyBinding {
                sequence,
                command,
                label: label(command),
            });
        }
    }
    Ok(Keymap { bindings })
}

fn command(name: &str) -> Option<AppCommand> {
    Some(match name {
        "navigate_up" => AppCommand::NavigateUp,
        "navigate_down" => AppCommand::NavigateDown,
        "open" => AppCommand::Enter,
        "parent" => AppCommand::GoParent,
        "first" => AppCommand::GoFirst,
        "last" => AppCommand::GoLast,
        "quit" => AppCommand::Quit,
        "create_file" => AppCommand::CreateFile,
        "create_directory" => AppCommand::CreateDirectory,
        "rename" => AppCommand::Rename,
        "trash" => AppCommand::Trash,
        "delete_permanently" => AppCommand::PermanentDelete,
        "copy" => AppCommand::Copy,
        "cut" => AppCommand::Cut,
        "paste" => AppCommand::Paste,
        "visual" => AppCommand::ToggleVisual,
        _ => return None,
    })
}

fn label(command: AppCommand) -> &'static str {
    match command {
        AppCommand::NavigateUp => "Up",
        AppCommand::NavigateDown => "Down",
        AppCommand::Enter => "Open",
        AppCommand::GoParent => "Parent",
        AppCommand::GoFirst => "First item",
        AppCommand::GoLast => "Last item",
        AppCommand::Quit => "Quit",
        AppCommand::CreateFile => "Create file",
        AppCommand::CreateDirectory => "Create directory",
        AppCommand::Rename => "Rename",
        AppCommand::Trash => "Trash",
        AppCommand::PermanentDelete => "Delete permanently",
        AppCommand::Copy => "Copy",
        AppCommand::Cut => "Cut",
        AppCommand::Paste => "Paste",
        AppCommand::ToggleVisual => "Visual selection",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_are_valid() {
        assert!(!Keymap::defaults().bindings().is_empty());
    }
    #[test]
    fn rejects_unknown_commands_and_duplicates() {
        assert!(
            parse("[bindings]\nwat = 'z'")
                .unwrap_err()
                .contains("unknown")
        );
        assert!(
            parse("[bindings]\ncopy = 'z'\ncut = 'z'")
                .unwrap_err()
                .contains("duplicate")
        );
    }
    #[test]
    fn invalid_file_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keymap.toml");
        fs::write(&path, "not toml").unwrap();
        let (keymap, warning) = load_or_default(Some(&path));
        assert!(!keymap.bindings().is_empty());
        assert!(warning.is_some());
    }
}
