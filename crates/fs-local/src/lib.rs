//! Asynchronous local-directory enumeration through GIO.

use std::{
    rc::Rc,
    time::{Duration, UNIX_EPOCH},
};

use gio::prelude::*;
use pathpilot_core::{FileEntry, FileKind, Generation, Location};
use tracing::{debug, warn};

const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::is-hidden,standard::is-symlink,standard::size,standard::content-type,time::modified";
const BATCH_SIZE: i32 = 256;

#[derive(Debug)]
pub enum DirectoryEvent {
    Batch {
        generation: Generation,
        entries: Vec<FileEntry>,
    },
    Finished {
        generation: Generation,
    },
    Failed {
        generation: Generation,
        message: String,
    },
}

pub fn load_directory(
    location: &Location,
    generation: Generation,
    on_event: impl Fn(DirectoryEvent) + 'static,
) -> gio::Cancellable {
    let cancellable = gio::Cancellable::new();
    let file = gio::File::for_uri(location.uri());
    let on_event: Rc<dyn Fn(DirectoryEvent)> = Rc::new(on_event);

    file.enumerate_children_async(
        ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        glib::Priority::DEFAULT,
        Some(&cancellable),
        {
            let cancellable = cancellable.clone();
            let on_event = on_event.clone();
            move |result| match result {
                Ok(enumerator) => next_batch(enumerator, cancellable, generation, on_event),
                Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {
                    debug!(
                        generation = generation.value(),
                        "directory enumeration cancelled"
                    );
                }
                Err(error) => {
                    warn!(generation = generation.value(), %error, "directory enumeration failed");
                    on_event(DirectoryEvent::Failed {
                        generation,
                        message: error.to_string(),
                    });
                }
            }
        },
    );

    cancellable
}

fn next_batch(
    enumerator: gio::FileEnumerator,
    cancellable: gio::Cancellable,
    generation: Generation,
    on_event: Rc<dyn Fn(DirectoryEvent)>,
) {
    let callback_cancellable = cancellable.clone();
    enumerator.next_files_async(BATCH_SIZE, glib::Priority::DEFAULT, Some(&cancellable), {
        let enumerator_for_callback = enumerator.clone();
        move |result| match result {
            Ok(infos) if infos.is_empty() => {
                on_event(DirectoryEvent::Finished { generation });
            }
            Ok(infos) => {
                let entries = infos
                    .iter()
                    .map(|info| file_entry(&enumerator_for_callback, info))
                    .collect();
                on_event(DirectoryEvent::Batch {
                    generation,
                    entries,
                });
                next_batch(
                    enumerator_for_callback,
                    callback_cancellable,
                    generation,
                    on_event,
                );
            }
            Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {
                debug!(generation = generation.value(), "directory batch cancelled");
            }
            Err(error) => {
                warn!(generation = generation.value(), %error, "directory batch failed");
                on_event(DirectoryEvent::Failed {
                    generation,
                    message: error.to_string(),
                });
            }
        }
    });
}

fn file_entry(enumerator: &gio::FileEnumerator, info: &gio::FileInfo) -> FileEntry {
    let file_type = info.file_type();
    let is_symlink = info.is_symlink();
    let kind = if is_symlink {
        FileKind::Symlink
    } else {
        match file_type {
            gio::FileType::Directory => FileKind::Directory,
            gio::FileType::Regular => FileKind::Regular,
            gio::FileType::SymbolicLink => FileKind::Symlink,
            gio::FileType::Special | gio::FileType::Shortcut | gio::FileType::Mountable => {
                FileKind::Special
            }
            _ => FileKind::Unknown,
        }
    };
    let modified = info.modification_date_time().and_then(|date_time| {
        let seconds = date_time.to_unix();
        u64::try_from(seconds)
            .ok()
            .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds))
    });

    FileEntry {
        location: Location::new(enumerator.child(info).uri()),
        display_name: info.display_name().to_string(),
        kind,
        size: (info.size() >= 0).then_some(info.size() as u64),
        modified,
        content_type: info.content_type().map(|value| value.to_string()),
        is_hidden: info.is_hidden(),
        is_symlink,
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, fs, rc::Rc};

    use super::*;
    use pathpilot_core::GenerationTracker;

    #[test]
    fn asynchronously_enumerates_typed_entries() {
        let temporary = tempfile::tempdir().expect("create temporary directory");
        fs::write(temporary.path().join("hello.txt"), b"hello").expect("create test file");
        fs::create_dir(temporary.path().join("folder")).expect("create test directory");

        let file = gio::File::for_path(temporary.path());
        let location = Location::new(file.uri());
        let generation = GenerationTracker::default().advance();
        let entries = Rc::new(RefCell::new(Vec::new()));
        let failure = Rc::new(RefCell::new(None));
        let main_loop = glib::MainLoop::new(None, false);

        let _cancellable = load_directory(&location, generation, {
            let entries = entries.clone();
            let failure = failure.clone();
            let main_loop = main_loop.clone();
            move |event| match event {
                DirectoryEvent::Batch {
                    generation: received,
                    entries: batch,
                } => {
                    assert_eq!(received, generation);
                    entries.borrow_mut().extend(batch);
                }
                DirectoryEvent::Finished {
                    generation: received,
                } => {
                    assert_eq!(received, generation);
                    main_loop.quit();
                }
                DirectoryEvent::Failed { message, .. } => {
                    *failure.borrow_mut() = Some(message);
                    main_loop.quit();
                }
            }
        });
        main_loop.run();

        assert_eq!(failure.borrow().as_deref(), None);
        let entries = entries.borrow();
        let file = entries
            .iter()
            .find(|entry| entry.display_name == "hello.txt")
            .expect("file is enumerated");
        assert_eq!(file.kind, FileKind::Regular);
        assert_eq!(file.size, Some(5));
        assert!(file.content_type.is_some());
        assert!(
            entries.iter().any(|entry| {
                entry.display_name == "folder" && entry.kind == FileKind::Directory
            })
        );
    }
}
