//! GTK composition and directory presentation.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use gtk::{gdk, gio, glib, prelude::*};
use pathpilot_core::{
    AppCommand, FileEntry, FileKind, GenerationTracker, KeyResult, KeySequenceParser, Location,
};
use pathpilot_fs_local::{DirectoryEvent, load_directory};
use tracing::{debug, info, info_span, warn};

#[derive(Default)]
struct DirectoryLoadState {
    generation: RefCell<GenerationTracker>,
    cancellable: RefCell<Option<gio::Cancellable>>,
}

impl Drop for DirectoryLoadState {
    fn drop(&mut self) {
        if let Some(cancellable) = self.cancellable.get_mut().take() {
            cancellable.cancel();
        }
    }
}

pub fn build_window(app: &gtk::Application) -> gtk::ApplicationWindow {
    let startup_span = info_span!("build_window");
    let _guard = startup_span.enter();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("PathPilot — Directory Model")
        .default_width(1200)
        .default_height(720)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let columns = gtk::Paned::new(gtk::Orientation::Horizontal);
    columns.set_wide_handle(true);

    let parent = placeholder("Parent", "Parent navigation arrives in Milestone 4");
    let current_and_preview = gtk::Paned::new(gtk::Orientation::Horizontal);
    current_and_preview.set_wide_handle(true);
    let preview = placeholder("Preview", "Preview work starts in Phase 2");

    let (list, selection, store) = directory_list();
    let current = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&list)
        .build();

    current_and_preview.set_start_child(Some(&current));
    current_and_preview.set_end_child(Some(&preview));
    current_and_preview.set_position(650);
    current_and_preview.set_resize_start_child(true);
    current_and_preview.set_resize_end_child(true);

    columns.set_start_child(Some(&parent));
    columns.set_end_child(Some(&current_and_preview));
    columns.set_position(240);
    columns.set_resize_start_child(true);
    columns.set_resize_end_child(true);

    let location = gio::File::for_path(".");
    let location = Location::new(location.uri());
    let location_label = gtk::Label::builder()
        .label(location.uri())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .margin_start(10)
        .margin_end(10)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    let status = gtk::Label::builder()
        .label("Loading directory…")
        .xalign(0.0)
        .margin_start(10)
        .margin_end(10)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    connect_selection_status(&selection, &status);
    install_keyboard_controller(&window, &selection, &list);

    root.append(&location_label);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&columns);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&status);
    window.set_child(Some(&root));

    let load_state = Rc::new(DirectoryLoadState::default());
    start_directory_load(&location, &store, &status, &load_state);
    window.connect_close_request(move |_| {
        if let Some(cancellable) = load_state.cancellable.borrow_mut().take() {
            cancellable.cancel();
        }
        glib::Propagation::Proceed
    });

    info!(location = location.uri(), "window constructed");
    window
}

fn directory_list() -> (gtk::ListView, gtk::SingleSelection, gio::ListStore) {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let sorter = gtk::CustomSorter::new(|left, right| {
        let left = left
            .downcast_ref::<glib::BoxedAnyObject>()
            .expect("directory model contains BoxedAnyObject");
        let right = right
            .downcast_ref::<glib::BoxedAnyObject>()
            .expect("directory model contains BoxedAnyObject");
        left.borrow::<FileEntry>()
            .compare_name(&right.borrow::<FileEntry>())
            .into()
    });
    let sorted = gtk::SortListModel::new(Some(store.clone()), Some(sorter));
    let selection = gtk::SingleSelection::new(Some(sorted));
    selection.set_autoselect(true);
    selection.set_can_unselect(false);

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        item.downcast_ref::<gtk::ListItem>()
            .expect("factory setup receives ListItem")
            .set_child(Some(&create_row()));
    });
    factory.connect_bind(|_, item| bind_row(item));

    let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
    list.set_single_click_activate(false);
    list.set_vexpand(true);

    (list, selection, store)
}

fn create_row() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_start(8);
    row.set_margin_end(8);
    row.set_margin_top(3);
    row.set_margin_bottom(3);

    let icon = gtk::Image::new();
    icon.set_icon_size(gtk::IconSize::Normal);
    let name = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let kind = metadata_label(90);
    let size = metadata_label(80);
    let modified = metadata_label(135);
    row.append(&icon);
    row.append(&name);
    row.append(&kind);
    row.append(&size);
    row.append(&modified);
    row
}

fn metadata_label(width: i32) -> gtk::Label {
    let label = gtk::Label::builder()
        .xalign(0.0)
        .width_request(width)
        .build();
    label.add_css_class("dim-label");
    label
}

fn bind_row(item: &glib::Object) {
    let item = item
        .downcast_ref::<gtk::ListItem>()
        .expect("factory bind receives ListItem");
    let object = item
        .item()
        .and_downcast::<glib::BoxedAnyObject>()
        .expect("directory model contains FileEntry values");
    let entry = object.borrow::<FileEntry>();
    let row = item
        .child()
        .and_downcast::<gtk::Box>()
        .expect("factory child is a Box");
    let icon = row
        .first_child()
        .and_downcast::<gtk::Image>()
        .expect("first row child is an Image");
    let name = next_label(&icon);
    let kind = next_label(&name);
    let size = next_label(&kind);
    let modified = next_label(&size);

    let content_type = entry.content_type.as_deref().unwrap_or_else(|| {
        if entry.kind == FileKind::Directory {
            "inode/directory"
        } else {
            "application/octet-stream"
        }
    });
    icon.set_from_gicon(&gio::content_type_get_icon(content_type));
    name.set_label(&entry.display_name);
    kind.set_label(file_kind_label(entry.kind));
    size.set_label(&format_size(entry.size));
    modified.set_label(&format_modified(entry.modified));
    row.set_tooltip_text(Some(entry.location.uri()));
}

fn next_label(widget: &impl IsA<gtk::Widget>) -> gtk::Label {
    widget
        .next_sibling()
        .and_downcast::<gtk::Label>()
        .expect("row metadata child is a Label")
}

fn file_kind_label(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "Folder",
        FileKind::Regular => "File",
        FileKind::Symlink => "Link",
        FileKind::Special => "Special",
        FileKind::Unknown => "Unknown",
    }
}

fn format_size(size: Option<u64>) -> String {
    let Some(bytes) = size else {
        return "—".to_owned();
    };
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
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

fn format_modified(modified: Option<SystemTime>) -> String {
    let Some(seconds) = modified
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_secs()).ok())
    else {
        return "—".to_owned();
    };
    glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|value| value.format("%Y-%m-%d %H:%M").ok())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".to_owned())
}

fn start_directory_load(
    location: &Location,
    store: &gio::ListStore,
    status: &gtk::Label,
    state: &Rc<DirectoryLoadState>,
) {
    if let Some(previous) = state.cancellable.borrow_mut().take() {
        previous.cancel();
    }
    store.remove_all();
    status.set_label("Loading directory…");
    let generation = state.generation.borrow_mut().advance();
    let started = Instant::now();
    let location_uri = location.uri().to_owned();

    let cancellable = load_directory(
        location,
        generation,
        glib::clone!(
            #[weak]
            store,
            #[weak]
            status,
            #[strong]
            state,
            move |event| {
                let event_generation = match &event {
                    DirectoryEvent::Batch { generation, .. }
                    | DirectoryEvent::Finished { generation }
                    | DirectoryEvent::Failed { generation, .. } => *generation,
                };
                if !state.generation.borrow().accepts(event_generation) {
                    debug!(
                        generation = event_generation.value(),
                        "discarding stale directory result"
                    );
                    return;
                }

                match event {
                    DirectoryEvent::Batch { entries, .. } => {
                        let objects: Vec<_> =
                            entries.into_iter().map(glib::BoxedAnyObject::new).collect();
                        store.extend_from_slice(&objects);
                        status.set_label(&format!("Loading… {} entries", store.n_items()));
                        debug!(
                            generation = event_generation.value(),
                            batch_size = objects.len(),
                            total = store.n_items(),
                            "directory batch published"
                        );
                    }
                    DirectoryEvent::Finished { .. } => {
                        state.cancellable.borrow_mut().take();
                        status.set_label(&format!("{} entries", store.n_items()));
                        info!(
                            generation = event_generation.value(),
                            entry_count = store.n_items(),
                            elapsed_ms = started.elapsed().as_millis(),
                            location = location_uri,
                            "directory load finished"
                        );
                    }
                    DirectoryEvent::Failed { message, .. } => {
                        state.cancellable.borrow_mut().take();
                        status.set_label(&format!("Could not load directory: {message}"));
                        warn!(
                            generation = event_generation.value(),
                            location = location_uri,
                            error = message,
                            "directory load failed"
                        );
                    }
                }
            }
        ),
    );
    *state.cancellable.borrow_mut() = Some(cancellable);
}

fn connect_selection_status(selection: &gtk::SingleSelection, status: &gtk::Label) {
    selection.connect_selected_notify(glib::clone!(
        #[weak]
        status,
        move |selection| {
            let selected = selection.selected();
            let total = selection.n_items();
            if selected != gtk::INVALID_LIST_POSITION {
                status.set_label(&format!("Selected: {} / {total}", selected + 1));
                debug!(selected_index = selected, "selection changed");
            }
        }
    ));
}

fn install_keyboard_controller(
    window: &gtk::ApplicationWindow,
    selection: &gtk::SingleSelection,
    list: &gtk::ListView,
) {
    let parser = Rc::new(RefCell::new(KeySequenceParser::default()));
    let controller = gtk::EventControllerKey::new();
    controller.connect_key_pressed(glib::clone!(
        #[weak]
        selection,
        #[weak]
        list,
        #[strong]
        parser,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, modifiers| {
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
                    dispatch(command, &selection, &list);
                    glib::Propagation::Stop
                }
                KeyResult::Pending => glib::Propagation::Stop,
                KeyResult::Ignored => glib::Propagation::Proceed,
            }
        }
    ));
    window.add_controller(controller);
}

fn dispatch(command: AppCommand, selection: &gtk::SingleSelection, list: &gtk::ListView) {
    let count = selection.n_items();
    if count == 0 {
        return;
    }
    let current = selection.selected().min(count - 1);
    let target = match command {
        AppCommand::NavigateUp => current.saturating_sub(1),
        AppCommand::NavigateDown => (current + 1).min(count - 1),
        AppCommand::GoFirst => 0,
        AppCommand::GoLast => count - 1,
    };

    selection.set_selected(target);
    list.scroll_to(target, gtk::ListScrollFlags::FOCUS, None);
}

fn placeholder(title: &str, description: &str) -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(16);
    content.set_margin_end(16);

    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("title-3");
    heading.set_xalign(0.0);
    let body = gtk::Label::new(Some(description));
    body.set_wrap(true);
    body.set_xalign(0.0);
    body.add_css_class("dim-label");
    content.append(&heading);
    content.append(&body);
    content.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_file_sizes() {
        assert_eq!(format_size(None), "—");
        assert_eq!(format_size(Some(512)), "512 B");
        assert_eq!(format_size(Some(1536)), "1.5 KiB");
    }
}
