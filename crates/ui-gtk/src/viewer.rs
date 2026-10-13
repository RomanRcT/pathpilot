//! Independent, read-only preview windows with window-local file navigation.
use std::{
    any::Any,
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use pathpilot_config::UiSettings;
use pathpilot_core::{FileEntry, FileKind, Location, SortMode};
use pathpilot_preview::PreviewContent;

use crate::{
    directory_pane::DirectoryPane,
    preview_pane::{PreviewPane, document_page_direction},
};

pub(super) struct ViewerSource {
    pub selected: FileEntry,
    pub files: Vec<FileEntry>,
    pub content: Option<PreviewContent>,
    pub ui: UiSettings,
    pub sort: SortMode,
    pub context: String,
    /// Keep archive extraction/staging alive without retaining the browser itself.
    pub resources: Vec<Rc<dyn Any>>,
}

pub(super) fn can_view(entry: &FileEntry) -> bool {
    matches!(
        entry.kind,
        FileKind::Regular | FileKind::Symlink | FileKind::Unknown
    )
}

struct Files {
    entries: Vec<FileEntry>,
    index: usize,
}

impl Files {
    fn current(&self) -> Option<&FileEntry> {
        self.entries.get(self.index)
    }

    fn step(&mut self, offset: i32) -> bool {
        let next = if offset < 0 {
            self.index.saturating_sub(offset.unsigned_abs() as usize)
        } else {
            self.index
                .saturating_add(offset as usize)
                .min(self.entries.len().saturating_sub(1))
        };
        if next == self.index {
            return false;
        }
        self.index = next;
        true
    }

    fn replace(&mut self, entries: Vec<FileEntry>) {
        let current = self.current().map(|entry| entry.location.clone());
        self.entries = entries.into_iter().filter(can_view).collect();
        self.index = current
            .and_then(|location| {
                self.entries
                    .iter()
                    .position(|entry| entry.location == location)
            })
            .unwrap_or_else(|| self.index.min(self.entries.len().saturating_sub(1)));
    }
}

struct Viewer {
    window: glib::WeakRef<adw::ApplicationWindow>,
    preview: PreviewPane,
    model: DirectoryPane,
    directory: Option<Location>,
    files: RefCell<Files>,
    title: gtk::Label,
    status: gtk::Label,
    resources: RefCell<Vec<Rc<dyn Any>>>,
    closed: Cell<bool>,
    force_refresh: Cell<bool>,
    directory_loading: Cell<bool>,
    keys: gtk::EventControllerKey,
}

pub(super) fn open(application: &gtk::Application, source: ViewerSource) -> adw::ApplicationWindow {
    build(application, source).0
}

fn build(
    application: &gtk::Application,
    source: ViewerSource,
) -> (adw::ApplicationWindow, Rc<Viewer>) {
    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("PathPilot Viewer")
        .default_width((source.ui.window_width * 2 / 3).clamp(700, 1200))
        .default_height(source.ui.window_height.max(600))
        .build();
    let model = DirectoryPane::new("Files");
    model.set_show_hidden(source.ui.show_hidden);
    model.set_sort_mode(source.sort);
    let preview = PreviewPane::new(
        source.ui.preview_line_numbers,
        source.ui.preview_delay_ms,
        &source.ui.markdown_theme,
        model.remote_cache(),
    );
    preview.set_preview_only(true);
    let title = gtk::Label::builder()
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    title.add_css_class("heading");
    let context = gtk::Label::builder()
        .label(&source.context)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .tooltip_text(&source.context)
        .build();
    context.add_css_class("dim-label");
    let heading = gtk::Box::new(gtk::Orientation::Vertical, 0);
    heading.append(&title);
    heading.append(&context);
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&heading));
    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    status.add_css_class("dim-label");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&preview.widget);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&status);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&root));
    window.set_content(Some(&toolbar));
    let directory = gio::File::for_uri(source.selected.location.uri())
        .parent()
        .map(|file| Location::new(file.uri()));
    let mut entries: Vec<_> = source
        .files
        .into_iter()
        .filter(|entry| {
            can_view(entry)
                && (source.ui.show_hidden || !entry.is_hidden)
                && gio::File::for_uri(entry.location.uri())
                    .parent()
                    .map(|file| Location::new(file.uri()))
                    == directory
                && entry.location != source.selected.location
        })
        .collect();
    entries.push(source.selected.clone());
    entries.sort_by(|left, right| left.compare(right, source.sort));
    let index = entries
        .iter()
        .position(|entry| entry.location == source.selected.location)
        .unwrap_or(0);
    let controller = gtk::EventControllerKey::new();
    let viewer = Rc::new(Viewer {
        window: window.downgrade(),
        preview,
        model,
        directory,
        files: RefCell::new(Files { entries, index }),
        title,
        status,
        resources: RefCell::new(source.resources),
        closed: Cell::new(false),
        force_refresh: Cell::new(false),
        directory_loading: Cell::new(false),
        keys: controller.clone(),
    });
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&viewer);
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        let Some(viewer) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let Some(window) = viewer.window.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if viewer.closed.get() || window.visible_dialog().is_some() {
            return glib::Propagation::Proceed;
        }
        match key_action(key, modifiers) {
            Some(Action::Close) => window.close(),
            Some(Action::File(offset)) => viewer.step(offset),
            Some(Action::Page(forward)) => {
                if !viewer.preview.change_document_page(forward) {
                    return glib::Propagation::Proceed;
                }
            }
            Some(Action::Full) => {
                if let Some(entry) = viewer.files.borrow().current().cloned() {
                    viewer.preview.schedule_full(entry, |_| {});
                }
            }
            Some(Action::Reload) => viewer.reload(true),
            Some(Action::Cancel) => {
                viewer.preview.cancel_loading();
            }
            None => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(viewer.keys.clone());
    let weak = Rc::downgrade(&viewer);
    window.connect_map(move |_| {
        if let Some(viewer) = weak.upgrade() {
            viewer.preview.focus_content();
        }
    });
    let close_viewer = viewer.clone();
    window.connect_close_request(move |_| {
        close_viewer.closed.set(true);
        close_viewer.model.cancel();
        close_viewer.preview.cancel();
        close_viewer.resources.borrow_mut().clear();
        glib::Propagation::Proceed
    });
    viewer.show_current(source.content);
    window.present();
    viewer.reload(false);
    (window, viewer)
}

impl Viewer {
    fn refresh_heading(&self) {
        let files = self.files.borrow();
        let name = files
            .current()
            .map_or("No files", |entry| entry.display_name.as_str());
        self.title.set_label(name);
        self.title
            .set_tooltip_text(files.current().map(|entry| entry.location.uri()));
        if let Some(window) = self.window.upgrade() {
            window.set_title(Some(&format!("{name} — PathPilot Viewer")));
        }
        let position = if files.entries.is_empty() {
            0
        } else {
            files.index + 1
        };
        self.status.set_label(&format!(
            "{position} / {} · j/k files · n/N pages · u full · Ctrl+R reload · q close",
            files.entries.len()
        ));
    }

    fn show_current(&self, content: Option<PreviewContent>) {
        self.preview.widget.set_focusable(true);
        self.preview.widget.grab_focus();
        self.refresh_heading();
        if let Some(entry) = self.files.borrow().current().cloned() {
            match content {
                Some(content) => self.preview.restore_content(&entry, content),
                None => self.preview.schedule(entry),
            }
        } else {
            self.preview.show_empty();
        }
    }

    fn step(&self, offset: i32) {
        let changed = self.files.borrow_mut().step(offset);
        if changed {
            self.show_current(None);
        }
    }

    fn reload(self: &Rc<Self>, force: bool) {
        let Some(directory) = self.directory.as_ref() else {
            return;
        };
        self.force_refresh.set(force);
        self.directory_loading.set(true);
        self.refresh_heading();
        if force {
            self.model.invalidate_cache(directory);
        }
        let weak = Rc::downgrade(self);
        let cached = self.model.load(directory, move |result| {
            if let Some(viewer) = weak.upgrade()
                && !viewer.closed.get()
            {
                viewer.directory_loaded(result);
            }
        });
        if cached {
            self.directory_loaded(Ok(()));
        }
    }

    fn directory_loaded(&self, result: Result<(), String>) {
        self.directory_loading.set(false);
        if let Err(error) = result {
            self.status
                .set_label(&format!("Could not refresh files: {error} · q close"));
            return;
        }
        let previous = self.files.borrow().current().cloned();
        self.files
            .borrow_mut()
            .replace(self.model.visible_entries());
        let current = self.files.borrow().current().cloned();
        if self.force_refresh.replace(false) || previous != current {
            if let Some(entry) = current.as_ref() {
                self.preview.invalidate(entry);
            }
            self.show_current(None);
        } else {
            self.refresh_heading();
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Close,
    File(i32),
    Page(bool),
    Full,
    Reload,
    Cancel,
}

fn key_action(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<Action> {
    let modifiers = modifiers
        & (gdk::ModifierType::SHIFT_MASK
            | gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::META_MASK);
    if let Some(forward) = document_page_direction(key, modifiers) {
        return Some(Action::Page(forward));
    }
    if modifiers == gdk::ModifierType::CONTROL_MASK {
        return match key {
            gdk::Key::q => Some(Action::Close),
            gdk::Key::r => Some(Action::Reload),
            _ => None,
        };
    }
    if !modifiers.is_empty() {
        return None;
    }
    match key {
        gdk::Key::q => Some(Action::Close),
        gdk::Key::j => Some(Action::File(1)),
        gdk::Key::k => Some(Action::File(-1)),
        gdk::Key::u => Some(Action::Full),
        gdk::Key::Escape => Some(Action::Cancel),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn entry(name: &str, kind: FileKind) -> FileEntry {
        FileEntry {
            location: Location::new(format!("file:///tmp/{name}")),
            display_name: name.to_owned(),
            kind,
            size: Some(10),
            modified: None,
            unix_mode: None,
            content_type: Some("text/plain".to_owned()),
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        }
    }

    #[test]
    fn files_keep_the_current_uri_when_refreshed_and_stop_at_boundaries() {
        let first = entry("a.txt", FileKind::Regular);
        let second = entry("b.txt", FileKind::Regular);
        let mut files = Files {
            entries: vec![first.clone(), second.clone()],
            index: 1,
        };
        assert!(!files.step(1));
        files.replace(vec![
            entry("folder", FileKind::Directory),
            second.clone(),
            first.clone(),
        ]);
        assert_eq!(files.index, 0);
        assert_eq!(files.current(), Some(&second));
        assert!(!files.step(-1));
        assert!(files.step(1));
        assert_eq!(files.current(), Some(&first));
        files.replace(Vec::new());
        assert!(files.current().is_none());
        assert!(!files.step(1));
    }

    #[test]
    fn viewer_keys_keep_files_separate_from_document_pages_and_native_scrolling() {
        let normal = gdk::ModifierType::empty();
        assert_eq!(key_action(gdk::Key::j, normal), Some(Action::File(1)));
        assert_eq!(key_action(gdk::Key::k, normal), Some(Action::File(-1)));
        assert_eq!(key_action(gdk::Key::n, normal), Some(Action::Page(true)));
        assert_eq!(
            key_action(gdk::Key::N, gdk::ModifierType::SHIFT_MASK),
            Some(Action::Page(false))
        );
        assert_eq!(
            key_action(gdk::Key::Page_Down, gdk::ModifierType::CONTROL_MASK),
            Some(Action::Page(true))
        );
        assert_eq!(key_action(gdk::Key::Page_Down, normal), None);
        assert_eq!(key_action(gdk::Key::Down, normal), None);
        assert_eq!(key_action(gdk::Key::q, normal), Some(Action::Close));
        assert_eq!(
            key_action(gdk::Key::j, gdk::ModifierType::CONTROL_MASK),
            None
        );
    }
    fn wait_until(condition: impl Fn() -> bool) {
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !condition() {
            assert!(
                std::time::Instant::now() < deadline,
                "viewer task timed out"
            );
            while context.pending() {
                context.iteration(false);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn key(viewer: &Viewer, key: gdk::Key, modifiers: gdk::ModifierType) {
        assert!(
            viewer
                .keys
                .emit_by_name::<bool>("key-pressed", &[&key, &0u32, &modifiers])
        );
    }

    fn page(preview: &PreviewPane, entry: &FileEntry) -> Option<u32> {
        match preview.snapshot(entry) {
            Some(PreviewContent::Document(document)) => Some(document.page_number),
            _ => None,
        }
    }

    #[allow(dead_code)] // Invoked by the dedicated main-thread integration harness.
    pub(crate) fn run_gui_check() {
        adw::init().unwrap();
        let application = adw::Application::builder()
            .application_id("io.github.RomanRcT.PathPilot.ViewerTest")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        application.register(None::<&gio::Cancellable>).unwrap();
        let owner = Rc::new(tempfile::tempdir().unwrap());
        let path = owner.path().to_owned();
        std::fs::write(
            path.join("00-paper.pdf"),
            include_bytes!("../tests/fixtures/two-pages.pdf"),
        )
        .unwrap();
        std::fs::write(
            path.join("01-readme.md"),
            include_str!("../tests/fixtures/markdown.md"),
        )
        .unwrap();
        std::fs::write(
            path.join("landscape.svg"),
            include_str!("../tests/fixtures/landscape.svg"),
        )
        .unwrap();
        std::fs::write(path.join("02-notes.txt"), "Text preview\nSecond line").unwrap();
        std::fs::write(path.join(".hidden.txt"), "hidden").unwrap();
        std::fs::create_dir(path.join("subdirectory")).unwrap();
        let model = DirectoryPane::new("Source files");
        let loaded = Rc::new(Cell::new(false));
        let received = loaded.clone();
        model.load(
            &Location::new(gio::File::for_path(&path).uri()),
            move |result| {
                result.unwrap();
                received.set(true);
            },
        );
        wait_until(|| loaded.get());
        let entries = model.visible_entries();
        let paper = entries
            .iter()
            .find(|entry| entry.display_name == "00-paper.pdf")
            .unwrap()
            .clone();
        let markdown = entries
            .iter()
            .find(|entry| entry.display_name == "01-readme.md")
            .unwrap()
            .clone();
        let text = entries
            .iter()
            .find(|entry| entry.display_name == "02-notes.txt")
            .unwrap()
            .clone();
        let source = PreviewPane::new(false, 0, "compact", model.remote_cache());
        source.set_preview_only(true);
        let source_window = adw::ApplicationWindow::builder()
            .application(&application)
            .default_width(600)
            .default_height(600)
            .build();
        source_window.set_content(Some(&source.widget));
        source_window.present();
        source.focus_content();
        source.schedule(paper.clone());
        wait_until(|| page(&source, &paper) == Some(1));
        assert!(source.change_document_page(true));
        wait_until(|| page(&source, &paper) == Some(2));
        let (window, viewer) = build(
            application.upcast_ref(),
            ViewerSource {
                selected: paper.clone(),
                files: entries,
                content: source.snapshot(&paper),
                ui: UiSettings::default(),
                sort: SortMode::default(),
                context: "Source folder".to_owned(),
                resources: vec![owner.clone()],
            },
        );
        wait_until(|| !viewer.directory_loading.get());
        assert!(window.transient_for().is_none());
        assert_eq!(
            page(&viewer.preview, &paper),
            Some(2),
            "detach preserves the displayed page"
        );
        assert_eq!(viewer.files.borrow().entries.len(), 4); // PDF, Markdown, text and SVG.
        let normal = gdk::ModifierType::empty();
        key(&viewer, gdk::Key::N, gdk::ModifierType::SHIFT_MASK);
        wait_until(|| page(&viewer.preview, &paper) == Some(1));
        assert_eq!(page(&source, &paper), Some(2), "page state is independent");
        key(&viewer, gdk::Key::n, normal);
        wait_until(|| page(&viewer.preview, &paper) == Some(2));
        // Moving the source to a different file and closing it leaves the viewer alive.
        source.schedule(text.clone());
        wait_until(|| source.snapshot(&text).is_some());
        assert_eq!(page(&viewer.preview, &paper), Some(2));
        source_window.close();
        source.cancel();
        model.cancel();
        drop(owner);
        assert!(path.exists(), "viewer retains the archive/staging resource");
        assert!(
            application
                .windows()
                .iter()
                .any(|candidate| candidate == &window)
        );
        key(&viewer, gdk::Key::j, normal);
        assert_eq!(viewer.files.borrow().current(), Some(&markdown));
        wait_until(|| {
            matches!(
                viewer.preview.snapshot(&markdown),
                Some(PreviewContent::Markdown(_))
            )
        });
        assert!(window.title().unwrap().contains("01-readme.md"));
        key(&viewer, gdk::Key::j, normal);
        assert_eq!(viewer.files.borrow().current(), Some(&text));
        wait_until(|| viewer.preview.snapshot(&text).is_some());
        key(&viewer, gdk::Key::k, normal);
        key(&viewer, gdk::Key::k, normal);
        wait_until(|| page(&viewer.preview, &paper) == Some(2));
        key(&viewer, gdk::Key::k, normal);
        assert_eq!(viewer.files.borrow().current(), Some(&paper));
        std::fs::write(path.join("03-new.txt"), "new file").unwrap();
        key(&viewer, gdk::Key::r, gdk::ModifierType::CONTROL_MASK);
        wait_until(|| !viewer.directory_loading.get());
        assert_eq!(viewer.files.borrow().entries.len(), 5);
        wait_until(|| page(&viewer.preview, &paper).is_some());
        if let Some(output) = std::env::var_os("PATHPILOT_TEST_VIEWER_SCREENSHOT") {
            let until = std::time::Instant::now() + std::time::Duration::from_millis(150);
            wait_until(|| std::time::Instant::now() >= until);
            let paintable = gtk::WidgetPaintable::new(Some(&window));
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
            let node = snapshot.to_node().unwrap();
            window
                .renderer()
                .unwrap()
                .render_texture(&node, None)
                .save_to_png(output)
                .unwrap();
        }
        key(&viewer, gdk::Key::q, normal);
        assert!(viewer.closed.get());
        assert!(!window.is_visible());
        assert!(
            !path.exists(),
            "staging is released when the last viewer closes"
        );
    }
}
