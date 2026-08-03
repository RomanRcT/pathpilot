//! Safe, cancelable GIO primitives for mutating local filesystem operations.

use gio::prelude::*;
use pathpilot_core::{Location, OperationId, OperationKind};
use tracing::{debug, warn};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationErrorKind {
    AlreadyExists,
    PermissionDenied,
    NotFound,
    InvalidName,
    Cancelled,
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
    let result = result.map_err(classify_error);
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

fn classify_error(error: glib::Error) -> OperationError {
    let kind = if error.matches(gio::IOErrorEnum::Exists) {
        OperationErrorKind::AlreadyExists
    } else if error.matches(gio::IOErrorEnum::PermissionDenied) {
        OperationErrorKind::PermissionDenied
    } else if error.matches(gio::IOErrorEnum::NotFound) {
        OperationErrorKind::NotFound
    } else if error.matches(gio::IOErrorEnum::Cancelled) {
        OperationErrorKind::Cancelled
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
    use std::{cell::RefCell, fs, rc::Rc};

    use super::*;

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
}
