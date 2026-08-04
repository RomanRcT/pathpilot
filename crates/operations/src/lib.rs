//! Safe, cancelable GIO primitives for mutating local filesystem operations.

use gio::prelude::*;
use std::{
    collections::VecDeque,
    sync::mpsc::{self, TryRecvError},
    time::Duration,
};

use pathpilot_core::{Location, OperationId, OperationKind, OperationProgress};
use tracing::{debug, warn};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationErrorKind {
    AlreadyExists,
    PermissionDenied,
    NotFound,
    InvalidName,
    Cancelled,
    InvalidDestination,
    Unsupported,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationError {
    pub kind: OperationErrorKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct OperationResult {
    pub id: OperationId,
    pub kind: OperationKind,
    pub resulting_location: Option<Location>,
    pub result: Result<(), OperationError>,
}

#[derive(Clone)]
pub struct OperationHandle {
    id: OperationId,
    cancellable: gio::Cancellable,
}

impl OperationHandle {
    pub fn id(&self) -> OperationId {
        self.id
    }

    pub fn cancel(&self) {
        self.cancellable.cancel();
    }
}

pub fn create_file(
    id: OperationId,
    parent: &Location,
    name: &str,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    let kind = OperationKind::CreateFile {
        parent: parent.clone(),
        name: name.to_owned(),
    };
    let Some(target) = child_for_name(parent, name) else {
        return finish_invalid(id, kind, on_finished);
    };
    let resulting_location = Location::new(target.uri());
    let cancellable = gio::Cancellable::new();
    let callback_cancellable = cancellable.clone();
    target.create_async(
        gio::FileCreateFlags::NONE,
        glib::Priority::DEFAULT,
        Some(&cancellable),
        move |result| match result {
            Ok(stream) => stream.close_async(
                glib::Priority::DEFAULT,
                Some(&callback_cancellable),
                move |result| publish(id, kind, Some(resulting_location), result, on_finished),
            ),
            Err(error) => publish(id, kind, None, Err(error), on_finished),
        },
    );
    OperationHandle { id, cancellable }
}

pub fn create_directory(
    id: OperationId,
    parent: &Location,
    name: &str,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    let kind = OperationKind::CreateDirectory {
        parent: parent.clone(),
        name: name.to_owned(),
    };
    let Some(target) = child_for_name(parent, name) else {
        return finish_invalid(id, kind, on_finished);
    };
    let resulting_location = Location::new(target.uri());
    let cancellable = gio::Cancellable::new();
    target.make_directory_async(glib::Priority::DEFAULT, Some(&cancellable), {
        let kind = kind.clone();
        move |result| publish(id, kind, Some(resulting_location), result, on_finished)
    });
    OperationHandle { id, cancellable }
}

pub fn rename(
    id: OperationId,
    source: &Location,
    new_name: &str,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    let kind = OperationKind::Rename {
        source: source.clone(),
        new_name: new_name.to_owned(),
    };
    if !valid_name(new_name) {
        return finish_invalid(id, kind, on_finished);
    }
    let file = gio::File::for_uri(source.uri());
    let cancellable = gio::Cancellable::new();
    file.set_display_name_async(new_name, glib::Priority::DEFAULT, Some(&cancellable), {
        let kind = kind.clone();
        move |result| match result {
            Ok(location) => publish(
                id,
                kind,
                Some(Location::new(location.uri())),
                Ok(()),
                on_finished,
            ),
            Err(error) => publish(id, kind, None, Err(error), on_finished),
        }
    });
    OperationHandle { id, cancellable }
}

pub fn trash(
    id: OperationId,
    target: &Location,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    let kind = OperationKind::Trash {
        target: target.clone(),
    };
    let file = gio::File::for_uri(target.uri());
    let cancellable = gio::Cancellable::new();
    file.trash_async(glib::Priority::DEFAULT, Some(&cancellable), {
        let kind = kind.clone();
        move |result| publish(id, kind, None, result, on_finished)
    });
    OperationHandle { id, cancellable }
}

pub fn delete_permanently(
    id: OperationId,
    target: &Location,
    on_progress: impl Fn(OperationProgress) + 'static,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    let kind = OperationKind::Delete {
        target: target.clone(),
    };
    run_background_tree_operation(id, kind, None, on_progress, on_finished, {
        let target = target.uri().to_owned();
        move |cancellable, sender| delete_tree(&target, &cancellable, &sender)
    })
}

pub fn copy_item(
    id: OperationId,
    source: &Location,
    destination: &Location,
    on_progress: impl Fn(u64, Option<u64>) + 'static,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    copy_item_with_progress(
        id,
        source,
        destination,
        move |progress| on_progress(progress.completed_bytes, progress.total_bytes),
        on_finished,
    )
}

pub fn copy_item_with_progress(
    id: OperationId,
    source: &Location,
    destination: &Location,
    on_progress: impl Fn(OperationProgress) + 'static,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    let kind = OperationKind::Copy {
        source: source.clone(),
        destination: destination.clone(),
    };
    if invalid_transfer_destination(source, destination) {
        return finish_invalid_destination(id, kind, on_finished);
    }
    let source_uri = source.uri().to_owned();
    let destination_uri = destination.uri().to_owned();
    let resulting_location = destination.clone();
    run_background_tree_operation(
        id,
        kind,
        Some(resulting_location),
        on_progress,
        on_finished,
        move |cancellable, sender| copy_tree(&source_uri, &destination_uri, &cancellable, &sender),
    )
}

pub fn move_item(
    id: OperationId,
    source: &Location,
    destination: &Location,
    on_progress: impl Fn(u64, Option<u64>) + 'static,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    let kind = OperationKind::Move {
        source: source.clone(),
        destination: destination.clone(),
    };
    let source_file = gio::File::for_uri(source.uri());
    let destination_file = gio::File::for_uri(destination.uri());
    let cancellable = gio::Cancellable::new();
    if invalid_transfer_destination(source, destination) {
        return finish_invalid_destination(id, kind, on_finished);
    }
    let progress = Box::new(move |current: i64, total: i64| {
        on_progress(current.max(0) as u64, (total >= 0).then_some(total as u64));
    });
    let resulting_location = destination.clone();
    source_file.move_async(
        &destination_file,
        gio::FileCopyFlags::NONE,
        glib::Priority::DEFAULT,
        Some(&cancellable),
        Some(progress),
        move |result| publish(id, kind, Some(resulting_location), result, on_finished),
    );
    OperationHandle { id, cancellable }
}

fn invalid_transfer_destination(source: &Location, destination: &Location) -> bool {
    let source = gio::File::for_uri(source.uri());
    let destination = gio::File::for_uri(destination.uri());
    source == destination || destination.has_prefix(&source)
}

fn finish_invalid_destination(
    id: OperationId,
    kind: OperationKind,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    let cancellable = gio::Cancellable::new();
    glib::idle_add_local_once(move || {
        on_finished(OperationResult {
            id,
            kind,
            resulting_location: None,
            result: Err(OperationError {
                kind: OperationErrorKind::InvalidDestination,
                message: "An item cannot be copied or moved into itself".to_owned(),
            }),
        });
    });
    OperationHandle { id, cancellable }
}

enum BackgroundEvent {
    Progress(OperationProgress),
    Finished(Result<(), glib::Error>),
}

fn run_background_tree_operation(
    id: OperationId,
    kind: OperationKind,
    resulting_location: Option<Location>,
    on_progress: impl Fn(OperationProgress) + 'static,
    on_finished: impl Fn(OperationResult) + 'static,
    operation: impl FnOnce(gio::Cancellable, mpsc::Sender<BackgroundEvent>) -> Result<(), glib::Error>
    + Send
    + 'static,
) -> OperationHandle {
    let cancellable = gio::Cancellable::new();
    let worker_cancellable = cancellable.clone();
    let (sender, receiver) = mpsc::channel();
    let worker_sender = sender.clone();
    std::thread::spawn(move || {
        let result = operation(worker_cancellable, worker_sender);
        let _ = sender.send(BackgroundEvent::Finished(result));
    });
    glib::timeout_add_local(Duration::from_millis(16), move || {
        loop {
            match receiver.try_recv() {
                Ok(BackgroundEvent::Progress(progress)) => on_progress(progress),
                Ok(BackgroundEvent::Finished(result)) => {
                    publish(
                        id,
                        kind.clone(),
                        resulting_location.clone(),
                        result,
                        &on_finished,
                    );
                    return glib::ControlFlow::Break;
                }
                Err(TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            }
        }
    });
    OperationHandle { id, cancellable }
}

#[derive(Clone)]
struct CopyEntry {
    source: gio::File,
    destination: gio::File,
    file_type: gio::FileType,
    size: u64,
}

fn copy_tree(
    source_uri: &str,
    destination_uri: &str,
    cancellable: &gio::Cancellable,
    sender: &mpsc::Sender<BackgroundEvent>,
) -> Result<(), glib::Error> {
    let source = gio::File::for_uri(source_uri);
    let destination = gio::File::for_uri(destination_uri);
    if destination.query_exists(Some(cancellable)) {
        return Err(glib::Error::new(
            gio::IOErrorEnum::Exists,
            "The destination already exists",
        ));
    }
    let entries = collect_copy_entries(&source, &destination, cancellable)?;
    let total_items = entries.len() as u64;
    let total_bytes = entries.iter().map(|entry| entry.size).sum();
    let mut progress = OperationProgress {
        total_items: Some(total_items),
        total_bytes: Some(total_bytes),
        ..OperationProgress::default()
    };
    let mut destination_created = false;
    for (index, entry) in entries.into_iter().enumerate() {
        let completed_before = progress.completed_bytes;
        let result = if entry.file_type == gio::FileType::Directory {
            entry.destination.make_directory(Some(cancellable))
        } else {
            entry.source.copy(
                &entry.destination,
                gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA,
                Some(cancellable),
                Some(&mut |current, _| {
                    progress.completed_bytes = completed_before + current.max(0) as u64;
                    let _ = sender.send(BackgroundEvent::Progress(progress));
                }),
            )
        };
        if let Err(error) = result {
            let safe_to_clean = destination_created
                || (!error.matches(gio::IOErrorEnum::Exists)
                    && destination.query_exists(None::<&gio::Cancellable>));
            if safe_to_clean {
                let _ = remove_tree(&destination, None, None);
            }
            return Err(error);
        }
        if index == 0 {
            destination_created = true;
        }
        progress.completed_items += 1;
        if entry.file_type != gio::FileType::Directory {
            progress.completed_bytes = completed_before.saturating_add(entry.size);
        }
        let _ = sender.send(BackgroundEvent::Progress(progress));
    }
    Ok(())
}

fn collect_copy_entries(
    source: &gio::File,
    destination: &gio::File,
    cancellable: &gio::Cancellable,
) -> Result<Vec<CopyEntry>, glib::Error> {
    let mut pending = VecDeque::from([(source.clone(), destination.clone())]);
    let mut entries = Vec::new();
    while let Some((source, destination)) = pending.pop_front() {
        let info = source.query_info(
            "standard::type,standard::size",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            Some(cancellable),
        )?;
        let file_type = info.file_type();
        entries.push(CopyEntry {
            source: source.clone(),
            destination: destination.clone(),
            file_type,
            size: if file_type == gio::FileType::Regular {
                info.size().max(0) as u64
            } else {
                0
            },
        });
        if file_type == gio::FileType::Directory {
            let enumerator = source.enumerate_children(
                "standard::name",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                Some(cancellable),
            )?;
            while let Some(child_info) = enumerator.next_file(Some(cancellable))? {
                let name = child_info.name();
                pending.push_back((source.child(&name), destination.child(&name)));
            }
        }
    }
    entries.sort_by_key(|entry| entry.destination.uri().matches('/').count());
    Ok(entries)
}

fn delete_tree(
    target_uri: &str,
    cancellable: &gio::Cancellable,
    sender: &mpsc::Sender<BackgroundEvent>,
) -> Result<(), glib::Error> {
    let target = gio::File::for_uri(target_uri);
    let total_items = count_tree(&target, cancellable)?;
    let mut progress = OperationProgress {
        total_items: Some(total_items),
        ..OperationProgress::default()
    };
    remove_tree(&target, Some(cancellable), Some((&mut progress, sender)))
}

fn count_tree(target: &gio::File, cancellable: &gio::Cancellable) -> Result<u64, glib::Error> {
    let info = target.query_info(
        "standard::type",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        Some(cancellable),
    )?;
    let mut count = 1;
    if info.file_type() == gio::FileType::Directory {
        let enumerator = target.enumerate_children(
            "standard::name",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            Some(cancellable),
        )?;
        while let Some(info) = enumerator.next_file(Some(cancellable))? {
            count += count_tree(&target.child(info.name()), cancellable)?;
        }
    }
    Ok(count)
}

fn remove_tree(
    target: &gio::File,
    cancellable: Option<&gio::Cancellable>,
    mut reporting: Option<(&mut OperationProgress, &mpsc::Sender<BackgroundEvent>)>,
) -> Result<(), glib::Error> {
    let info = target.query_info(
        "standard::type",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        cancellable,
    )?;
    if info.file_type() == gio::FileType::Directory {
        let enumerator = target.enumerate_children(
            "standard::name",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            cancellable,
        )?;
        while let Some(info) = enumerator.next_file(cancellable)? {
            if let Some((progress, sender)) = reporting.as_mut() {
                remove_tree(
                    &target.child(info.name()),
                    cancellable,
                    Some((*progress, *sender)),
                )?;
            } else {
                remove_tree(&target.child(info.name()), cancellable, None)?;
            }
        }
    }
    target.delete(cancellable)?;
    if let Some((progress, sender)) = reporting {
        progress.completed_items += 1;
        let _ = sender.send(BackgroundEvent::Progress(*progress));
    }
    Ok(())
}

fn child_for_name(parent: &Location, name: &str) -> Option<gio::File> {
    valid_name(name).then(|| gio::File::for_uri(parent.uri()).child(name))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

fn finish_invalid(
    id: OperationId,
    kind: OperationKind,
    on_finished: impl Fn(OperationResult) + 'static,
) -> OperationHandle {
    let cancellable = gio::Cancellable::new();
    on_finished(OperationResult {
        id,
        kind,
        resulting_location: None,
        result: Err(OperationError {
            kind: OperationErrorKind::InvalidName,
            message: "The filename is empty or contains a path separator".to_owned(),
        }),
    });
    OperationHandle { id, cancellable }
}

fn publish(
    id: OperationId,
    kind: OperationKind,
    resulting_location: Option<Location>,
    result: Result<(), glib::Error>,
    on_finished: impl Fn(OperationResult),
) {
    let affected = affected_location(&kind);
    let result = result.map_err(|error| {
        let mut error = classify_error(error);
        error.message = format!("{affected}: {}", error.message);
        error
    });
    if let Err(error) = &result {
        warn!(operation_id = id.value(), ?error.kind, "operation failed");
    } else {
        debug!(operation_id = id.value(), "operation completed");
    }
    on_finished(OperationResult {
        id,
        kind,
        resulting_location,
        result,
    });
}

fn affected_location(kind: &OperationKind) -> &str {
    match kind {
        OperationKind::CreateFile { parent, .. }
        | OperationKind::CreateDirectory { parent, .. } => parent.uri(),
        OperationKind::Rename { source, .. }
        | OperationKind::Copy { source, .. }
        | OperationKind::Move { source, .. } => source.uri(),
        OperationKind::Trash { target } | OperationKind::Delete { target } => target.uri(),
    }
}

fn classify_error(error: glib::Error) -> OperationError {
    let kind = if error.matches(gio::IOErrorEnum::Exists) {
        OperationErrorKind::AlreadyExists
    } else if error.matches(gio::IOErrorEnum::PermissionDenied) {
        OperationErrorKind::PermissionDenied
    } else if error.matches(gio::IOErrorEnum::NotFound) {
        OperationErrorKind::NotFound
    } else if error.matches(gio::IOErrorEnum::Cancelled) {
        OperationErrorKind::Cancelled
    } else if error.matches(gio::IOErrorEnum::IsDirectory)
        || error.matches(gio::IOErrorEnum::WouldRecurse)
        || error.matches(gio::IOErrorEnum::NotSupported)
    {
        OperationErrorKind::Unsupported
    } else {
        OperationErrorKind::Other
    };
    OperationError {
        kind,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        fs,
        rc::Rc,
        sync::{Mutex, MutexGuard},
    };

    use super::*;

    static GIO_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn gio_test_lock() -> MutexGuard<'static, ()> {
        GIO_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn run_operation(
        start: impl FnOnce(Box<dyn Fn(OperationResult)>) -> OperationHandle,
    ) -> OperationResult {
        let result = Rc::new(RefCell::new(None));
        let main_loop = glib::MainLoop::new(None, false);
        let _handle = start(Box::new({
            let result = result.clone();
            let main_loop = main_loop.clone();
            move |value| {
                *result.borrow_mut() = Some(value);
                main_loop.quit();
            }
        }));
        main_loop.run();
        result.borrow_mut().take().expect("operation completes")
    }

    #[test]
    fn creates_and_renames_without_overwriting() {
        let _guard = gio_test_lock();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let parent = Location::new(gio::File::for_path(temporary.path()).uri());
        let created = run_operation(|callback| {
            create_file(OperationId::new(1), &parent, "first.txt", callback)
        });
        assert!(created.result.is_ok());
        assert!(temporary.path().join("first.txt").exists());

        let source = created.resulting_location.expect("created URI");
        let renamed =
            run_operation(|callback| rename(OperationId::new(2), &source, "second.txt", callback));
        assert!(renamed.result.is_ok());
        assert!(temporary.path().join("second.txt").exists());

        fs::write(temporary.path().join("conflict.txt"), b"existing").expect("write fixture");
        let conflict = run_operation(|callback| {
            create_file(OperationId::new(3), &parent, "conflict.txt", callback)
        });
        assert_eq!(
            conflict.result.expect_err("must not overwrite").kind,
            OperationErrorKind::AlreadyExists
        );
    }

    #[test]
    fn rejects_path_traversal_names() {
        let _guard = gio_test_lock();
        let parent = Location::new("file:///tmp");
        let result = Rc::new(RefCell::new(None));
        let _handle = create_directory(OperationId::new(4), &parent, "../escape", {
            let result = result.clone();
            move |value| *result.borrow_mut() = Some(value)
        });
        assert_eq!(
            result
                .borrow_mut()
                .take()
                .expect("validation is immediate")
                .result
                .expect_err("invalid name")
                .kind,
            OperationErrorKind::InvalidName
        );
    }

    #[test]
    fn copies_files_and_moves_directories_without_overwriting() {
        let _guard = gio_test_lock();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let source_path = temporary.path().join("source.txt");
        let copied_path = temporary.path().join("copied.txt");
        fs::write(&source_path, b"payload").expect("write source");
        let source = Location::new(gio::File::for_path(&source_path).uri());
        let copied = Location::new(gio::File::for_path(&copied_path).uri());
        let result = run_operation(|callback| {
            copy_item(OperationId::new(5), &source, &copied, |_, _| {}, callback)
        });
        assert!(result.result.is_ok());
        assert_eq!(fs::read(&copied_path).expect("read copy"), b"payload");

        let conflict = run_operation(|callback| {
            copy_item(OperationId::new(6), &source, &copied, |_, _| {}, callback)
        });
        assert_eq!(
            conflict.result.expect_err("must not overwrite").kind,
            OperationErrorKind::AlreadyExists
        );

        let directory_path = temporary.path().join("directory");
        let moved_path = temporary.path().join("moved-directory");
        fs::create_dir(&directory_path).expect("create directory");
        fs::write(directory_path.join("child.txt"), b"child").expect("write child");
        let directory = Location::new(gio::File::for_path(&directory_path).uri());
        let moved = Location::new(gio::File::for_path(&moved_path).uri());
        let result = run_operation(|callback| {
            move_item(OperationId::new(7), &directory, &moved, |_, _| {}, callback)
        });
        assert!(result.result.is_ok());
        assert!(!directory_path.exists());
        assert!(moved_path.join("child.txt").exists());
    }

    #[test]
    fn recursively_copies_directories_and_preserves_nested_files() {
        let _guard = gio_test_lock();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let source_path = temporary.path().join("source-tree");
        let destination_path = temporary.path().join("copied-tree");
        fs::create_dir_all(source_path.join("nested/deep")).expect("create source tree");
        fs::write(source_path.join("root.txt"), b"root").expect("write root file");
        fs::write(source_path.join("nested/deep/leaf.txt"), b"leaf").expect("write nested file");
        let source = Location::new(gio::File::for_path(&source_path).uri());
        let destination = Location::new(gio::File::for_path(&destination_path).uri());

        let result = run_operation(|callback| {
            copy_item(
                OperationId::new(9),
                &source,
                &destination,
                |_, _| {},
                callback,
            )
        });

        assert!(result.result.is_ok());
        assert_eq!(
            fs::read(destination_path.join("root.txt")).expect("read copied root file"),
            b"root"
        );
        assert_eq!(
            fs::read(destination_path.join("nested/deep/leaf.txt"))
                .expect("read copied nested file"),
            b"leaf"
        );
    }

    #[test]
    fn permanently_deletes_non_empty_directory_trees() {
        let _guard = gio_test_lock();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let target_path = temporary.path().join("delete-tree");
        fs::create_dir_all(target_path.join("nested")).expect("create target tree");
        fs::write(target_path.join("nested/file.txt"), b"payload").expect("write target file");
        let target = Location::new(gio::File::for_path(&target_path).uri());

        let result = run_operation(|callback| {
            delete_permanently(OperationId::new(10), &target, |_| {}, callback)
        });

        assert!(result.result.is_ok());
        assert!(!target_path.exists());
    }

    #[test]
    fn rejects_recursive_transfer_destination() {
        let _guard = gio_test_lock();
        let source = Location::new("file:///tmp/source-directory");
        let destination = Location::new("file:///tmp/source-directory/child/copy");
        let result = run_operation(|callback| {
            move_item(
                OperationId::new(8),
                &source,
                &destination,
                |_, _| {},
                callback,
            )
        });
        assert_eq!(
            result.result.expect_err("recursive target").kind,
            OperationErrorKind::InvalidDestination
        );
    }
}
