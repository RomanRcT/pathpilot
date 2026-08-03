use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk::{gdk, gio, glib, prelude::*};
use pathpilot_core::{FileEntry, FileKind, Generation};
use pathpilot_preview::{PreviewContent, PreviewGate, PreviewLimits, PreviewResult, load_preview};
use tracing::debug;

use crate::directory_pane::DirectoryPane;

const PREVIEW_DELAY: Duration = Duration::from_millis(75);

#[derive(Default)]
struct PreviewState {
    gate: RefCell<PreviewGate>,
    delay: RefCell<Option<glib::SourceId>>,
    cancellable: RefCell<Option<gio::Cancellable>>,
}

#[derive(Clone)]
pub struct PreviewPane {
    pub widget: gtk::Box,
    stack: gtk::Stack,
    directory: DirectoryPane,
    metadata: gtk::Label,
    text: gtk::TextView,
    picture: gtk::Picture,
    state: Rc<PreviewState>,
}

impl PreviewPane {
    pub fn new() -> Self {
        let directory = DirectoryPane::new("Directory preview");
        let metadata = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .yalign(0.0)
            .margin_start(16)
            .margin_end(16)
            .margin_top(16)
            .margin_bottom(16)
            .build();
        metadata.set_selectable(true);

        let text = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::None)
            .left_margin(10)
            .right_margin(10)
            .top_margin(10)
            .bottom_margin(10)
            .build();
        let text_scroller = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&text)
            .build();

        let picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .hexpand(true)
            .vexpand(true)
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        let stack = gtk::Stack::builder()
            .hexpand(true)
            .vexpand(true)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(100)
            .build();
        stack.add_named(&metadata, Some("metadata"));
        stack.add_named(&text_scroller, Some("text"));
        stack.add_named(&picture, Some("image"));
        stack.add_named(&directory.widget, Some("directory"));

        let title = gtk::Label::builder()
            .label("Preview")
            .xalign(0.0)
            .margin_start(8)
            .margin_end(8)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        title.add_css_class("heading");
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&title);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        widget.append(&stack);

        Self {
            widget,
            stack,
            directory,
            metadata,
            text,
            picture,
            state: Rc::new(PreviewState::default()),
        }
    }

    pub fn schedule(&self, entry: FileEntry) {
        self.cancel();
        let generation = self.state.gate.borrow_mut().begin();
        self.metadata
            .set_label(&basic_metadata(&entry, "Loading preview…"));
        self.stack.set_visible_child_name("metadata");

        let pane = self.clone();
        let state = self.state.clone();
        let source = glib::timeout_add_local_once(PREVIEW_DELAY, move || {
            state.delay.borrow_mut().take();
            if state.gate.borrow().accepts(generation) {
                pane.start(entry, generation);
            }
        });
        *self.state.delay.borrow_mut() = Some(source);
    }

    pub fn show_empty(&self) {
        self.cancel();
        self.metadata.set_label("No selection");
        self.stack.set_visible_child_name("metadata");
    }

    pub fn cancel(&self) {
        self.state.gate.borrow_mut().begin();
        if let Some(source) = self.state.delay.borrow_mut().take() {
            source.remove();
        }
        if let Some(cancellable) = self.state.cancellable.borrow_mut().take() {
            cancellable.cancel();
        }
        self.directory.cancel();
    }

    fn start(&self, entry: FileEntry, generation: Generation) {
        if entry.kind == FileKind::Directory {
            self.stack.set_visible_child_name("directory");
            self.directory.load(&entry.location, |_| {});
            return;
        }

        let pane = self.clone();
        let state = self.state.clone();
        let render_entry = entry.clone();
        let cancellable = load_preview(
            &entry,
            generation,
            PreviewLimits::default(),
            move |result| {
                state.cancellable.borrow_mut().take();
                if !state.gate.borrow().accepts(result.generation) {
                    debug!(
                        generation = result.generation.value(),
                        "discarding stale preview result"
                    );
                    return;
                }
                pane.render(&render_entry, result);
            },
        );
        *self.state.cancellable.borrow_mut() = Some(cancellable);
    }

    fn render(&self, entry: &FileEntry, result: PreviewResult) {
        match result.content {
            Ok(PreviewContent::Text(preview)) => {
                if preview.text.is_empty() {
                    self.metadata
                        .set_label(&basic_metadata(entry, "Empty file — nothing to preview"));
                    self.stack.set_visible_child_name("metadata");
                    return;
                }
                let suffix = if preview.truncated {
                    "\n\n[Preview truncated]"
                } else {
                    ""
                };
                self.text
                    .buffer()
                    .set_text(&format!("{}{suffix}", preview.text));
                self.stack.set_visible_child_name("text");
            }
            Ok(PreviewContent::Image(preview)) => {
                let texture = gdk::Texture::for_pixbuf(&preview.pixbuf);
                self.picture.set_paintable(Some(&texture));
                self.stack.set_visible_child_name("image");
            }
            Ok(PreviewContent::Unsupported) => {
                self.metadata
                    .set_label(&basic_metadata(entry, "Preview is not supported"));
                self.stack.set_visible_child_name("metadata");
            }
            Err(message) => {
                self.metadata
                    .set_label(&basic_metadata(entry, &format!("Preview error: {message}")));
                self.stack.set_visible_child_name("metadata");
            }
        }
    }
}

fn basic_metadata(entry: &FileEntry, status: &str) -> String {
    format!(
        "{}\n\nType: {}\nSize: {}\nURI: {}\n\n{status}",
        entry.display_name,
        entry.content_type.as_deref().unwrap_or("unknown"),
        entry
            .size
            .map_or_else(|| "unknown".to_owned(), |value| format!("{value} bytes")),
        entry.location.uri()
    )
}
