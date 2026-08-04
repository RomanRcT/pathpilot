use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};

use gtk::{gdk, gio, glib, prelude::*};
use pathpilot_core::{FileEntry, FileKind, Generation};
use pathpilot_preview::{
    MarkdownPreview, MarkdownStyle, PreviewCache, PreviewContent, PreviewGate, PreviewLimits,
    PreviewResult, StyledTextPreview, load_preview,
};
use tracing::debug;
use vte::prelude::*;

use crate::directory_pane::DirectoryPane;

const PREVIEW_DELAY: Duration = Duration::from_millis(75);

struct PreviewState {
    gate: RefCell<PreviewGate>,
    delay: RefCell<Option<glib::SourceId>>,
    cancellable: RefCell<Option<gio::Cancellable>>,
    cache: RefCell<PreviewCache>,
    has_rendered_content: Cell<bool>,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            gate: RefCell::new(PreviewGate::default()),
            delay: RefCell::new(None),
            cancellable: RefCell::new(None),
            cache: RefCell::new(PreviewCache::new(24)),
            has_rendered_content: Cell::new(false),
        }
    }
}

#[derive(Clone)]
pub struct PreviewPane {
    pub widget: gtk::Box,
    stack: gtk::Stack,
    title: gtk::Label,
    terminal: vte::Terminal,
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
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
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
        let terminal = vte::Terminal::new();
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.set_scrollback_lines(10_000);
        stack.add_named(&metadata, Some("metadata"));
        stack.add_named(&text_scroller, Some("text"));
        stack.add_named(&picture, Some("image"));
        stack.add_named(&directory.widget, Some("directory"));
        stack.add_named(&terminal, Some("editor"));

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
            title,
            terminal,
            directory,
            metadata,
            text,
            picture,
            state: Rc::new(PreviewState::default()),
        }
    }

    pub fn terminal(&self) -> vte::Terminal {
        self.terminal.clone()
    }

    pub fn show_editor(&self, name: &str) {
        self.cancel();
        self.title.set_label(&format!("Editing — {name}"));
        self.terminal.reset(true, true);
        self.stack.set_visible_child_name("editor");
        self.terminal.grab_focus();
    }

    pub fn leave_editor(&self) {
        self.title.set_label("Preview");
    }

    pub fn invalidate(&self, entry: &FileEntry) {
        self.state.cache.borrow_mut().invalidate(&entry.location);
        self.state.has_rendered_content.set(false);
    }

    pub fn schedule(&self, entry: FileEntry) {
        self.cancel();
        let generation = self.state.gate.borrow_mut().begin();
        if !self.state.has_rendered_content.get() {
            self.metadata
                .set_label(&basic_metadata(&entry, "Loading preview…"));
            self.stack.set_visible_child_name("metadata");
        }

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
        self.state.has_rendered_content.set(false);
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
            self.state.has_rendered_content.set(true);
            self.stack.set_visible_child_name("directory");
            self.directory.load(&entry.location, |_| {});
            return;
        }

        if let Some(content) = self.state.cache.borrow_mut().get(&entry) {
            self.render(
                &entry,
                PreviewResult {
                    generation,
                    content: Ok(content),
                },
            );
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
                if let Ok(content) = &result.content {
                    state
                        .cache
                        .borrow_mut()
                        .insert(&render_entry, content.clone());
                }
                pane.render(&render_entry, result);
            },
        );
        *self.state.cancellable.borrow_mut() = Some(cancellable);
    }

    fn render(&self, entry: &FileEntry, result: PreviewResult) {
        self.state.has_rendered_content.set(true);
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
            Ok(PreviewContent::StyledText(preview)) => {
                self.render_styled_text(preview);
                self.stack.set_visible_child_name("text");
            }
            Ok(PreviewContent::Markdown(preview)) => {
                self.render_markdown(preview);
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

    fn render_styled_text(&self, preview: StyledTextPreview) {
        let buffer = gtk::TextBuffer::new(None);
        let suffix = if preview.truncated {
            "\n\n[Preview truncated]"
        } else {
            ""
        };
        buffer.set_text(&format!("{}{suffix}", preview.text));

        let background_tag = gtk::TextTag::builder()
            .background_rgba(&rgba(preview.background))
            .build();
        buffer.tag_table().add(&background_tag);
        buffer.apply_tag(&background_tag, &buffer.start_iter(), &buffer.end_iter());

        let mut tags = HashMap::new();
        for span in preview.spans {
            let key = (span.foreground, span.bold, span.italic, span.underline);
            let tag = tags.entry(key).or_insert_with(|| {
                let tag = gtk::TextTag::builder()
                    .foreground_rgba(&rgba(span.foreground))
                    .weight(if span.bold { 700 } else { 400 })
                    .style(if span.italic {
                        gtk::pango::Style::Italic
                    } else {
                        gtk::pango::Style::Normal
                    })
                    .underline(if span.underline {
                        gtk::pango::Underline::Single
                    } else {
                        gtk::pango::Underline::None
                    })
                    .build();
                buffer.tag_table().add(&tag);
                tag
            });
            buffer.apply_tag(
                tag,
                &buffer.iter_at_offset(span.start),
                &buffer.iter_at_offset(span.end),
            );
        }
        self.text.set_buffer(Some(&buffer));
    }

    fn render_markdown(&self, preview: MarkdownPreview) {
        let buffer = gtk::TextBuffer::new(None);
        let suffix = if preview.truncated {
            "\n[Preview truncated]"
        } else {
            ""
        };
        buffer.set_text(&format!("{}{suffix}", preview.text));

        for span in preview.spans {
            let builder = gtk::TextTag::builder();
            let tag = match span.style {
                MarkdownStyle::Heading(level) => builder
                    .weight(700)
                    .scale(match level {
                        1 => 1.8,
                        2 => 1.5,
                        3 => 1.3,
                        _ => 1.1,
                    })
                    .build(),
                MarkdownStyle::Strong => builder.weight(700).build(),
                MarkdownStyle::Emphasis | MarkdownStyle::Quote => {
                    builder.style(gtk::pango::Style::Italic).build()
                }
                MarkdownStyle::Code => builder
                    .family("monospace")
                    .background_rgba(&gdk::RGBA::new(0.20, 0.22, 0.26, 1.0))
                    .foreground_rgba(&gdk::RGBA::new(0.90, 0.90, 0.90, 1.0))
                    .build(),
                MarkdownStyle::Link => builder
                    .foreground_rgba(&gdk::RGBA::new(0.25, 0.55, 0.95, 1.0))
                    .underline(gtk::pango::Underline::Single)
                    .build(),
            };
            buffer.tag_table().add(&tag);
            buffer.apply_tag(
                &tag,
                &buffer.iter_at_offset(span.start),
                &buffer.iter_at_offset(span.end),
            );
        }
        self.text.set_buffer(Some(&buffer));
    }
}

fn rgba(color: (u8, u8, u8)) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(color.0) / 255.0,
        f32::from(color.1) / 255.0,
        f32::from(color.2) / 255.0,
        1.0,
    )
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
