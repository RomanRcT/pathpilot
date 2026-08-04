//! Safe, cancelable GIO primitives for mutating local filesystem operations.

use futures_util::StreamExt;
use gio::prelude::*;
use std::{
    cell::Cell,
    collections::VecDeque,
    rc::Rc,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferSpec {
    pub source: Location,
    pub destination: Location,
}

#[derive(Clone, Debug)]
pub struct BatchFailure {
    pub location: Location,
    pub error: OperationError,
}

#[derive(Clone, Debug)]
pub struct BatchOperationResult {
    pub id: OperationId,
    pub resulting_locations: Vec<Location>,
    pub failures: Vec<BatchFailure>,
    pub cancelled: bool,
}

impl BatchOperationResult {
    pub fn succeeded(&self) -> bool {
        !self.cancelled && self.failures.is_empty()
    }
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
    let source_file = gio::File::for_uri(source.uri());
    let destination_file = gio::File::for_uri(destination.uri());
    let resulting_location = destination.clone();
    let cancellable = gio::Cancellable::new();
    let operation_cancellable = cancellable.clone();
    let created_root_directory = Rc::new(Cell::new(false));
    let operation_created_root = created_root_directory.clone();
    glib::MainContext::ref_thread_default().spawn_local(async move {
        let operation = copy_tree_async(
            &source_file,
            &destination_file,
            on_progress,
            operation_created_root,
        );
        let result = match gio::CancellableFuture::new(operation, operation_cancellable).await {
            Ok(result) => result,
            Err(_) => {
                if created_root_directory.get() {
                    let _ = remove_tree_async(&destination_file).await;
                }
                Err(glib::Error::new(
                    gio::IOErrorEnum::Cancelled,
                    "The copy operation was cancelled",
                ))
            }
        };
        publish(id, kind, Some(resulting_location), result, on_finished);
    });
    OperationHandle { id, cancellable }
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

pub fn copy_items(
    id: OperationId,
    transfers: Vec<TransferSpec>,
    on_progress: impl Fn(OperationProgress) + 'static,
    on_finished: impl Fn(BatchOperationResult) + 'static,
) -> OperationHandle {
    let total = transfers.len();
    run_batch(
        id,
        total,
        on_progress,
        on_finished,
        move |cancellable, sender| {
            let mut resulting_locations = Vec::new();
            let mut failures = Vec::new();
            for (index, transfer) in transfers.into_iter().enumerate() {
                if cancellable.is_cancelled() {
                    break;
                }
                let source = gio::File::for_uri(transfer.source.uri());
                let destination = gio::File::for_uri(transfer.destination.uri());
                let result =
                    if invalid_transfer_destination(&transfer.source, &transfer.destination) {
                        Err(glib::Error::new(
                            gio::IOErrorEnum::InvalidArgument,
                            "Invalid destination",
                        ))
                    } else {
                        copy_tree(&source, &destination, &cancellable)
                    };
                collect_batch_result(
                    result,
                    transfer.source,
                    Some(transfer.destination),
                    &mut resulting_locations,
                    &mut failures,
                );
                let _ = sender.send(BatchEvent::Progress(top_level_progress(index + 1, total)));
            }
            BatchWorkerResult {
                resulting_locations,
                failures,
                cancelled: cancellable.is_cancelled(),
            }
        },
    )
}

pub fn move_items(
    id: OperationId,
    transfers: Vec<TransferSpec>,
    on_progress: impl Fn(OperationProgress) + 'static,
    on_finished: impl Fn(BatchOperationResult) + 'static,
) -> OperationHandle {
    let total = transfers.len();
    run_batch(
        id,
        total,
        on_progress,
        on_finished,
        move |cancellable, sender| {
            let mut resulting_locations = Vec::new();
            let mut failures = Vec::new();
            for (index, transfer) in transfers.into_iter().enumerate() {
                if cancellable.is_cancelled() {
                    break;
                }
                let result =
                    if invalid_transfer_destination(&transfer.source, &transfer.destination) {
                        Err(glib::Error::new(
                            gio::IOErrorEnum::InvalidArgument,
                            "Invalid destination",
                        ))
                    } else {
                        gio::File::for_uri(transfer.source.uri()).move_(
                            &gio::File::for_uri(transfer.destination.uri()),
                            gio::FileCopyFlags::NONE,
                            Some(&cancellable),
                            None,
                        )
                    };
                collect_batch_result(
                    result,
                    transfer.source,
                    Some(transfer.destination),
                    &mut resulting_locations,
                    &mut failures,
                );
                let _ = sender.send(BatchEvent::Progress(top_level_progress(index + 1, total)));
            }
            BatchWorkerResult {
                resulting_locations,
                failures,
                cancelled: cancellable.is_cancelled(),
            }
        },
    )
}

pub fn trash_items(
    id: OperationId,
    targets: Vec<Location>,
    on_progress: impl Fn(OperationProgress) + 'static,
    on_finished: impl Fn(BatchOperationResult) + 'static,
) -> OperationHandle {
    run_target_batch(
        id,
        targets,
        on_progress,
        on_finished,
        |file, cancellable| file.trash(Some(cancellable)),
    )
}

pub fn delete_items(
    id: OperationId,
    targets: Vec<Location>,
    on_progress: impl Fn(OperationProgress) + 'static,
    on_finished: impl Fn(BatchOperationResult) + 'static,
) -> OperationHandle {
    run_target_batch(
        id,
        targets,
        on_progress,
        on_finished,
        |file, cancellable| remove_tree(file, Some(cancellable), None),
    )
}

fn run_target_batch(
    id: OperationId,
    targets: Vec<Location>,
    on_progress: impl Fn(OperationProgress) + 'static,
    on_finished: impl Fn(BatchOperationResult) + 'static,
    operation: impl Fn(&gio::File, &gio::Cancellable) -> Result<(), glib::Error> + Send + 'static,
) -> OperationHandle {
    let total = targets.len();
    run_batch(
        id,
        total,
        on_progress,
        on_finished,
        move |cancellable, sender| {
            let mut failures = Vec::new();
            for (index, target) in targets.into_iter().enumerate() {
                if cancellable.is_cancelled() {
                    break;
                }
                if let Err(error) = operation(&gio::File::for_uri(target.uri()), &cancellable) {
                    failures.push(BatchFailure {
                        location: target,
                        error: classify_error(error),
                    });
                }
                let _ = sender.send(BatchEvent::Progress(top_level_progress(index + 1, total)));
            }
            BatchWorkerResult {
                resulting_locations: Vec::new(),
                failures,
                cancelled: cancellable.is_cancelled(),
            }
        },
    )
}

fn top_level_progress(completed: usize, total: usize) -> OperationProgress {
    OperationProgress {
        completed_items: completed as u64,
        total_items: Some(total as u64),
        ..OperationProgress::default()
    }
}

fn collect_batch_result(
    result: Result<(), glib::Error>,
    source: Location,
    destination: Option<Location>,
    successes: &mut Vec<Location>,
    failures: &mut Vec<BatchFailure>,
) {
    match result {
        Ok(()) => successes.extend(destination),
        Err(error) => failures.push(BatchFailure {
            location: source,
            error: classify_error(error),
        }),
    }
}

struct BatchWorkerResult {
    resulting_locations: Vec<Location>,
    failures: Vec<BatchFailure>,
    cancelled: bool,
}

enum BatchEvent {
    Progress(OperationProgress),
    Finished(BatchWorkerResult),
}

fn run_batch(
    id: OperationId,
    total: usize,
    on_progress: impl Fn(OperationProgress) + 'static,
    on_finished: impl Fn(BatchOperationResult) + 'static,
    operation: impl FnOnce(gio::Cancellable, mpsc::Sender<BatchEvent>) -> BatchWorkerResult
    + Send
    + 'static,
) -> OperationHandle {
    let cancellable = gio::Cancellable::new();
    let worker_cancellable = cancellable.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = operation(worker_cancellable, sender.clone());
        let _ = sender.send(BatchEvent::Finished(result));
    });
    on_progress(top_level_progress(0, total));
    glib::timeout_add_local(Duration::from_millis(16), move || {
        loop {
            match receiver.try_recv() {
                Ok(BatchEvent::Progress(progress)) => on_progress(progress),
                Ok(BatchEvent::Finished(result)) => {
                    on_finished(BatchOperationResult {
                        id,
                        resulting_locations: result.resulting_locations,
                        failures: result.failures,
                        cancelled: result.cancelled,
                    });
                    return glib::ControlFlow::Break;
                }
                Err(TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => return glib::ControlFlow::Break,
            }
        }
    });
    OperationHandle { id, cancellable }
}

fn copy_tree(
    source: &gio::File,
    destination: &gio::File,
    cancellable: &gio::Cancellable,
) -> Result<(), glib::Error> {
    if destination.query_exists(Some(cancellable)) {
        return Err(glib::Error::new(
            gio::IOErrorEnum::Exists,
            "The destination already exists",
        ));
    }
    let info = source.query_info(
        "standard::type",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        Some(cancellable),
    )?;
    if info.file_type() != gio::FileType::Directory {
        return source.copy(
            destination,
            gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA,
            Some(cancellable),
            None,
        );
    }
    destination.make_directory(Some(cancellable))?;
    let result = (|| {
        let enumerator = source.enumerate_children(
            "standard::name",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            Some(cancellable),
        )?;
        while let Some(child) = enumerator.next_file(Some(cancellable))? {
            copy_tree(
                &source.child(child.name()),
                &destination.child(child.name()),
                cancellable,
            )?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = remove_tree(destination, None::<&gio::Cancellable>, None);
    }
    result
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

async fn copy_tree_async(
    source: &gio::File,
    destination: &gio::File,
    on_progress: impl Fn(OperationProgress),
    created_root_directory: Rc<Cell<bool>>,
) -> Result<(), glib::Error> {
    match destination
        .query_info_future(
            "standard::type",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            glib::Priority::DEFAULT,
        )
        .await
    {
        Ok(_) => {
            return Err(glib::Error::new(
                gio::IOErrorEnum::Exists,
                "The destination already exists",
            ));
        }
        Err(error) if error.matches(gio::IOErrorEnum::NotFound) => {}
        Err(error) => return Err(error),
    }
    let entries = collect_copy_entries_async(source, destination).await?;
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
            entry
                .destination
                .make_directory_future(glib::Priority::DEFAULT)
                .await
        } else {
            let (copy, mut updates) = entry.source.copy_future(
                &entry.destination,
                gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA,
                glib::Priority::DEFAULT,
            );
            let report_progress = async {
                while let Some((current, _)) = updates.next().await {
                    progress.completed_bytes = completed_before + current.max(0) as u64;
                    on_progress(progress);
                }
            };
            let (result, ()) = futures_util::future::join(copy, report_progress).await;
            result
        };
        if let Err(error) = result {
            if destination_created {
                let _ = remove_tree_async(destination).await;
            }
            return Err(error);
        }
        if index == 0 {
            destination_created = true;
            if entry.file_type == gio::FileType::Directory {
                created_root_directory.set(true);
            }
        }
        progress.completed_items += 1;
        if entry.file_type != gio::FileType::Directory {
            progress.completed_bytes = completed_before.saturating_add(entry.size);
        }
        on_progress(progress);
    }
    Ok(())
}

async fn collect_copy_entries_async(
    source: &gio::File,
    destination: &gio::File,
) -> Result<Vec<CopyEntry>, glib::Error> {
    let mut pending = VecDeque::from([(source.clone(), destination.clone())]);
    let mut entries = Vec::new();
    while let Some((source, destination)) = pending.pop_front() {
        let info = source
            .query_info_future(
                "standard::type,standard::size",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                glib::Priority::DEFAULT,
            )
            .await?;
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
            let enumerator = source
                .enumerate_children_future(
                    "standard::name",
                    gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                    glib::Priority::DEFAULT,
                )
                .await?;
            loop {
                let children = enumerator
                    .next_files_future(256, glib::Priority::DEFAULT)
                    .await?;
                if children.is_empty() {
                    break;
                }
                for child_info in children {
                    let name = child_info.name();
                    pending.push_back((source.child(&name), destination.child(&name)));
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.destination.uri().matches('/').count());
    Ok(entries)
}

async fn remove_tree_async(target: &gio::File) -> Result<(), glib::Error> {
    let mut pending = VecDeque::from([target.clone()]);
    let mut entries = Vec::new();
    while let Some(entry) = pending.pop_front() {
        let info = entry
            .query_info_future(
                "standard::type",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                glib::Priority::DEFAULT,
            )
            .await?;
        if info.file_type() == gio::FileType::Directory {
            let enumerator = entry
                .enumerate_children_future(
                    "standard::name",
                    gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                    glib::Priority::DEFAULT,
                )
                .await?;
            loop {
                let children = enumerator
                    .next_files_future(256, glib::Priority::DEFAULT)
                    .await?;
                if children.is_empty() {
                    break;
                }
                pending.extend(children.into_iter().map(|info| entry.child(info.name())));
            }
        }
        entries.push(entry);
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.uri().matches('/').count()));
    for entry in entries {
        entry.delete_future(glib::Priority::DEFAULT).await?;
    }
    Ok(())
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

    fn run_batch_operation(
        start: impl FnOnce(Box<dyn Fn(BatchOperationResult)>) -> OperationHandle,
    ) -> BatchOperationResult {
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
        result
            .borrow_mut()
            .take()
            .expect("batch operation completes")
    }

    #[test]
    fn batch_copy_continues_after_an_item_failure() {
        let _guard = gio_test_lock();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let first = temporary.path().join("first.txt");
        let second = temporary.path().join("second.txt");
        let first_copy = temporary.path().join("first-copy.txt");
        let conflict = temporary.path().join("conflict.txt");
        fs::write(&first, b"first").expect("write first fixture");
        fs::write(&second, b"second").expect("write second fixture");
        fs::write(&conflict, b"existing").expect("write conflict fixture");
        let location = |path: &std::path::Path| Location::new(gio::File::for_path(path).uri());
        let result = run_batch_operation(|callback| {
            copy_items(
                OperationId::new(20),
                vec![
                    TransferSpec {
                        source: location(&first),
                        destination: location(&first_copy),
                    },
                    TransferSpec {
                        source: location(&second),
                        destination: location(&conflict),
                    },
                ],
                |_| {},
                callback,
            )
        });
        assert!(!result.succeeded());
        assert_eq!(result.resulting_locations, vec![location(&first_copy)]);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].location, location(&second));
        assert_eq!(
            result.failures[0].error.kind,
            OperationErrorKind::AlreadyExists
        );
        assert_eq!(
            fs::read(first_copy).expect("read successful copy"),
            b"first"
        );
        assert_eq!(
            fs::read(conflict).expect("conflict is preserved"),
            b"existing"
        );
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

        let move_source_path = temporary.path().join("move-source.txt");
        let move_destination_path = temporary.path().join("move-destination.txt");
        fs::write(&move_source_path, b"move payload").expect("write move source");
        let move_source = Location::new(gio::File::for_path(&move_source_path).uri());
        let move_destination = Location::new(gio::File::for_path(&move_destination_path).uri());
        let result = run_operation(|callback| {
            move_item(
                OperationId::new(11),
                &move_source,
                &move_destination,
                |_, _| {},
                callback,
            )
        });
        assert!(result.result.is_ok());
        assert!(!move_source_path.exists());
        assert_eq!(
            fs::read(move_destination_path).expect("read moved file"),
            b"move payload"
        );
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
    fn cancelling_copy_reports_cancelled_and_leaves_no_destination() {
        let _guard = gio_test_lock();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let source_path = temporary.path().join("source.txt");
        let destination_path = temporary.path().join("destination.txt");
        fs::write(&source_path, vec![42_u8; 1024 * 1024]).expect("write source");
        let source = Location::new(gio::File::for_path(source_path).uri());
        let destination = Location::new(gio::File::for_path(&destination_path).uri());
        let result = Rc::new(RefCell::new(None));
        let main_loop = glib::MainLoop::new(None, false);
        let handle = copy_item(OperationId::new(12), &source, &destination, |_, _| {}, {
            let result = result.clone();
            let main_loop = main_loop.clone();
            move |value| {
                *result.borrow_mut() = Some(value);
                main_loop.quit();
            }
        });
        handle.cancel();
        main_loop.run();
        let result = result.borrow_mut().take().expect("copy completes");

        assert_eq!(
            result.result.expect_err("copy is cancelled").kind,
            OperationErrorKind::Cancelled
        );
        assert!(!destination_path.exists());
    }

    #[test]
    fn cancelling_before_preflight_never_removes_an_existing_destination() {
        let _guard = gio_test_lock();
        let temporary = tempfile::tempdir().expect("create temporary directory");
        let source_path = temporary.path().join("source.txt");
        let destination_path = temporary.path().join("destination.txt");
        fs::write(&source_path, b"source").expect("write source");
        fs::write(&destination_path, b"existing").expect("write destination");
        let source = Location::new(gio::File::for_path(source_path).uri());
        let destination = Location::new(gio::File::for_path(&destination_path).uri());
        let result = Rc::new(RefCell::new(None));
        let main_loop = glib::MainLoop::new(None, false);
        let handle = copy_item(OperationId::new(13), &source, &destination, |_, _| {}, {
            let result = result.clone();
            let main_loop = main_loop.clone();
            move |value| {
                *result.borrow_mut() = Some(value);
                main_loop.quit();
            }
        });
        handle.cancel();
        main_loop.run();

        assert_eq!(
            result
                .borrow_mut()
                .take()
                .expect("copy completes")
                .result
                .expect_err("copy is cancelled")
                .kind,
            OperationErrorKind::Cancelled
        );
        assert_eq!(
            fs::read(destination_path).expect("read destination"),
            b"existing"
        );
    }

    #[test]
    fn classifies_structured_gio_errors() {
        for (gio_kind, expected) in [
            (gio::IOErrorEnum::Exists, OperationErrorKind::AlreadyExists),
            (
                gio::IOErrorEnum::PermissionDenied,
                OperationErrorKind::PermissionDenied,
            ),
            (gio::IOErrorEnum::NotFound, OperationErrorKind::NotFound),
            (gio::IOErrorEnum::Cancelled, OperationErrorKind::Cancelled),
            (
                gio::IOErrorEnum::NotSupported,
                OperationErrorKind::Unsupported,
            ),
            (gio::IOErrorEnum::Failed, OperationErrorKind::Other),
        ] {
            let error = glib::Error::new(gio_kind, "fixture");
            assert_eq!(classify_error(error).kind, expected);
        }
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
