//! Loading and validation for user-configurable keyboard bindings.

use pathpilot_core::{AppCommand, KeyBinding};
use serde::{Deserialize, Serialize};
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

    pub fn key_labels(&self) -> Vec<(AppCommand, String)> {
        let mut labels = std::collections::HashMap::<AppCommand, Vec<String>>::new();
        for binding in &self.bindings {
            labels
                .entry(binding.command)
                .or_default()
                .push(binding.sequence.clone());
        }
        labels
            .into_iter()
            .map(|(command, keys)| (command, keys.join(" / ")))
            .collect()
    }

    pub fn command_reference(&self) -> Vec<(String, &'static str)> {
        self.bindings
            .iter()
            .map(|binding| (binding.sequence.clone(), binding.label))
            .collect()
    }
}

pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join("pathpilot/keymap.toml"))
}

pub fn settings_path() -> Option<PathBuf> {
    default_path().and_then(|path| path.parent().map(|parent| parent.join("config.toml")))
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    pub ui: UiSettings,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bookmarks: Vec<Bookmark>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub open_with: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Bookmark {
    pub key: String,
    pub label: String,
    pub uri: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct UiSettings {
    pub hints_enabled: bool,
    pub confirm_permanent_delete: bool,
    pub preview_delay_ms: u64,
    pub preview_line_numbers: bool,
    pub color_scheme: String,
    pub sort_key: String,
    pub sort_descending: bool,
    pub compact_rows: bool,
    pub show_hidden: bool,
    pub window_width: i32,
    pub window_height: i32,
    pub window_maximized: bool,
    pub last_location: Option<String>,
    pub pane_layout: String,
    pub browse_outer_position: i32,
    pub browse_right_position: i32,
    pub focus_right_position: i32,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            hints_enabled: false,
            confirm_permanent_delete: true,
            preview_delay_ms: 75,
            preview_line_numbers: true,
            color_scheme: "system".to_owned(),
            sort_key: "name".to_owned(),
            sort_descending: false,
            compact_rows: true,
            show_hidden: false,
            window_width: 1400,
            window_height: 760,
            window_maximized: false,
            last_location: None,
            pane_layout: "browse".to_owned(),
            browse_outer_position: 360,
            browse_right_position: 560,
            focus_right_position: 460,
        }
    }
}

pub fn load_settings(path: Option<&Path>) -> (Settings, Option<String>) {
    let Some(path) = path else {
        return (Settings::default(), None);
    };
    if !path.exists() {
        return (Settings::default(), None);
    }
    match fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|source| toml::from_str::<Settings>(&source).map_err(|error| error.to_string()))
    {
        Ok(settings) => match validate_bookmarks(&settings.bookmarks) {
            Ok(()) => (settings, None),
            Err(error) => (Settings::default(), Some(error)),
        },
        Err(error) => (Settings::default(), Some(error)),
    }
}

fn validate_bookmarks(bookmarks: &[Bookmark]) -> Result<(), String> {
    let mut keys = HashSet::new();
    for bookmark in bookmarks {
        let mut characters = bookmark.key.chars();
        let Some(key) = characters.next() else {
            return Err("bookmark key is empty".to_owned());
        };
        if characters.next().is_some() || !key.is_ascii_alphanumeric() {
            return Err(format!("invalid bookmark key `{}`", bookmark.key));
        }
        if ['g', 'h', 'd', 'r'].contains(&key) || !keys.insert(key) {
            return Err(format!("duplicate or reserved bookmark key `{key}`"));
        }
        if bookmark.label.trim().is_empty() {
            return Err(format!("bookmark `{key}` has an empty label"));
        }
        if !bookmark.uri.contains("://") {
            return Err(format!("bookmark `{key}` has an invalid URI"));
        }
    }
    Ok(())
}

pub fn save_settings(path: &Path, settings: &Settings) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "settings path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(
        path,
        toml::to_string_pretty(settings).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
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
        "copy_name" => AppCommand::CopyName,
        "copy_directory_path" => AppCommand::CopyDirectoryPath,
        "copy_full_path" => AppCommand::CopyFullPath,
        "cut" => AppCommand::Cut,
        "paste" => AppCommand::Paste,
        "visual" => AppCommand::ToggleVisual,
        "cycle_layout" => AppCommand::CycleLayout,
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
        AppCommand::CopyName => "Copy filename",
        AppCommand::CopyDirectoryPath => "Copy directory path",
        AppCommand::CopyFullPath => "Copy full path",
        AppCommand::Cut => "Cut",
        AppCommand::Paste => "Paste",
        AppCommand::ToggleVisual => "Visual selection",
        AppCommand::CycleLayout => "Cycle pane layout",
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
    #[test]
    fn settings_round_trip_and_missing_fields_use_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut settings = Settings::default();
        settings.ui.hints_enabled = true;
        settings.bookmarks.push(Bookmark {
            key: "w".to_owned(),
            label: "Work".to_owned(),
            uri: "file:///srv/work".to_owned(),
        });
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(Some(&path)).0, settings);
        fs::write(&path, "[ui]\nhints_enabled = true\n").unwrap();
        let loaded = load_settings(Some(&path)).0;
        assert!(loaded.ui.hints_enabled);
        assert!(loaded.ui.confirm_permanent_delete);
    }

    #[test]
    fn empty_bookmarks_are_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_settings(&path, &Settings::default()).unwrap();
        let source = fs::read_to_string(path).unwrap();
        assert!(!source.contains("bookmarks"));
        assert!(!source.contains("open_with"));
    }

    #[test]
    fn open_with_history_round_trips_by_content_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut settings = Settings::default();
        settings.open_with.insert(
            "text/plain".to_owned(),
            vec!["org.gnome.TextEditor.desktop".to_owned()],
        );
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(Some(&path)).0, settings);
    }

    #[test]
    fn rejects_reserved_duplicate_and_invalid_bookmarks() {
        assert!(
            validate_bookmarks(&[Bookmark {
                key: "h".to_owned(),
                label: "Other home".to_owned(),
                uri: "file:///tmp".to_owned()
            }])
            .is_err()
        );
        assert!(
            validate_bookmarks(&[Bookmark {
                key: "w".to_owned(),
                label: "Work".to_owned(),
                uri: "/tmp".to_owned()
            }])
            .is_err()
        );
        assert!(
            validate_bookmarks(&[
                Bookmark {
                    key: "w".to_owned(),
                    label: "One".to_owned(),
                    uri: "file:///one".to_owned()
                },
                Bookmark {
                    key: "w".to_owned(),
                    label: "Two".to_owned(),
                    uri: "file:///two".to_owned()
                },
            ])
            .is_err()
        );
    }
}
