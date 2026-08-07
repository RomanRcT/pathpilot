use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};

use gtk::{gdk, gio, glib, prelude::*};
use pathpilot_core::{FileEntry, FileKind, Generation, SortMode};
use pathpilot_preview::{
    MarkdownPreview, MarkdownStyle, PreviewCache, PreviewContent, PreviewGate, PreviewLimits,
    PreviewMode, PreviewResult, StyledTextPreview, load_preview,
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
    loading: Cell<bool>,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            gate: RefCell::new(PreviewGate::default()),
            delay: RefCell::new(None),
            cancellable: RefCell::new(None),
            cache: RefCell::new(PreviewCache::new(24)),
            has_rendered_content: Cell::new(false),
            loading: Cell::new(false),
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
    spinner: gtk::Spinner,
    show_line_numbers: bool,
    state: Rc<PreviewState>,
}

impl PreviewPane {
    pub fn new(show_line_numbers: bool) -> Self {
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
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .width_request(48)
            .height_request(48)
            .build();
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.set_scrollback_lines(10_000);
        stack.add_named(&metadata, Some("metadata"));
        stack.add_named(&text_scroller, Some("text"));
        stack.add_named(&picture, Some("image"));
        stack.add_named(&directory.widget, Some("directory"));
        stack.add_named(&terminal, Some("editor"));
        stack.add_named(&spinner, Some("loading"));

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
            spinner,
            show_line_numbers,
            state: Rc::new(PreviewState::default()),
        }
    }

    pub fn terminal(&self) -> vte::Terminal {
        self.terminal.clone()
    }

    pub fn set_show_hidden(&self, show_hidden: bool) {
        self.directory.set_show_hidden(show_hidden);
    }

    pub fn set_sort_mode(&self, mode: SortMode) {
        self.directory.set_sort_mode(mode);
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
        self.title.set_label("Preview");
        if entry.archive_format.is_some() {
            self.metadata.add_css_class("archive-metadata");
        } else {
            self.metadata.remove_css_class("archive-metadata");
        }
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
                pane.start(entry, generation, PreviewMode::Automatic, None);
            }
        });
        *self.state.delay.borrow_mut() = Some(source);
    }

    pub fn schedule_full(&self, entry: FileEntry, on_finished: impl Fn(bool) + 'static) {
        self.cancel();
        let generation = self.state.gate.borrow_mut().begin();
        self.state.loading.set(true);
        self.title.set_label(&format!(
            "Full preview — {} · Escape cancels",
            entry.display_name
        ));
        self.spinner.start();
        self.stack.set_visible_child_name("loading");
        self.start(
            entry,
            generation,
            PreviewMode::Full,
            Some(Rc::new(on_finished)),
        );
    }

    pub fn cancel_loading(&self) -> bool {
        if !self.state.loading.get() {
            return false;
        }
        self.cancel();
        self.title.set_label("Preview");
        self.metadata.set_label("Full preview cancelled");
        self.stack.set_visible_child_name("metadata");
        true
    }

    pub fn show_empty(&self) {
        self.cancel();
        self.title.set_label("Preview");
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
        self.state.loading.set(false);
        self.spinner.stop();
    }

    fn start(
        &self,
        entry: FileEntry,
        generation: Generation,
        mode: PreviewMode,
        on_finished: Option<Rc<dyn Fn(bool)>>,
    ) {
        if entry.kind == FileKind::Directory {
            self.state.has_rendered_content.set(true);
            self.stack.set_visible_child_name("directory");
            self.directory.load(&entry.location, |_| {});
            return;
        }

        if mode == PreviewMode::Automatic
            && let Some(content) = self.state.cache.borrow_mut().get(&entry)
        {
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
            adw::StyleManager::default().is_dark(),
            mode,
            move |result| {
                state.cancellable.borrow_mut().take();
                if !state.gate.borrow().accepts(result.generation) {
                    debug!(
                        generation = result.generation.value(),
                        "discarding stale preview result"
                    );
                    return;
                }
                if let Some(on_finished) = on_finished.as_ref() {
                    on_finished(result.content.is_ok());
                }
                state.loading.set(false);
                pane.spinner.stop();
                pane.title.set_label("Preview");
                if mode == PreviewMode::Automatic
                    && let Ok(content) = &result.content
                {
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
                    "\n\n[Fast plain-text preview truncated · Press u for full preview]"
                } else {
                    "\n\n[Fast plain-text preview · Press u for full syntax preview]"
                };
                let source = format!("{}{suffix}", preview.text);
                let buffer = self.text.buffer();
                buffer.set_text(&self.display_text(&source));
                self.apply_line_number_style(&buffer, &source);
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
        let source = format!("{}{suffix}", preview.text);
        buffer.set_text(&self.display_text(&source));

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
            self.apply_tag_safely(
                &buffer,
                tag,
                self.preview_offset(&source, span.start, false),
                self.preview_offset(&source, span.end, true),
            );
        }
        self.apply_line_number_style(&buffer, &source);
        self.text.set_buffer(Some(&buffer));
    }

    fn render_markdown(&self, preview: MarkdownPreview) {
        let dark = adw::StyleManager::default().is_dark();
        let buffer = gtk::TextBuffer::new(None);
        let suffix = if preview.truncated {
            "\n[Preview truncated]"
        } else {
            ""
        };
        let source = format!("{}{suffix}", preview.text);
        buffer.set_text(&self.display_text(&source));

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
                    .background_rgba(&if dark {
                        gdk::RGBA::new(0.20, 0.22, 0.26, 1.0)
                    } else {
                        gdk::RGBA::new(0.92, 0.93, 0.94, 1.0)
                    })
                    .foreground_rgba(&if dark {
                        gdk::RGBA::new(0.90, 0.90, 0.90, 1.0)
                    } else {
                        gdk::RGBA::new(0.16, 0.17, 0.19, 1.0)
                    })
                    .build(),
                MarkdownStyle::Link => builder
                    .foreground_rgba(&if dark {
                        gdk::RGBA::new(0.35, 0.65, 1.0, 1.0)
                    } else {
                        gdk::RGBA::new(0.12, 0.38, 0.78, 1.0)
                    })
                    .underline(gtk::pango::Underline::Single)
                    .build(),
            };
            buffer.tag_table().add(&tag);
            self.apply_tag_safely(
                &buffer,
                &tag,
                self.preview_offset(&source, span.start, false),
                self.preview_offset(&source, span.end, true),
            );
        }
        self.apply_line_number_style(&buffer, &source);
        self.text.set_buffer(Some(&buffer));
    }

    fn display_text(&self, text: &str) -> String {
        if self.show_line_numbers {
            add_line_numbers(text)
        } else {
            text.to_owned()
        }
    }

    fn preview_offset(&self, text: &str, offset: i32, at_end: bool) -> i32 {
        if self.show_line_numbers {
            numbered_offset(text, offset, at_end)
        } else {
            offset
        }
    }

    fn apply_line_number_style(&self, buffer: &gtk::TextBuffer, text: &str) {
        if !self.show_line_numbers {
            return;
        }
        let tag = gtk::TextTag::builder()
            .foreground_rgba(&if adw::StyleManager::default().is_dark() {
                gdk::RGBA::new(0.55, 0.57, 0.60, 1.0)
            } else {
                gdk::RGBA::new(0.45, 0.47, 0.50, 1.0)
            })
            .build();
        buffer.tag_table().add(&tag);
        let prefix = line_number_width(text) + 3;
        let mut position = 0_usize;
        for line in text.split_inclusive('\n') {
            self.apply_tag_safely(
                buffer,
                &tag,
                i32::try_from(position).unwrap_or(i32::MAX),
                i32::try_from(position.saturating_add(prefix)).unwrap_or(i32::MAX),
            );
            position = position.saturating_add(prefix + line.chars().count());
        }
    }

    fn apply_tag_safely(&self, buffer: &gtk::TextBuffer, tag: &gtk::TextTag, start: i32, end: i32) {
        let character_count = buffer.char_count().max(0);
        let start = start.clamp(0, character_count);
        let end = end.clamp(0, character_count);
        if start >= end {
            return;
        }
        buffer.apply_tag(
            tag,
            &buffer.iter_at_offset(start),
            &buffer.iter_at_offset(end),
        );
    }
}

fn line_number_width(text: &str) -> usize {
    text.lines().count().max(1).to_string().len()
}

fn add_line_numbers(text: &str) -> String {
    let width = line_number_width(text);
    text.split_inclusive('\n')
        .enumerate()
        .map(|(index, line)| format!("{:>width$} │ {line}", index + 1))
        .collect()
}

fn numbered_offset(text: &str, offset: i32, at_end: bool) -> i32 {
    let offset = offset.max(0) as usize;
    let lines_before = text
        .chars()
        .take(offset)
        .filter(|character| *character == '\n')
        .count();
    let ends_at_newline = at_end && offset > 0 && text.chars().nth(offset - 1) == Some('\n');
    let prefixes = lines_before + 1 - usize::from(ends_at_newline);
    i32::try_from(offset.saturating_add(prefixes * (line_number_width(text) + 3)))
        .unwrap_or(i32::MAX)
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
    let archive = entry
        .archive_format
        .as_ref()
        .map_or_else(String::new, |format| {
            format!("\nArchive: {format} — press l to browse contents")
        });
    format!(
        "{}\n\nType: {}{archive}\nSize: {}\nURI: {}\n\n{status}",
        entry.display_name,
        entry.content_type.as_deref().unwrap_or("unknown"),
        entry
            .size
            .map_or_else(|| "unknown".to_owned(), |value| format!("{value} bytes")),
        entry.location.uri()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_numbers_preserve_line_boundaries_for_styled_offsets() {
        let source = "a\nb";
        assert_eq!(add_line_numbers(source), "1 │ a\n2 │ b");
        assert_eq!(numbered_offset(source, 0, false), 4);
        assert_eq!(numbered_offset(source, 2, true), 6);
        assert_eq!(numbered_offset(source, 2, false), 10);
    }

    #[test]
    fn numbered_offsets_stay_inside_unicode_display_text() {
        let source = "αβ\n🙂 text\nlast";
        let displayed = add_line_numbers(source);
        let character_count = displayed.chars().count() as i32;
        for offset in 0..=source.chars().count() as i32 {
            for at_end in [false, true] {
                let mapped = numbered_offset(source, offset, at_end);
                assert!((0..=character_count).contains(&mapped));
            }
        }
    }
}
