//! Bounded, cancelable preview loading without GTK widgets.

use std::rc::Rc;

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

#[derive(Debug)]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug)]
pub struct ImagePreview {
    pub pixbuf: gdk_pixbuf::Pixbuf,
}

#[derive(Debug)]
pub enum PreviewContent {
    Text(TextPreview),
    Image(ImagePreview),
    Unsupported,
}

#[derive(Debug)]
pub struct PreviewResult {
    pub generation: Generation,
    pub content: Result<PreviewContent, String>,
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
    let callback_cancellable = cancellable.clone();
    file.read_async(
        glib::Priority::LOW,
        Some(cancellable),
        move |result| match result {
            Ok(stream) => {
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
                            on_result(PreviewResult {
                                generation,
                                content: Ok(PreviewContent::Text(TextPreview {
                                    text: String::from_utf8_lossy(visible).into_owned(),
                                    truncated,
                                })),
                            });
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
    let callback_cancellable = cancellable.clone();
    file.read_async(
        glib::Priority::LOW,
        Some(cancellable),
        move |result| match result {
            Ok(stream) => gdk_pixbuf::Pixbuf::from_stream_at_scale_async(
                &stream,
                limits.image_width,
                limits.image_height,
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
        let PreviewContent::Text(preview) = result.content.expect("preview succeeds") else {
            panic!("expected text preview");
        };
        assert_eq!(preview.text, "abcd");
        assert!(preview.truncated);
    }
}
