//! Bounded, cancelable preview loading without GTK widgets.

use std::{
    collections::VecDeque,
    rc::Rc,
    sync::{OnceLock, mpsc},
    time::Duration,
};

use gio::prelude::*;
use pathpilot_core::{FileEntry, Generation, GenerationTracker};
use tracing::{debug, warn};

#[derive(Clone, Copy, Debug)]
pub struct PreviewLimits {
    pub max_text_bytes: usize,
    pub image_width: i32,
    pub image_height: i32,
}

impl Default for PreviewLimits {
    fn default() -> Self {
        Self {
            max_text_bytes: 1024 * 1024,
            image_width: 1200,
            image_height: 1200,
        }
    }
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
pub enum MarkdownStyle {
    Heading(u8),
    Strong,
    Emphasis,
    Code,
    Link,
    Quote,
}

#[derive(Clone, Debug)]
pub struct MarkdownSpan {
    pub start: i32,
    pub end: i32,
    pub style: MarkdownStyle,
}

#[derive(Clone, Debug)]
pub struct MarkdownPreview {
    pub text: String,
    pub spans: Vec<MarkdownSpan>,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
pub enum PreviewContent {
    Text(TextPreview),
    StyledText(StyledTextPreview),
    Markdown(MarkdownPreview),
    Image(ImagePreview),
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
    } else if content_type.starts_with("image/") {
        load_image(entry, generation, limits, &cancellable, on_result);
    } else if is_text_content_type(content_type) {
        load_text(entry, generation, limits, &cancellable, on_result);
    } else {
        on_result(PreviewResult {
            generation,
            content: Ok(PreviewContent::Unsupported),
        });
    }

    cancellable
}

fn load_text(
    entry: &FileEntry,
    generation: Generation,
    limits: PreviewLimits,
    cancellable: &gio::Cancellable,
    on_result: Rc<dyn Fn(PreviewResult)>,
) {
    let file = gio::File::for_uri(entry.location.uri());
    let display_name = entry.display_name.clone();
    let callback_cancellable = cancellable.clone();
    file.read_async(
        glib::Priority::LOW,
        Some(cancellable),
        move |result| match result {
            Ok(stream) => {
                let highlight_cancellable = callback_cancellable.clone();
                stream.read_bytes_async(
                    limits.max_text_bytes.saturating_add(1),
                    glib::Priority::LOW,
                    Some(&callback_cancellable),
                    move |result| match result {
                        Ok(bytes) => {
                            let bytes = bytes.as_ref();
                            if bytes.contains(&0) {
                                on_result(PreviewResult {
                                    generation,
                                    content: Err("Binary data cannot be shown as text".to_owned()),
                                });
                                return;
                            }
                            let truncated = bytes.len() > limits.max_text_bytes;
                            let visible = &bytes[..bytes.len().min(limits.max_text_bytes)];
                            highlight_async(
                                String::from_utf8_lossy(visible).into_owned(),
                                truncated,
                                display_name,
                                generation,
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
    cancellable: &gio::Cancellable,
    on_result: Rc<dyn Fn(PreviewResult)>,
) {
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_cancellable = cancellable.clone();
    std::thread::spawn(move || {
        let result = if is_markdown_name(&display_name) {
            render_markdown(text, truncated, &worker_cancellable).map(ProcessedText::Markdown)
        } else {
            highlight_text(text, truncated, &display_name, &worker_cancellable)
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
    let theme = &theme_set.themes["base16-ocean.dark"];
    let background = theme
        .settings
        .background
        .map_or((43, 48, 59), |color| (color.r, color.g, color.b));
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
    use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let parser = Parser::new_ext(&source, options);
    let mut text = String::new();
    let mut spans = Vec::new();
    let mut open: Vec<(TagEnd, i32, MarkdownStyle)> = Vec::new();

    for event in parser {
        if cancellable.is_cancelled() {
            return Err("Preview cancelled".to_owned());
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => open.push((
                    TagEnd::Heading(level),
                    char_len(&text),
                    MarkdownStyle::Heading(match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    }),
                )),
                Tag::Strong => open.push((TagEnd::Strong, char_len(&text), MarkdownStyle::Strong)),
                Tag::Emphasis => {
                    open.push((TagEnd::Emphasis, char_len(&text), MarkdownStyle::Emphasis));
                }
                Tag::CodeBlock(_) => {
                    open.push((TagEnd::CodeBlock, char_len(&text), MarkdownStyle::Code));
                }
                Tag::Link { .. } => open.push((TagEnd::Link, char_len(&text), MarkdownStyle::Link)),
                Tag::BlockQuote(kind) => {
                    text.push_str("│ ");
                    open.push((
                        TagEnd::BlockQuote(kind),
                        char_len(&text),
                        MarkdownStyle::Quote,
                    ));
                }
                Tag::Item => text.push_str("• "),
                _ => {}
            },
            Event::End(end) => {
                if let Some(index) = open.iter().rposition(|(tag, _, _)| *tag == end) {
                    let (_, start, style) = open.remove(index);
                    spans.push(MarkdownSpan {
                        start,
                        end: char_len(&text),
                        style,
                    });
                }
                match end {
                    TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item => text.push('\n'),
                    TagEnd::TableCell => text.push_str(" │ "),
                    TagEnd::TableRow => text.push('\n'),
                    TagEnd::CodeBlock => text.push('\n'),
                    _ => {}
                }
            }
            Event::Text(value) => text.push_str(&value),
            Event::Code(value) => {
                let start = char_len(&text);
                text.push_str(&value);
                spans.push(MarkdownSpan {
                    start,
                    end: char_len(&text),
                    style: MarkdownStyle::Code,
                });
            }
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::Rule => text.push_str("────────\n"),
            Event::TaskListMarker(checked) => {
                text.push_str(if checked { "☑ " } else { "☐ " });
            }
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::FootnoteReference(value) => text.push_str(&format!("[{value}]")),
            Event::InlineMath(value) | Event::DisplayMath(value) => text.push_str(&value),
        }
    }

    Ok(MarkdownPreview {
        text,
        spans,
        truncated,
    })
}

fn char_len(value: &str) -> i32 {
    i32::try_from(value.chars().count()).unwrap_or(i32::MAX)
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
    use std::{cell::RefCell, fs, rc::Rc};

    use super::*;
    use pathpilot_core::{FileKind, GenerationTracker, Location};

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
        };
        let generation = GenerationTracker::default().advance();
        let result = Rc::new(RefCell::new(None));
        let _cancellable = load_preview(&entry, generation, PreviewLimits::default(), {
            let result = result.clone();
            move |preview| *result.borrow_mut() = Some(preview)
        });

        let result = result.borrow_mut().take().expect("preview completes");
        let PreviewContent::Text(preview) = result.content.expect("preview succeeds") else {
            panic!("expected text preview");
        };
        assert!(preview.text.is_empty());
        assert!(!preview.truncated);
    }

    #[test]
    fn text_preview_is_bounded_and_reports_truncation() {
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
    fn yaml_source_produces_highlight_spans() {
        let cancellable = gio::Cancellable::new();
        let preview = highlight_text(
            "name: pathpilot\nenabled: true\n".to_owned(),
            false,
            "config.yaml",
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

        assert!(preview.text.contains("Title"));
        assert!(preview.text.contains("bold and code"));
        assert!(!preview.text.contains("script"));
        assert!(
            preview
                .spans
                .iter()
                .any(|span| matches!(span.style, MarkdownStyle::Heading(1)))
        );
        assert!(
            preview
                .spans
                .iter()
                .any(|span| matches!(span.style, MarkdownStyle::Code))
        );
    }

    #[test]
    fn image_dimensions_only_scale_down() {
        assert_eq!(fit_dimensions(32, 32, 1200, 1200), (32, 32));
        assert_eq!(fit_dimensions(2400, 1200, 1200, 1200), (1200, 600));
        assert_eq!(fit_dimensions(800, 1600, 1200, 1200), (600, 1200));
    }
}
