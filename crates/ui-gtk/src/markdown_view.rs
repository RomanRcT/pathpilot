//! Lazy HTML reader: trusted bundled styles, inert Markdown, no page scripts.
use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use pathpilot_core::Location;
use pathpilot_preview::MarkdownPreview;
use webkit6::{NavigationPolicyDecision, PolicyDecisionType, prelude::*};

const CSP: &str = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src file: data:; font-src 'none'; base-uri 'none'; form-action 'none'";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Theme {
    #[default]
    Compact,
    Github,
    Reading,
}

impl Theme {
    fn from_name(name: &str) -> Self {
        match name {
            "github" => Self::Github,
            "reading" => Self::Reading,
            _ => Self::Compact,
        }
    }

    fn css(self) -> &'static str {
        match self {
            Self::Compact => include_str!("../../../data/markdown/compact.css"),
            Self::Github => include_str!("../../../data/markdown/github.css"),
            Self::Reading => include_str!("../../../data/markdown/reading.css"),
        }
    }
}

struct State {
    widget: gtk::Box,
    theme: Theme,
    view: RefCell<Option<webkit6::WebView>>,
    style_manager: adw::StyleManager,
    style_handler: RefCell<Option<glib::SignalHandlerId>>,
}

impl State {
    fn apply_color_scheme(&self) {
        let Some(view) = self.view.borrow().as_ref().cloned() else {
            return;
        };
        let dark = self.style_manager.is_dark();
        let css = if dark {
            include_str!("../../../data/markdown/dark.css")
        } else {
            include_str!("../../../data/markdown/light.css")
        };
        if let Some(manager) = view.user_content_manager() {
            manager.remove_all_style_sheets();
            manager.add_style_sheet(&webkit6::UserStyleSheet::new(
                css,
                webkit6::UserContentInjectedFrames::TopFrame,
                webkit6::UserStyleLevel::User,
                &[],
                &[],
            ));
        }
        view.set_background_color(&if dark {
            gdk::RGBA::new(0.114, 0.141, 0.133, 1.0)
        } else {
            gdk::RGBA::WHITE
        });
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(handler) = self.style_handler.get_mut().take() {
            self.style_manager.disconnect(handler);
        }
    }
}

#[derive(Clone)]
pub(super) struct MarkdownView(Rc<State>);

impl MarkdownView {
    pub(super) fn new(theme: &str) -> Self {
        let state = Rc::new(State {
            widget: gtk::Box::builder().hexpand(true).vexpand(true).build(),
            theme: Theme::from_name(theme),
            view: RefCell::new(None),
            style_manager: adw::StyleManager::default(),
            style_handler: RefCell::new(None),
        });
        let weak = Rc::downgrade(&state);
        let handler = state.style_manager.connect_dark_notify(move |_| {
            if let Some(state) = weak.upgrade() {
                state.apply_color_scheme();
            }
        });
        *state.style_handler.borrow_mut() = Some(handler);
        Self(state)
    }

    pub(super) fn widget(&self) -> &gtk::Box {
        &self.0.widget
    }

    pub(super) fn render(&self, preview: MarkdownPreview, location: &Location) {
        let view = self.ensure_view();
        let base = local_base_uri(location);
        view.load_html(&document(&preview, self.0.theme), base.as_deref());
    }

    pub(super) fn stop_loading(&self) {
        if let Some(view) = self.0.view.borrow().as_ref() {
            view.stop_loading();
        }
    }

    pub(super) fn focus(&self) {
        if let Some(view) = self.0.view.borrow().as_ref() {
            view.grab_focus();
        }
    }

    fn ensure_view(&self) -> webkit6::WebView {
        if let Some(view) = self.0.view.borrow().as_ref() {
            return view.clone();
        }
        let settings = webkit6::Settings::builder()
            .enable_javascript(false)
            .enable_javascript_markup(false)
            .enable_media(false)
            .enable_media_stream(false)
            .allow_file_access_from_file_urls(true)
            .allow_universal_access_from_file_urls(false)
            .build();
        let view = webkit6::WebView::builder()
            .settings(&settings)
            .network_session(&webkit6::NetworkSession::new_ephemeral())
            .hexpand(true)
            .vexpand(true)
            .build();
        view.connect_decide_policy(|view, decision, kind| {
            if !matches!(
                kind,
                PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction
            ) {
                return false;
            }
            let Some(navigation) = decision.downcast_ref::<NavigationPolicyDecision>() else {
                return false;
            };
            let Some(mut action) = navigation.navigation_action() else {
                return false;
            };
            let Some(uri) = action.request().and_then(|request| request.uri()) else {
                return false;
            };
            if action.navigation_type() == webkit6::NavigationType::Other {
                // load_html() installs the generated document without navigating.
                return false;
            }
            if same_document_fragment(view.uri().as_deref(), &uri) {
                return false;
            }
            decision.ignore();
            if action.is_user_gesture() && external_link_allowed(&uri) {
                gio::AppInfo::launch_default_for_uri_async(
                    &uri,
                    None::<&gio::AppLaunchContext>,
                    None::<&gio::Cancellable>,
                    |result| {
                        if let Err(error) = result {
                            tracing::warn!(%error, "could not open Markdown link");
                        }
                    },
                );
            }
            true
        });
        self.0.widget.append(&view);
        *self.0.view.borrow_mut() = Some(view.clone());
        self.0.apply_color_scheme();
        view
    }
}

fn local_base_uri(location: &Location) -> Option<String> {
    let file = gio::File::for_uri(location.uri());
    if !file.is_native() {
        return None;
    }
    let mut uri = file.parent()?.uri().to_string();
    if !uri.ends_with('/') {
        uri.push('/');
    }
    Some(uri)
}

fn same_document_fragment(current: Option<&str>, target: &str) -> bool {
    target.contains('#')
        && current.is_some_and(|current| current.split('#').next() == target.split('#').next())
}

fn external_link_allowed(uri: &str) -> bool {
    gio::File::for_uri(uri).is_native()
        || uri.starts_with("https://")
        || uri.starts_with("http://")
        || uri.starts_with("mailto:")
}

fn document(preview: &MarkdownPreview, theme: Theme) -> String {
    let class = if theme == Theme::Github {
        "markdown-body"
    } else {
        ""
    };
    let notice = if preview.truncated {
        "<p class=\"preview-truncated\">Markdown preview truncated · Press u for full preview</p>"
    } else {
        ""
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\"><style>{}\nbody.markdown-body{{padding:24px;margin:0;box-sizing:border-box}}article{{max-width:760px;margin:auto}}.preview-truncated{{opacity:.65;font:13px system-ui}}img{{max-width:100%;height:auto}}body{{overflow-wrap:break-word}}</style></head><body class=\"{class}\"><article>{}{notice}</article></body></html>",
        theme.css(),
        preview.html
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn document_uses_bundled_theme_and_a_truncation_notice() {
        let preview = MarkdownPreview {
            html: "<h1>Example</h1>".to_owned(),
            truncated: true,
        };
        for name in ["compact", "github", "reading", "unknown"] {
            let html = document(&preview, Theme::from_name(name));
            assert!(html.contains("<h1>Example</h1>"));
            assert!(html.contains("Press u for full preview"));
            assert!(html.contains("script-src 'none'"));
        }
        assert_eq!(Theme::from_name("unknown"), Theme::Compact);
        assert!(!external_link_allowed("javascript:alert(1)"));
        assert!(!external_link_allowed("data:text/html,evil"));
        assert!(same_document_fragment(
            Some("file:///docs/"),
            "file:///docs/#heading"
        ));
        assert!(!same_document_fragment(
            Some("file:///docs/"),
            "https://example.com/#heading"
        ));
    }
    fn wait_until(condition: impl Fn() -> bool) {
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !condition() {
            assert!(
                std::time::Instant::now() < deadline,
                "WebKit task timed out"
            );
            while context.pending() {
                context.iteration(false);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn evaluate(view: &webkit6::WebView, script: &str) -> webkit6::javascriptcore::Value {
        let result = Rc::new(RefCell::new(None));
        let received = result.clone();
        view.evaluate_javascript(
            script,
            None,
            None,
            None::<&gio::Cancellable>,
            move |value| {
                *received.borrow_mut() = Some(value);
            },
        );
        wait_until(|| result.borrow().is_some());
        result
            .borrow_mut()
            .take()
            .unwrap()
            .expect("trusted inspection script")
    }

    // WebKit must initialize and shut down on the process main thread.
    // The dedicated harness calls this function; Rust unit-test threads cannot.
    #[allow(dead_code)]
    pub(crate) fn run_visual_check() {
        use pathpilot_core::{FileEntry, FileKind, GenerationTracker};
        use pathpilot_preview::{PreviewContent, PreviewLimits, PreviewMode, load_preview};
        use std::cell::Cell;
        adw::init().unwrap();
        let manager = adw::StyleManager::default();
        manager.set_color_scheme(adw::ColorScheme::ForceLight);
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("Документы с пробелами");
        std::fs::create_dir(&directory).unwrap();
        let source = include_str!("../tests/fixtures/markdown.md");
        let path = directory.join("sample.md");
        std::fs::write(&path, source).unwrap();
        std::fs::write(
            directory.join("landscape.svg"),
            include_str!("../tests/fixtures/landscape.svg"),
        )
        .unwrap();
        let location = Location::new(gio::File::for_path(path).uri());
        let entry = FileEntry {
            location: location.clone(),
            display_name: "sample.md".to_owned(),
            kind: FileKind::Regular,
            size: Some(source.len() as u64),
            modified: None,
            unix_mode: None,
            content_type: Some("text/markdown".to_owned()),
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        };
        let loaded = Rc::new(RefCell::new(None));
        let received = loaded.clone();
        let _cancellable = load_preview(
            &entry,
            GenerationTracker::default().advance(),
            PreviewLimits::default(),
            false,
            PreviewMode::Full,
            move |result| {
                *received.borrow_mut() = Some(result);
            },
        );
        wait_until(|| loaded.borrow().is_some());
        let PreviewContent::Markdown(preview) =
            loaded.borrow_mut().take().unwrap().content.unwrap()
        else {
            panic!("expected Markdown HTML")
        };
        let window = gtk::Window::builder()
            .default_width(1200)
            .default_height(800)
            .build();
        let columns = gtk::Box::builder().homogeneous(true).spacing(4).build();
        window.set_child(Some(&columns));
        let mut views = Vec::new();
        for theme in ["compact", "github", "reading"] {
            let reader = MarkdownView::new(theme);
            assert!(reader.0.view.borrow().is_none(), "reader must be lazy");
            columns.append(reader.widget());
            reader.render(preview.clone(), &location);
            let view = reader.0.view.borrow().as_ref().unwrap().clone();
            let finished = Rc::new(Cell::new(false));
            let notified = finished.clone();
            view.connect_load_changed(move |_, event| {
                if event == webkit6::LoadEvent::Finished {
                    notified.set(true);
                }
            });
            views.push((theme, reader, view, finished));
        }
        window.present();
        for (_, _, _, finished) in &views {
            wait_until(|| finished.get());
        }
        for (theme, _, view, _) in &views {
            assert!(
                !webkit6::prelude::WebViewExt::settings(view)
                    .unwrap()
                    .enables_javascript()
            );
            // Page scripts were disabled while loading. Enable only the trusted
            // test inspection API now; production readers never enable scripts.
            webkit6::prelude::WebViewExt::settings(view)
                .unwrap()
                .set_enable_javascript(true);
            assert!(evaluate(view, "document.querySelectorAll('table').length === 1 && document.querySelectorAll('input[type=checkbox]').length === 3").to_boolean());
            assert!(evaluate(view, "document.images.length === 1 && document.images[0].complete && document.images[0].naturalWidth > 0").to_boolean());
            assert!(
                evaluate(
                    view,
                    "typeof window.untrustedMarkdownScript === 'undefined'"
                )
                .to_boolean()
            );
            assert!(evaluate(view, "document.body.scrollWidth <= window.innerWidth").to_boolean());
            let font_size =
                evaluate(view, "parseFloat(getComputedStyle(document.body).fontSize)").to_double();
            assert_eq!(
                font_size,
                match *theme {
                    "reading" => 18.0,
                    "github" => 16.0,
                    _ => 15.0,
                }
            );
            if let Some(directory) = std::env::var_os("PATHPILOT_TEST_MARKDOWN_DIR") {
                let directory = std::path::PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                let texture = glib::MainContext::default()
                    .block_on(view.snapshot_future(
                        webkit6::SnapshotRegion::Visible,
                        webkit6::SnapshotOptions::NONE,
                    ))
                    .unwrap();
                texture
                    .save_to_png(directory.join(format!("{theme}-light.png")))
                    .unwrap();
            }
            evaluate(view, "window.scrollTo(0, 500)");
        }
        manager.set_color_scheme(adw::ColorScheme::ForceDark);
        for (theme, _, view, _) in &views {
            wait_until(|| {
                evaluate(
                    view,
                    "getComputedStyle(document.documentElement).colorScheme === 'dark'",
                )
                .to_boolean()
            });
            assert!((evaluate(view, "window.scrollY").to_double() - 500.0).abs() < 2.0);
            if let Some(directory) = std::env::var_os("PATHPILOT_TEST_MARKDOWN_DIR") {
                evaluate(view, "window.scrollTo(0,0)");
                let texture = glib::MainContext::default()
                    .block_on(view.snapshot_future(
                        webkit6::SnapshotRegion::Visible,
                        webkit6::SnapshotOptions::NONE,
                    ))
                    .unwrap();
                texture
                    .save_to_png(
                        std::path::PathBuf::from(directory).join(format!("{theme}-dark.png")),
                    )
                    .unwrap();
            }
        }
        // Native fragment navigation stays in the generated document.
        let view = &views[0].2;
        evaluate(
            view,
            "document.querySelector('a[href=\"#reading-note\"]').click()",
        );
        wait_until(|| evaluate(view, "location.hash === '#reading-note'").to_boolean());
        assert!(evaluate(view, "window.scrollY > 500").to_boolean());
        for (_, reader, _, _) in &views {
            reader.focus();
            reader.stop_loading();
        }
        window.close();
    }
}
