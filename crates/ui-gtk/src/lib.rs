//! GTK composition for three-column local filesystem navigation.

mod directory_pane;
mod preview_pane;

use std::{
    cell::{Cell, RefCell},
    process::Command,
    rc::{Rc, Weak},
    sync::mpsc,
    time::{Duration, Instant},
};

use directory_pane::DirectoryPane;
use gtk::{gdk, gio, glib, prelude::*};
use pathpilot_core::{
    AppCommand, AppMode, ClipboardAction, ClipboardItem, CommandPalette, FileEntry, FileKind,
    FilenameFind, InputModeKind, KeyResult, KeySequenceParser, Location, NavigationState,
    OperationClipboard, OperationId, PaneLayout, present_location,
};
use pathpilot_operations::{
    BatchOperationResult, OperationHandle, OperationResult, TransferSpec, copy_items,
    create_directory, create_file, delete_items, move_items, rename, trash_items,
};
use preview_pane::PreviewPane;
use tracing::{debug, info, info_span, warn};

#[derive(Clone)]
enum PlaceTarget {
    FirstItem,
    Location(Location),
}

#[derive(Clone)]
struct PlaceBinding {
    key: char,
    label: String,
    target: PlaceTarget,
}

#[derive(Clone, Copy)]
enum ClipboardText {
    Name,
    DirectoryPath,
    FullPath,
}

impl ClipboardText {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "filename",
            Self::DirectoryPath => "directory path",
            Self::FullPath => "full path",
        }
    }
}

struct Browser {
    navigation: RefCell<NavigationState>,
    parent: DirectoryPane,
    current: DirectoryPane,
    preview: PreviewPane,
    location_label: gtk::Label,
    status_bar: gtk::Box,
    status: gtk::Label,
    git_status: gtk::Label,
    next_operation_id: Cell<u64>,
    find: RefCell<FilenameFind>,
    command_palette: RefCell<CommandPalette>,
    mode: RefCell<AppMode>,
    input_source: RefCell<Option<Location>>,
    input_bar: gtk::Box,
    input_title: gtk::Label,
    input_entry: gtk::Entry,
    input_help: gtk::Label,
    operation_clipboard: RefCell<Option<OperationClipboard>>,
    active_operation: RefCell<Option<OperationHandle>>,
    git_summary: RefCell<Option<String>>,
    git_probe: Cell<u64>,
    pane_layout: Cell<PaneLayout>,
    layout_panes: RefCell<Option<(gtk::Paned, gtk::Paned)>>,
    browse_outer_position: Cell<i32>,
    browse_right_position: Cell<i32>,
    focus_right_position: Cell<i32>,
    settings: Rc<RefCell<pathpilot_config::Settings>>,
    settings_path: Option<std::path::PathBuf>,
}

impl Browser {
    fn new(
        initial: Location,
        settings: Rc<RefCell<pathpilot_config::Settings>>,
        settings_path: Option<std::path::PathBuf>,
    ) -> Rc<Self> {
        let status = gtk::Label::builder()
            .label("NORMAL")
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let git_status = gtk::Label::builder().xalign(1.0).visible(false).build();
        git_status.add_css_class("git-status");
        let status_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .build();
        status_bar.add_css_class("status-line");
        status_bar.add_css_class("status-normal");
        status_bar.append(&status);
        status_bar.append(&git_status);
        let styled_status_bar = status_bar.clone();
        status.connect_label_notify(move |label| {
            for class in [
                "status-normal",
                "status-visual",
                "status-find",
                "status-command",
                "status-input",
            ] {
                styled_status_bar.remove_css_class(class);
            }
            let class = if label.label().starts_with("VISUAL") {
                "status-visual"
            } else if label.label().starts_with("FIND") {
                "status-find"
            } else if label.label().starts_with("COMMAND") {
                "status-command"
            } else if label.label().starts_with("INPUT") {
                "status-input"
            } else {
                "status-normal"
            };
            styled_status_bar.add_css_class(class);
        });
        let input_title = gtk::Label::builder().xalign(1.0).width_chars(16).build();
        input_title.add_css_class("key-hint-key");
        let input_entry = gtk::Entry::builder()
            .width_chars(40)
            .hexpand(true)
            .placeholder_text("Type a name…")
            .activates_default(false)
            .build();
        let input_help = gtk::Label::builder()
            .label("Enter accept · Esc cancel")
            .xalign(0.0)
            .build();
        let input_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .halign(gtk::Align::Center)
            .visible(false)
            .build();
        input_bar.add_css_class("interaction-panel-content");
        input_bar.append(&input_title);
        input_bar.append(&input_entry);
        input_bar.append(&input_help);
        let ui = settings.borrow().ui.clone();
        Rc::new(Self {
            navigation: RefCell::new(NavigationState::new(initial)),
            parent: DirectoryPane::new("Parent"),
            current: DirectoryPane::new("Current"),
            preview: PreviewPane::new(),
            location_label: gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .margin_start(10)
                .margin_end(10)
                .margin_top(6)
                .margin_bottom(6)
                .build(),
            status_bar,
            status,
            git_status,
            next_operation_id: Cell::new(1),
            find: RefCell::new(FilenameFind::default()),
            command_palette: RefCell::new(CommandPalette::default()),
            mode: RefCell::new(AppMode::default()),
            input_source: RefCell::new(None),
            input_bar,
            input_title,
            input_entry,
            input_help,
            operation_clipboard: RefCell::new(None),
            active_operation: RefCell::new(None),
            git_summary: RefCell::new(None),
            git_probe: Cell::new(0),
            pane_layout: Cell::new(match ui.pane_layout.as_str() {
                "focus_preview" => PaneLayout::FocusPreview,
                "preview_only" => PaneLayout::PreviewOnly,
                _ => PaneLayout::Browse,
            }),
            layout_panes: RefCell::new(None),
            browse_outer_position: Cell::new(ui.browse_outer_position),
            browse_right_position: Cell::new(ui.browse_right_position),
            focus_right_position: Cell::new(ui.focus_right_position),
            settings,
            settings_path,
        })
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.current
            .selection
            .connect_selection_changed(move |_, _, _| {
                if let Some(browser) = weak.upgrade() {
                    browser.selection_changed(browser.current.cursor_position());
                }
            });

        connect_activation(&self.current, Rc::downgrade(self));
        connect_activation(&self.parent, Rc::downgrade(self));

        let weak = Rc::downgrade(self);
        self.input_entry.connect_changed(move |entry| {
            let Some(browser) = weak.upgrade() else {
                return;
            };
            let updated = {
                let mut mode = browser.mode.borrow_mut();
                mode.text_input_mut().is_some_and(|input| {
                    input.set_value(entry.text());
                    true
                })
            };
            if updated {
                browser.refresh_input_bar();
            } else if *browser.mode.borrow() == AppMode::Command {
                browser.command_palette.borrow_mut().set_query(entry.text());
                browser.refresh_command_palette();
            }
        });
        let weak = Rc::downgrade(self);
        self.input_entry.connect_activate(move |_| {
            let Some(browser) = weak.upgrade() else {
                return;
            };
            if *browser.mode.borrow() == AppMode::Command {
                browser.submit_command_palette();
            } else if browser.submit_text_input() {
                browser.hide_input_bar();
            } else {
                browser.refresh_input_bar();
            }
        });
    }

    fn initial_load(self: &Rc<Self>) {
        self.reload_columns(None, None);
    }

    fn selection_changed(self: &Rc<Self>, selected: u32) {
        let total = self.current.selection.n_items();
        if total == 0 || selected == gtk::INVALID_LIST_POSITION {
            self.status.set_label("NORMAL  No selection");
            self.preview.show_empty();
            return;
        }
        let visual_count = {
            let mut mode = self.mode.borrow_mut();
            mode.visual_mut().map(|visual| {
                visual.set_cursor(selected, total);
                self.current.selected_entries().len()
            })
        };
        if let Some(count) = visual_count {
            let metadata = self
                .current
                .selected_entry()
                .map_or_else(String::new, |entry| {
                    format!("  {}", compact_metadata(&entry))
                });
            self.status
                .set_label(&format!("{:<8}{count:>3} selected{metadata}", "VISUAL"));
        } else {
            let clipboard = self.clipboard_status();
            let metadata = self.current.selected_entry().map_or_else(
                || "No selection".to_owned(),
                |entry| compact_metadata(&entry),
            );
            self.status
                .set_label(&format!("{:<8}{metadata}{clipboard}", "NORMAL"));
        }
        self.update_preview();
    }

    fn clipboard_status(&self) -> String {
        self.operation_clipboard
            .borrow()
            .as_ref()
            .map(|clipboard| format!(" · {}", clipboard.status_label()))
            .unwrap_or_default()
    }

    fn update_preview(self: &Rc<Self>) {
        let Some(entry) = self.current.selected_entry() else {
            self.preview.show_empty();
            return;
        };
        self.preview.schedule(entry);
    }

    fn dispatch(self: &Rc<Self>, command: AppCommand, window: &gtk::ApplicationWindow) -> bool {
        if command == AppCommand::Quit {
            return true;
        }
        if self.mode.borrow().visual().is_some()
            && !matches!(
                command,
                AppCommand::NavigateUp
                    | AppCommand::NavigateDown
                    | AppCommand::GoFirst
                    | AppCommand::GoLast
                    | AppCommand::Copy
                    | AppCommand::CopyName
                    | AppCommand::CopyDirectoryPath
                    | AppCommand::CopyFullPath
                    | AppCommand::Cut
                    | AppCommand::Trash
                    | AppCommand::PermanentDelete
                    | AppCommand::CycleLayout
                    | AppCommand::ToggleVisual
            )
        {
            self.status
                .set_label("VISUAL  Finish selection before running this command");
            return false;
        }
        match command {
            AppCommand::NavigateUp => self.move_cursor(-1),
            AppCommand::NavigateDown => self.move_cursor(1),
            AppCommand::GoFirst => self.select_position(0),
            AppCommand::GoLast => {
                let count = self.current.selection.n_items();
                if count > 0 {
                    self.select_position(count - 1);
                }
            }
            AppCommand::Enter => {
                if let Some(entry) = self.current.selected_entry() {
                    self.open_entry(entry);
                }
            }
            AppCommand::GoParent => self.go_parent(),
            AppCommand::CreateFile => self.start_create(false),
            AppCommand::CreateDirectory => self.start_create(true),
            AppCommand::Rename => self.start_rename(),
            AppCommand::Trash => self.confirm_trash(window),
            AppCommand::PermanentDelete => self.confirm_permanent_delete(window),
            AppCommand::Copy => self.store_operation_clipboard(ClipboardAction::Copy),
            AppCommand::CopyName => self.copy_selection_text(window, ClipboardText::Name),
            AppCommand::CopyDirectoryPath => {
                self.copy_selection_text(window, ClipboardText::DirectoryPath)
            }
            AppCommand::CopyFullPath => self.copy_selection_text(window, ClipboardText::FullPath),
            AppCommand::Cut => self.store_operation_clipboard(ClipboardAction::Move),
            AppCommand::Paste => self.paste_operation_clipboard(window),
            AppCommand::ToggleVisual => self.toggle_visual(),
            AppCommand::CycleLayout => self.cycle_pane_layout(),
            AppCommand::Quit => unreachable!("quit handled before navigation dispatch"),
        }
        false
    }

    fn toggle_visual(self: &Rc<Self>) {
        if self.mode.borrow().visual().is_some() {
            self.leave_visual();
            return;
        }
        let position = self.current.cursor_position();
        if self.current.selection.n_items() > 0 && self.mode.borrow_mut().begin_visual(position) {
            self.current.begin_visual();
            self.status
                .set_label("VISUAL  1 selected · j/k extend · v/Escape finish");
        }
    }

    fn cycle_pane_layout(&self) {
        let Some((outer, right)) = self.layout_panes.borrow().as_ref().cloned() else {
            return;
        };
        if self.pane_layout.get() == PaneLayout::Browse {
            self.browse_outer_position.set(outer.position());
            self.browse_right_position.set(right.position());
        } else if self.pane_layout.get() == PaneLayout::FocusPreview {
            self.focus_right_position.set(right.position());
        }
        let layout = self.pane_layout.get().next();
        self.pane_layout.set(layout);
        match layout {
            PaneLayout::Browse => {
                self.parent.widget.set_visible(true);
                self.current.widget.set_visible(true);
                outer.set_position(self.browse_outer_position.get());
                right.set_position(self.browse_right_position.get());
            }
            PaneLayout::FocusPreview => {
                self.parent.widget.set_visible(false);
                self.current.widget.set_visible(true);
                right.set_position(self.focus_right_position.get());
            }
            PaneLayout::PreviewOnly => {
                self.parent.widget.set_visible(false);
                self.current.widget.set_visible(false);
            }
        }
        self.status
            .set_label(&format!("NORMAL  Layout: {}", layout.label()));
    }

    fn restore_pane_layout(&self) {
        let layout = self.pane_layout.get();
        let Some((outer, right)) = self.layout_panes.borrow().as_ref().cloned() else {
            return;
        };
        outer.set_position(self.browse_outer_position.get());
        match layout {
            PaneLayout::Browse => {
                self.parent.widget.set_visible(true);
                self.current.widget.set_visible(true);
                right.set_position(self.browse_right_position.get());
            }
            PaneLayout::FocusPreview => {
                self.parent.widget.set_visible(false);
                self.current.widget.set_visible(true);
                right.set_position(self.focus_right_position.get());
            }
            PaneLayout::PreviewOnly => {
                self.parent.widget.set_visible(false);
                self.current.widget.set_visible(false);
            }
        }
    }

    fn persist_window_state(&self, window: &gtk::ApplicationWindow) {
        if let Some((outer, right)) = self.layout_panes.borrow().as_ref() {
            match self.pane_layout.get() {
                PaneLayout::Browse => {
                    self.browse_outer_position.set(outer.position());
                    self.browse_right_position.set(right.position());
                }
                PaneLayout::FocusPreview => self.focus_right_position.set(right.position()),
                PaneLayout::PreviewOnly => {}
            }
        }
        let mut settings = self.settings.borrow_mut();
        if !window.is_maximized() {
            settings.ui.window_width = window.width();
            settings.ui.window_height = window.height();
        }
        settings.ui.window_maximized = window.is_maximized();
        settings.ui.last_location = Some(self.navigation.borrow().current().uri().to_owned());
        settings.ui.pane_layout = match self.pane_layout.get() {
            PaneLayout::Browse => "browse",
            PaneLayout::FocusPreview => "focus_preview",
            PaneLayout::PreviewOnly => "preview_only",
        }
        .to_owned();
        settings.ui.browse_outer_position = self.browse_outer_position.get();
        settings.ui.browse_right_position = self.browse_right_position.get();
        settings.ui.focus_right_position = self.focus_right_position.get();
        if let Some(path) = self.settings_path.as_deref()
            && let Err(error) = pathpilot_config::save_settings(path, &settings)
        {
            warn!(%error, "could not persist window state");
        }
    }

    fn leave_visual(self: &Rc<Self>) -> bool {
        if self.mode.borrow().visual().is_none() {
            return false;
        }
        self.mode.borrow_mut().cancel();
        self.current.end_visual();
        self.selection_changed(self.current.cursor_position());
        true
    }

    fn operation_id(&self) -> OperationId {
        let value = self.next_operation_id.get();
        self.next_operation_id.set(value.wrapping_add(1));
        OperationId::new(value)
    }

    fn operation_entries(&self) -> Vec<FileEntry> {
        if self.mode.borrow().visual().is_some() {
            self.current.selected_entries()
        } else {
            self.current.selected_entry().into_iter().collect()
        }
    }

    fn store_operation_clipboard(self: &Rc<Self>, action: ClipboardAction) {
        let entries = self.operation_entries();
        if entries.is_empty() {
            self.status.set_label("NORMAL  Nothing selected");
            return;
        }
        let verb = match action {
            ClipboardAction::Copy => "Copied",
            ClipboardAction::Move => "Cut",
        };
        self.status
            .set_label(&format!("NORMAL  {verb}: {} item(s)", entries.len()));
        *self.operation_clipboard.borrow_mut() = Some(OperationClipboard {
            action,
            items: entries
                .into_iter()
                .map(|entry| ClipboardItem {
                    source: entry.location,
                    display_name: entry.display_name,
                })
                .collect(),
        });
        self.leave_visual();
    }

    fn copy_selection_text(self: &Rc<Self>, window: &gtk::ApplicationWindow, kind: ClipboardText) {
        let entries = self.operation_entries();
        if entries.is_empty() {
            self.status.set_label("NORMAL  Nothing selected");
            return;
        }
        let values = entries.iter().map(|entry| match kind {
            ClipboardText::Name => entry.display_name.clone(),
            ClipboardText::DirectoryPath => {
                let file = gio::File::for_uri(entry.location.uri());
                file.parent().map_or_else(
                    || present_location(&entry.location, None).full,
                    |parent| present_location(&Location::new(parent.uri()), None).full,
                )
            }
            ClipboardText::FullPath => present_location(&entry.location, None).full,
        });
        gtk::prelude::WidgetExt::display(window)
            .clipboard()
            .set_text(&values.collect::<Vec<_>>().join("\n"));
        self.status.set_label(&format!(
            "NORMAL  Copied {} for {} item(s)",
            kind.label(),
            entries.len()
        ));
        self.leave_visual();
    }

    fn paste_operation_clipboard(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        if self.active_operation.borrow().is_some() {
            self.status
                .set_label("NORMAL  Another file operation is already running");
            return;
        }
        let Some(clipboard) = self.operation_clipboard.borrow().clone() else {
            self.status
                .set_label("NORMAL  Operation clipboard is empty");
            return;
        };
        let parent = gio::File::for_uri(self.navigation.borrow().current().uri());
        let conflicts = clipboard
            .items
            .iter()
            .filter(|item| {
                parent
                    .child(&item.display_name)
                    .query_exists(None::<&gio::Cancellable>)
            })
            .count();
        if conflicts > 0 {
            self.confirm_keep_both(window, clipboard, parent, conflicts);
            return;
        }
        let transfers = clipboard
            .items
            .iter()
            .map(|item| TransferSpec {
                source: item.source.clone(),
                destination: Location::new(parent.child(&item.display_name).uri()),
            })
            .collect();
        self.start_paste(clipboard, transfers);
    }

    #[allow(deprecated)]
    fn confirm_keep_both(
        self: &Rc<Self>,
        window: &gtk::ApplicationWindow,
        clipboard: OperationClipboard,
        parent: gio::File,
        conflicts: usize,
    ) {
        let dialog = gtk::MessageDialog::builder()
            .transient_for(window)
            .modal(true)
            .message_type(gtk::MessageType::Question)
            .buttons(gtk::ButtonsType::None)
            .text(format!("{conflicts} destination conflict(s)"))
            .secondary_text("Keep all items using unique names where needed?")
            .build();
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Keep Both", gtk::ResponseType::Accept);
        dialog.set_default_response(gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(browser) = weak.upgrade()
            {
                let transfers = clipboard
                    .items
                    .iter()
                    .map(|item| {
                        let exact = parent.child(&item.display_name);
                        let destination = if exact.query_exists(None::<&gio::Cancellable>) {
                            unique_destination(&parent, &item.display_name)
                        } else {
                            Location::new(exact.uri())
                        };
                        TransferSpec {
                            source: item.source.clone(),
                            destination,
                        }
                    })
                    .collect();
                browser.start_paste(clipboard.clone(), transfers);
            }
            dialog.close();
        });
        dialog.present();
    }

    fn start_paste(self: &Rc<Self>, clipboard: OperationClipboard, transfers: Vec<TransferSpec>) {
        let action = clipboard.action;
        let weak_finished = Rc::downgrade(self);
        let finished = move |result| {
            if let Some(browser) = weak_finished.upgrade() {
                browser.batch_finished(result, action == ClipboardAction::Move);
            }
        };
        self.status
            .set_label(&format!("NORMAL  Pasting {} item(s)…", transfers.len()));
        let weak_progress = Rc::downgrade(self);
        let progress = move |progress: pathpilot_core::OperationProgress| {
            if let Some(browser) = weak_progress.upgrade() {
                browser.status.set_label(&format!(
                    "NORMAL  Processing {} / {} items",
                    progress.completed_items,
                    progress.total_items.unwrap_or(0)
                ));
            }
        };
        let handle = match action {
            ClipboardAction::Copy => copy_items(self.operation_id(), transfers, progress, finished),
            ClipboardAction::Move => move_items(self.operation_id(), transfers, progress, finished),
        };
        *self.active_operation.borrow_mut() = Some(handle);
    }

    fn cancel_active_operation(&self) -> bool {
        let active = self.active_operation.borrow();
        let Some(handle) = active.as_ref() else {
            return false;
        };
        handle.cancel();
        self.status.set_label("NORMAL  Cancelling operation…");
        true
    }

    fn start_find(&self) {
        if !self.mode.borrow_mut().begin_find() {
            return;
        }
        let position = self.current.cursor_position();
        self.find.borrow_mut().start(position);
        self.status.set_label("FIND  Type a filename");
    }

    fn start_command_palette(&self) {
        if !self.mode.borrow_mut().begin_command() {
            return;
        }
        self.command_palette.borrow_mut().reset();
        self.input_title.set_label("Command");
        self.input_entry
            .set_placeholder_text(Some("Type a command…"));
        self.input_entry.set_text("");
        self.input_bar.set_visible(true);
        self.refresh_command_palette();
        self.input_entry.grab_focus();
        self.status.set_label("COMMAND  Search and run an action");
    }

    fn refresh_command_palette(&self) {
        let palette = self.command_palette.borrow();
        let matches = palette.matches();
        let description = if matches.is_empty() {
            "No matching command".to_owned()
        } else {
            matches
                .iter()
                .enumerate()
                .skip(palette.selected_index().saturating_sub(5))
                .take(6)
                .map(|(index, item)| {
                    let marker = if index == palette.selected_index() {
                        "›"
                    } else {
                        " "
                    };
                    format!("{marker} {}  [{}]", item.title, item.keys)
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        self.input_help.set_label(&description);
    }

    fn move_command_palette(&self, offset: i32) {
        self.command_palette.borrow_mut().move_selection(offset);
        self.refresh_command_palette();
    }

    fn cancel_command_palette(&self) {
        self.mode.borrow_mut().cancel();
        self.hide_input_bar();
        self.input_entry.set_placeholder_text(Some("Type a name…"));
        self.status.set_label("NORMAL  Command palette cancelled");
    }

    fn submit_command_palette(self: &Rc<Self>) {
        let command = self.command_palette.borrow().selected_command();
        self.mode.borrow_mut().cancel();
        self.hide_input_bar();
        self.input_entry.set_placeholder_text(Some("Type a name…"));
        let Some(command) = command else {
            self.status.set_label("NORMAL  No matching command");
            return;
        };
        if let Some(window) = self
            .current
            .widget
            .root()
            .and_downcast::<gtk::ApplicationWindow>()
            && self.dispatch(command, &window)
        {
            window.close();
        }
    }

    fn update_find(&self, character: Option<char>) -> (String, bool) {
        let names = self.current.names();
        let mut find = self.find.borrow_mut();
        let position = match character {
            Some(character) => find.push(character, &names),
            None => find.pop(&names),
        };
        if let Some(position) = position {
            self.select_position(position);
        }
        let query = find.query().to_owned();
        let matched = position.is_some() || query.is_empty();
        self.status.set_label(if matched {
            "FIND  Enter accept · Escape cancel"
        } else {
            "FIND  No matching filename"
        });
        (query, matched)
    }

    fn accept_find(&self) {
        self.find.borrow_mut().accept();
        self.mode.borrow_mut().finish_find();
        self.status.set_label("NORMAL  Find accepted · n/N repeat");
    }

    fn cancel_find(&self) {
        let position = self.find.borrow_mut().cancel();
        self.mode.borrow_mut().cancel();
        self.select_position(position);
        self.status.set_label("NORMAL  Find cancelled");
    }

    fn repeat_find(&self, forward: bool) {
        let names = self.current.names();
        let position = self.find.borrow_mut().repeat(&names, forward);
        if let Some(position) = position {
            self.select_position(position);
            self.status.set_label("NORMAL  Find match · n/N repeat");
        } else {
            self.status
                .set_label("NORMAL  No previous find query or match");
        }
    }

    fn start_create(&self, directory: bool) {
        let kind = if directory {
            InputModeKind::CreateDirectory
        } else {
            InputModeKind::CreateFile
        };
        if self.mode.borrow_mut().begin_text_input(kind, "") {
            self.input_source.borrow_mut().take();
            self.input_entry.set_text("");
            self.show_input_bar();
            self.status.set_label(&format!(
                "INPUT  {} · Enter accept · Escape cancel",
                kind.label()
            ));
        }
    }

    fn start_rename(&self) {
        let Some(entry) = self.current.selected_entry() else {
            self.status.set_label("NORMAL  Nothing selected");
            return;
        };
        if self
            .mode
            .borrow_mut()
            .begin_text_input(InputModeKind::Rename, entry.display_name)
        {
            *self.input_source.borrow_mut() = Some(entry.location);
            let initial = self
                .mode
                .borrow()
                .text_input()
                .map_or_else(String::new, |input| input.value().to_owned());
            self.input_entry.set_text(&initial);
            self.input_entry.select_region(0, -1);
            self.show_input_bar();
            self.status
                .set_label("INPUT  Rename · Enter accept · Escape cancel");
        }
    }

    fn show_input_bar(&self) {
        self.refresh_input_bar();
        self.input_bar.set_visible(true);
        self.input_entry.grab_focus();
    }

    fn refresh_input_bar(&self) {
        let mode = self.mode.borrow();
        let Some(input) = mode.text_input() else {
            return;
        };
        self.input_title.set_label(input.kind().label());
        self.input_help
            .set_label(input.error().unwrap_or("Enter accept · Esc cancel"));
    }

    fn hide_input_bar(&self) {
        self.input_bar.set_visible(false);
        self.current.list.grab_focus();
    }

    fn cancel_text_input(&self) {
        self.mode.borrow_mut().cancel();
        self.input_source.borrow_mut().take();
        self.hide_input_bar();
        self.status.set_label("NORMAL  Input cancelled");
    }

    fn submit_text_input(self: &Rc<Self>) -> bool {
        let Some((kind, value)) = self.mode.borrow_mut().submit_text_input() else {
            self.status.set_label("INPUT  Invalid name");
            return false;
        };
        let source = self.input_source.borrow_mut().take();
        let callback_browser = Rc::downgrade(self);
        let callback = move |result| {
            if let Some(browser) = callback_browser.upgrade() {
                browser.operation_finished(result);
            }
        };
        match kind {
            InputModeKind::CreateFile => create_file(
                self.operation_id(),
                self.navigation.borrow().current(),
                &value,
                callback,
            ),
            InputModeKind::CreateDirectory => create_directory(
                self.operation_id(),
                self.navigation.borrow().current(),
                &value,
                callback,
            ),
            InputModeKind::Rename => {
                let Some(source) = source else {
                    self.status
                        .set_label("NORMAL  Rename source is unavailable");
                    return true;
                };
                rename(self.operation_id(), &source, &value, callback)
            }
        };
        self.status.set_label("NORMAL  Operation started…");
        true
    }

    #[allow(deprecated)]
    fn confirm_trash(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        if self.active_operation.borrow().is_some() {
            self.status
                .set_label("NORMAL  Another file operation is already running");
            return;
        }
        let entries = self.operation_entries();
        if entries.is_empty() {
            return;
        }
        let targets: Vec<_> = entries.iter().map(|entry| entry.location.clone()).collect();
        let dialog = gtk::MessageDialog::builder()
            .transient_for(window)
            .modal(true)
            .message_type(gtk::MessageType::Warning)
            .buttons(gtk::ButtonsType::OkCancel)
            .text(format!("Move {} item(s) to Trash?", entries.len()))
            .secondary_text(selection_summary(&entries))
            .build();
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Ok
                && let Some(browser) = weak.upgrade()
            {
                browser.leave_visual();
                let weak_progress = Rc::downgrade(&browser);
                let callback_browser = Rc::downgrade(&browser);
                let handle = trash_items(
                    browser.operation_id(),
                    targets.clone(),
                    move |progress| {
                        if let Some(browser) = weak_progress.upgrade() {
                            browser.status.set_label(&format!(
                                "NORMAL  Trashing {} / {} items",
                                progress.completed_items,
                                progress.total_items.unwrap_or(0)
                            ));
                        }
                    },
                    move |result| {
                        if let Some(browser) = callback_browser.upgrade() {
                            browser.batch_finished(result, false);
                        }
                    },
                );
                *browser.active_operation.borrow_mut() = Some(handle);
            }
            dialog.close();
        });
        dialog.present();
    }

    #[allow(deprecated)]
    fn confirm_permanent_delete(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        if self.active_operation.borrow().is_some() {
            self.status
                .set_label("NORMAL  Another file operation is already running");
            return;
        }
        let entries = self.operation_entries();
        if entries.is_empty() {
            return;
        }
        let targets: Vec<_> = entries.iter().map(|entry| entry.location.clone()).collect();
        let dialog = gtk::MessageDialog::builder()
            .transient_for(window)
            .modal(true)
            .message_type(gtk::MessageType::Error)
            .buttons(gtk::ButtonsType::None)
            .text(format!("Delete {} item(s) permanently?", entries.len()))
            .secondary_text(format!(
                "{}\n\nThis cannot be undone. Directories and all their contents will be removed.",
                selection_summary(&entries)
            ))
            .build();
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Delete Permanently", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(browser) = weak.upgrade()
            {
                browser.leave_visual();
                let weak_progress = Rc::downgrade(&browser);
                let weak_finished = Rc::downgrade(&browser);
                browser.status.set_label("NORMAL  Deleting permanently…");
                let handle = delete_items(
                    browser.operation_id(),
                    targets.clone(),
                    move |progress| {
                        if let Some(browser) = weak_progress.upgrade() {
                            browser.status.set_label(&format!(
                                "NORMAL  Deleting {} / {} items",
                                progress.completed_items,
                                progress.total_items.unwrap_or(0)
                            ));
                        }
                    },
                    move |result| {
                        if let Some(browser) = weak_finished.upgrade() {
                            browser.batch_finished(result, false);
                        }
                    },
                );
                *browser.active_operation.borrow_mut() = Some(handle);
            }
            dialog.close();
        });
        dialog.present();
    }

    fn operation_finished(self: &Rc<Self>, result: OperationResult) {
        let is_active = self
            .active_operation
            .borrow()
            .as_ref()
            .is_some_and(|handle| handle.id() == result.id);
        if is_active {
            self.active_operation.borrow_mut().take();
        }
        match result.result {
            Ok(()) => {
                self.status.set_label("NORMAL  Operation completed");
                self.reload_columns(result.resulting_location, None);
            }
            Err(error) => self
                .status
                .set_label(&format!("NORMAL  Operation failed: {}", error.message)),
        }
    }

    fn batch_finished(self: &Rc<Self>, result: BatchOperationResult, clear_move_clipboard: bool) {
        let is_active = self
            .active_operation
            .borrow()
            .as_ref()
            .is_some_and(|handle| handle.id() == result.id);
        if is_active {
            self.active_operation.borrow_mut().take();
        }
        if result.succeeded() && clear_move_clipboard {
            self.operation_clipboard.borrow_mut().take();
        }
        let preferred = result.resulting_locations.first().cloned();
        if result.cancelled {
            self.status.set_label("NORMAL  Batch operation cancelled");
        } else if let Some(failure) = result.failures.first() {
            self.status.set_label(&format!(
                "NORMAL  Completed with {} error(s); {}: {}",
                result.failures.len(),
                failure.location.uri(),
                failure.error.message
            ));
        } else {
            self.status.set_label("NORMAL  Batch operation completed");
        }
        self.reload_columns(preferred, None);
    }

    fn move_cursor(&self, offset: i32) {
        let count = self.current.selection.n_items();
        if count == 0 {
            return;
        }
        let current = self.current.cursor_position().min(count - 1);
        let target = if offset < 0 {
            current.saturating_sub(offset.unsigned_abs())
        } else {
            current.saturating_add(offset as u32).min(count - 1)
        };
        self.select_position(target);
    }

    fn select_position(&self, position: u32) {
        self.current.select_position(position);
    }

    fn open_entry(self: &Rc<Self>, entry: FileEntry) {
        if entry.kind == FileKind::Directory {
            self.navigate_to(entry.location, None);
            return;
        }

        let uri = entry.location.uri().to_owned();
        let callback_uri = uri.clone();
        gio::AppInfo::launch_default_for_uri_async(
            &uri,
            None::<&gio::AppLaunchContext>,
            None::<&gio::Cancellable>,
            move |result| {
                if let Err(error) = result {
                    warn!(%error, uri = callback_uri, "could not open file with default application");
                }
            },
        );
    }

    fn open_with_apps(&self, entry: &FileEntry) -> Vec<gio::AppInfo> {
        let Some(content_type) = entry.content_type.as_deref() else {
            return Vec::new();
        };
        let history = self.settings.borrow();
        let Some(ids) = history.open_with.get(content_type) else {
            return Vec::new();
        };
        let installed = gio::AppInfo::all();
        ids.iter()
            .filter_map(|id| {
                installed
                    .iter()
                    .find(|app| app.id().as_deref() == Some(id.as_str()))
                    .cloned()
            })
            .collect()
    }

    fn launch_with(self: &Rc<Self>, entry: FileEntry, app: gio::AppInfo) {
        let uri = entry.location.uri().to_owned();
        let name = entry.display_name.clone();
        let app_name = app.display_name().to_string();
        let weak = Rc::downgrade(self);
        app.launch_uris_async(
            &[uri.as_str()],
            None::<&gio::AppLaunchContext>,
            None::<&gio::Cancellable>,
            move |result| {
                if let Some(browser) = weak.upgrade() {
                    match result {
                        Ok(()) => browser
                            .status
                            .set_label(&format!("NORMAL  Opened {name} with {app_name}")),
                        Err(error) => {
                            browser.status.set_label(&format!(
                                "NORMAL  Could not open {name} with {app_name}: {error}"
                            ));
                            warn!(%error, file = name, application = app_name, "open-with launch failed");
                        }
                    }
                }
            },
        );
    }

    fn remember_open_with(&self, content_type: &str, app: &gio::AppInfo) {
        let Some(id) = app.id() else {
            return;
        };
        let mut settings = self.settings.borrow_mut();
        let history = settings
            .open_with
            .entry(content_type.to_owned())
            .or_default();
        if !history.iter().any(|existing| existing == id.as_str()) {
            history.push(id.to_string());
        }
        if let Some(path) = self.settings_path.as_deref()
            && let Err(error) = pathpilot_config::save_settings(path, &settings)
        {
            warn!(%error, "could not persist open-with history");
        }
    }

    #[allow(deprecated)]
    fn show_open_with_dialog(self: &Rc<Self>, window: &gtk::ApplicationWindow, entry: FileEntry) {
        if entry.kind == FileKind::Directory {
            self.status
                .set_label("NORMAL  Open With is unavailable for directories");
            return;
        }
        let Some(content_type) = entry.content_type.clone() else {
            self.status
                .set_label("NORMAL  The selected file has no known content type");
            return;
        };
        let file = gio::File::for_uri(entry.location.uri());
        let dialog = gtk::AppChooserDialog::new(
            Some(window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &file,
        );
        dialog.set_heading(&format!("Open {} with", entry.display_name));
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if matches!(response, gtk::ResponseType::Ok | gtk::ResponseType::Accept)
                && let Some(app) = dialog.app_info()
                && let Some(browser) = weak.upgrade()
            {
                browser.remember_open_with(&content_type, &app);
                browser.launch_with(entry.clone(), app);
            }
            dialog.close();
        });
        dialog.present();
    }

    fn go_parent(self: &Rc<Self>) {
        let current = self.navigation.borrow().current().clone();
        let file = gio::File::for_uri(current.uri());
        let Some(parent) = file.parent() else {
            self.status.set_label("NORMAL  Already at filesystem root");
            return;
        };
        self.navigate_to(Location::new(parent.uri()), Some(current));
    }

    fn navigate_to(self: &Rc<Self>, location: Location, preferred: Option<Location>) {
        self.find.borrow_mut().reset();
        *self.mode.borrow_mut() = AppMode::Normal;
        self.current.end_visual();
        self.input_source.borrow_mut().take();
        self.hide_input_bar();
        let selected = self.current.cursor_position();
        if self.current.selection.n_items() > 0 {
            self.navigation.borrow_mut().remember_cursor(selected);
        }
        let restored = self.navigation.borrow_mut().navigate_to(location);
        self.reload_columns(preferred, restored);
    }

    fn reload_columns(
        self: &Rc<Self>,
        preferred: Option<Location>,
        restored_position: Option<u32>,
    ) {
        let location = self.navigation.borrow().current().clone();
        self.start_git_probe(&location);
        let presentation = present_location(
            &location,
            std::env::var_os("HOME")
                .as_deref()
                .map(std::path::Path::new),
        );
        self.location_label.set_label(&presentation.compact);
        self.location_label.set_tooltip_text(Some(location.uri()));
        self.status.set_label("NORMAL  Loading…");

        let weak = Rc::downgrade(self);
        self.current.load(&location, move |success| {
            let Some(browser) = weak.upgrade() else {
                return;
            };
            if !success {
                return;
            }
            let selected_preferred = preferred
                .as_ref()
                .is_some_and(|location| browser.current.select_location(location));
            if !selected_preferred {
                browser
                    .current
                    .select_position(restored_position.unwrap_or(0));
            }
            browser.selection_changed(browser.current.cursor_position());
        });

        let file = gio::File::for_uri(location.uri());
        if let Some(parent_file) = file.parent() {
            let parent_location = Location::new(parent_file.uri());
            let current_location = location.clone();
            let parent_pane = self.parent.clone();
            self.parent.load(&parent_location, move |success| {
                if success {
                    parent_pane.select_location(&current_location);
                }
            });
        } else {
            self.parent.show_message("Parent", "Filesystem root");
        }
        self.preview.show_empty();
        info!(location = location.uri(), "navigation started");
    }

    fn start_git_probe(self: &Rc<Self>, location: &Location) {
        self.git_summary.borrow_mut().take();
        self.git_status.set_visible(false);
        let generation = self.git_probe.get().wrapping_add(1);
        self.git_probe.set(generation);
        let Some(path) = gio::File::for_uri(location.uri()).path() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let output = Command::new("git")
                .args(["-C"])
                .arg(path)
                .args(["status", "--porcelain=v1", "--branch"])
                .output();
            let summary = output
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| parse_git_status(&String::from_utf8_lossy(&output.stdout)));
            let _ = sender.send(summary);
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(25), move || {
            match receiver.try_recv() {
                Ok(summary) => {
                    if let Some(browser) = weak.upgrade()
                        && browser.git_probe.get() == generation
                    {
                        *browser.git_summary.borrow_mut() = summary;
                        if let Some(summary) = browser.git_summary.borrow().as_deref() {
                            browser.git_status.set_label(summary);
                            browser.git_status.set_visible(true);
                        } else {
                            browser.git_status.set_visible(false);
                        }
                        if matches!(*browser.mode.borrow(), AppMode::Normal | AppMode::Visual(_)) {
                            browser.selection_changed(browser.current.cursor_position());
                        }
                    }
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    fn cancel(&self) {
        self.cancel_active_operation();
        self.parent.cancel();
        self.current.cancel();
        self.preview.cancel();
    }
}

pub fn build_window(app: &gtk::Application) -> gtk::ApplicationWindow {
    let startup_span = info_span!("build_window");
    let _guard = startup_span.enter();
    let settings_path = pathpilot_config::settings_path();
    let (settings, warning) = pathpilot_config::load_settings(settings_path.as_deref());
    if let Some(error) = warning.as_deref() {
        warn!(%error, "invalid settings; using defaults");
    }
    // Do not replace a malformed user-edited file with defaults on shutdown.
    // Persistence resumes automatically after the user fixes the file and restarts.
    let settings_path = warning.is_none().then_some(settings_path).flatten();
    let fallback = || Location::new(gio::File::for_path(".").uri());
    let initial = settings
        .ui
        .last_location
        .as_ref()
        .map_or_else(fallback, |uri| {
            let file = gio::File::for_uri(uri);
            if file.query_exists(None::<&gio::Cancellable>) {
                Location::new(uri.clone())
            } else {
                fallback()
            }
        });
    let window_width = settings.ui.window_width;
    let window_height = settings.ui.window_height;
    let window_maximized = settings.ui.window_maximized;
    let settings = Rc::new(RefCell::new(settings));
    let browser = Browser::new(initial, settings, settings_path);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("PathPilot — Preview Prototype")
        .default_width(window_width)
        .default_height(window_height)
        .build();
    if window_maximized {
        window.maximize();
    }
    let columns = three_column_layout(&browser);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&browser.location_label);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&columns);
    let interaction_panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .visible(true)
        .build();
    interaction_panel.add_css_class("interaction-panel");
    let key_hints = gtk::Grid::builder()
        .halign(gtk::Align::Center)
        .column_spacing(12)
        .row_spacing(6)
        .visible(false)
        .build();
    key_hints.add_css_class("interaction-panel-content");
    interaction_panel.append(&key_hints);
    interaction_panel.append(&browser.input_bar);
    root.append(&interaction_panel);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&browser.status_bar);
    install_hint_css();
    window.set_child(Some(&root));

    browser.connect();
    install_keyboard_controller(&window, Rc::downgrade(&browser), &key_hints);
    let close_browser = browser.clone();
    let close_window = window.clone();
    window.connect_close_request(move |_| {
        close_browser.persist_window_state(&close_window);
        close_browser.cancel();
        glib::Propagation::Proceed
    });
    browser.initial_load();
    browser.restore_pane_layout();

    info!("window constructed");
    window
}

fn three_column_layout(browser: &Browser) -> gtk::Paned {
    let outer = gtk::Paned::new(gtk::Orientation::Horizontal);
    let right = gtk::Paned::new(gtk::Orientation::Horizontal);
    outer.set_wide_handle(true);
    right.set_wide_handle(true);
    outer.set_start_child(Some(&browser.parent.widget));
    outer.set_end_child(Some(&right));
    outer.set_position(360);
    outer.set_resize_start_child(true);
    outer.set_resize_end_child(true);
    right.set_start_child(Some(&browser.current.widget));
    right.set_end_child(Some(&browser.preview.widget));
    right.set_position(560);
    right.set_resize_start_child(true);
    right.set_resize_end_child(true);
    *browser.layout_panes.borrow_mut() = Some((outer.clone(), right));
    outer
}

fn connect_activation(pane: &DirectoryPane, browser: Weak<Browser>) {
    let list = pane.list.clone();
    let pane = pane.clone();
    list.connect_activate(move |_, position| {
        pane.select_position(position);
        let entry = pane.selected_entry();
        if let (Some(browser), Some(entry)) = (browser.upgrade(), entry) {
            browser.open_entry(entry);
        }
    });
}

fn default_places(bookmarks: &[pathpilot_config::Bookmark]) -> Vec<PlaceBinding> {
    let mut places = vec![PlaceBinding {
        key: 'g',
        label: "First item".to_owned(),
        target: PlaceTarget::FirstItem,
    }];
    if let Some(home) = std::env::var_os("HOME") {
        places.push(PlaceBinding {
            key: 'h',
            label: "Home".to_owned(),
            target: PlaceTarget::Location(Location::new(gio::File::for_path(home).uri())),
        });
    }
    if let Some(downloads) = glib::user_special_dir(glib::UserDirectory::Downloads)
        && downloads.is_dir()
    {
        places.push(PlaceBinding {
            key: 'd',
            label: "Downloads".to_owned(),
            target: PlaceTarget::Location(Location::new(gio::File::for_path(downloads).uri())),
        });
    }
    places.push(PlaceBinding {
        key: 'r',
        label: "Filesystem root".to_owned(),
        target: PlaceTarget::Location(Location::new(gio::File::for_path("/").uri())),
    });
    places.extend(bookmarks.iter().filter_map(|bookmark| {
        Some(PlaceBinding {
            key: bookmark.key.chars().next()?,
            label: bookmark.label.clone(),
            target: PlaceTarget::Location(Location::new(bookmark.uri.clone())),
        })
    }));
    places
}

fn install_keyboard_controller(
    window: &gtk::ApplicationWindow,
    browser: Weak<Browser>,
    key_hints: &gtk::Grid,
) {
    let path = pathpilot_config::default_path();
    let (keymap, warning) = pathpilot_config::load_or_default(path.as_deref());
    if let Some(error) = warning {
        warn!(%error, "invalid user keymap; using built-in defaults");
    }
    if let Some(browser) = browser.upgrade() {
        browser
            .command_palette
            .borrow_mut()
            .set_key_labels(keymap.key_labels());
    }
    let places = Rc::new(browser.upgrade().map_or_else(
        || default_places(&[]),
        |browser| default_places(&browser.settings.borrow().bookmarks),
    ));
    let mut reference = keymap.command_reference();
    reference.retain(|(keys, _)| !keys.starts_with('g'));
    reference.extend([
        ("f".to_owned(), "Find by name"),
        ("o …".to_owned(), "Open with application"),
        (":".to_owned(), "Command palette"),
        ("F1".to_owned(), "Hide hints"),
    ]);
    reference.extend(places.iter().map(|place| {
        (
            format!("g {}", place.key),
            Box::leak(place.label.clone().into_boxed_str()) as &'static str,
        )
    }));
    let command_reference = Rc::new(reference);
    let (settings, settings_path) = browser.upgrade().map_or_else(
        || {
            (
                Rc::new(RefCell::new(pathpilot_config::Settings::default())),
                None,
            )
        },
        |browser| (browser.settings.clone(), browser.settings_path.clone()),
    );
    let hints_initially_enabled = settings.borrow().ui.hints_enabled;
    let parser = Rc::new(RefCell::new(KeySequenceParser::with_bindings(
        Duration::MAX,
        keymap.bindings().to_vec(),
    )));
    let hints_enabled = Rc::new(Cell::new(hints_initially_enabled));
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak_window = window.downgrade();
    if hints_initially_enabled {
        show_command_reference(key_hints, &command_reference);
    }
    let key_hints = key_hints.clone();
    let command_reference = command_reference.clone();
    let settings = settings.clone();
    let settings_path = settings_path.clone();
    let places = places.clone();
    let place_pending = Rc::new(Cell::new(false));
    let place_pending_for_keys = place_pending.clone();
    let open_with_pending = Rc::new(RefCell::new(None::<(FileEntry, Vec<gio::AppInfo>)>));
    let open_with_pending_for_keys = open_with_pending.clone();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        if let Some(browser) = browser.upgrade()
            && *browser.mode.borrow() == AppMode::Command
        {
            match key {
                gdk::Key::Escape => browser.cancel_command_palette(),
                gdk::Key::Up => browser.move_command_palette(-1),
                gdk::Key::Down => browser.move_command_palette(1),
                gdk::Key::Return | gdk::Key::KP_Enter => browser.submit_command_palette(),
                _ => return glib::Propagation::Proceed,
            }
            restore_hint_panel(&key_hints, hints_enabled.get(), &command_reference);
            return glib::Propagation::Stop;
        }

        if let Some(browser) = browser.upgrade()
            && browser.mode.borrow().text_input().is_some()
        {
            if key == gdk::Key::Escape {
                browser.cancel_text_input();
                restore_hint_panel(&key_hints, hints_enabled.get(), &command_reference);
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }

        if let Some(browser) = browser.upgrade()
            && *browser.mode.borrow() == AppMode::Find
        {
            match key {
                gdk::Key::Escape => {
                    browser.cancel_find();
                    restore_hint_panel(&key_hints, hints_enabled.get(), &command_reference);
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    browser.accept_find();
                    restore_hint_panel(&key_hints, hints_enabled.get(), &command_reference);
                }
                gdk::Key::BackSpace => {
                    let (query, matched) = browser.update_find(None);
                    show_find_query(&key_hints, &query, matched);
                }
                _ if !modifiers.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK,
                ) =>
                {
                    if let Some(character) = key.to_unicode()
                        && !character.is_control()
                    {
                        let (query, matched) = browser.update_find(Some(character));
                        show_find_query(&key_hints, &query, matched);
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            return glib::Propagation::Stop;
        }

        if key == gdk::Key::Escape {
            place_pending_for_keys.set(false);
            open_with_pending_for_keys.borrow_mut().take();
            if browser
                .upgrade()
                .is_some_and(|browser| browser.leave_visual())
            {
                restore_hint_panel(&key_hints, hints_enabled.get(), &command_reference);
                return glib::Propagation::Stop;
            }
            if browser
                .upgrade()
                .is_some_and(|browser| browser.cancel_active_operation())
            {
                return glib::Propagation::Stop;
            }
            parser.borrow_mut().reset();
            if hints_enabled.get() {
                show_command_reference(&key_hints, &command_reference);
            } else {
                key_hints.set_visible(false);
            }
            return glib::Propagation::Stop;
        }
        let opens_command_palette = key.to_unicode() == Some(':')
            || (matches!(key, gdk::Key::p | gdk::Key::P)
                && modifiers
                    .contains(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK));
        if opens_command_palette {
            parser.borrow_mut().reset();
            key_hints.set_visible(false);
            if let Some(browser) = browser.upgrade() {
                browser.start_command_palette();
            }
            return glib::Propagation::Stop;
        }
        let conventional_command = if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            match key {
                gdk::Key::c => Some(AppCommand::Copy),
                gdk::Key::x => Some(AppCommand::Cut),
                gdk::Key::v => Some(AppCommand::Paste),
                gdk::Key::q => Some(AppCommand::Quit),
                gdk::Key::n | gdk::Key::N if modifiers.contains(gdk::ModifierType::SHIFT_MASK) => {
                    Some(AppCommand::CreateDirectory)
                }
                gdk::Key::n => Some(AppCommand::CreateFile),
                _ => None,
            }
        } else {
            None
        };
        if modifiers.intersects(
            gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK,
        ) && conventional_command.is_none()
        {
            return glib::Propagation::Proceed;
        }

        if key == gdk::Key::F1 {
            parser.borrow_mut().reset();
            let enabled = !hints_enabled.get();
            hints_enabled.set(enabled);
            settings.borrow_mut().ui.hints_enabled = enabled;
            if let Some(path) = settings_path.as_deref()
                && let Err(error) = pathpilot_config::save_settings(path, &settings.borrow())
            {
                warn!(%error, "could not persist settings");
            }
            if enabled {
                show_command_reference(&key_hints, &command_reference);
            }
            key_hints.set_visible(enabled);
            return glib::Propagation::Stop;
        }

        if !parser.borrow().is_pending()
            && let Some(character) = key.to_unicode()
        {
            if let Some((entry, apps)) = open_with_pending_for_keys.borrow_mut().take() {
                if let Some(browser) = browser.upgrade()
                    && let Some(window) = weak_window.upgrade()
                {
                    if character == 'e' {
                        browser.show_open_with_dialog(&window, entry);
                    } else if let Some(index) = character.to_digit(10)
                        && index > 0
                        && let Some(app) = apps.get(index as usize - 1)
                    {
                        browser.launch_with(entry, app.clone());
                    }
                }
                restore_hint_panel(&key_hints, hints_enabled.get(), &command_reference);
                return glib::Propagation::Stop;
            }
            if place_pending_for_keys.replace(false) {
                if let Some(place) = places.iter().find(|place| place.key == character)
                    && let Some(browser) = browser.upgrade()
                {
                    match &place.target {
                        PlaceTarget::FirstItem => browser.select_position(0),
                        PlaceTarget::Location(location) => {
                            browser.navigate_to(location.clone(), None)
                        }
                    }
                    restore_hint_panel(&key_hints, hints_enabled.get(), &command_reference);
                } else {
                    restore_hint_panel(&key_hints, hints_enabled.get(), &command_reference);
                }
                return glib::Propagation::Stop;
            }
            if character == 'g' {
                parser.borrow_mut().reset();
                place_pending_for_keys.set(true);
                populate_hint_grid(
                    &key_hints,
                    places
                        .iter()
                        .map(|place| (place.key.to_string(), place.label.as_str())),
                );
                key_hints.set_visible(true);
                return glib::Propagation::Stop;
            }
            if character == 'o' {
                parser.borrow_mut().reset();
                if let Some(browser) = browser.upgrade() {
                    let Some(entry) = browser.current.selected_entry() else {
                        browser.status.set_label("NORMAL  Nothing selected");
                        return glib::Propagation::Stop;
                    };
                    if entry.kind == FileKind::Directory {
                        browser
                            .status
                            .set_label("NORMAL  Open With is unavailable for directories");
                        return glib::Propagation::Stop;
                    }
                    let apps = browser.open_with_apps(&entry);
                    let mut hints: Vec<_> = apps
                        .iter()
                        .take(9)
                        .enumerate()
                        .map(|(index, app)| {
                            ((index + 1).to_string(), app.display_name().to_string())
                        })
                        .collect();
                    hints.push(("e".to_owned(), "Choose another application…".to_owned()));
                    populate_hint_grid(
                        &key_hints,
                        hints
                            .iter()
                            .map(|(key, label)| (key.clone(), label.as_str())),
                    );
                    key_hints.set_visible(true);
                    *open_with_pending_for_keys.borrow_mut() = Some((entry, apps));
                }
                return glib::Propagation::Stop;
            }
            match character {
                'f' => {
                    if let Some(browser) = browser.upgrade() {
                        browser.start_find();
                        show_find_query(&key_hints, "", true);
                    }
                    return glib::Propagation::Stop;
                }
                'n' | 'N' => {
                    if let Some(browser) = browser.upgrade() {
                        browser.repeat_find(character == 'n');
                    }
                    return glib::Propagation::Stop;
                }
                '/' => {
                    if let Some(browser) = browser.upgrade() {
                        browser
                            .status
                            .set_label("NORMAL  Filtering with / is reserved for a future release");
                    }
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }

        let key_result = match conventional_command {
            Some(command) => KeyResult::Command(command),
            None => match key {
                gdk::Key::F2 => KeyResult::Command(AppCommand::Rename),
                gdk::Key::Delete if modifiers.contains(gdk::ModifierType::SHIFT_MASK) => {
                    KeyResult::Command(AppCommand::PermanentDelete)
                }
                gdk::Key::Delete => KeyResult::Command(AppCommand::Trash),
                _ => {
                    let Some(character) = key.to_unicode() else {
                        return glib::Propagation::Proceed;
                    };
                    parser.borrow_mut().feed(character, Instant::now())
                }
            },
        };
        match key_result {
            KeyResult::Command(command) => {
                if hints_enabled.get() {
                    show_command_reference(&key_hints, &command_reference);
                } else {
                    key_hints.set_visible(false);
                }
                if let Some(browser) = browser.upgrade() {
                    debug!(?command, "dispatching keyboard command");
                    if let Some(window) = weak_window.upgrade() {
                        if browser.dispatch(command, &window) {
                            window.close();
                        } else if browser.mode.borrow().text_input().is_some() {
                            key_hints.set_visible(false);
                        }
                    }
                }
                glib::Propagation::Stop
            }
            KeyResult::Pending(pending) => {
                populate_hint_grid(
                    &key_hints,
                    pending
                        .hints
                        .iter()
                        .map(|hint| (hint.key.to_string(), hint.label)),
                );
                key_hints.set_visible(true);
                glib::Propagation::Stop
            }
            KeyResult::Ignored => glib::Propagation::Proceed,
        }
    });
    window.add_controller(controller);
}

fn restore_hint_panel(grid: &gtk::Grid, hints_enabled: bool, reference: &[(String, &'static str)]) {
    if hints_enabled {
        show_command_reference(grid, reference);
    } else {
        grid.set_visible(false);
    }
}

fn show_find_query(grid: &gtk::Grid, query: &str, matched: bool) {
    let value = if query.is_empty() {
        "Type to find…"
    } else {
        query
    };
    let state = if matched {
        "Enter accept · Esc cancel"
    } else {
        "No match"
    };
    populate_hint_grid(grid, [("Find".to_owned(), value), (String::new(), state)]);
    grid.set_visible(true);
}

fn show_command_reference(grid: &gtk::Grid, reference: &[(String, &'static str)]) {
    populate_hint_grid(
        grid,
        reference.iter().map(|(keys, label)| (keys.clone(), *label)),
    );
    grid.set_visible(true);
}

fn populate_hint_grid<'a>(grid: &gtk::Grid, hints: impl IntoIterator<Item = (String, &'a str)>) {
    while let Some(child) = grid.first_child() {
        grid.remove(&child);
    }
    for (index, (keys, label)) in hints.into_iter().enumerate() {
        let group = (index % 3) as i32;
        let row = (index / 3) as i32;
        let key = gtk::Label::builder()
            .label(keys)
            .xalign(1.0)
            .width_chars(10)
            .build();
        key.add_css_class("key-hint-key");
        let description = gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .width_chars(16)
            .build();
        grid.attach(&key, group * 2, row, 1, 1);
        grid.attach(&description, group * 2 + 1, row, 1, 1);
    }
}

fn unique_destination(parent: &gio::File, display_name: &str) -> Location {
    let (stem, extension) = display_name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .unwrap_or((display_name, ""));
    for number in 2_u64.. {
        let name = if extension.is_empty() {
            format!("{stem} ({number})")
        } else {
            format!("{stem} ({number}).{extension}")
        };
        let candidate = parent.child(name);
        if !candidate.query_exists(None::<&gio::Cancellable>) {
            return Location::new(candidate.uri());
        }
    }
    unreachable!("the unique-name counter is unbounded")
}

fn selection_summary(entries: &[FileEntry]) -> String {
    const MAX_NAMES: usize = 3;
    let mut names = entries
        .iter()
        .take(MAX_NAMES)
        .map(|entry| entry.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if entries.len() > MAX_NAMES {
        names.push_str(&format!(" and {} more", entries.len() - MAX_NAMES));
    }
    names
}

fn compact_metadata(entry: &FileEntry) -> String {
    let kind = match entry.kind {
        FileKind::Directory => "Folder",
        FileKind::Regular => entry.content_type.as_deref().unwrap_or("File"),
        FileKind::Symlink => "Symlink",
        FileKind::Special => "Special",
        FileKind::Unknown => "Unknown",
    };
    let permissions = entry
        .unix_mode
        .map_or_else(|| "---------".to_owned(), format_permissions);
    let size = if entry.kind == FileKind::Directory {
        "—".to_owned()
    } else {
        entry.size.map_or_else(|| "—".to_owned(), compact_size)
    };
    let modified = if let Some(modified) = entry.modified
        && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
        && let Ok(date) = glib::DateTime::from_unix_local(duration.as_secs() as i64)
        && let Ok(value) = date.format("%Y-%m-%d %H:%M")
    {
        value.to_string()
    } else {
        "—".to_owned()
    };
    format!("{permissions:<9}  {modified:<16}  {size:>9}  {kind}")
}

fn parse_git_status(output: &str) -> Option<String> {
    let mut lines = output.lines();
    let branch = lines
        .next()?
        .strip_prefix("## ")?
        .split("...")
        .next()
        .unwrap_or("HEAD");
    let dirty = lines.next().is_some();
    Some(format!(
        " {branch}  {}",
        if dirty { " modified" } else { " clean" }
    ))
}

fn compact_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn format_permissions(mode: u32) -> String {
    let mut value = String::with_capacity(9);
    for (mask, character) in [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ] {
        value.push(if mode & mask == 0 { '-' } else { character });
    }
    value
}

fn install_hint_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        ".interaction-panel-content { background-color: @window_bg_color; border-top: 1px solid alpha(@window_fg_color, 0.16); padding: 10px 16px; } .key-hint-key { font-family: monospace; font-weight: bold; color: @accent_color; } .status-line { padding: 7px 10px; font-family: 'Symbols Nerd Font Mono', 'JetBrainsMono Nerd Font', 'Noto Sans Mono', monospace; } .git-status { font-weight: bold; } .status-normal { background: #42566a; color: #f4f7fa; } .status-visual { background: #8a752e; color: #fff8dc; } .status-find { background: #376b5b; color: #f1fff9; } .status-command { background: #604c7a; color: #faf5ff; } .status-input { background: #496278; color: #f4f8ff; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_and_dirty_git_status() {
        assert_eq!(
            parse_git_status("## main\n"),
            Some(" main   clean".to_owned())
        );
        assert_eq!(
            parse_git_status("## feature...origin/feature [ahead 1]\n M file\n"),
            Some(" feature   modified".to_owned())
        );
        assert_eq!(parse_git_status("fatal"), None);
    }

    #[test]
    fn formats_permissions_and_sizes_compactly() {
        assert_eq!(format_permissions(0o100754), "rwxr-xr--");
        assert_eq!(compact_size(1_536), "1.5 KiB");
    }
}
