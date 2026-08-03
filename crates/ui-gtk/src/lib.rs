//! GTK composition for the Phase 0 performance spike.

use std::{cell::RefCell, rc::Rc, time::Instant};

use gtk::{gdk, glib, prelude::*};
use pathpilot_core::{AppCommand, KeyResult, KeySequenceParser};
use tracing::{debug, info, info_span};

pub const SYNTHETIC_ENTRY_COUNT: u32 = 100_000;

pub fn build_window(app: &gtk::Application) -> gtk::ApplicationWindow {
    let startup_span = info_span!("build_window");
    let _guard = startup_span.enter();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("PathPilot — Phase 0")
        .default_width(1200)
        .default_height(720)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let columns = gtk::Paned::new(gtk::Orientation::Horizontal);
    columns.set_wide_handle(true);

    let parent = placeholder("Parent", "Synthetic parent column");
    let current_and_preview = gtk::Paned::new(gtk::Orientation::Horizontal);
    current_and_preview.set_wide_handle(true);
    let preview = placeholder("Preview", "Preview work starts in Phase 2");

    let (list, selection, model_duration) = synthetic_list();
    let current = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&list)
        .build();

    current_and_preview.set_start_child(Some(&current));
    current_and_preview.set_end_child(Some(&preview));
    current_and_preview.set_position(560);
    current_and_preview.set_resize_start_child(true);
    current_and_preview.set_resize_end_child(true);

    columns.set_start_child(Some(&parent));
    columns.set_end_child(Some(&current_and_preview));
    columns.set_position(260);
    columns.set_resize_start_child(true);
    columns.set_resize_end_child(true);

    let status = gtk::Label::builder()
        .label(format!("Selected: 1 / {SYNTHETIC_ENTRY_COUNT}"))
        .xalign(0.0)
        .margin_start(10)
        .margin_end(10)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    connect_selection_status(&selection, &status);
    install_keyboard_controller(&window, &selection, &list);

    root.append(&columns);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&status);
    window.set_child(Some(&root));

    info!(
        entry_count = SYNTHETIC_ENTRY_COUNT,
        model_creation_ms = model_duration.as_millis(),
        "window constructed"
    );
    window
}

fn synthetic_list() -> (gtk::ListView, gtk::SingleSelection, std::time::Duration) {
    let started = Instant::now();
    let strings: Vec<String> = (0..SYNTHETIC_ENTRY_COUNT)
        .map(|index| format!("synthetic-file-{index:06}.txt"))
        .collect();
    let model = gtk::StringList::new(&strings.iter().map(String::as_str).collect::<Vec<_>>());
    let elapsed = started.elapsed();

    let selection = gtk::SingleSelection::new(Some(model));
    selection.set_autoselect(true);
    selection.set_can_unselect(false);

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let label = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(8)
            .margin_end(8)
            .margin_top(3)
            .margin_bottom(3)
            .build();
        item.downcast_ref::<gtk::ListItem>()
            .expect("factory setup receives ListItem")
            .set_child(Some(&label));
    });
    factory.connect_bind(|_, item| {
        let item = item
            .downcast_ref::<gtk::ListItem>()
            .expect("factory bind receives ListItem");
        let string_object = item
            .item()
            .and_downcast::<gtk::StringObject>()
            .expect("StringList contains StringObject values");
        let label = item
            .child()
            .and_downcast::<gtk::Label>()
            .expect("factory child is a Label");
        label.set_label(&string_object.string());
    });

    let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
    list.set_single_click_activate(false);
    list.set_vexpand(true);

    (list, selection, elapsed)
}

fn connect_selection_status(selection: &gtk::SingleSelection, status: &gtk::Label) {
    selection.connect_selected_notify(glib::clone!(
        #[weak]
        status,
        move |selection| {
            let selected = selection.selected();
            status.set_label(&format!(
                "Selected: {} / {SYNTHETIC_ENTRY_COUNT}",
                selected + 1
            ));
            debug!(selected_index = selected, "selection changed");
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
    let current = selection.selected().min(SYNTHETIC_ENTRY_COUNT - 1);
    let target = match command {
        AppCommand::NavigateUp => current.saturating_sub(1),
        AppCommand::NavigateDown => (current + 1).min(SYNTHETIC_ENTRY_COUNT - 1),
        AppCommand::GoFirst => 0,
        AppCommand::GoLast => SYNTHETIC_ENTRY_COUNT - 1,
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
