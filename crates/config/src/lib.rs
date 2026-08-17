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

pub fn bookmarks_path() -> Option<PathBuf> {
    default_path().and_then(|path| path.parent().map(|parent| parent.join("bookmarks.toml")))
}

pub fn open_with_path() -> Option<PathBuf> {
    default_path().and_then(|path| path.parent().map(|parent| parent.join("open-with.toml")))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    pub ui: UiSettings,
    pub bookmarks: Vec<Bookmark>,
    pub open_with: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct ConfigFile {
    ui: UiSettings,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bookmarks: Vec<Bookmark>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    open_with: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct BookmarksFile {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bookmarks: Vec<Bookmark>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct OpenWithFile {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    open_with: BTreeMap<String, Vec<String>>,
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
    let config = match load_toml::<ConfigFile>(path) {
        Ok(Some(config)) => config,
        Ok(None) => ConfigFile::default(),
        Err(error) => return (Settings::default(), Some(error)),
    };
    if let Err(error) = validate_bookmarks(&config.bookmarks) {
        return (Settings::default(), Some(error));
    }

    let bookmarks_path = path.with_file_name("bookmarks.toml");
    let open_with_path = path.with_file_name("open-with.toml");
    let bookmarks_exist = bookmarks_path.exists();
    let open_with_exists = open_with_path.exists();
    let bookmarks = match load_toml::<BookmarksFile>(&bookmarks_path) {
        Ok(Some(file)) => file.bookmarks,
        Ok(None) => config.bookmarks.clone(),
        Err(error) => return (Settings::default(), Some(error)),
    };
    if let Err(error) = validate_bookmarks(&bookmarks) {
        return (Settings::default(), Some(error));
    }
    let open_with = match load_toml::<OpenWithFile>(&open_with_path) {
        Ok(Some(file)) => file.open_with,
        Ok(None) => config.open_with.clone(),
        Err(error) => return (Settings::default(), Some(error)),
    };

    let settings = Settings {
        ui: config.ui,
        bookmarks,
        open_with,
    };
    let migrating_bookmarks = !bookmarks_exist && !config.bookmarks.is_empty();
    let migrating_open_with = !open_with_exists && !config.open_with.is_empty();
    if migrating_bookmarks || migrating_open_with {
        if migrating_bookmarks
            && let Err(error) = save_bookmarks(&bookmarks_path, &settings.bookmarks)
        {
            return (
                settings,
                Some(format!("could not migrate bookmarks: {error}")),
            );
        }
        if migrating_open_with
            && let Err(error) = save_open_with(&open_with_path, &settings.open_with)
        {
            return (
                settings,
                Some(format!("could not migrate Open With history: {error}")),
            );
        }
        if let Err(error) = save_settings(path, &settings) {
            return (
                settings,
                Some(format!("could not finish settings migration: {error}")),
            );
        }
    }
    (settings, None)
}

fn load_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map_err(|error| format!("{}: {error}", path.display()))
        .and_then(|source| {
            toml::from_str(&source)
                .map(Some)
                .map_err(|error| format!("{}: {error}", path.display()))
        })
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
        // Keep accepting l/w from older bookmark files. The UI reserves those
        // keys for remote connections when creating new bookmarks, but making
        // them a config-level error would discard otherwise valid UI settings.
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
    save_toml(
        path,
        &ConfigFile {
            ui: settings.ui.clone(),
            ..ConfigFile::default()
        },
    )
}

pub fn save_bookmarks(path: &Path, bookmarks: &[Bookmark]) -> Result<(), String> {
    validate_bookmarks(bookmarks)?;
    save_toml(
        path,
        &BookmarksFile {
            bookmarks: bookmarks.to_vec(),
        },
    )
}

pub fn save_open_with(
    path: &Path,
    open_with: &BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    save_toml(
        path,
        &OpenWithFile {
            open_with: open_with.clone(),
        },
    )
}

fn save_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "settings path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(
        path,
        toml::to_string_pretty(value).map_err(|error| error.to_string())?,
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
        "full_preview" => AppCommand::FullPreview,
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
        AppCommand::FullPreview => "Full preview",
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
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(Some(&path)).0, settings);
        fs::write(&path, "[ui]\nhints_enabled = true\n").unwrap();
        let loaded = load_settings(Some(&path)).0;
        assert!(loaded.ui.hints_enabled);
        assert!(loaded.ui.confirm_permanent_delete);
    }

    #[test]
    fn config_contains_only_ui_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut settings = Settings::default();
        settings.bookmarks.push(Bookmark {
            key: "p".to_owned(),
            label: "Work".to_owned(),
            uri: "file:///srv/work".to_owned(),
        });
        settings.open_with.insert(
            "text/plain".to_owned(),
            vec!["org.gnome.TextEditor.desktop".to_owned()],
        );
        save_settings(&path, &settings).unwrap();
        let source = fs::read_to_string(path).unwrap();
        assert!(!source.contains("bookmarks"));
        assert!(!source.contains("open_with"));
    }

    #[test]
    fn companion_files_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let bookmarks_path = dir.path().join("bookmarks.toml");
        let open_with_path = dir.path().join("open-with.toml");
        let mut settings = Settings::default();
        settings.bookmarks.push(Bookmark {
            key: "p".to_owned(),
            label: "Work".to_owned(),
            uri: "file:///srv/work".to_owned(),
        });
        settings.open_with.insert(
            "text/plain".to_owned(),
            vec!["org.gnome.TextEditor.desktop".to_owned()],
        );
        save_settings(&config_path, &settings).unwrap();
        save_bookmarks(&bookmarks_path, &settings.bookmarks).unwrap();
        save_open_with(&open_with_path, &settings.open_with).unwrap();
        assert_eq!(load_settings(Some(&config_path)).0, settings);
    }

    #[test]
    fn legacy_remote_keys_do_not_discard_ui_settings() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[ui]\nwindow_width = 1777\n").unwrap();
        fs::write(
            dir.path().join("bookmarks.toml"),
            "bookmarks = [{ key = \"w\", label = \"Work\", uri = \"file:///srv/work\" }]\n",
        )
        .unwrap();

        let (settings, warning) = load_settings(Some(&config_path));
        assert!(warning.is_none());
        assert_eq!(settings.ui.window_width, 1777);
        assert_eq!(settings.bookmarks[0].key, "w");
    }

    #[test]
    fn legacy_config_is_migrated_to_companion_files() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
bookmarks = [{ key = "p", label = "Work", uri = "file:///srv/work" }]

[open_with]
"text/plain" = ["org.gnome.TextEditor.desktop"]
"#,
        )
        .unwrap();

        let (settings, warning) = load_settings(Some(&config_path));
        assert!(warning.is_none());
        assert_eq!(settings.bookmarks[0].key, "p");
        assert_eq!(
            settings.open_with["text/plain"],
            ["org.gnome.TextEditor.desktop"]
        );
        assert!(dir.path().join("bookmarks.toml").exists());
        assert!(dir.path().join("open-with.toml").exists());
        let config = fs::read_to_string(config_path).unwrap();
        assert!(!config.contains("bookmarks"));
        assert!(!config.contains("open_with"));
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
                key: "p".to_owned(),
                label: "Work".to_owned(),
                uri: "/tmp".to_owned()
            }])
            .is_err()
        );
        assert!(
            validate_bookmarks(&[
                Bookmark {
                    key: "p".to_owned(),
                    label: "One".to_owned(),
                    uri: "file:///one".to_owned()
                },
                Bookmark {
                    key: "p".to_owned(),
                    label: "Two".to_owned(),
                    uri: "file:///two".to_owned()
                },
            ])
            .is_err()
        );
    }
}
