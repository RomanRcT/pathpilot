use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
    time::UNIX_EPOCH,
};

use gio::prelude::*;
use pathpilot_core::{FileEntry, Location};
use tempfile::TempDir;

pub struct ArchiveSession {
    pub archive: Location,
    pub archive_name: String,
    pub root: Location,
    pub root_path: PathBuf,
    format: String,
    initial: BTreeMap<PathBuf, Fingerprint>,
    _temporary: TempDir,
}

pub enum ArchiveOpenEvent {
    Progress(u8),
    Finished(Result<ArchiveSession, String>),
}

pub struct ArchiveOpenHandle {
    cancelled: Arc<AtomicBool>,
}

impl ArchiveOpenHandle {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Fingerprint {
    directory: bool,
    size: u64,
    modified: Option<u128>,
}

impl ArchiveSession {
    pub fn open_async(entry: FileEntry) -> (ArchiveOpenHandle, mpsc::Receiver<ArchiveOpenEvent>) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let thread_cancelled = cancelled.clone();
        thread::spawn(move || {
            let result = Self::extract(&entry, &thread_cancelled, &sender);
            let _ = sender.send(ArchiveOpenEvent::Finished(result));
        });
        (ArchiveOpenHandle { cancelled }, receiver)
    }

    fn extract(
        entry: &FileEntry,
        cancelled: &AtomicBool,
        sender: &mpsc::Sender<ArchiveOpenEvent>,
    ) -> Result<Self, String> {
        let archive_path = gio::File::for_uri(entry.location.uri())
            .path()
            .ok_or_else(|| "Only local archives are supported".to_owned())?;
        let temporary = tempfile::Builder::new()
            .prefix("pathpilot-archive-")
            .tempdir()
            .map_err(|error| error.to_string())?;
        let total_size = archive_uncompressed_size(&archive_path);
        let mut child = Command::new("7z")
            .arg("x")
            .arg("-y")
            .arg("-bso0")
            .arg("-bse0")
            .arg(format!("-o{}", temporary.path().display()))
            .arg(&archive_path)
            .spawn()
            .map_err(|error| format!("Could not start 7z: {error}"))?;
        let mut last_progress = 0;
        let status = loop {
            if cancelled.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Archive extraction cancelled".to_owned());
            }
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                break status;
            }
            if let Some(total) = total_size.filter(|total| *total > 0)
                && let Ok(extracted) = directory_size(temporary.path())
            {
                let progress = ((extracted.saturating_mul(100) / total).min(99)) as u8;
                if progress != last_progress {
                    last_progress = progress;
                    let _ = sender.send(ArchiveOpenEvent::Progress(progress));
                }
            }
            thread::sleep(Duration::from_millis(100));
        };
        if cancelled.load(Ordering::Relaxed) {
            return Err("Archive extraction cancelled".to_owned());
        }
        if !status.success() {
            return Err("Could not extract archive".to_owned());
        }
        let _ = sender.send(ArchiveOpenEvent::Progress(100));
        let format = entry
            .archive_format
            .clone()
            .unwrap_or_else(|| "zip".to_owned());
        let root_path = temporary.path().to_path_buf();
        let initial = snapshot(&root_path).map_err(|error| error.to_string())?;
        Ok(Self {
            archive: entry.location.clone(),
            archive_name: entry.display_name.clone(),
            root: Location::new(gio::File::for_path(&root_path).uri()),
            root_path,
            format,
            initial,
            _temporary: temporary,
        })
    }

    pub fn contains(&self, location: &Location) -> bool {
        gio::File::for_uri(location.uri())
            .path()
            .is_some_and(|path| path.starts_with(&self.root_path))
    }

    pub fn changed(&self) -> bool {
        snapshot(&self.root_path).map_or(true, |current| current != self.initial)
    }

    pub fn save(&self) -> Result<(), String> {
        if self.format.eq_ignore_ascii_case("rar") {
            return Err("RAR archives are read-only because 7z cannot create RAR files".to_owned());
        }
        let archive_path = gio::File::for_uri(self.archive.uri())
            .path()
            .ok_or_else(|| "Archive is no longer a local file".to_owned())?;
        let parent = archive_path
            .parent()
            .ok_or_else(|| "Archive has no parent".to_owned())?;
        let suffix = archive_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("archive");
        let replacement = tempfile::Builder::new()
            .prefix(".pathpilot-save-")
            .suffix(&format!(".{suffix}"))
            .tempfile_in(parent)
            .map_err(|error| error.to_string())?;
        let replacement_path = replacement.path().to_path_buf();
        drop(replacement);
        let _ = fs::remove_file(&replacement_path);
        let format = normalize_format(&self.format);
        if snapshot(&self.root_path)
            .map_err(|error| error.to_string())?
            .is_empty()
        {
            match format {
                "zip" => fs::write(
                    &replacement_path,
                    b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
                )
                .map_err(|error| error.to_string())?,
                "tar" => {
                    fs::write(&replacement_path, [0_u8; 1024]).map_err(|error| error.to_string())?
                }
                _ => return Err(format!("Empty {format} archives cannot be created")),
            }
            return fs::rename(&replacement_path, &archive_path)
                .map_err(|error| format!("Could not replace archive: {error}"));
        }
        let output = Command::new("7z")
            .current_dir(&self.root_path)
            .arg("a")
            .arg(format!("-t{format}"))
            .arg("-y")
            .arg(&replacement_path)
            .arg(".")
            .output()
            .map_err(|error| format!("Could not start 7z: {error}"))?;
        if !output.status.success() {
            let _ = fs::remove_file(&replacement_path);
            return Err(command_error("Could not rebuild archive", &output));
        }
        fs::rename(&replacement_path, &archive_path)
            .map_err(|error| format!("Could not replace archive: {error}"))
    }
}

fn archive_uncompressed_size(path: &Path) -> Option<u64> {
    let output = Command::new("7z")
        .args(["l", "-slt", "-bso1", "-bse0"])
        .arg(path)
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    let listing = String::from_utf8_lossy(&output.stdout);
    Some(
        listing
            .lines()
            .filter_map(|line| line.strip_prefix("Size = ")?.trim().parse::<u64>().ok())
            .sum(),
    )
}

fn directory_size(path: &Path) -> io::Result<u64> {
    let mut size = 0_u64;
    for item in fs::read_dir(path)? {
        let item = item?;
        let metadata = item.metadata()?;
        size = size.saturating_add(if metadata.is_dir() {
            directory_size(&item.path())?
        } else {
            metadata.len()
        });
    }
    Ok(size)
}

fn normalize_format(format: &str) -> &str {
    match format.to_ascii_lowercase().as_str() {
        "gzip" => "gzip",
        "bzip2" => "bzip2",
        "xz" => "xz",
        "tar" => "tar",
        "7z" => "7z",
        _ => "zip",
    }
}

fn command_error(prefix: &str, output: &std::process::Output) -> String {
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}: {detail}")
    }
}

fn snapshot(root: &Path) -> io::Result<BTreeMap<PathBuf, Fingerprint>> {
    fn visit(
        root: &Path,
        path: &Path,
        result: &mut BTreeMap<PathBuf, Fingerprint>,
    ) -> io::Result<()> {
        for item in fs::read_dir(path)? {
            let item = item?;
            let path = item.path();
            let metadata = fs::symlink_metadata(&path)?;
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            result.insert(
                relative,
                Fingerprint {
                    directory: metadata.is_dir(),
                    size: metadata.len(),
                    modified: metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map(|value| value.as_nanos()),
                },
            );
            if metadata.is_dir() {
                visit(root, &path, result)?;
            }
        }
        Ok(())
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result)?;
    Ok(result)
}

pub fn copy_to_staging(
    entries: &[FileEntry],
) -> Result<(TempDir, Vec<(Location, String)>), String> {
    let staging = tempfile::Builder::new()
        .prefix("pathpilot-cut-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for entry in entries {
        let source = gio::File::for_uri(entry.location.uri())
            .path()
            .ok_or("Non-local archive item")?;
        let destination = staging.path().join(&entry.display_name);
        copy_tree(&source, &destination).map_err(|e| e.to_string())?;
        items.push((
            Location::new(gio::File::for_path(destination).uri()),
            entry.display_name.clone(),
        ));
    }
    Ok((staging, items))
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    if source.is_dir() {
        fs::create_dir_all(destination)?;
        for item in fs::read_dir(source)? {
            let item = item?;
            copy_tree(&item.path(), &destination.join(item.file_name()))?;
        }
    } else {
        fs::copy(source, destination)?;
    }
    Ok(())
}
