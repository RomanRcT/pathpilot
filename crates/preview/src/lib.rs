//! Bounded, cancelable preview loading without GTK widgets.

pub mod document;
mod native_document;

use std::{
    collections::VecDeque,
    rc::Rc,
    sync::{OnceLock, mpsc},
    time::Duration,
};

use gio::prelude::*;
use pathpilot_core::{FileEntry, Generation, GenerationTracker, Location};
use tracing::{debug, warn};

#[derive(Clone, Copy, Debug)]
pub struct PreviewLimits {
    pub max_text_bytes: usize,
    pub max_rich_text_bytes: usize,
    pub image_width: i32,
    pub image_height: i32,
}

impl Default for PreviewLimits {
    fn default() -> Self {
        Self {
            max_text_bytes: 256 * 1024,
            max_rich_text_bytes: 24 * 1024,
            image_width: 1200,
            image_height: 1200,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PreviewMode {
    #[default]
    Automatic,
    Full,
}

#[derive(Clone, Debug)]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub struct ImagePreview {
    pub pixbuf: gdk_pixbuf::Pixbuf,
}

#[derive(Clone, Debug)]
pub struct StyledSpan {
    pub start: i32,
    pub end: i32,
    pub foreground: (u8, u8, u8),
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Debug)]
pub struct StyledTextPreview {
    pub text: String,
    pub spans: Vec<StyledSpan>,
    pub background: (u8, u8, u8),
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub struct MarkdownPreview {
    pub html: String,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub enum PreviewContent {
    Text(TextPreview),
    StyledText(StyledTextPreview),
    Markdown(MarkdownPreview),
    Image(ImagePreview),
    Document(document::DocumentPreview),
    Unsupported,
}

#[derive(Debug)]
pub struct PreviewResult {
    pub generation: Generation,
    pub content: Result<PreviewContent, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviewCacheKey {
    uri: String,
    size: Option<u64>,
    modified: Option<std::time::SystemTime>,
}

#[derive(Debug)]
pub struct PreviewCache {
    capacity: usize,
    entries: VecDeque<(PreviewCacheKey, PreviewContent)>,
}

impl PreviewCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    pub fn get(&mut self, entry: &FileEntry) -> Option<PreviewContent> {
        let key = PreviewCacheKey::from(entry);
        let position = self
            .entries
            .iter()
            .position(|(candidate, _)| *candidate == key)?;
        let cached = self.entries.remove(position)?;
        let result = cached.1.clone();
        self.entries.push_back(cached);
        Some(result)
    }

    pub fn insert(&mut self, entry: &FileEntry, content: PreviewContent) {
        if self.capacity == 0 {
            return;
        }
        if let PreviewContent::Document(document) = &content
            && (entry.size.is_none()
                || entry.modified.is_none()
                || document.session.retained_bytes
                    + document.pixbuf.byte_length() as u64
                    + 16 * 1024 * 1024
                    > 128 * 1024 * 1024)
        {
            return;
        }
        let key = PreviewCacheKey::from(entry);
        if let Some(position) = self
            .entries
            .iter()
            .position(|(candidate, _)| *candidate == key)
        {
            self.entries.remove(position);
        }
        self.entries.push_back((key, content));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
        // Documents retain converted PDFs and a small page cache. Bound both
        // their count and storage, independently of the lightweight text cache.
        loop {
            let mut count = 0;
            let mut bytes = 0;
            for (_, content) in &self.entries {
                if let PreviewContent::Document(document) = content {
                    count += 1;
                    bytes += document.session.retained_bytes
                        + document.pixbuf.byte_length() as u64
                        + 16 * 1024 * 1024;
                }
            }
            if count <= 4 && bytes <= 128 * 1024 * 1024 {
                break;
            }
            let oldest = self
                .entries
                .iter()
                .position(|(_, content)| matches!(content, PreviewContent::Document(_)))
                .expect("document budget exceeded");
            self.entries.remove(oldest);
        }
    }

    pub fn invalidate(&mut self, location: &Location) {
        self.entries.retain(|(key, _)| key.uri != location.uri());
    }
}

impl From<&FileEntry> for PreviewCacheKey {
    fn from(entry: &FileEntry) -> Self {
        Self {
            uri: entry.location.uri().to_owned(),
            size: entry.size,
            modified: entry.modified,
        }
    }
}

enum ProcessedText {
    Styled(StyledTextPreview),
    Markdown(MarkdownPreview),
}

#[derive(Debug, Default)]
pub struct PreviewGate {
    generations: GenerationTracker,
}

impl PreviewGate {
    pub fn begin(&mut self) -> Generation {
        self.generations.advance()
    }

    pub fn accepts(&self, generation: Generation) -> bool {
        self.generations.accepts(generation)
    }
}

pub fn load_preview(
    entry: &FileEntry,
    generation: Generation,
    limits: PreviewLimits,
    dark: bool,
    mode: PreviewMode,
    on_result: impl Fn(PreviewResult) + 'static,
) -> gio::Cancellable {
    let cancellable = gio::Cancellable::new();
    let on_result: Rc<dyn Fn(PreviewResult)> = Rc::new(on_result);
    let content_type = entry.content_type.as_deref().unwrap_or_default();

    if entry.size == Some(0) {
        on_result(PreviewResult {
            generation,
            content: Ok(PreviewContent::Text(TextPreview {
                text: String::new(),
                truncated: false,
            })),
        });
    } else if document::is_document(entry) {
        document::load_document(entry, generation, limits, &cancellable, on_result);
    } else if content_type.starts_with("image/") {
        load_image(entry, generation, limits, &cancellable, on_result);
    } else {
        load_text(
            entry,
            generation,
            limits,
            dark,
            mode,
            &cancellable,
            on_result,
        );
    }
    cancellable
}

fn load_text(
    entry: &FileEntry,
    generation: Generation,
    limits: PreviewLimits,
    dark: bool,
    mode: PreviewMode,
    cancellable: &gio::Cancellable,
    on_result: Rc<dyn Fn(PreviewResult)>,
) {
    let file = gio::File::for_uri(entry.location.uri());
    let display_name = entry.display_name.clone();
    let original_entry = entry.clone();
    let sniff = !is_text_content_type(entry.content_type.as_deref().unwrap_or_default());
    if mode == PreviewMode::Full && !sniff {
        let full_cancellable = cancellable.clone();
        file.load_contents_async(Some(cancellable), move |result| match result {
            Ok((bytes, _)) => {
                if bytes.contains(&0) {
                    on_result(PreviewResult {
                        generation,
                        content: Err("Binary data cannot be shown as text".to_owned()),
                    });
                    return;
                }
                highlight_async(
                    String::from_utf8_lossy(&bytes).into_owned(),
                    false,
                    display_name,
                    generation,
                    dark,
                    &full_cancellable,
                    on_result,
                );
            }
            Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {
                debug!(
                    generation = generation.value(),
                    "full text preview cancelled"
                );
            }
            Err(error) => publish_error(generation, error, &on_result),
        });
        return;
    }
    let callback_cancellable = cancellable.clone();
    file.read_async(
        glib::Priority::LOW,
        Some(cancellable),
        move |result| match result {
            Ok(stream) => {
                let highlight_cancellable = callback_cancellable.clone();
                let byte_limit = limits.max_text_bytes;
                stream.read_bytes_async(
                    byte_limit.saturating_add(1),
                    glib::Priority::LOW,
                    Some(&callback_cancellable),
                    move |result| match result {
                        Ok(bytes) => {
                            let bytes = bytes.as_ref();
                            if sniff && !looks_like_text(bytes, bytes.len() > byte_limit) {
                                on_result(PreviewResult {
                                    generation,
                                    content: Ok(PreviewContent::Unsupported),
                                });
                                return;
                            }
                            if bytes.contains(&0) {
                                on_result(PreviewResult {
                                    generation,
                                    content: Err("Binary data cannot be shown as text".to_owned()),
                                });
                                return;
                            }
                            if sniff && mode == PreviewMode::Full {
                                let mut text_entry = original_entry.clone();
                                text_entry.content_type = Some("text/plain".to_owned());
                                load_text(
                                    &text_entry,
                                    generation,
                                    limits,
                                    dark,
                                    mode,
                                    &highlight_cancellable,
                                    on_result,
                                );
                                return;
                            }
                            let truncated = bytes.len() > byte_limit;
                            let visible = &bytes[..bytes.len().min(byte_limit)];
                            let visible = match std::str::from_utf8(visible) {
                                Err(error) if truncated && error.error_len().is_none() => {
                                    &visible[..error.valid_up_to()]
                                }
                                _ => visible,
                            };
                            // The syntax-highlighting budget must not turn Markdown
                            // into plain text. Its automatic HTML preview remains
                            // bounded by the general text read limit instead.
                            let rich = is_markdown_name(&display_name)
                                || visible.len() <= limits.max_rich_text_bytes;
                            if !rich {
                                on_result(PreviewResult {
                                    generation,
                                    content: Ok(PreviewContent::Text(TextPreview {
                                        text: String::from_utf8_lossy(visible).into_owned(),
                                        truncated,
                                    })),
                                });
                                return;
                            }
                            highlight_async(
                                String::from_utf8_lossy(visible).into_owned(),
                                truncated,
                                display_name,
                                generation,
                                dark,
                                &highlight_cancellable,
                                on_result,
                            );
                        }
                        Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {
                            debug!(generation = generation.value(), "text preview cancelled");
                        }
                        Err(error) => publish_error(generation, error, &on_result),
                    },
                );
            }
            Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {
                debug!(
                    generation = generation.value(),
                    "text preview open cancelled"
                );
            }
            Err(error) => publish_error(generation, error, &on_result),
        },
    );
}

fn load_image(
    entry: &FileEntry,
    generation: Generation,
    limits: PreviewLimits,
    cancellable: &gio::Cancellable,
    on_result: Rc<dyn Fn(PreviewResult)>,
) {
    let file = gio::File::for_uri(entry.location.uri());
    let Some(path) = file.path() else {
        publish_message(
            generation,
            "Image preview requires a local file",
            &on_result,
        );
        return;
    };
    let info_cancellable = cancellable.clone();
    gdk_pixbuf::Pixbuf::file_info_async(path, Some(cancellable), move |result| match result {
        Ok(Some((_, width, height))) => {
            let dimensions = fit_dimensions(width, height, limits.image_width, limits.image_height);
            decode_image(file, dimensions, generation, info_cancellable, on_result);
        }
        Ok(None) => publish_message(generation, "Unsupported image format", &on_result),
        Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {
            debug!(generation = generation.value(), "image info cancelled");
        }
        Err(error) => publish_error(generation, error, &on_result),
    });
}

fn decode_image(
    file: gio::File,
    dimensions: (i32, i32),
    generation: Generation,
    cancellable: gio::Cancellable,
    on_result: Rc<dyn Fn(PreviewResult)>,
) {
    let callback_cancellable = cancellable.clone();
    file.read_async(
        glib::Priority::LOW,
        Some(&cancellable),
        move |result| match result {
            Ok(stream) => gdk_pixbuf::Pixbuf::from_stream_at_scale_async(
                &stream,
                dimensions.0,
                dimensions.1,
                true,
                Some(&callback_cancellable),
                move |result| match result {
                    Ok(pixbuf) => on_result(PreviewResult {
                        generation,
                        content: Ok(PreviewContent::Image(ImagePreview { pixbuf })),
                    }),
                    Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {
                        debug!(generation = generation.value(), "image preview cancelled");
                    }
                    Err(error) => publish_error(generation, error, &on_result),
                },
            ),
            Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {
                debug!(
                    generation = generation.value(),
                    "image preview open cancelled"
                );
            }
            Err(error) => publish_error(generation, error, &on_result),
        },
    );
}

fn fit_dimensions(width: i32, height: i32, max_width: i32, max_height: i32) -> (i32, i32) {
    if width <= 0 || height <= 0 || max_width <= 0 || max_height <= 0 {
        return (max_width.max(1), max_height.max(1));
    }
    let scale = (f64::from(max_width) / f64::from(width))
        .min(f64::from(max_height) / f64::from(height))
        .min(1.0);
    (
        (f64::from(width) * scale).round().max(1.0) as i32,
        (f64::from(height) * scale).round().max(1.0) as i32,
    )
}

fn publish_error(
    generation: Generation,
    error: glib::Error,
    on_result: &Rc<dyn Fn(PreviewResult)>,
) {
    warn!(generation = generation.value(), %error, "preview load failed");
    on_result(PreviewResult {
        generation,
        content: Err(error.to_string()),
    });
}

fn publish_message(generation: Generation, message: &str, on_result: &Rc<dyn Fn(PreviewResult)>) {
    on_result(PreviewResult {
        generation,
        content: Err(message.to_owned()),
    });
}

fn highlight_async(
    text: String,
    truncated: bool,
    display_name: String,
    generation: Generation,
    dark: bool,
    cancellable: &gio::Cancellable,
    on_result: Rc<dyn Fn(PreviewResult)>,
) {
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_cancellable = cancellable.clone();
    std::thread::spawn(move || {
        let result = if is_markdown_name(&display_name) {
            render_markdown(text, truncated, &worker_cancellable).map(ProcessedText::Markdown)
        } else {
            highlight_text(text, truncated, &display_name, dark, &worker_cancellable)
                .map(ProcessedText::Styled)
        };
        let _ = sender.send(result);
    });

    let poll_cancellable = cancellable.clone();
    glib::timeout_add_local(Duration::from_millis(5), move || {
        if poll_cancellable.is_cancelled() {
            return glib::ControlFlow::Break;
        }
        match receiver.try_recv() {
            Ok(Ok(processed)) => {
                let content = match processed {
                    ProcessedText::Styled(preview) => PreviewContent::StyledText(preview),
                    ProcessedText::Markdown(preview) => PreviewContent::Markdown(preview),
                };
                on_result(PreviewResult {
                    generation,
                    content: Ok(content),
                });
                glib::ControlFlow::Break
            }
            Ok(Err(message)) => {
                on_result(PreviewResult {
                    generation,
                    content: Err(message),
                });
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn highlight_text(
    text: String,
    truncated: bool,
    display_name: &str,
    dark: bool,
    cancellable: &gio::Cancellable,
) -> Result<StyledTextPreview, String> {
    use syntect::{
        easy::HighlightLines,
        highlighting::{FontStyle, ThemeSet},
        parsing::SyntaxSet,
        util::LinesWithEndings,
    };

    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();
    let syntax_set = SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines);
    let theme_set = THEME_SET.get_or_init(ThemeSet::load_defaults);
    let syntax = syntax_set
        .find_syntax_for_file(display_name)
        .ok()
        .flatten()
        .or_else(|| syntax_set.find_syntax_by_first_line(&text))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let theme = &theme_set.themes[if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    }];
    let background = theme
        .settings
        .background
        .map_or(if dark { (43, 48, 59) } else { (255, 255, 255) }, |color| {
            (color.r, color.g, color.b)
        });
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut spans = Vec::new();
    let mut char_offset = 0_i32;

    for line in LinesWithEndings::from(&text) {
        if cancellable.is_cancelled() {
            return Err("Preview cancelled".to_owned());
        }
        let ranges = highlighter
            .highlight_line(line, syntax_set)
            .map_err(|error| error.to_string())?;
        for (style, fragment) in ranges {
            let length = i32::try_from(fragment.chars().count()).unwrap_or(i32::MAX);
            let end = char_offset.saturating_add(length);
            spans.push(StyledSpan {
                start: char_offset,
                end,
                foreground: (style.foreground.r, style.foreground.g, style.foreground.b),
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
                underline: style.font_style.contains(FontStyle::UNDERLINE),
            });
            char_offset = end;
        }
    }

    Ok(StyledTextPreview {
        text,
        spans,
        background,
        truncated,
    })
}

fn is_markdown_name(display_name: &str) -> bool {
    display_name.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown")
    })
}

fn render_markdown(
    source: String,
    truncated: bool,
    cancellable: &gio::Cancellable,
) -> Result<MarkdownPreview, String> {
    use pulldown_cmark::{Event, Options, Parser, Tag};

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let mut events: Vec<_> = Parser::new_ext(&source, options)
        .take_while(|_| !cancellable.is_cancelled())
        .filter_map(|event| match event {
            Event::Html(_) | Event::InlineHtml(_) => None,
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => Some(Event::Start(Tag::Link {
                link_type,
                dest_url: if safe_markdown_url(&dest_url, false) {
                    dest_url
                } else {
                    "".into()
                },
                title,
                id,
            })),
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => Some(Event::Start(Tag::Image {
                link_type,
                dest_url: if safe_markdown_url(&dest_url, true) {
                    dest_url
                } else {
                    "".into()
                },
                title,
                id,
            })),
            event => Some(event),
        })
        .collect();
    if cancellable.is_cancelled() {
        return Err("Preview cancelled".to_owned());
    }
    add_markdown_heading_ids(&mut events);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    Ok(MarkdownPreview { html, truncated })
}

fn add_markdown_heading_ids(events: &mut [pulldown_cmark::Event<'_>]) {
    use pulldown_cmark::{Event, Tag, TagEnd};
    let mut used = std::collections::HashSet::new();
    // Explicit ids take precedence even when they occur later in the document.
    for event in events.iter() {
        if let Event::Start(Tag::Heading { id: Some(id), .. }) = event {
            used.insert(id.to_string());
        }
    }
    for index in 0..events.len() {
        if !matches!(events[index], Event::Start(Tag::Heading { id: None, .. })) {
            continue;
        }
        let text = events[index + 1..]
            .iter()
            .take_while(|event| !matches!(event, Event::End(TagEnd::Heading(_))))
            .filter_map(|event| match event {
                Event::Text(value) | Event::Code(value) => Some(value.as_ref()),
                _ => None,
            })
            .collect::<String>();
        let slug: String = text
            .trim()
            .to_lowercase()
            .chars()
            .filter_map(|character| {
                if character.is_whitespace() {
                    Some('-')
                } else if character.is_alphanumeric() || matches!(character, '-' | '_') {
                    Some(character)
                } else {
                    None
                }
            })
            .collect();
        let slug = if slug.is_empty() {
            "section".to_owned()
        } else {
            slug
        };
        let mut id = slug.clone();
        let mut suffix = 1;
        while !used.insert(id.clone()) {
            id = format!("{slug}-{suffix}");
            suffix += 1;
        }
        if let Event::Start(Tag::Heading { id: heading_id, .. }) = &mut events[index] {
            *heading_id = Some(id.into());
        }
    }
}

fn safe_markdown_url(url: &str, image: bool) -> bool {
    if url.chars().any(char::is_control) {
        return false;
    }
    let url = url.trim();
    let Some((scheme, _)) = url.split_once(':') else {
        return true;
    };
    // A colon after a slash belongs to a relative path, not a URI scheme.
    if scheme.contains(['/', '?', '#']) {
        return true;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" | "file" => true,
        "mailto" => !image,
        "data" => image && url.to_ascii_lowercase().starts_with("data:image/"),
        _ => false,
    }
}

// Unknown MIME types must be valid UTF-8 without binary control bytes.
// A bounded read may end halfway through the last UTF-8 character.
fn looks_like_text(bytes: &[u8], may_be_truncated: bool) -> bool {
    if bytes
        .iter()
        .any(|byte| matches!(byte, 0..=8 | 11 | 14..=31 | 127))
    {
        return false;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(error) => may_be_truncated && error.error_len().is_none(),
    }
}

fn is_text_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || matches!(
            content_type,
            "application/json"
                | "application/toml"
                | "application/xml"
                | "application/javascript"
                | "application/yaml"
                | "application/x-yaml"
                | "application/x-shellscript"
        )
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, fs, rc::Rc, sync::Mutex};

    use super::*;
    use pathpilot_core::{FileKind, GenerationTracker, Location};

    pub(super) fn async_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn unknown_text_detection_rejects_binary_and_handles_utf8_boundaries() {
        assert!(looks_like_text("Привет\nhello\t世界".as_bytes(), false));
        assert!(!looks_like_text(b"header\0binary", false));
        assert!(!looks_like_text(b"\x01\x02some binary without NUL", false));
        assert!(!looks_like_text(b"\xff\xfe", false));
        assert!(looks_like_text(b"text \xe2\x82", true));
        assert!(!looks_like_text(b"text \xe2\x82", false));
        assert!(!looks_like_text(b"text \xe2!", true));
    }

    #[test]
    fn unknown_mime_files_are_sniffed_and_full_text_can_be_loaded() {
        let _guard = async_test_guard();
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("unknown.data");
        for (mime, source, mode, expected) in [
            (
                Some("application/octet-stream"),
                "Привет\nhello".as_bytes(),
                PreviewMode::Automatic,
                true,
            ),
            (
                Some("application/x-custom"),
                b"header\tvalue\n".as_slice(),
                PreviewMode::Automatic,
                true,
            ),
            (None, b"no mime\n".as_slice(), PreviewMode::Full, true),
            (
                None,
                b"\xff\x01binary".as_slice(),
                PreviewMode::Automatic,
                false,
            ),
            (
                Some("application/octet-stream"),
                b"head\0binary".as_slice(),
                PreviewMode::Full,
                false,
            ),
        ] {
            fs::write(&path, source).unwrap();
            let entry = FileEntry {
                location: Location::new(gio::File::for_path(&path).uri()),
                display_name: "unknown.data".to_owned(),
                kind: FileKind::Regular,
                size: Some(source.len() as u64),
                modified: None,
                unix_mode: None,
                content_type: mime.map(str::to_owned),
                is_hidden: false,
                is_symlink: false,
                archive_format: None,
            };
            let result = Rc::new(RefCell::new(None));
            let main_loop = glib::MainLoop::new(None, false);
            let _cancellable = load_preview(
                &entry,
                GenerationTracker::default().advance(),
                PreviewLimits::default(),
                false,
                mode,
                {
                    let result = result.clone();
                    let main_loop = main_loop.clone();
                    move |preview| {
                        *result.borrow_mut() = Some(preview.content);
                        main_loop.quit();
                    }
                },
            );
            main_loop.run();
            let content = result.borrow_mut().take().unwrap().unwrap();
            if expected {
                let text = match content {
                    PreviewContent::Text(text) => text.text,
                    PreviewContent::StyledText(text) => text.text,
                    _ => panic!("expected text for {mime:?}"),
                };
                assert_eq!(text.as_bytes(), source);
            } else {
                assert!(matches!(content, PreviewContent::Unsupported));
            }
        }
    }

    #[test]
    fn stale_preview_results_are_rejected() {
        let mut gate = PreviewGate::default();
        let stale = gate.begin();
        let current = gate.begin();

        assert!(!gate.accepts(stale));
        assert!(gate.accepts(current));
    }

    #[test]
    fn recognizes_supported_text_content_types() {
        assert!(is_text_content_type("text/plain"));
        assert!(is_text_content_type("text/yaml"));
        assert!(is_text_content_type("application/json"));
        assert!(is_text_content_type("application/yaml"));
        assert!(is_text_content_type("application/x-yaml"));
        assert!(!is_text_content_type("application/pdf"));
    }

    #[test]
    fn empty_file_has_an_empty_text_preview_regardless_of_mime_type() {
        let _guard = async_test_guard();
        let entry = FileEntry {
            location: Location::new("file:///tmp/empty"),
            display_name: "empty".to_owned(),
            kind: FileKind::Regular,
            size: Some(0),
            modified: None,
            unix_mode: None,
            content_type: Some("application/x-zerosize".to_owned()),
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        };
        let generation = GenerationTracker::default().advance();
        let result = Rc::new(RefCell::new(None));
        let _cancellable = load_preview(
            &entry,
            generation,
            PreviewLimits::default(),
            false,
            PreviewMode::Automatic,
            {
                let result = result.clone();
                move |preview| *result.borrow_mut() = Some(preview)
            },
        );

        let result = result.borrow_mut().take().expect("preview completes");
        let PreviewContent::Text(preview) = result.content.expect("preview succeeds") else {
            panic!("expected text preview");
        };
        assert!(preview.text.is_empty());
        assert!(!preview.truncated);
    }

    #[test]
    fn text_preview_is_bounded_and_reports_truncation() {
        let _guard = async_test_guard();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let path = temporary.path().join("sample.txt");
        fs::write(&path, b"abcdef").expect("write preview fixture");
        let entry = FileEntry {
            location: Location::new(gio::File::for_path(path).uri()),
            display_name: "sample.txt".to_owned(),
            kind: FileKind::Regular,
            size: Some(6),
            modified: None,
            unix_mode: None,
            content_type: Some("text/plain".to_owned()),
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        };
        let generation = GenerationTracker::default().advance();
        let result = Rc::new(RefCell::new(None));
        let main_loop = glib::MainLoop::new(None, false);
        let _cancellable = load_preview(
            &entry,
            generation,
            PreviewLimits {
                max_text_bytes: 4,
                ..PreviewLimits::default()
            },
            false,
            PreviewMode::Automatic,
            {
                let result = result.clone();
                let main_loop = main_loop.clone();
                move |preview| {
                    *result.borrow_mut() = Some(preview);
                    main_loop.quit();
                }
            },
        );
        main_loop.run();

        let result = result.borrow_mut().take().expect("preview completes");
        assert_eq!(result.generation, generation);
        let PreviewContent::StyledText(preview) = result.content.expect("preview succeeds") else {
            panic!("expected styled text preview");
        };
        assert_eq!(preview.text, "abcd");
        assert!(preview.truncated);
    }

    #[test]
    fn large_source_uses_fast_plain_text_in_automatic_mode() {
        let _guard = async_test_guard();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let path = temporary.path().join("large.html");
        let source = format!("<html>{}</html>", "<div>content</div>".repeat(3_000));
        fs::write(&path, &source).expect("write preview fixture");
        let entry = FileEntry {
            location: Location::new(gio::File::for_path(path).uri()),
            display_name: "large.html".to_owned(),
            kind: FileKind::Regular,
            size: Some(source.len() as u64),
            modified: None,
            unix_mode: None,
            content_type: Some("text/html".to_owned()),
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        };
        let generation = GenerationTracker::default().advance();
        let result = Rc::new(RefCell::new(None));
        let main_loop = glib::MainLoop::new(None, false);
        let _cancellable = load_preview(
            &entry,
            generation,
            PreviewLimits::default(),
            false,
            PreviewMode::Automatic,
            {
                let result = result.clone();
                let main_loop = main_loop.clone();
                move |preview| {
                    *result.borrow_mut() = Some(preview);
                    main_loop.quit();
                }
            },
        );
        main_loop.run();
        assert!(matches!(
            result.borrow_mut().take().unwrap().content,
            Ok(PreviewContent::Text(_))
        ));
    }

    #[test]
    fn large_markdown_keeps_html_in_automatic_and_full_modes() {
        let _guard = async_test_guard();
        let temporary = tempfile::tempdir().unwrap();
        let limits = PreviewLimits::default();
        for repeats in [1_500, 12_000] {
            let source = format!(
                "# Large Markdown\n\n{}END-MARKER",
                "A paragraph with **bold** text.\n\n".repeat(repeats)
            );
            assert!(source.len() > limits.max_rich_text_bytes);
            let path = temporary.path().join("large.md");
            fs::write(&path, &source).unwrap();
            // GIO can report text/plain for Markdown, as it does for the reported file.
            let entry = FileEntry {
                location: Location::new(gio::File::for_path(path).uri()),
                display_name: "large.md".to_owned(),
                kind: FileKind::Regular,
                size: Some(source.len() as u64),
                modified: None,
                unix_mode: None,
                content_type: Some("text/plain".to_owned()),
                is_hidden: false,
                is_symlink: false,
                archive_format: None,
            };
            for mode in [PreviewMode::Automatic, PreviewMode::Full] {
                let result = Rc::new(RefCell::new(None));
                let main_loop = glib::MainLoop::new(None, false);
                let received = result.clone();
                let finished = main_loop.clone();
                let _cancellable = load_preview(
                    &entry,
                    GenerationTracker::default().advance(),
                    limits,
                    false,
                    mode,
                    move |preview| {
                        *received.borrow_mut() = Some(preview);
                        finished.quit();
                    },
                );
                main_loop.run();
                let PreviewContent::Markdown(preview) =
                    result.borrow_mut().take().unwrap().content.unwrap()
                else {
                    panic!("large Markdown must use HTML, not plain text")
                };
                assert!(
                    preview
                        .html
                        .contains("<h1 id=\"large-markdown\">Large Markdown</h1>")
                );
                assert!(preview.html.contains("<strong>bold</strong>"));
                let truncated =
                    mode == PreviewMode::Automatic && source.len() > limits.max_text_bytes;
                assert_eq!(preview.truncated, truncated);
                assert_eq!(preview.html.contains("END-MARKER"), !truncated);
            }
        }
    }

    #[test]
    fn full_preview_reads_through_end_of_large_file() {
        let _guard = async_test_guard();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let path = temporary.path().join("large.txt");
        let source = format!("{}END-MARKER", "line of text\n".repeat(50_000));
        fs::write(&path, &source).expect("write preview fixture");
        let entry = FileEntry {
            location: Location::new(gio::File::for_path(path).uri()),
            display_name: "large.txt".to_owned(),
            kind: FileKind::Regular,
            size: Some(source.len() as u64),
            modified: None,
            unix_mode: None,
            content_type: Some("text/plain".to_owned()),
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        };
        let generation = GenerationTracker::default().advance();
        let result = Rc::new(RefCell::new(None));
        let main_loop = glib::MainLoop::new(None, false);
        let _cancellable = load_preview(
            &entry,
            generation,
            PreviewLimits::default(),
            false,
            PreviewMode::Full,
            {
                let result = result.clone();
                let main_loop = main_loop.clone();
                move |preview| {
                    *result.borrow_mut() = Some(preview);
                    main_loop.quit();
                }
            },
        );
        main_loop.run();
        let result = result.borrow_mut().take().unwrap().content.unwrap();
        let PreviewContent::StyledText(preview) = result else {
            panic!("expected styled full preview");
        };
        assert!(preview.text.ends_with("END-MARKER"));
        assert!(!preview.truncated);
    }

    #[test]
    fn yaml_source_produces_highlight_spans() {
        let cancellable = gio::Cancellable::new();
        let preview = highlight_text(
            "name: pathpilot\nenabled: true\n".to_owned(),
            false,
            "config.yaml",
            false,
            &cancellable,
        )
        .expect("highlight succeeds");

        assert!(!preview.spans.is_empty());
        assert_eq!(preview.text, "name: pathpilot\nenabled: true\n");
    }

    #[test]
    fn preview_cache_is_lru_and_metadata_sensitive() {
        let mut cache = PreviewCache::new(1);
        let first = FileEntry {
            location: Location::new("file:///tmp/first.txt"),
            display_name: "first.txt".to_owned(),
            kind: FileKind::Regular,
            size: Some(1),
            modified: None,
            unix_mode: None,
            content_type: Some("text/plain".to_owned()),
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        };
        let mut changed = first.clone();
        changed.size = Some(2);
        cache.insert(
            &first,
            PreviewContent::Text(TextPreview {
                text: "a".to_owned(),
                truncated: false,
            }),
        );

        assert!(cache.get(&first).is_some());
        assert!(cache.get(&changed).is_none());
        cache.invalidate(&first.location);
        assert!(cache.get(&first).is_none());
        cache.insert(
            &changed,
            PreviewContent::Text(TextPreview {
                text: "ab".to_owned(),
                truncated: false,
            }),
        );
        assert!(cache.get(&first).is_none());
        assert!(cache.get(&changed).is_some());
    }

    #[test]
    fn markdown_renders_structure_without_raw_html() {
        let cancellable = gio::Cancellable::new();
        let preview = render_markdown(
            "# Title\n\n**bold** and `code`\n\n<script>alert(1)</script>".to_owned(),
            false,
            &cancellable,
        )
        .expect("markdown render succeeds");

        assert!(preview.html.contains("<h1 id=\"title\">Title</h1>"));
        assert!(preview.html.contains("<strong>bold</strong>"));
        assert!(preview.html.contains("<code>code</code>"));
        assert!(!preview.html.contains("script"));
    }

    #[test]
    fn markdown_keeps_tables_images_tasks_and_escapes_code() {
        let preview = render_markdown(
            "| Name | Value |\n| --- | --- |\n| A | B |\n\n![Photo](images/local.png)\n\n- [x] Done\n\n~~old~~\n\n```html\n<script>example</script>\n```".to_owned(),
            false, &gio::Cancellable::new(),
        ).unwrap();
        assert!(preview.html.contains("<table>"));
        assert!(preview.html.contains("<td>A</td>"));
        assert!(preview.html.contains("src=\"images/local.png\""));
        assert!(preview.html.contains("type=\"checkbox\""));
        assert!(preview.html.contains("<del>old</del>"));
        assert!(
            preview
                .html
                .contains("&lt;script&gt;example&lt;/script&gt;")
        );
    }

    #[test]
    fn markdown_rejects_script_links_and_active_raw_html() {
        let preview = render_markdown(
            "[bad](javascript:alert%281%29) [also bad](vbscript:bad) ![bad](data:text/html,evil) <img src=x onerror=alert(1)>".to_owned(),
            false, &gio::Cancellable::new(),
        ).unwrap();
        assert!(!preview.html.contains("javascript:"));
        assert!(!preview.html.contains("vbscript:"));
        assert!(!preview.html.contains("data:text/html"));
        assert!(!preview.html.contains("onerror"));
        assert!(safe_markdown_url("../images/a.png", true));
        assert!(!safe_markdown_url("java\nscript:bad", false));
    }

    #[test]
    fn markdown_heading_anchors_are_stable_and_unique() {
        let preview = render_markdown(
            "# Начало работы\n\n# Начало работы\n\n# Explicit {#custom}\n\n# Custom".to_owned(),
            false,
            &gio::Cancellable::new(),
        )
        .unwrap();
        for id in ["начало-работы", "начало-работы-1", "custom", "custom-1"]
        {
            assert!(preview.html.contains(&format!("id=\"{id}\"")));
        }
        let cancelled = gio::Cancellable::new();
        cancelled.cancel();
        assert!(render_markdown("# Heading".to_owned(), false, &cancelled).is_err());
    }

    #[test]
    fn image_dimensions_only_scale_down() {
        assert_eq!(fit_dimensions(32, 32, 1200, 1200), (32, 32));
        assert_eq!(fit_dimensions(2400, 1200, 1200, 1200), (1200, 600));
        assert_eq!(fit_dimensions(800, 1600, 1200, 1200), (600, 1200));
    }
}
