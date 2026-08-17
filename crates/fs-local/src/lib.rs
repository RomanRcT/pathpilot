//! Asynchronous directory enumeration through local and GVfs-backed GIO locations.

use std::{
    fs::File,
    io::Read,
    rc::Rc,
    time::{Duration, Instant, UNIX_EPOCH},
};

use gio::prelude::*;
use pathpilot_core::{FileEntry, FileKind, Generation, Location};
use tracing::{debug, info, warn};

const LOCAL_ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::is-hidden,standard::is-symlink,standard::size,standard::content-type,time::modified,unix::mode";
// Keep remote enumeration minimal. GVfs SMB may resolve requested metadata
// before returning the enumerator, turning even a one-entry folder into a
// multi-second operation on high-latency servers.
const REMOTE_ATTRIBUTES: &str =
    "standard::name,standard::display-name,standard::type,standard::is-hidden";
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
    let is_native = file.is_native();
    let attributes = if is_native {
        LOCAL_ATTRIBUTES
    } else {
        REMOTE_ATTRIBUTES
    };
    let started = Instant::now();
    let location_uri = location.uri().to_owned();
    let on_event: Rc<dyn Fn(DirectoryEvent)> = Rc::new(on_event);
    let query_flags = if is_native {
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS
    } else {
        gio::FileQueryInfoFlags::NONE
    };

    file.enumerate_children_async(
        attributes,
        query_flags,
        glib::Priority::DEFAULT,
        Some(&cancellable),
        {
            let cancellable = cancellable.clone();
            let on_event = on_event.clone();
            let location_uri = location_uri.clone();
            move |result| match result {
                Ok(enumerator) => {
                    info!(
                        generation = generation.value(),
                        elapsed_ms = started.elapsed().as_millis(),
                        location = location_uri,
                        "directory enumerator ready"
                    );
                    next_batch(
                        enumerator,
                        cancellable,
                        generation,
                        on_event,
                        BATCH_SIZE,
                        started,
                        location_uri,
                    );
                }
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
    batch_size: i32,
    started: Instant,
    location_uri: String,
) {
    let callback_cancellable = cancellable.clone();
    enumerator.next_files_async(batch_size, glib::Priority::DEFAULT, Some(&cancellable), {
        let enumerator_for_callback = enumerator.clone();
        move |result| match result {
            Ok(infos) if infos.is_empty() => {
                info!(
                    generation = generation.value(),
                    elapsed_ms = started.elapsed().as_millis(),
                    location = location_uri,
                    "directory enumeration reached end"
                );
                on_event(DirectoryEvent::Finished { generation });
            }
            Ok(infos) => {
                info!(
                    generation = generation.value(),
                    batch_entries = infos.len(),
                    elapsed_ms = started.elapsed().as_millis(),
                    location = location_uri,
                    "directory batch received"
                );
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
                    BATCH_SIZE,
                    started,
                    location_uri,
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
    let is_symlink =
        info.has_attribute(gio::FILE_ATTRIBUTE_STANDARD_IS_SYMLINK) && info.is_symlink();
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
    let modified = info
        .has_attribute(gio::FILE_ATTRIBUTE_TIME_MODIFIED)
        .then(|| info.modification_date_time())
        .flatten()
        .and_then(|date_time| {
            let seconds = date_time.to_unix();
            u64::try_from(seconds)
                .ok()
                .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds))
        });

    let child = enumerator.child(info);
    let display_name = info.display_name().to_string();
    // GVfs may expose a FUSE path even for a remote URI. Opening that path here
    // performs synchronous network I/O on the GTK thread once per file and can
    // freeze navigation for many seconds. Signature probing is a local-only
    // feature, so never use the path reported for a non-native GFile.
    let archive_format = (kind == FileKind::Regular && child.is_native())
        .then(|| child.path())
        .flatten()
        .and_then(|path| detect_archive_signature(&path));
    let content_type = info
        .has_attribute(gio::FILE_ATTRIBUTE_STANDARD_CONTENT_TYPE)
        .then(|| info.content_type())
        .flatten()
        .map(|value| value.to_string())
        .or_else(|| {
            (kind == FileKind::Regular).then(|| {
                let (guessed, _) = gio::content_type_guess(Some(&display_name), None);
                guessed.to_string()
            })
        });
    let is_hidden = if info.has_attribute(gio::FILE_ATTRIBUTE_STANDARD_IS_HIDDEN) {
        info.is_hidden()
    } else {
        display_name.starts_with('.')
    };
    FileEntry {
        location: Location::new(child.uri()),
        display_name,
        kind,
        size: info
            .has_attribute(gio::FILE_ATTRIBUTE_STANDARD_SIZE)
            .then(|| info.size())
            .filter(|size| *size >= 0)
            .map(|size| size as u64),
        modified,
        unix_mode: info
            .has_attribute(gio::FILE_ATTRIBUTE_UNIX_MODE)
            .then(|| info.attribute_uint32(gio::FILE_ATTRIBUTE_UNIX_MODE))
            .filter(|mode| *mode != 0),
        content_type,
        is_hidden,
        is_symlink,
        archive_format,
    }
}

/// Cheap content-based recognition for formats supported by the archive backend.
pub fn detect_archive_signature(path: &std::path::Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut header = [0_u8; 512];
    let count = file.read(&mut header).ok()?;
    let bytes = &header[..count];
    let format = if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
    {
        "zip"
    } else if bytes.starts_with(b"7z\xBC\xAF\x27\x1C") {
        "7z"
    } else if bytes.starts_with(b"Rar!\x1A\x07") {
        "rar"
    } else if bytes.starts_with(&[0x1f, 0x8b]) {
        "gzip"
    } else if bytes.starts_with(b"BZh") {
        "bzip2"
    } else if bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        "xz"
    } else if bytes.get(257..262) == Some(b"ustar") {
        "tar"
    } else {
        return None;
    };
    Some(format.to_owned())
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

    #[test]
    fn detects_archives_by_signature_without_using_extension() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let disguised_zip = temporary.path().join("payload.data");
        fs::write(&disguised_zip, b"PK\x03\x04rest").expect("write zip signature");
        assert_eq!(
            detect_archive_signature(&disguised_zip).as_deref(),
            Some("zip")
        );

        let ordinary = temporary.path().join("archive.zip");
        fs::write(&ordinary, b"not really an archive").expect("write ordinary file");
        assert_eq!(detect_archive_signature(&ordinary), None);
    }
}
