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
    AppCommand, COMMAND_REFERENCE, FileEntry, FileKind, FilenameFind, KeyResult, KeySequenceParser,
    Location, NavigationState, OperationId,
};
use pathpilot_operations::{OperationResult, create_directory, create_file, rename, trash};
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
            AppCommand::Quit => unreachable!("quit handled before navigation dispatch"),
        }
        false
    }

    fn operation_id(&self) -> OperationId {
        let value = self.next_operation_id.get();
        self.next_operation_id.set(value.wrapping_add(1));
        OperationId::new(value)
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

    fn operation_finished(self: &Rc<Self>, result: OperationResult) {
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
            parser.borrow_mut().reset();
            if hints_enabled.get() {
                show_command_reference(&key_hints);
            } else {
                key_hints.set_visible(false);
            }
            return glib::Propagation::Stop;
        }
        if modifiers.intersects(
            gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK,
        ) {
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

        let key_result = match key {
            gdk::Key::F2 => KeyResult::Command(AppCommand::Rename),
            gdk::Key::Delete => KeyResult::Command(AppCommand::Trash),
            _ => {
                let Some(character) = key.to_unicode() else {
                    return glib::Propagation::Proceed;
                };
                parser.borrow_mut().feed(character, Instant::now())
            }
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
