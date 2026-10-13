//! Window-local tabs keep their browser widgets and asynchronous state alive.
use super::*;

struct Tab {
    browser: Rc<Browser>,
    root: gtk::Box,
    controller: gtk::EventControllerKey,
    scroll: RefCell<Vec<(gtk::Adjustment, f64)>>,
}

pub(super) struct Workspace {
    window: glib::WeakRef<adw::ApplicationWindow>,
    settings: Rc<RefCell<pathpilot_config::Settings>>,
    settings_path: Option<std::path::PathBuf>,
    tabs: RefCell<Vec<Tab>>,
    active: Cell<usize>,
    switch_generation: Cell<u64>,
    stack: gtk::Stack,
    strip: gtk::Box,
    strip_scroller: gtk::ScrolledWindow,
    selection_hint: gtk::Label,
    selection_origin: Cell<Option<usize>>,
    selection_source: RefCell<Option<glib::SourceId>>,
    header: adw::HeaderBar,
    hidden_button: gtk::ToggleButton,
    terminal_button: gtk::ToggleButton,
    updating_header: Cell<bool>,
    clipboard: Rc<RefCell<Option<OperationClipboard>>>,
    prepared_copy: Rc<RefCell<Option<PreparedCopyManifest>>>,
    clipboard_staging: Rc<RefCell<Vec<tempfile::TempDir>>>,
}

impl Workspace {
    pub(super) fn new(
        window: &adw::ApplicationWindow,
        settings: Rc<RefCell<pathpilot_config::Settings>>,
        settings_path: Option<std::path::PathBuf>,
    ) -> Rc<Self> {
        let header = adw::HeaderBar::new();
        let hidden_button = gtk::ToggleButton::builder()
            .icon_name("view-reveal-symbolic")
            .tooltip_text("Show hidden items")
            .build();
        let terminal_button = gtk::ToggleButton::builder()
            .icon_name("utilities-terminal-symbolic")
            .tooltip_text("Show terminal (o t)")
            .build();
        header.pack_start(&hidden_button);
        header.pack_start(&terminal_button);
        let menu = gio::Menu::new();
        menu.append(Some("About PathPilot"), Some("app.about"));
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .tooltip_text("Main menu")
                .menu_model(&menu)
                .build(),
        );
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        strip.set_margin_start(8);
        strip.set_margin_end(8);
        let strip_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&strip)
            .visible(false)
            .build();
        let selection_hint = gtk::Label::builder().visible(false).build();
        selection_hint.add_css_class("accent");
        let stack = gtk::Stack::builder()
            .hexpand(true)
            .vexpand(true)
            .hhomogeneous(false)
            .vhomogeneous(false)
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&strip_scroller);
        root.append(&selection_hint);
        root.append(&stack);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&root));
        window.set_content(Some(&toolbar));
        let workspace = Rc::new(Self {
            window: window.downgrade(),
            settings,
            settings_path,
            tabs: RefCell::new(Vec::new()),
            active: Cell::new(0),
            switch_generation: Cell::new(0),
            stack,
            strip,
            strip_scroller,
            selection_hint,
            selection_origin: Cell::new(None),
            selection_source: RefCell::new(None),
            header,
            hidden_button,
            terminal_button,
            updating_header: Cell::new(false),
            clipboard: Rc::default(),
            prepared_copy: Rc::default(),
            clipboard_staging: Rc::default(),
        });
        let weak = Rc::downgrade(&workspace);
        workspace.hidden_button.connect_toggled(move |button| {
            if let Some(workspace) = weak.upgrade()
                && !workspace.updating_header.get()
                && let Some(browser) = workspace.active_browser()
            {
                browser.set_show_hidden(button.is_active());
            }
        });
        let weak = Rc::downgrade(&workspace);
        workspace.terminal_button.connect_toggled(move |button| {
            if let Some(workspace) = weak.upgrade()
                && !workspace.updating_header.get()
                && let Some(browser) = workspace.active_browser()
                && button.is_active() != browser.terminal_visible.get()
            {
                browser.toggle_terminal();
            }
        });
        let weak = Rc::downgrade(&workspace);
        window.connect_map(move |_| {
            if let Some(browser) = weak
                .upgrade()
                .and_then(|workspace| workspace.active_browser())
            {
                Self::focus_browser(&browser);
            }
        });
        workspace
    }

    fn active_browser(&self) -> Option<Rc<Browser>> {
        self.tabs
            .borrow()
            .get(self.active.get())
            .map(|tab| tab.browser.clone())
    }

    pub(super) fn is_active(&self, browser: &Rc<Browser>) -> bool {
        self.active_browser()
            .is_some_and(|active| Rc::ptr_eq(&active, browser))
    }

    pub(super) fn refresh_header(&self) {
        let Some(browser) = self.active_browser() else {
            return;
        };
        self.updating_header.set(true);
        self.header.set_title_widget(Some(&browser.location_label));
        self.hidden_button.set_active(browser.current.show_hidden());
        self.terminal_button
            .set_active(browser.terminal_visible.get());
        self.updating_header.set(false);
    }

    fn refresh_strip(self: &Rc<Self>) {
        while let Some(child) = self.strip.first_child() {
            self.strip.remove(&child);
        }
        let tabs = self.tabs.borrow();
        self.strip_scroller
            .set_visible(tabs.len() > 1 || self.selection_origin.get().is_some());
        for (index, tab) in tabs.iter().enumerate() {
            let location = tab.browser.navigation.borrow().current().clone();
            let file = gio::File::for_uri(location.uri());
            let name = file
                .basename()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| present_location(&location, None).compact);
            let label = gtk::Label::builder()
                .label(format!("{}  {}", index + 1, name))
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .max_width_chars(24)
                .build();
            let button = gtk::Button::builder()
                .child(&label)
                .tooltip_text(location.uri())
                .focus_on_click(false)
                .build();
            button.add_css_class("flat");
            if index == self.active.get() {
                button.add_css_class("pathpilot-tab-active");
            }
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(workspace) = weak.upgrade() {
                    workspace.finish_selection(false);
                    workspace.activate(index, true);
                }
            });
            self.strip.append(&button);
        }
        drop(tabs);
        // Keep the selected tab visible when the strip overflows.
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(workspace) = weak.upgrade() {
                let child = workspace
                    .strip
                    .observe_children()
                    .item(workspace.active.get() as u32)
                    .and_downcast::<gtk::Widget>();
                if let Some(bounds) = child.and_then(|child| child.compute_bounds(&workspace.strip))
                {
                    workspace
                        .strip_scroller
                        .hadjustment()
                        .clamp_page(bounds.x() as f64, (bounds.x() + bounds.width()) as f64);
                }
            }
        });
    }

    pub(super) fn add_tab(self: &Rc<Self>, initial: Location) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let mut browser = Browser::new(initial, self.settings.clone(), self.settings_path.clone());
        // No other strong reference exists until the tab is composed.
        let state = Rc::get_mut(&mut browser).expect("new browser is uniquely owned");
        state.operation_clipboard = self.clipboard.clone();
        state.prepared_copy = self.prepared_copy.clone();
        state.archive_clipboard_staging = self.clipboard_staging.clone();
        *browser.workspace.borrow_mut() = Rc::downgrade(self);
        let show_hidden = self.settings.borrow().ui.show_hidden;
        browser.parent.set_show_hidden(show_hidden);
        browser.current.set_show_hidden(show_hidden);
        browser.preview.set_show_hidden(show_hidden);
        browser.parent.set_sort_mode(browser.sort_mode.get());
        browser.current.set_sort_mode(browser.sort_mode.get());
        browser.preview.set_sort_mode(browser.sort_mode.get());
        browser.connect_editor_exit();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let content = gtk::Paned::new(gtk::Orientation::Vertical);
        content.set_start_child(Some(&three_column_layout(&browser)));
        content.set_end_child(Some(&browser.terminal_panel));
        content.set_resize_start_child(true);
        content.set_resize_end_child(true);
        content.set_shrink_start_child(false);
        content.set_position((self.settings.borrow().ui.window_height * 2 / 3).max(300));
        root.append(&content);
        let interaction = gtk::Box::new(gtk::Orientation::Vertical, 0);
        interaction.add_css_class("interaction-panel");
        let hints = gtk::Grid::builder()
            .halign(gtk::Align::Center)
            .column_spacing(12)
            .row_spacing(6)
            .visible(false)
            .build();
        hints.add_css_class("interaction-panel-content");
        interaction.append(&hints);
        interaction.append(&browser.input_bar);
        root.append(&interaction);
        root.append(&browser.operation_shelf);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&browser.status_bar);
        browser.connect();
        let controller = install_keyboard_controller(&window, Rc::downgrade(&browser), &hints);
        let weak = Rc::downgrade(self);
        browser.location_label.connect_label_notify(move |_| {
            if let Some(workspace) = weak.upgrade() {
                workspace.refresh_strip();
            }
        });
        let weak = Rc::downgrade(self);
        browser.terminal_panel.connect_visible_notify(move |_| {
            if let Some(workspace) = weak.upgrade() {
                workspace.refresh_header();
            }
        });
        self.stack.add_child(&root);
        let index = self.tabs.borrow().len();
        self.tabs.borrow_mut().push(Tab {
            browser: browser.clone(),
            root,
            controller,
            scroll: RefCell::new(Vec::new()),
        });
        self.activate(index, true);
        browser.initial_load();
        browser.restore_pane_layout();
    }

    pub(super) fn create_tab(self: &Rc<Self>) {
        let Some(browser) = self.active_browser() else {
            return;
        };
        // Archive extraction belongs to the original tab. Start beside the archive
        // rather than exposing another tab to a temporary directory it does not own.
        let location = browser
            .archive_sessions
            .borrow()
            .first()
            .and_then(|session| gio::File::for_uri(session.archive.uri()).parent())
            .map_or_else(
                || browser.navigation.borrow().current().clone(),
                |file| Location::new(file.uri()),
            );
        self.add_tab(location);
    }

    fn activate(self: &Rc<Self>, index: usize, focus: bool) {
        let Some((browser, root, mut scroll)) = self.tabs.borrow().get(index).map(|tab| {
            (
                tab.browser.clone(),
                tab.root.clone(),
                tab.scroll.borrow().clone(),
            )
        }) else {
            return;
        };
        let changed = self.active.get() != index;
        if changed {
            if let Some(tab) = self.tabs.borrow().get(self.active.get()) {
                let mut scroll = Vec::new();
                Self::collect_scroll(&tab.root, &mut scroll);
                *tab.scroll.borrow_mut() = scroll;
            }
            if let Some(window) = self.window.upgrade() {
                gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
            }
        }
        if !changed && focus {
            scroll.clear();
            Self::collect_scroll(&root, &mut scroll);
        }
        let generation = self.switch_generation.get().wrapping_add(1);
        self.switch_generation.set(generation);
        self.active.set(index);
        self.stack.set_visible_child(&root);
        self.refresh_header();
        self.refresh_strip();
        if focus {
            Self::focus_browser(&browser);
        }
        if (changed || focus) && !scroll.is_empty() {
            for (adjustment, value) in &scroll {
                adjustment.set_value(*value);
            }
            let weak = Rc::downgrade(self);
            let browser = Rc::downgrade(&browser);
            let frames = Cell::new(0);
            root.add_tick_callback(move |_, _| {
                let active =
                    weak.upgrade()
                        .zip(browser.upgrade())
                        .is_some_and(|(workspace, browser)| {
                            workspace.switch_generation.get() == generation
                                && workspace.is_active(&browser)
                        });
                if !active {
                    return glib::ControlFlow::Break;
                }
                frames.set(frames.get() + 1);
                if frames.get() < 2 {
                    return glib::ControlFlow::Continue;
                }
                // GTK may scroll the focused row during the first layout frame.
                for (adjustment, value) in &scroll {
                    adjustment.set_value(*value);
                }
                glib::ControlFlow::Break
            });
        }
    }

    fn collect_scroll(widget: &impl IsA<gtk::Widget>, scroll: &mut Vec<(gtk::Adjustment, f64)>) {
        if let Some(scroller) = widget.as_ref().downcast_ref::<gtk::ScrolledWindow>() {
            for adjustment in [scroller.hadjustment(), scroller.vadjustment()] {
                let value = adjustment.value();
                scroll.push((adjustment, value));
            }
        }
        let mut child = widget.as_ref().first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            Self::collect_scroll(&widget, scroll);
        }
    }

    fn focus_browser(browser: &Browser) {
        if browser.editing.get() {
            browser.preview.focus_editor();
        } else if browser.mode.borrow().text_input().is_some()
            || *browser.mode.borrow() == AppMode::Command
        {
            browser.input_entry.grab_focus();
        } else if browser.directory_loading.get() || !browser.current.list.grab_focus() {
            browser.focus_current_pane();
        }
    }

    pub(super) fn select_number(self: &Rc<Self>, index: usize) {
        self.activate(index, true);
    }

    pub(super) fn close_active(self: &Rc<Self>) {
        if let Some(browser) = self.active_browser() {
            self.close_browser(&browser);
        }
    }

    pub(super) fn close_browser(self: &Rc<Self>, browser: &Rc<Browser>) {
        if self.tabs.borrow().len() == 1 {
            browser
                .status
                .set_label("NORMAL  Keep at least one tab open");
            return;
        }
        if browser.editing.get() || browser.active_operation.borrow().is_some() {
            browser
                .status
                .set_label("NORMAL  Finish editing or the file operation before closing this tab");
            return;
        }
        if !browser.archive_sessions.borrow().is_empty() {
            if let Some(window) = self.window.upgrade() {
                browser.close_tab_after_archive.set(true);
                browser.leave_archive(&window);
            }
            return;
        }
        let Some(index) = self
            .tabs
            .borrow()
            .iter()
            .position(|tab| Rc::ptr_eq(&tab.browser, browser))
        else {
            return;
        };
        self.finish_selection(false);
        let tab = self.tabs.borrow_mut().remove(index);
        tab.browser.cancel();
        if let Some(window) = self.window.upgrade() {
            window.remove_controller(&tab.controller);
        }
        self.stack.remove(&tab.root);
        let active = self.active.get();
        let next = if index < active {
            active - 1
        } else {
            active.min(self.tabs.borrow().len() - 1)
        };
        self.active.set(usize::MAX);
        self.activate(next, true);
    }

    pub(super) fn begin_selection(self: &Rc<Self>) {
        if self.tabs.borrow().len() < 2 {
            return;
        }
        self.selection_origin.set(Some(self.active.get()));
        self.selection_hint.set_visible(true);
        self.selection_hint
            .set_label(if self.settings.borrow().ui.tab_switch_delay_ms == 0 {
                "TABS  j next · k previous · Enter accept · Esc cancel"
            } else {
                "TABS  j next · k previous · pause to accept · Enter accept · Esc cancel"
            });
        self.refresh_strip();
    }

    fn arm_selection_commit(self: &Rc<Self>) {
        if let Some(source) = self.selection_source.borrow_mut().take() {
            source.remove();
        }
        let delay = self.settings.borrow().ui.tab_switch_delay_ms;
        if delay == 0 {
            return;
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            if let Some(workspace) = weak.upgrade() {
                workspace.selection_source.borrow_mut().take();
                workspace.finish_selection(false);
            }
        });
        *self.selection_source.borrow_mut() = Some(source);
    }

    fn finish_selection(self: &Rc<Self>, cancel: bool) {
        if let Some(source) = self.selection_source.borrow_mut().take() {
            source.remove();
        }
        let Some(origin) = self.selection_origin.take() else {
            return;
        };
        self.selection_hint.set_visible(false);
        self.activate(if cancel { origin } else { self.active.get() }, true);
    }

    pub(super) fn handle_selection_key(
        self: &Rc<Self>,
        key: gdk::Key,
        modifiers: gdk::ModifierType,
    ) -> bool {
        if self.selection_origin.get().is_none() {
            return false;
        }
        match key {
            gdk::Key::Escape => self.finish_selection(true),
            gdk::Key::Return | gdk::Key::KP_Enter => self.finish_selection(false),
            gdk::Key::j | gdk::Key::k
                if !modifiers.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK,
                ) =>
            {
                let count = self.tabs.borrow().len();
                let index = if key == gdk::Key::j {
                    (self.active.get() + 1) % count
                } else {
                    (self.active.get() + count - 1) % count
                };
                self.activate(index, false);
                self.arm_selection_commit();
            }
            _ => {}
        }
        true
    }

    pub(super) fn close_window(self: &Rc<Self>) -> glib::Propagation {
        self.finish_selection(false);
        let Some(window) = self.window.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let archive_browser = self
            .tabs
            .borrow()
            .iter()
            .enumerate()
            .find(|(_, tab)| !tab.browser.archive_sessions.borrow().is_empty())
            .map(|(index, tab)| (index, tab.browser.clone()));
        if let Some((index, browser)) = archive_browser {
            self.activate(index, true);
            browser.close_after_archive.set(true);
            browser.leave_archive(&window);
            return glib::Propagation::Stop;
        }
        if let Some(browser) = self.active_browser() {
            browser.persist_window_state(&window);
        }
        for tab in self.tabs.borrow().iter() {
            tab.browser.cancel();
        }
        glib::Propagation::Proceed
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Some(source) = self.selection_source.get_mut().take() {
            source.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pump_for(duration: Duration) {
        let context = glib::MainContext::default();
        let started = Instant::now();
        while started.elapsed() < duration {
            while context.pending() {
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_until(condition: impl Fn() -> bool) {
        let started = Instant::now();
        while !condition() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "GTK task timed out"
            );
            pump_for(Duration::from_millis(10));
        }
    }

    fn key(workspace: &Workspace, key: gdk::Key) {
        let controller = workspace.tabs.borrow()[workspace.active.get()]
            .controller
            .clone();
        let result = controller
            .emit_by_name::<bool>("key-pressed", &[&key, &0u32, &gdk::ModifierType::empty()]);
        assert!(result);
    }

    fn sequence(workspace: &Workspace, suffix: gdk::Key) {
        key(workspace, gdk::Key::t);
        key(workspace, suffix);
    }

    #[test]
    #[ignore = "requires a graphical session; run this test separately with --ignored"]
    fn workspace_tabs_keep_state_share_clipboard_and_commit_selection() {
        adw::init().unwrap();
        install_hint_css();
        let app = adw::Application::builder()
            .application_id("io.github.RomanRcT.PathPilot.TabsTest")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(None::<&gio::Cancellable>).unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let first_path = temporary.path().join("first");
        let second_path = temporary.path().join("second");
        std::fs::create_dir_all(&first_path).unwrap();
        std::fs::create_dir_all(&second_path).unwrap();
        for index in 0..150 {
            std::fs::write(first_path.join(format!("file-{index:03}.txt")), "test text").unwrap();
        }
        let first_location = Location::new(gio::File::for_path(&first_path).uri());
        let second_location = Location::new(gio::File::for_path(&second_path).uri());
        let settings = Rc::new(RefCell::new(pathpilot_config::Settings::default()));
        let window = adw::ApplicationWindow::builder()
            .application(&app)
            .default_width(1100)
            .default_height(700)
            .build();
        let workspace = Workspace::new(&window, settings.clone(), None);
        workspace.add_tab(first_location.clone());
        window.present();
        let first = workspace.active_browser().unwrap();
        wait_until(|| !first.directory_loading.get());
        pump_for(Duration::from_millis(100));
        first.current.select_position(40);
        first.current.toggle_selection();
        let selected = first.current.selected_entries();
        let original_cursor = first.current.cursor_position();
        let scroller = first
            .current
            .widget
            .first_child()
            .unwrap()
            .next_sibling()
            .unwrap()
            .next_sibling()
            .unwrap()
            .downcast::<gtk::ScrolledWindow>()
            .unwrap();
        scroller.vadjustment().set_value(300.0);
        pump_for(Duration::from_millis(50));
        let scroll = scroller.vadjustment().value();
        assert!(scroll > 0.0);
        first.store_operation_clipboard(ClipboardAction::Copy);
        let copied = first.current.selected_entries()[0].display_name.clone();
        sequence(&workspace, gdk::Key::c);
        assert_eq!(workspace.tabs.borrow().len(), 2);
        assert_eq!(workspace.active.get(), 1);
        let second = workspace.active_browser().unwrap();
        wait_until(|| !second.directory_loading.get());
        assert_eq!(second.navigation.borrow().current(), &first_location);
        second.navigate_to(second_location.clone(), None);
        wait_until(|| !second.directory_loading.get());
        assert!(Rc::ptr_eq(
            &first.operation_clipboard,
            &second.operation_clipboard
        ));
        second.paste_operation_clipboard(&window);
        wait_until(|| {
            second.active_operation.borrow().is_none() && second_path.join(&copied).exists()
        });
        sequence(&workspace, gdk::Key::_1);
        assert_eq!(workspace.active.get(), 0);
        pump_for(Duration::from_millis(50));
        assert_eq!(first.current.cursor_position(), original_cursor);
        assert_eq!(
            first
                .current
                .selected_entries()
                .iter()
                .map(|entry| &entry.location)
                .collect::<Vec<_>>(),
            selected
                .iter()
                .map(|entry| &entry.location)
                .collect::<Vec<_>>()
        );
        // Theme borders and pixel rounding can shift the viewport by one pixel.
        wait_until(|| (scroller.vadjustment().value() - scroll).abs() <= 2.0);
        // Enter-only selection wraps, blocks file commands, and cancels to the origin.
        settings.borrow_mut().ui.tab_switch_delay_ms = 0;
        sequence(&workspace, gdk::Key::s);
        key(&workspace, gdk::Key::k);
        assert_eq!(workspace.active.get(), 1);
        key(&workspace, gdk::Key::d);
        assert!(!second.confirmation_pending());
        key(&workspace, gdk::Key::j);
        assert_eq!(workspace.active.get(), 0);
        key(&workspace, gdk::Key::j);
        pump_for(Duration::from_millis(550));
        assert!(workspace.selection_origin.get().is_some());
        key(&workspace, gdk::Key::Escape);
        assert_eq!(workspace.active.get(), 0);
        sequence(&workspace, gdk::Key::s);
        key(&workspace, gdk::Key::j);
        key(&workspace, gdk::Key::Return);
        assert!(workspace.selection_origin.get().is_none());
        assert_eq!(workspace.active.get(), 1);
        // Repeated keys restart the timer instead of committing after the first key.
        settings.borrow_mut().ui.tab_switch_delay_ms = 500;
        sequence(&workspace, gdk::Key::s);
        key(&workspace, gdk::Key::k);
        pump_for(Duration::from_millis(300));
        key(&workspace, gdk::Key::j);
        pump_for(Duration::from_millis(300));
        assert!(workspace.selection_origin.get().is_some());
        wait_until(|| workspace.selection_origin.get().is_none());
        assert_eq!(workspace.active.get(), 1);
        let focus = gtk::prelude::GtkWindowExt::focus(&window).expect("active pane has focus");
        assert!(focus == second.current.widget || focus.is_ancestor(&second.current.widget));
        // A cancelled timer cannot end a subsequent Enter-only selection.
        sequence(&workspace, gdk::Key::s);
        key(&workspace, gdk::Key::k);
        key(&workspace, gdk::Key::Escape);
        settings.borrow_mut().ui.tab_switch_delay_ms = 0;
        sequence(&workspace, gdk::Key::s);
        pump_for(Duration::from_millis(550));
        assert!(workspace.selection_origin.get().is_some());
        key(&workspace, gdk::Key::Return);
        if let Some(path) = std::env::var_os("PATHPILOT_TEST_TABS_SCREENSHOT") {
            pump_for(Duration::from_millis(100));
            let paintable = gtk::WidgetPaintable::new(Some(&window));
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
            let node = snapshot.to_node().expect("window rendered");
            window
                .renderer()
                .unwrap()
                .render_texture(&node, None)
                .save_to_png(path)
                .unwrap();
        }
        // Closing chooses the adjacent tab, cleans up, and never closes the last tab.
        sequence(&workspace, gdk::Key::x);
        assert_eq!(workspace.tabs.borrow().len(), 1);
        assert_eq!(workspace.active.get(), 0);
        sequence(&workspace, gdk::Key::x);
        assert_eq!(workspace.tabs.borrow().len(), 1);
        assert!(!workspace.strip_scroller.is_visible());
        sequence(&workspace, gdk::Key::s);
        assert!(workspace.selection_origin.get().is_none());
        assert!(workspace.selection_source.borrow().is_none());
        assert!(!workspace.selection_hint.is_visible());
        assert!(!workspace.strip_scroller.is_visible());
        sequence(&workspace, gdk::Key::_9);
        assert_eq!(workspace.active.get(), 0);
        // Completing a move in another tab must not clear a newer clipboard.
        let third_path = temporary.path().join("third");
        std::fs::create_dir(&third_path).unwrap();
        first.current.select_position(40);
        first.store_operation_clipboard(ClipboardAction::Move);
        let moving_name = first.operation_clipboard.borrow().as_ref().unwrap().items[0]
            .display_name
            .clone();
        workspace.add_tab(Location::new(gio::File::for_path(&third_path).uri()));
        let moving_browser = workspace.active_browser().unwrap();
        wait_until(|| !moving_browser.directory_loading.get());
        moving_browser.paste_operation_clipboard(&window);
        first.current.select_position(41);
        first.store_operation_clipboard(ClipboardAction::Copy);
        let newer_clipboard = first.operation_clipboard.borrow().clone();
        wait_until(|| {
            moving_browser.active_operation.borrow().is_none()
                && third_path.join(&moving_name).exists()
        });
        assert_eq!(*workspace.clipboard.borrow(), newer_clipboard);
        workspace.close_active();
        assert_eq!(workspace.tabs.borrow().len(), 1);
        assert_eq!(workspace.close_window(), glib::Propagation::Proceed);
        window.close();
    }
}
