//! GTK composition for three-column local filesystem navigation.

mod directory_pane;
mod preview_pane;

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::Instant,
};

use directory_pane::DirectoryPane;
use gtk::{gdk, gio, glib, prelude::*};
use pathpilot_core::{
    AppCommand, AppMode, COMMAND_REFERENCE, ClipboardAction, ClipboardItem, CommandPalette,
    FileEntry, FileKind, FilenameFind, InputModeKind, KeyResult, KeySequenceParser, Location,
    NavigationState, OperationClipboard, OperationId,
};
use pathpilot_operations::{
    BatchOperationResult, OperationHandle, OperationResult, TransferSpec, copy_items,
    create_directory, create_file, delete_items, move_items, rename, trash_items,
};
use preview_pane::PreviewPane;
use tracing::{debug, info, info_span, warn};

struct Browser {
    navigation: RefCell<NavigationState>,
    parent: DirectoryPane,
    current: DirectoryPane,
    preview: PreviewPane,
    location_label: gtk::Label,
    status: gtk::Label,
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
}

impl Browser {
    fn new(initial: Location) -> Rc<Self> {
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
            status: gtk::Label::builder()
                .label("NORMAL")
                .xalign(0.0)
                .margin_start(10)
                .margin_end(10)
                .margin_top(6)
                .margin_bottom(6)
                .build(),
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
            self.status.set_label(&format!(
                "VISUAL  {count} selected · j/k extend · v/Escape finish"
            ));
        } else {
            let clipboard = self.clipboard_status();
            self.status.set_label(&format!(
                "NORMAL  Selected: {} / {total}  h/j/k/l navigate · q quit{clipboard}",
                selected + 1,
            ));
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
                    | AppCommand::Cut
                    | AppCommand::Trash
                    | AppCommand::PermanentDelete
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
            AppCommand::Cut => self.store_operation_clipboard(ClipboardAction::Move),
            AppCommand::Paste => self.paste_operation_clipboard(window),
            AppCommand::ToggleVisual => self.toggle_visual(),
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
        self.location_label.set_label(location.uri());
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
    let initial = Location::new(gio::File::for_path(".").uri());
    let browser = Browser::new(initial);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("PathPilot — Preview Prototype")
        .default_width(1400)
        .default_height(760)
        .build();
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
    root.append(&browser.status);
    install_hint_css();
    window.set_child(Some(&root));

    browser.connect();
    install_keyboard_controller(&window, Rc::downgrade(&browser), &key_hints);
    let close_browser = browser.clone();
    window.connect_close_request(move |_| {
        close_browser.cancel();
        glib::Propagation::Proceed
    });
    browser.initial_load();

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

fn install_keyboard_controller(
    window: &gtk::ApplicationWindow,
    browser: Weak<Browser>,
    key_hints: &gtk::Grid,
) {
    let parser = Rc::new(RefCell::new(KeySequenceParser::default()));
    let hints_enabled = Rc::new(Cell::new(false));
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak_window = window.downgrade();
    let key_hints = key_hints.clone();
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
            restore_hint_overlay(&key_hints, hints_enabled.get());
            return glib::Propagation::Stop;
        }

        if let Some(browser) = browser.upgrade()
            && browser.mode.borrow().text_input().is_some()
        {
            if key == gdk::Key::Escape {
                browser.cancel_text_input();
                restore_hint_overlay(&key_hints, hints_enabled.get());
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
                    restore_hint_overlay(&key_hints, hints_enabled.get());
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    browser.accept_find();
                    restore_hint_overlay(&key_hints, hints_enabled.get());
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
            if browser
                .upgrade()
                .is_some_and(|browser| browser.leave_visual())
            {
                restore_hint_overlay(&key_hints, hints_enabled.get());
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
                show_command_reference(&key_hints);
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
            if enabled {
                show_command_reference(&key_hints);
            }
            key_hints.set_visible(enabled);
            return glib::Propagation::Stop;
        }

        if !parser.borrow().is_pending()
            && let Some(character) = key.to_unicode()
        {
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
                    show_command_reference(&key_hints);
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

fn restore_hint_overlay(grid: &gtk::Grid, hints_enabled: bool) {
    if hints_enabled {
        show_command_reference(grid);
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

fn show_command_reference(grid: &gtk::Grid) {
    populate_hint_grid(
        grid,
        COMMAND_REFERENCE
            .iter()
            .map(|hint| (hint.keys.to_owned(), hint.label)),
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

fn install_hint_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        ".interaction-panel-content { background-color: @window_bg_color; border-top: 1px solid alpha(@window_fg_color, 0.16); padding: 10px 16px; } .key-hint-key { font-family: monospace; font-weight: bold; color: @accent_color; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
