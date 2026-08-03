//! GTK composition for three-column local filesystem navigation.

mod directory_pane;

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
    time::Instant,
};

use directory_pane::DirectoryPane;
use gtk::{gdk, gio, glib, prelude::*};
use pathpilot_core::{
    AppCommand, FileEntry, FileKind, KeyResult, KeySequenceParser, Location, NavigationState,
};
use tracing::{debug, info, info_span, warn};

struct Browser {
    navigation: RefCell<NavigationState>,
    parent: DirectoryPane,
    current: DirectoryPane,
    preview: DirectoryPane,
    location_label: gtk::Label,
    status: gtk::Label,
}

impl Browser {
    fn new(initial: Location) -> Rc<Self> {
        Rc::new(Self {
            navigation: RefCell::new(NavigationState::new(initial)),
            parent: DirectoryPane::new("Parent"),
            current: DirectoryPane::new("Current"),
            preview: DirectoryPane::new("Preview"),
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
        connect_activation(&self.preview, Rc::downgrade(self));
    }

    fn initial_load(self: &Rc<Self>) {
        self.reload_columns(None, None);
    }

    fn selection_changed(self: &Rc<Self>, selected: u32) {
        let total = self.current.selection.n_items();
        if selected == gtk::INVALID_LIST_POSITION {
            self.status.set_label("NORMAL  No selection");
            self.preview.show_message("Preview", "No selection");
            return;
        }
        self.status.set_label(&format!(
            "NORMAL  Selected: {} / {total}  h/j/k/l navigate",
            selected + 1
        ));
        self.update_preview();
    }

    fn update_preview(self: &Rc<Self>) {
        let Some(entry) = self.current.selected_entry() else {
            self.preview.show_message("Preview", "No selection");
            return;
        };
        if entry.kind == FileKind::Directory {
            self.preview.load(&entry.location, |_| {});
        } else {
            self.preview.show_message(
                &entry.display_name,
                &format!(
                    "{} · {} · {}",
                    kind_label(entry.kind),
                    format_size(entry.size),
                    entry.content_type.as_deref().unwrap_or("unknown type")
                ),
            );
        }
    }

    fn dispatch(self: &Rc<Self>, command: AppCommand) {
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
        self.preview.show_message("Preview", "Loading selection…");
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
        .title("PathPilot — Three-Column Navigation")
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
    window.set_child(Some(&root));

    browser.connect();
    install_keyboard_controller(&window, Rc::downgrade(&browser));
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

fn install_keyboard_controller(window: &gtk::ApplicationWindow, browser: Weak<Browser>) {
    let parser = Rc::new(RefCell::new(KeySequenceParser::default()));
    let controller = gtk::EventControllerKey::new();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        if modifiers.intersects(
            gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK,
        ) {
            return glib::Propagation::Proceed;
        }

        let Some(character) = key.to_unicode() else {
            return glib::Propagation::Proceed;
        };
        match parser.borrow_mut().feed(character, Instant::now()) {
            KeyResult::Command(command) => {
                if let Some(browser) = browser.upgrade() {
                    debug!(?command, "dispatching keyboard command");
                    browser.dispatch(command);
                }
                glib::Propagation::Stop
            }
            KeyResult::Pending => glib::Propagation::Stop,
            KeyResult::Ignored => glib::Propagation::Proceed,
        }
    });
    window.add_controller(controller);
}

fn kind_label(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "folder",
        FileKind::Regular => "file",
        FileKind::Symlink => "symbolic link",
        FileKind::Special => "special file",
        FileKind::Unknown => "unknown",
    }
}

fn format_size(size: Option<u64>) -> String {
    size.map_or_else(
        || "unknown size".to_owned(),
        |bytes| format!("{bytes} bytes"),
    )
}
