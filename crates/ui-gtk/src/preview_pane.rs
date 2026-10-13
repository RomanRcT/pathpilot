use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};

use gtk::{gdk, gio, glib, prelude::*};
use pathpilot_core::{FileEntry, FileKind, Generation, SortMode};
use pathpilot_preview::document::{DocumentPreview, load_document_page};
use pathpilot_preview::{
    PreviewCache, PreviewContent, PreviewGate, PreviewLimits, PreviewMode, PreviewResult,
    StyledTextPreview, load_preview,
};
use tracing::{debug, info};
use vte::prelude::*;

use crate::directory_pane::{DirectoryPane, SharedDirectoryCache};
use crate::markdown_view::MarkdownView;

const MAX_GTK_STYLE_SPANS: usize = 8_000;
const MAX_GTK_LINE_NUMBER_TAGS: usize = 2_000;
const MIN_REMOTE_DIRECTORY_PREVIEW_DELAY: Duration = Duration::from_millis(350);

struct PreviewState {
    gate: RefCell<PreviewGate>,
    delay: RefCell<Option<glib::SourceId>>,
    cancellable: RefCell<Option<gio::Cancellable>>,
    cache: RefCell<PreviewCache>,
    has_rendered_content: Cell<bool>,
    loading: Cell<bool>,
    document: RefCell<Option<DocumentPreview>>,
    rendered_entry: RefCell<Option<FileEntry>>,
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
            document: RefCell::new(None),
            rendered_entry: RefCell::new(None),
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
    markdown: MarkdownView,
    preview_only: Rc<Cell<bool>>,
    document_navigation: gtk::Label,
    spinner: gtk::Spinner,
    loading_label: gtk::Label,
    show_line_numbers: bool,
    preview_delay: Duration,
    state: Rc<PreviewState>,
}

impl PreviewPane {
    pub fn new(
        show_line_numbers: bool,
        preview_delay_ms: u64,
        markdown_theme: &str,
        remote_cache: SharedDirectoryCache,
    ) -> Self {
        let directory = DirectoryPane::with_remote_cache("Directory preview", remote_cache);
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
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
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
            // Hidden text/directory pages must not determine the document viewport.
            .hhomogeneous(false)
            .vhomogeneous(false)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(100)
            .build();
        let terminal = vte::Terminal::new();
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .width_request(64)
            .height_request(64)
            .visible(true)
            .build();
        spinner.add_css_class("preview-spinner");
        let loading_label = gtk::Label::builder()
            .label("Loading full preview…\nPress Escape to cancel")
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            .build();
        loading_label.add_css_class("dim-label");
        let loading_page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        loading_page.append(&spinner);
        loading_page.append(&loading_label);
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.set_scrollback_lines(10_000);
        stack.add_named(&metadata, Some("metadata"));
        stack.add_named(&text_scroller, Some("text"));
        stack.add_named(&picture, Some("image"));
        stack.add_named(&directory.widget, Some("directory"));
        stack.add_named(&terminal, Some("editor"));
        stack.add_named(&loading_page, Some("loading"));
        let markdown = MarkdownView::new(markdown_theme);
        stack.add_named(markdown.widget(), Some("markdown"));

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
        let document_navigation = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .margin_top(6)
            .margin_bottom(6)
            .visible(false)
            .build();
        document_navigation.add_css_class("dim-label");
        widget.append(&document_navigation);

        Self {
            widget,
            stack,
            title,
            terminal,
            directory,
            metadata,
            text,
            picture,
            markdown,
            preview_only: Rc::new(Cell::new(false)),
            document_navigation,
            spinner,
            loading_label,
            show_line_numbers,
            preview_delay: Duration::from_millis(preview_delay_ms),
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

    pub fn focus_editor(&self) {
        self.terminal.grab_focus();
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

    pub fn snapshot(&self, entry: &FileEntry) -> Option<PreviewContent> {
        if self.state.rendered_entry.borrow().as_ref() == Some(entry)
            && let Some(document) = self.state.document.borrow().clone()
        {
            return Some(PreviewContent::Document(document));
        }
        self.state.cache.borrow_mut().get(entry)
    }

    pub fn restore_content(&self, entry: &FileEntry, content: PreviewContent) {
        self.cancel();
        let generation = self.state.gate.borrow_mut().begin();
        self.state.cache.borrow_mut().insert(entry, content.clone());
        self.render(
            entry,
            PreviewResult {
                generation,
                content: Ok(content),
            },
        );
        self.title.set_label("Preview");
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
        self.state.loading.set(true);
        self.loading_label.set_label("Loading preview…");
        self.stack.set_visible_child_name("loading");
        self.spinner.start();

        let delay = if entry.kind == FileKind::Directory
            && !gio::File::for_uri(entry.location.uri()).is_native()
        {
            self.preview_delay.max(MIN_REMOTE_DIRECTORY_PREVIEW_DELAY)
        } else {
            self.preview_delay
        };
        let pane = self.clone();
        let state = self.state.clone();
        let source = glib::timeout_add_local_once(delay, move || {
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
        self.stack.set_visible_child_name("loading");
        self.loading_label
            .set_label("Loading full preview…\nPress Escape to cancel");
        self.spinner.start();
        self.spinner.queue_draw();
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

    pub fn show_location_loading(&self, location: &str) {
        self.cancel();
        self.title.set_label("Opening location");
        self.loading_label
            .set_label(&format!("Loading folder…\n{location}"));
        self.stack.set_visible_child_name("loading");
        self.spinner.start();
    }

    pub fn show_location_error(&self, error: &str) {
        self.cancel();
        self.title.set_label("Could not open location");
        self.metadata.set_label(error);
        self.stack.set_visible_child_name("metadata");
    }

    pub fn cancel(&self) {
        self.markdown.stop_loading();
        self.state.document.borrow_mut().take();
        self.document_navigation.set_visible(false);
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
            let pane = self.clone();
            let state = self.state.clone();
            let used_cache = self.directory.load(&entry.location, move |_| {
                if state.gate.borrow().accepts(generation) {
                    state.loading.set(false);
                    pane.spinner.stop();
                }
            });
            if used_cache {
                self.state.loading.set(false);
                self.spinner.stop();
            }
            return;
        }

        if (mode == PreviewMode::Automatic || pathpilot_preview::document::is_document(&entry))
            && let Some(content) = self.state.cache.borrow_mut().get(&entry)
        {
            self.render(
                &entry,
                PreviewResult {
                    generation,
                    content: Ok(content),
                },
            );
            self.state.loading.set(false);
            self.spinner.stop();
            self.title.set_label(match mode {
                PreviewMode::Automatic => "Preview",
                PreviewMode::Full => "Full preview",
            });
            if let Some(on_finished) = on_finished.as_ref() {
                on_finished(true);
            }
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
                if !state.gate.borrow().accepts(result.generation) {
                    debug!(
                        generation = result.generation.value(),
                        "discarding stale preview result"
                    );
                    return;
                }
                state.cancellable.borrow_mut().take();
                let success = result.content.is_ok();
                if (mode == PreviewMode::Automatic
                    || pathpilot_preview::document::is_document(&render_entry))
                    && let Ok(content) = &result.content
                {
                    state
                        .cache
                        .borrow_mut()
                        .insert(&render_entry, content.clone());
                }
                pane.render(&render_entry, result);
                state.loading.set(false);
                pane.spinner.stop();
                pane.title.set_label(match mode {
                    PreviewMode::Automatic => "Preview",
                    PreviewMode::Full => "Full preview",
                });
                if let Some(on_finished) = on_finished.as_ref() {
                    on_finished(success);
                }
            },
        );
        *self.state.cancellable.borrow_mut() = Some(cancellable);
    }

    fn render(&self, entry: &FileEntry, result: PreviewResult) {
        self.state.has_rendered_content.set(true);
        *self.state.rendered_entry.borrow_mut() = Some(entry.clone());
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
                let buffer = gtk::TextBuffer::new(None);
                buffer.set_text(&self.display_text(&source));
                self.apply_line_number_style(&buffer, &source);
                self.text.set_buffer(Some(&buffer));
                info!(
                    file = entry.display_name,
                    source_chars = source.chars().count(),
                    buffer_chars = buffer.char_count(),
                    "plain preview buffer installed"
                );
                self.stack.set_visible_child_name("text");
            }
            Ok(PreviewContent::StyledText(preview)) => {
                self.render_styled_text(entry, preview);
                self.stack.set_visible_child_name("text");
            }
            Ok(PreviewContent::Markdown(preview)) => {
                self.markdown.render(preview, &entry.location);
                self.stack.set_visible_child_name("markdown");
            }
            Ok(PreviewContent::Image(preview)) => {
                let texture = gdk::Texture::for_pixbuf(&preview.pixbuf);
                self.picture.set_paintable(Some(&texture));
                self.stack.set_visible_child_name("image");
            }
            Ok(PreviewContent::Document(preview)) => {
                self.render_document(preview);
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
        if self.preview_only.get() && self.widget.is_mapped() {
            self.focus_content();
        }
    }

    fn render_document(&self, preview: DocumentPreview) {
        self.picture
            .set_paintable(Some(&gdk::Texture::for_pixbuf(&preview.pixbuf)));
        let location = preview
            .session
            .page_label(preview.page_number)
            .map_or_else(String::new, |label| format!("{label} · "));
        self.document_navigation.set_label(&format!(
            "{location}Page {} / {} · n next · N previous",
            preview.page_number, preview.session.page_count,
        ));
        self.document_navigation.set_visible(true);
        *self.state.document.borrow_mut() = Some(preview);
        self.stack.set_visible_child_name("image");
    }

    pub fn change_document_page(&self, forward: bool) -> bool {
        let Some(document) = self.state.document.borrow().clone() else {
            return false;
        };
        if self.state.loading.get() {
            return true;
        }
        let page = if forward {
            document.page_number.saturating_add(1)
        } else {
            document.page_number.saturating_sub(1)
        };
        if page == 0 || page > document.session.page_count {
            return true;
        }
        if let Some(cancellable) = self.state.cancellable.borrow_mut().take() {
            cancellable.cancel();
        }
        let generation = self.state.gate.borrow_mut().begin();
        self.state.loading.set(true);
        self.document_navigation
            .set_label(&format!("Loading page {page}… · Escape cancels"));
        let pane = self.clone();
        let cancellable = load_document_page(document.session, page, generation, move |result| {
            if !pane.state.gate.borrow().accepts(result.generation) {
                return;
            }
            pane.state.cancellable.borrow_mut().take();
            pane.state.loading.set(false);
            match result.content {
                Ok(PreviewContent::Document(document)) => pane.render_document(document),
                Err(error) => pane.document_navigation.set_label(&error),
                _ => {}
            }
        });
        *self.state.cancellable.borrow_mut() = Some(cancellable);
        true
    }

    fn render_styled_text(&self, entry: &FileEntry, preview: StyledTextPreview) {
        if preview.spans.len() > MAX_GTK_STYLE_SPANS {
            let source_char_count = preview.text.chars().count();
            let span_count = preview.spans.len();
            let banner = if preview.truncated {
                format!(
                    "[FULL PREVIEW TRUNCATED · {source_char_count} characters · syntax colors omitted for stability]\n\n"
                )
            } else {
                format!(
                    "[FULL PREVIEW · complete file · {source_char_count} characters · syntax colors omitted for stability]\n\n"
                )
            };
            let source = format!("{banner}{}", preview.text);
            let buffer = gtk::TextBuffer::new(None);
            buffer.set_text(&self.display_text(&source));
            self.apply_line_number_style(&buffer, &source);
            self.text.set_buffer(Some(&buffer));
            info!(
                file = entry.display_name,
                source_chars = source_char_count,
                buffer_chars = buffer.char_count(),
                span_count,
                "full preview buffer installed without syntax styling"
            );
            return;
        }
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
        info!(
            file = entry.display_name,
            source_chars = source.chars().count(),
            buffer_chars = buffer.char_count(),
            "styled preview buffer installed"
        );
    }

    pub fn set_preview_only(&self, enabled: bool) {
        self.preview_only.set(enabled);
        if enabled {
            self.focus_content();
        }
    }

    pub fn focus_content(&self) {
        match self.stack.visible_child_name().as_deref() {
            Some("markdown") => self.markdown.focus(),
            Some("text") => {
                self.text.grab_focus();
            }
            _ => {
                self.widget.set_focusable(true);
                self.widget.grab_focus();
            }
        }
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
        if !self.show_line_numbers || text.lines().count() > MAX_GTK_LINE_NUMBER_TAGS {
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

/// Shared document paging keys for the browser and independent viewer windows.
pub(super) fn document_page_direction(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<bool> {
    let modifiers = modifiers
        & (gdk::ModifierType::SHIFT_MASK
            | gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::META_MASK);
    match (key, modifiers) {
        (gdk::Key::Page_Up, gdk::ModifierType::CONTROL_MASK) => Some(false),
        (gdk::Key::Page_Down, gdk::ModifierType::CONTROL_MASK) => Some(true),
        (gdk::Key::n | gdk::Key::bracketright, modifiers) if modifiers.is_empty() => Some(true),
        (gdk::Key::N, modifiers)
            if modifiers.is_empty() || modifiers == gdk::ModifierType::SHIFT_MASK =>
        {
            Some(false)
        }
        (gdk::Key::bracketleft, modifiers) if modifiers.is_empty() => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a graphical session; run this test with --ignored --test-threads=1"]
    fn document_viewport_ignores_hidden_content_and_fits_resized_panes() {
        adw::init().unwrap();
        let pane = PreviewPane::new(
            false,
            0,
            "compact",
            Rc::new(RefCell::new(Default::default())),
        );
        pane.metadata
            .set_text(&"very-long-unbroken-path".repeat(100));
        pane.text.buffer().set_text(&"long source line".repeat(500));
        pane.stack.set_visible_child_name("image");
        pane.stack.set_transition_duration(0);
        let window = gtk::Window::builder()
            .default_width(900)
            .default_height(600)
            .build();
        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.set_start_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));
        split.set_end_child(Some(&pane.widget));
        window.set_child(Some(&split));
        window.present();
        let context = glib::MainContext::default();
        for (position, width, height) in [(200, 800, 1200), (600, 1200, 800), (400, 800, 1200)] {
            let pixbuf = gtk::gdk_pixbuf::Pixbuf::new(
                gtk::gdk_pixbuf::Colorspace::Rgb,
                false,
                8,
                width,
                height,
            )
            .unwrap();
            pixbuf.fill(0xffffffff);
            pane.picture
                .set_paintable(Some(&gdk::Texture::for_pixbuf(&pixbuf)));
            split.set_position(position);
            let deadline = std::time::Instant::now() + Duration::from_millis(150);
            while std::time::Instant::now() < deadline {
                while context.pending() {
                    context.iteration(false);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(pane.stack.measure(gtk::Orientation::Horizontal, -1).0 < 200);
            let viewport = pane.stack.compute_bounds(&window).unwrap();
            let picture = pane.picture.compute_bounds(&window).unwrap();
            assert!(picture.x() >= viewport.x());
            assert!(picture.x() + picture.width() <= viewport.x() + viewport.width() + 1.0);
            assert!(picture.y() >= viewport.y());
            assert!(picture.y() + picture.height() <= viewport.y() + viewport.height() + 1.0);
            assert!(
                (picture.x() + picture.width() / 2.0 - viewport.x() - viewport.width() / 2.0).abs()
                    < 1.0
            );
        }
        window.close();
    }

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
