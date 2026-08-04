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
    AppCommand, COMMAND_REFERENCE, ClipboardAction, FileEntry, FileKind, FilenameFind, KeyResult,
    KeySequenceParser, Location, NavigationState, OperationClipboard, OperationId, OperationKind,
};
use pathpilot_operations::{
    OperationHandle, OperationResult, copy_item_with_progress, create_directory, create_file,
    delete_permanently, move_item, rename, trash,
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
    operation_clipboard: RefCell<Option<OperationClipboard>>,
    active_operation: RefCell<Option<OperationHandle>>,
}

impl Browser {
    fn new(initial: Location) -> Rc<Self> {
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
            operation_clipboard: RefCell::new(None),
            active_operation: RefCell::new(None),
        })
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.current
            .selection
            .connect_selected_notify(move |selection| {
                if let Some(browser) = weak.upgrade() {
                    browser.selection_changed(selection.selected());
                }
            });

        connect_activation(&self.current, Rc::downgrade(self));
        connect_activation(&self.parent, Rc::downgrade(self));
    }

    fn initial_load(self: &Rc<Self>) {
        self.reload_columns(None, None);
    }

    fn selection_changed(self: &Rc<Self>, selected: u32) {
        let total = self.current.selection.n_items();
        if selected == gtk::INVALID_LIST_POSITION {
            self.status.set_label("NORMAL  No selection");
            self.preview.show_empty();
            return;
        }
        self.status.set_label(&format!(
            "NORMAL  Selected: {} / {total}  h/j/k/l navigate · q quit",
            selected + 1
        ));
        self.update_preview();
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
            AppCommand::CreateFile => self.prompt_create(window, false),
            AppCommand::CreateDirectory => self.prompt_create(window, true),
            AppCommand::Rename => self.prompt_rename(window),
            AppCommand::Trash => self.confirm_trash(window),
            AppCommand::PermanentDelete => self.confirm_permanent_delete(window),
            AppCommand::Copy => self.store_operation_clipboard(ClipboardAction::Copy),
            AppCommand::Cut => self.store_operation_clipboard(ClipboardAction::Move),
            AppCommand::Paste => self.paste_operation_clipboard(window),
            AppCommand::Quit => unreachable!("quit handled before navigation dispatch"),
        }
        false
    }

    fn operation_id(&self) -> OperationId {
        let value = self.next_operation_id.get();
        self.next_operation_id.set(value.wrapping_add(1));
        OperationId::new(value)
    }

    fn store_operation_clipboard(&self, action: ClipboardAction) {
        let Some(entry) = self.current.selected_entry() else {
            self.status.set_label("NORMAL  Nothing selected");
            return;
        };
        let verb = match action {
            ClipboardAction::Copy => "Copied",
            ClipboardAction::Move => "Cut",
        };
        self.status
            .set_label(&format!("NORMAL  {verb}: {}", entry.display_name));
        *self.operation_clipboard.borrow_mut() = Some(OperationClipboard {
            action,
            source: entry.location,
            display_name: entry.display_name,
        });
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
        let destination = Location::new(parent.child(&clipboard.display_name).uri());
        if gio::File::for_uri(destination.uri()).query_exists(None::<&gio::Cancellable>) {
            self.confirm_keep_both(window, clipboard, parent);
            return;
        }
        self.start_paste(clipboard, destination);
    }

    #[allow(deprecated)]
    fn confirm_keep_both(
        self: &Rc<Self>,
        window: &gtk::ApplicationWindow,
        clipboard: OperationClipboard,
        parent: gio::File,
    ) {
        let dialog = gtk::MessageDialog::builder()
            .transient_for(window)
            .modal(true)
            .message_type(gtk::MessageType::Question)
            .buttons(gtk::ButtonsType::None)
            .text("An item with this name already exists")
            .secondary_text("Keep both items with a unique name?")
            .build();
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Keep Both", gtk::ResponseType::Accept);
        dialog.set_default_response(gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(browser) = weak.upgrade()
            {
                browser.start_paste(
                    clipboard.clone(),
                    unique_destination(&parent, &clipboard.display_name),
                );
            }
            dialog.close();
        });
        dialog.present();
    }

    fn start_paste(self: &Rc<Self>, clipboard: OperationClipboard, destination: Location) {
        let weak_finished = Rc::downgrade(self);
        let finished = move |result| {
            if let Some(browser) = weak_finished.upgrade() {
                browser.operation_finished(result);
            }
        };
        self.status
            .set_label(&format!("NORMAL  Pasting {}…", clipboard.display_name));
        let handle = match clipboard.action {
            ClipboardAction::Copy => {
                let weak_progress = Rc::downgrade(self);
                copy_item_with_progress(
                    self.operation_id(),
                    &clipboard.source,
                    &destination,
                    move |progress| {
                        if let Some(browser) = weak_progress.upgrade() {
                            let items = format!(
                                "{} / {} items",
                                progress.completed_items,
                                progress.total_items.unwrap_or(0)
                            );
                            let message =
                                progress.total_bytes.filter(|total| *total > 0).map_or_else(
                                    || format!("NORMAL  Copying {items}"),
                                    |total| {
                                        format!(
                                            "NORMAL  Copying {items} · {} / {total} bytes",
                                            progress.completed_bytes
                                        )
                                    },
                                );
                            browser.status.set_label(&message);
                        }
                    },
                    finished,
                )
            }
            ClipboardAction::Move => {
                let weak_progress = Rc::downgrade(self);
                move_item(
                    self.operation_id(),
                    &clipboard.source,
                    &destination,
                    move |current, total| {
                        if let Some(browser) = weak_progress.upgrade() {
                            let message = total.filter(|total| *total > 0).map_or_else(
                                || format!("NORMAL  Moved {current} bytes"),
                                |total| format!("NORMAL  Moving {current} / {total} bytes"),
                            );
                            browser.status.set_label(&message);
                        }
                    },
                    finished,
                )
            }
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
        let position = self.current.selection.selected();
        let position = if position == gtk::INVALID_LIST_POSITION {
            0
        } else {
            position
        };
        self.find.borrow_mut().start(position);
        self.status.set_label("FIND  Type a filename");
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
        self.status.set_label("NORMAL  Find accepted · n/N repeat");
    }

    fn cancel_find(&self) {
        let position = self.find.borrow_mut().cancel();
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

    fn prompt_create(self: &Rc<Self>, window: &gtk::ApplicationWindow, directory: bool) {
        let title = if directory {
            "Create Directory"
        } else {
            "Create File"
        };
        let weak = Rc::downgrade(self);
        prompt_name(window, title, "", move |name| {
            let Some(browser) = weak.upgrade() else {
                return;
            };
            let parent = browser.navigation.borrow().current().clone();
            let id = browser.operation_id();
            let callback_browser = Rc::downgrade(&browser);
            let callback = move |result| {
                if let Some(browser) = callback_browser.upgrade() {
                    browser.operation_finished(result);
                }
            };
            if directory {
                create_directory(id, &parent, &name, callback);
            } else {
                create_file(id, &parent, &name, callback);
            }
        });
    }

    fn prompt_rename(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        let Some(entry) = self.current.selected_entry() else {
            return;
        };
        let initial = entry.display_name.clone();
        let weak = Rc::downgrade(self);
        prompt_name(window, "Rename", &initial, move |name| {
            let Some(browser) = weak.upgrade() else {
                return;
            };
            let callback_browser = Rc::downgrade(&browser);
            rename(
                browser.operation_id(),
                &entry.location,
                &name,
                move |result| {
                    if let Some(browser) = callback_browser.upgrade() {
                        browser.operation_finished(result);
                    }
                },
            );
        });
    }

    #[allow(deprecated)]
    fn confirm_trash(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        let Some(entry) = self.current.selected_entry() else {
            return;
        };
        let dialog = gtk::MessageDialog::builder()
            .transient_for(window)
            .modal(true)
            .message_type(gtk::MessageType::Warning)
            .buttons(gtk::ButtonsType::OkCancel)
            .text("Move item to Trash?")
            .secondary_text(&entry.display_name)
            .build();
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Ok
                && let Some(browser) = weak.upgrade()
            {
                let callback_browser = Rc::downgrade(&browser);
                trash(browser.operation_id(), &entry.location, move |result| {
                    if let Some(browser) = callback_browser.upgrade() {
                        browser.operation_finished(result);
                    }
                });
            }
            dialog.close();
        });
        dialog.present();
    }

    #[allow(deprecated)]
    fn confirm_permanent_delete(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        let Some(entry) = self.current.selected_entry() else {
            return;
        };
        let dialog = gtk::MessageDialog::builder()
            .transient_for(window)
            .modal(true)
            .message_type(gtk::MessageType::Error)
            .buttons(gtk::ButtonsType::None)
            .text("Delete permanently?")
            .secondary_text(format!(
                "{}\n\nThis cannot be undone. Directories and all their contents will be removed.",
                entry.display_name
            ))
            .build();
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Delete Permanently", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(browser) = weak.upgrade()
            {
                let weak_progress = Rc::downgrade(&browser);
                let weak_finished = Rc::downgrade(&browser);
                browser.status.set_label("NORMAL  Deleting permanently…");
                let handle = delete_permanently(
                    browser.operation_id(),
                    &entry.location,
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
                            browser.operation_finished(result);
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
        let completed_move = matches!(result.kind, OperationKind::Move { .. });
        match result.result {
            Ok(()) => {
                if completed_move {
                    self.operation_clipboard.borrow_mut().take();
                }
                self.status.set_label("NORMAL  Operation completed");
                self.reload_columns(result.resulting_location, None);
            }
            Err(error) => self
                .status
                .set_label(&format!("NORMAL  Operation failed: {}", error.message)),
        }
    }

    fn move_cursor(&self, offset: i32) {
        let count = self.current.selection.n_items();
        if count == 0 {
            return;
        }
        let current = self.current.selection.selected().min(count - 1);
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
        let selected = self.current.selection.selected();
        if selected != gtk::INVALID_LIST_POSITION {
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
            browser.selection_changed(browser.current.selection.selected());
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
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&browser.status);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&root));
    let key_hints = gtk::Grid::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .margin_bottom(42)
        .column_spacing(12)
        .row_spacing(6)
        .visible(false)
        .build();
    key_hints.add_css_class("key-hint-overlay");
    overlay.add_overlay(&key_hints);
    install_hint_css();
    window.set_child(Some(&overlay));

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
    let selection = pane.selection.clone();
    list.connect_activate(move |_, position| {
        selection.set_selected(position);
        let entry = selection
            .selected_item()
            .and_downcast::<glib::BoxedAnyObject>()
            .map(|object| object.borrow::<FileEntry>().clone());
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
    let weak_window = window.downgrade();
    let key_hints = key_hints.clone();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        if let Some(browser) = browser.upgrade()
            && browser.find.borrow().is_active()
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
                        }
                    }
                }
                glib::Propagation::Stop
            }
            KeyResult::Pending(pending) => {
                if hints_enabled.get() {
                    populate_hint_grid(
                        &key_hints,
                        pending
                            .hints
                            .iter()
                            .map(|hint| (hint.key.to_string(), hint.label)),
                    );
                    key_hints.set_visible(true);
                }
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

#[allow(deprecated)]
fn prompt_name(
    window: &gtk::ApplicationWindow,
    title: &str,
    initial: &str,
    on_accept: impl Fn(String) + 'static,
) {
    let dialog = gtk::Dialog::builder()
        .transient_for(window)
        .modal(true)
        .title(title)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("OK", gtk::ResponseType::Accept);
    dialog.set_default_response(gtk::ResponseType::Accept);
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    dialog.content_area().append(&entry);
    let response_entry = entry.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            on_accept(response_entry.text().to_string());
        }
        dialog.close();
    });
    dialog.present();
    entry.grab_focus();
    entry.select_region(0, -1);
}

fn install_hint_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        ".key-hint-overlay { background-color: alpha(@window_bg_color, 0.90); border-radius: 10px; padding: 10px 16px; box-shadow: 0 3px 12px alpha(black, 0.35); } .key-hint-key { font-family: monospace; font-weight: bold; color: @accent_color; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
