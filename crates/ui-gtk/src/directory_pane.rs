use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use gtk::{gio, glib, prelude::*};
use pathpilot_core::{
    FileEntry, FileKind, Generation, GenerationTracker, Location, SortMode, present_location,
};
use pathpilot_fs_local::{DirectoryEvent, load_directory};
use tracing::{debug, info, warn};

const REMOTE_CACHE_CAPACITY: usize = 32;

#[derive(Default)]
pub(crate) struct DirectoryCache {
    entries: HashMap<String, Vec<FileEntry>>,
    order: VecDeque<String>,
    in_flight: HashMap<String, InFlightDirectory>,
    next_subscriber: u64,
}

pub(crate) type SharedDirectoryCache = Rc<RefCell<DirectoryCache>>;
type RemoteLoadSubscriber = Box<dyn Fn(Result<Vec<FileEntry>, String>)>;

struct InFlightDirectory {
    subscribers: HashMap<u64, RemoteLoadSubscriber>,
}

impl DirectoryCache {
    fn get(&mut self, uri: &str) -> Option<Vec<FileEntry>> {
        let entries = self.entries.get(uri)?.clone();
        self.order.retain(|existing| existing != uri);
        self.order.push_back(uri.to_owned());
        Some(entries)
    }

    fn insert(&mut self, uri: String, entries: Vec<FileEntry>) {
        self.order.retain(|existing| existing != &uri);
        self.order.push_back(uri.clone());
        self.entries.insert(uri, entries);
        while self.order.len() > REMOTE_CACHE_CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.entries.remove(&expired);
            }
        }
    }

    fn remove(&mut self, uri: &str) {
        self.entries.remove(uri);
        self.order.retain(|existing| existing != uri);
    }

    fn subscribe(
        &mut self,
        uri: &str,
        subscriber: RemoteLoadSubscriber,
    ) -> Result<u64, RemoteLoadSubscriber> {
        let Some(load) = self.in_flight.get_mut(uri) else {
            return Err(subscriber);
        };
        let id = self.next_subscriber;
        self.next_subscriber = self.next_subscriber.wrapping_add(1);
        load.subscribers.insert(id, subscriber);
        Ok(id)
    }

    fn begin(&mut self, uri: String, subscriber: RemoteLoadSubscriber) -> u64 {
        let id = self.next_subscriber;
        self.next_subscriber = self.next_subscriber.wrapping_add(1);
        self.in_flight.insert(
            uri,
            InFlightDirectory {
                subscribers: HashMap::from([(id, subscriber)]),
            },
        );
        id
    }

    fn unsubscribe(&mut self, uri: &str, id: u64) {
        let became_orphaned = self.in_flight.get_mut(uri).is_some_and(|load| {
            load.subscribers.remove(&id);
            load.subscribers.is_empty()
        });
        if became_orphaned {
            debug!(
                location = uri,
                "remote directory load continuing as cache prefetch"
            );
        }
    }

    fn finish(&mut self, uri: &str) -> Vec<RemoteLoadSubscriber> {
        self.in_flight
            .remove(uri)
            .map(|load| load.subscribers.into_values().collect())
            .unwrap_or_default()
    }
}

#[derive(Default)]
struct LoadState {
    generation: RefCell<GenerationTracker>,
    cancellable: RefCell<Option<gio::Cancellable>>,
    pending_entries: RefCell<Vec<FileEntry>>,
    serving_cache: Cell<bool>,
    remote_subscription: RefCell<Option<(String, u64)>>,
}

#[derive(Clone)]
pub struct DirectoryPane {
    pub widget: gtk::Box,
    pub list: gtk::ListView,
    pub selection: gtk::MultiSelection,
    store: gio::ListStore,
    sorter: gtk::CustomSorter,
    sort_mode: Rc<Cell<SortMode>>,
    title: gtk::Label,
    role: String,
    status: gtk::Label,
    load_state: Rc<LoadState>,
    remote_cache: SharedDirectoryCache,
    cursor: Rc<Cell<u32>>,
    visual_anchor: Rc<Cell<Option<u32>>>,
    changing_selection: Rc<Cell<bool>>,
    independent_selection: Rc<Cell<bool>>,
    show_hidden: Rc<Cell<bool>>,
    visible_rows: Rc<RefCell<Vec<(u32, gtk::Box)>>>,
    independent_selected: Rc<RefCell<HashSet<u32>>>,
}

impl DirectoryPane {
    pub fn new(role: &str) -> Self {
        Self::with_remote_cache(role, Rc::new(RefCell::new(DirectoryCache::default())))
    }

    pub(crate) fn with_remote_cache(role: &str, remote_cache: SharedDirectoryCache) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let sort_mode = Rc::new(Cell::new(SortMode::default()));
        let sorter = gtk::CustomSorter::new({
            let sort_mode = sort_mode.clone();
            move |left, right| {
                let left = left
                    .downcast_ref::<glib::BoxedAnyObject>()
                    .expect("directory model contains BoxedAnyObject");
                let right = right
                    .downcast_ref::<glib::BoxedAnyObject>()
                    .expect("directory model contains BoxedAnyObject");
                left.borrow::<FileEntry>()
                    .compare(&right.borrow::<FileEntry>(), sort_mode.get())
                    .into()
            }
        });
        let sorted = gtk::SortListModel::new(Some(store.clone()), Some(sorter.clone()));
        let selection = gtk::MultiSelection::new(Some(sorted));
        let cursor = Rc::new(Cell::new(0));
        let visual_anchor = Rc::new(Cell::new(None::<u32>));
        let changing_selection = Rc::new(Cell::new(false));
        let independent_selection = Rc::new(Cell::new(false));
        let visible_rows = Rc::new(RefCell::new(Vec::new()));
        let independent_selected = Rc::new(RefCell::new(HashSet::new()));
        selection.connect_selection_changed({
            let cursor = cursor.clone();
            let visual_anchor = visual_anchor.clone();
            let changing_selection = changing_selection.clone();
            move |selection, position, n_items| {
                if changing_selection.get() || selection.n_items() == 0 || n_items == 0 {
                    return;
                }
                let end = position.saturating_add(n_items).min(selection.n_items());
                let candidate = (position..end)
                    .find(|position| selection.is_selected(*position))
                    .unwrap_or(position)
                    .min(selection.n_items() - 1);
                cursor.set(candidate);
                changing_selection.set(true);
                if let Some(anchor) = visual_anchor.get() {
                    let start = anchor.min(candidate);
                    selection.select_range(start, anchor.abs_diff(candidate) + 1, true);
                } else {
                    selection.select_item(candidate, true);
                }
                changing_selection.set(false);
            }
        });

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            item.downcast_ref::<gtk::ListItem>()
                .expect("factory setup receives ListItem")
                .set_child(Some(&create_row()));
        });
        {
            let visible_rows = visible_rows.clone();
            factory.connect_bind(move |_, item| bind_row(item, &visible_rows));
        }
        {
            let visible_rows = visible_rows.clone();
            factory.connect_unbind(move |_, item| {
                if let Some(row) = item
                    .downcast_ref::<gtk::ListItem>()
                    .and_then(|item| item.child())
                    .and_downcast::<gtk::Box>()
                {
                    visible_rows
                        .borrow_mut()
                        .retain(|(_, existing)| existing != &row);
                }
            });
        }

        let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
        {
            let visible_rows = visible_rows.clone();
            let selection = selection.clone();
            let cursor = cursor.clone();
            let independent_selected = independent_selected.clone();
            let independent_selection = independent_selection.clone();
            let visual_anchor = visual_anchor.clone();
            selection.clone().connect_selection_changed(move |_, _, _| {
                refresh_row_states(
                    &visible_rows,
                    &selection,
                    &independent_selected,
                    independent_selection.get(),
                    independent_selection.get() || visual_anchor.get().is_some(),
                    cursor.get(),
                );
            });
        }
        list.set_single_click_activate(false);
        list.set_vexpand(true);
        let scroller = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&list)
            .build();
        let title = gtk::Label::builder()
            .label(role)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .margin_start(8)
            .margin_end(8)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        title.add_css_class("heading");
        let status = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .margin_start(8)
            .margin_end(8)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        status.add_css_class("dim-label");
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&title);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        widget.append(&scroller);
        widget.append(&status);

        Self {
            widget,
            list,
            selection,
            store,
            sorter,
            sort_mode,
            title,
            role: role.to_owned(),
            status,
            load_state: Rc::new(LoadState::default()),
            remote_cache,
            cursor,
            visual_anchor,
            changing_selection,
            independent_selection,
            show_hidden: Rc::new(Cell::new(false)),
            visible_rows,
            independent_selected,
        }
    }

    pub fn set_show_hidden(&self, show_hidden: bool) {
        self.show_hidden.set(show_hidden);
    }

    pub fn set_sort_mode(&self, mode: SortMode) {
        if self.sort_mode.replace(mode) == mode {
            return;
        }
        let selected = self.selected_entry().map(|entry| entry.location);
        self.sorter.changed(gtk::SorterChange::Different);
        if let Some(location) = selected {
            self.select_location(&location);
        }
    }

    pub fn load(
        &self,
        location: &Location,
        on_finished: impl Fn(Result<(), String>) + 'static,
    ) -> bool {
        self.cancel();
        self.store.remove_all();
        self.independent_selection.set(false);
        self.independent_selected.borrow_mut().clear();
        let presentation = present_location(
            location,
            std::env::var_os("HOME")
                .as_deref()
                .map(std::path::Path::new),
        );
        self.title
            .set_label(&format!("{}  {}", self.role, presentation.compact));
        self.title.set_tooltip_text(Some(&presentation.full));
        let location_uri = location.uri().to_owned();
        let is_remote = !gio::File::for_uri(&location_uri).is_native();
        let cached = is_remote
            .then(|| self.remote_cache.borrow_mut().get(&location_uri))
            .flatten();
        let used_cache = cached.is_some();
        self.load_state.pending_entries.borrow_mut().clear();
        self.load_state.serving_cache.set(cached.is_some());
        if let Some(entries) = cached {
            info!(
                location = location_uri,
                pane = self.role,
                entry_count = entries.len(),
                "remote directory cache hit"
            );
            let objects = self.visible_objects(entries);
            self.store.extend_from_slice(&objects);
            self.status.set_label(&format!(
                "{} cached entries · Ctrl+R refreshes",
                self.store.n_items()
            ));
            return true;
        } else {
            self.status.set_label("Loading…");
        }
        let generation = self.load_state.generation.borrow_mut().advance();
        let started = Instant::now();
        let on_finished: Rc<dyn Fn(Result<(), String>)> = Rc::new(on_finished);
        let pane = self.clone();

        if is_remote {
            self.load_remote(location, location_uri, generation, started, on_finished);
            return false;
        }

        let cancellable = load_directory(
            location,
            generation,
            glib::clone!(
                #[strong]
                pane,
                #[strong]
                on_finished,
                move |event| {
                    let event_generation = match &event {
                        DirectoryEvent::Batch { generation, .. }
                        | DirectoryEvent::Finished { generation }
                        | DirectoryEvent::Failed { generation, .. } => *generation,
                    };
                    if !pane
                        .load_state
                        .generation
                        .borrow()
                        .accepts(event_generation)
                    {
                        debug!(
                            generation = event_generation.value(),
                            "discarding stale pane result"
                        );
                        return;
                    }

                    match event {
                        DirectoryEvent::Batch { entries, .. } => {
                            pane.load_state
                                .pending_entries
                                .borrow_mut()
                                .extend(entries.iter().cloned());
                            if !pane.load_state.serving_cache.get() {
                                let objects = pane.visible_objects(entries);
                                pane.store.extend_from_slice(&objects);
                            }
                            pane.status
                                .set_label(&format!("Loading… {} entries", pane.store.n_items()));
                        }
                        DirectoryEvent::Finished { .. } => {
                            pane.load_state.cancellable.borrow_mut().take();
                            let entries = pane
                                .load_state
                                .pending_entries
                                .borrow_mut()
                                .drain(..)
                                .collect::<Vec<_>>();
                            if is_remote {
                                pane.remote_cache
                                    .borrow_mut()
                                    .insert(location_uri.clone(), entries.clone());
                            }
                            if pane.load_state.serving_cache.replace(false) {
                                pane.store.remove_all();
                                let objects = pane.visible_objects(entries);
                                pane.store.extend_from_slice(&objects);
                            }
                            pane.status
                                .set_label(&format!("{} entries", pane.store.n_items()));
                            info!(
                                generation = event_generation.value(),
                                entry_count = pane.store.n_items(),
                                elapsed_ms = started.elapsed().as_millis(),
                                location = location_uri,
                                pane = pane.role,
                                "directory pane load finished"
                            );
                            on_finished(Ok(()));
                        }
                        DirectoryEvent::Failed { message, .. } => {
                            pane.load_state.cancellable.borrow_mut().take();
                            pane.status.set_label(&format!("Could not load: {message}"));
                            warn!(
                                generation = event_generation.value(),
                                location = location_uri,
                                error = message,
                                "directory pane load failed"
                            );
                            on_finished(Err(message));
                        }
                    }
                }
            ),
        );
        *self.load_state.cancellable.borrow_mut() = Some(cancellable);
        used_cache
    }

    pub fn load_cached(&self, location: &Location) -> bool {
        self.cancel();
        self.store.remove_all();
        self.independent_selection.set(false);
        self.independent_selected.borrow_mut().clear();
        let Some(entries) = self.remote_cache.borrow_mut().get(location.uri()) else {
            return false;
        };
        let presentation = present_location(
            location,
            std::env::var_os("HOME")
                .as_deref()
                .map(std::path::Path::new),
        );
        self.title
            .set_label(&format!("{}  {}", self.role, presentation.compact));
        self.title.set_tooltip_text(Some(&presentation.full));
        let entry_count = entries.len();
        let objects = self.visible_objects(entries);
        self.store.extend_from_slice(&objects);
        self.status
            .set_label(&format!("{} cached entries", self.store.n_items()));
        info!(
            location = location.uri(),
            pane = self.role,
            entry_count,
            "remote parent cache hit"
        );
        true
    }

    fn load_remote(
        &self,
        location: &Location,
        location_uri: String,
        generation: Generation,
        started: Instant,
        on_finished: Rc<dyn Fn(Result<(), String>)>,
    ) {
        let pane = self.clone();
        let subscriber: RemoteLoadSubscriber = Box::new(move |result| {
            pane.load_state.remote_subscription.borrow_mut().take();
            if !pane.load_state.generation.borrow().accepts(generation) {
                debug!(
                    generation = generation.value(),
                    location = location_uri,
                    pane = pane.role,
                    "discarding stale shared directory result"
                );
                return;
            }

            match result {
                Ok(entries) => {
                    pane.store.remove_all();
                    let objects = pane.visible_objects(entries);
                    pane.store.extend_from_slice(&objects);
                    pane.status
                        .set_label(&format!("{} entries", pane.store.n_items()));
                    info!(
                        generation = generation.value(),
                        entry_count = pane.store.n_items(),
                        elapsed_ms = started.elapsed().as_millis(),
                        location = location_uri,
                        pane = pane.role,
                        "directory pane load finished"
                    );
                    on_finished(Ok(()));
                }
                Err(message) => {
                    pane.status.set_label(&format!("Could not load: {message}"));
                    warn!(
                        generation = generation.value(),
                        location = location_uri,
                        pane = pane.role,
                        error = message,
                        "directory pane load failed"
                    );
                    on_finished(Err(message));
                }
            }
        });

        let subscriber = match self
            .remote_cache
            .borrow_mut()
            .subscribe(location.uri(), subscriber)
        {
            Ok(id) => {
                *self.load_state.remote_subscription.borrow_mut() =
                    Some((location.uri().to_owned(), id));
                info!(
                    location = location.uri(),
                    pane = self.role,
                    "joined in-flight remote directory load"
                );
                return;
            }
            Err(subscriber) => subscriber,
        };

        let subscriber_id = self
            .remote_cache
            .borrow_mut()
            .begin(location.uri().to_owned(), subscriber);
        *self.load_state.remote_subscription.borrow_mut() =
            Some((location.uri().to_owned(), subscriber_id));
        let entries = Rc::new(RefCell::new(Vec::new()));
        let remote_cache = self.remote_cache.clone();
        let request_uri = location.uri().to_owned();
        let _ = load_directory(location, generation, move |event| match event {
            DirectoryEvent::Batch { entries: batch, .. } => {
                entries.borrow_mut().extend(batch);
            }
            DirectoryEvent::Finished { .. } => {
                let result_entries = entries.borrow_mut().drain(..).collect::<Vec<_>>();
                let subscribers = {
                    let mut cache = remote_cache.borrow_mut();
                    cache.insert(request_uri.clone(), result_entries.clone());
                    cache.finish(&request_uri)
                };
                for subscriber in subscribers {
                    subscriber(Ok(result_entries.clone()));
                }
            }
            DirectoryEvent::Failed { message, .. } => {
                let subscribers = remote_cache.borrow_mut().finish(&request_uri);
                for subscriber in subscribers {
                    subscriber(Err(message.clone()));
                }
            }
        });
    }

    pub fn show_message(&self, title: &str, message: &str) {
        self.cancel();
        self.store.remove_all();
        self.title.set_label(title);
        self.title.set_tooltip_text(None);
        self.status.set_label(message);
    }

    pub fn invalidate_cache(&self, location: &Location) {
        self.remote_cache.borrow_mut().remove(location.uri());
    }

    pub(crate) fn remote_cache(&self) -> SharedDirectoryCache {
        self.remote_cache.clone()
    }

    fn visible_objects(&self, entries: Vec<FileEntry>) -> Vec<glib::BoxedAnyObject> {
        entries
            .into_iter()
            .filter(|entry| self.show_hidden.get() || !entry.is_hidden)
            .map(glib::BoxedAnyObject::new)
            .collect()
    }

    pub fn cancel(&self) {
        self.load_state.generation.borrow_mut().advance();
        if let Some((uri, id)) = self.load_state.remote_subscription.borrow_mut().take() {
            self.remote_cache.borrow_mut().unsubscribe(&uri, id);
        }
        if let Some(cancellable) = self.load_state.cancellable.borrow_mut().take() {
            cancellable.cancel();
        }
    }

    pub fn selected_entry(&self) -> Option<FileEntry> {
        self.selection
            .item(self.cursor_position())
            .and_downcast::<glib::BoxedAnyObject>()
            .map(|object| object.borrow::<FileEntry>().clone())
    }

    pub fn selected_entries(&self) -> Vec<FileEntry> {
        let positions: Vec<u32> = if self.independent_selection.get() {
            self.independent_selected.borrow().iter().copied().collect()
        } else {
            (0..self.selection.n_items())
                .filter(|position| self.selection.is_selected(*position))
                .collect()
        };
        positions
            .into_iter()
            .filter_map(|position| {
                self.selection
                    .item(position)
                    .and_downcast::<glib::BoxedAnyObject>()
                    .map(|object| object.borrow::<FileEntry>().clone())
            })
            .collect()
    }

    pub fn cursor_position(&self) -> u32 {
        self.cursor
            .get()
            .min(self.selection.n_items().saturating_sub(1))
    }

    pub fn names(&self) -> Vec<String> {
        (0..self.selection.n_items())
            .filter_map(|position| {
                self.selection
                    .item(position)
                    .and_downcast::<glib::BoxedAnyObject>()
                    .map(|object| object.borrow::<FileEntry>().display_name.clone())
            })
            .collect()
    }

    pub fn select_position(&self, position: u32) {
        if position < self.selection.n_items() {
            self.cursor.set(position);
            self.apply_selection();
            self.list
                .scroll_to(position, gtk::ListScrollFlags::FOCUS, None);
            self.refresh_row_states();
        }
    }

    pub fn toggle_selection(&self) {
        if self.selection.n_items() == 0 {
            return;
        }
        let current = self.cursor_position();
        if !self.independent_selection.get() {
            self.independent_selection.set(true);
            self.independent_selected.borrow_mut().insert(current);
            self.refresh_row_states();
            let pane = self.clone();
            glib::idle_add_local_once(move || pane.refresh_row_states());
            return;
        }
        self.independent_selection.set(true);
        if !self.independent_selected.borrow_mut().remove(&current) {
            self.independent_selected.borrow_mut().insert(current);
        } else {
            // The cursor remains selected by the normal GTK selection model;
            // only the independent set is toggled here.
        }
        self.refresh_row_states();
        let pane = self.clone();
        glib::idle_add_local_once(move || pane.refresh_row_states());
    }

    pub fn begin_visual(&self) {
        self.independent_selection.set(false);
        self.independent_selected.borrow_mut().clear();
        self.visual_anchor.set(Some(self.cursor_position()));
        self.apply_selection();
        self.refresh_row_states();
    }

    pub fn end_visual(&self) {
        self.visual_anchor.set(None);
        self.apply_selection();
        self.refresh_row_states();
    }

    fn apply_selection(&self) {
        if self.selection.n_items() == 0 {
            return;
        }
        let cursor = self.cursor_position();
        self.changing_selection.set(true);
        if let Some(anchor) = self.visual_anchor.get() {
            let start = anchor.min(cursor);
            self.selection
                .select_range(start, anchor.abs_diff(cursor) + 1, true);
        } else if self.independent_selection.get() {
            self.selection.select_item(cursor, true);
        } else {
            self.selection
                .select_item(cursor, !self.independent_selection.get());
        }
        self.changing_selection.set(false);
        self.refresh_row_states();
    }

    fn refresh_row_states(&self) {
        refresh_row_states(
            &self.visible_rows,
            &self.selection,
            &self.independent_selected,
            self.independent_selection.get(),
            self.independent_selection.get() || self.visual_anchor.get().is_some(),
            self.cursor_position(),
        );
    }

    pub fn select_location(&self, location: &Location) -> bool {
        for position in 0..self.selection.n_items() {
            let Some(object) = self
                .selection
                .item(position)
                .and_downcast::<glib::BoxedAnyObject>()
            else {
                continue;
            };
            if object.borrow::<FileEntry>().location == *location {
                self.select_position(position);
                return true;
            }
        }
        false
    }
}

fn create_row() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_start(8);
    row.set_margin_end(8);
    row.set_margin_top(3);
    row.set_margin_bottom(3);

    let icon = gtk::Image::new();
    icon.set_icon_size(gtk::IconSize::Normal);
    let name = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let kind = metadata_label(65);
    let size = metadata_label(75);
    let modified = metadata_label(125);
    row.append(&icon);
    row.append(&name);
    row.append(&kind);
    row.append(&size);
    row.append(&modified);
    row
}

fn metadata_label(width: i32) -> gtk::Label {
    let label = gtk::Label::builder()
        .xalign(0.0)
        .width_request(width)
        .build();
    label.add_css_class("dim-label");
    label
}

fn bind_row(item: &glib::Object, visible_rows: &Rc<RefCell<Vec<(u32, gtk::Box)>>>) {
    let item = item
        .downcast_ref::<gtk::ListItem>()
        .expect("factory bind receives ListItem");
    let object = item
        .item()
        .and_downcast::<glib::BoxedAnyObject>()
        .expect("directory model contains FileEntry values");
    let entry = object.borrow::<FileEntry>();
    let row = item
        .child()
        .and_downcast::<gtk::Box>()
        .expect("factory child is a Box");
    // ListView reuses row widgets while scrolling; never carry selection
    // styling from the item that was previously bound to this row.
    row.remove_css_class("independent-selected");
    row.remove_css_class("cursor-item");
    let mut row_child = row.first_child();
    while let Some(widget) = row_child {
        widget.remove_css_class("independent-selected");
        row_child = widget.next_sibling();
    }
    let position = item.position();
    {
        let mut rows = visible_rows.borrow_mut();
        rows.retain(|(_, existing)| existing != &row);
        rows.push((position, row.clone()));
    }
    let icon = row
        .first_child()
        .and_downcast::<gtk::Image>()
        .expect("first row child is an Image");
    let name = next_label(&icon);
    let kind = next_label(&name);
    let size = next_label(&kind);
    let modified = next_label(&size);

    let content_type = entry.content_type.as_deref().unwrap_or_else(|| {
        if entry.kind == FileKind::Directory {
            "inode/directory"
        } else {
            "application/octet-stream"
        }
    });
    icon.set_from_gicon(&gio::content_type_get_icon(content_type));
    name.set_label(&entry.display_name);
    kind.set_label(file_kind_label(entry.kind));
    size.set_label(&format_size(entry.size));
    modified.set_label(&format_modified(entry.modified));
    row.set_tooltip_text(Some(&format!(
        "{}\nModified: {}",
        entry.location.uri(),
        format_modified(entry.modified)
    )));
}

fn refresh_row_states(
    visible_rows: &Rc<RefCell<Vec<(u32, gtk::Box)>>>,
    selection: &gtk::MultiSelection,
    independent_selected: &Rc<RefCell<HashSet<u32>>>,
    independent: bool,
    highlight_selection: bool,
    cursor: u32,
) {
    for (position, row) in visible_rows.borrow().iter() {
        if *position == gtk::INVALID_LIST_POSITION {
            continue;
        }
        let selected = if !highlight_selection {
            false
        } else if independent {
            independent_selected.borrow().contains(position)
        } else {
            selection.is_selected(*position)
        };
        if selected {
            row.add_css_class("independent-selected");
        } else {
            row.remove_css_class("independent-selected");
        }
        let mut child = row.first_child();
        let mut label_index = 0;
        while let Some(widget) = child {
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                let plain_text = label.text().trim_start_matches("● ").to_owned();
                if selected {
                    let text = glib::markup_escape_text(&plain_text);
                    let marker = if label_index == 0 { "● " } else { "" };
                    label.set_markup(&format!(
                        "<span foreground=\"#d95555\" weight=\"bold\">{marker}{text}</span>"
                    ));
                } else {
                    label.set_label(&plain_text);
                }
                label_index += 1;
            }
            child = widget.next_sibling();
        }
        let _ = cursor;
    }
}

fn next_label(widget: &impl IsA<gtk::Widget>) -> gtk::Label {
    widget
        .next_sibling()
        .and_downcast::<gtk::Label>()
        .expect("row metadata child is a Label")
}

fn file_kind_label(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "Folder",
        FileKind::Regular => "File",
        FileKind::Symlink => "Link",
        FileKind::Special => "Special",
        FileKind::Unknown => "Unknown",
    }
}

fn format_size(size: Option<u64>) -> String {
    let Some(bytes) = size else {
        return "—".to_owned();
    };
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn format_modified(modified: Option<SystemTime>) -> String {
    let Some(seconds) = modified
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_secs()).ok())
    else {
        return "—".to_owned();
    };
    glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|value| value.format("%Y-%m-%d %H:%M").ok())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_file_sizes() {
        assert_eq!(format_size(None), "—");
        assert_eq!(format_size(Some(512)), "512 B");
        assert_eq!(format_size(Some(1536)), "1.5 KiB");
    }

    #[test]
    fn remote_directory_cache_is_bounded_and_promotes_hits() {
        let mut cache = DirectoryCache::default();
        for index in 0..REMOTE_CACHE_CAPACITY {
            cache.insert(format!("smb://server/{index}"), Vec::new());
        }
        assert!(cache.get("smb://server/0").is_some());

        cache.insert("smb://server/new".to_owned(), Vec::new());

        assert!(cache.get("smb://server/0").is_some());
        assert!(cache.get("smb://server/1").is_none());
        assert_eq!(cache.entries.len(), REMOTE_CACHE_CAPACITY);
    }

    #[test]
    fn remote_directory_loads_share_in_flight_subscribers() {
        let mut cache = DirectoryCache::default();
        let notifications = Rc::new(Cell::new(0));
        let first_notifications = notifications.clone();
        cache.begin(
            "smb://server/folder".to_owned(),
            Box::new(move |_| first_notifications.set(first_notifications.get() + 1)),
        );
        let second_notifications = notifications.clone();
        assert!(
            cache
                .subscribe(
                    "smb://server/folder",
                    Box::new(move |_| second_notifications.set(second_notifications.get() + 1)),
                )
                .is_ok()
        );

        let subscribers = cache.finish("smb://server/folder");
        assert_eq!(subscribers.len(), 2);
        for subscriber in subscribers {
            subscriber(Ok(Vec::new()));
        }
        assert_eq!(notifications.get(), 2);
        assert!(cache.finish("smb://server/folder").is_empty());
    }
}
