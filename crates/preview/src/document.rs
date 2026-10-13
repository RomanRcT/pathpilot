//! Lazy, cancellable document rendering through local Poppler and LibreOffice tools.

use std::{
    cell::Cell,
    collections::VecDeque,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::{Arc, Mutex, OnceLock, Weak, mpsc},
    time::{Duration, Instant},
};

use gio::prelude::*;

use crate::{FileEntry, Generation, PreviewContent, PreviewLimits, PreviewResult};

const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct DocumentSession {
    temporary: tempfile::TempDir,
    pdf: PathBuf,
    pub page_count: u32,
    limits: PreviewLimits,
    // Keep the initialized profile alive for subsequent document conversions.
    _office_profile: Option<Arc<OfficeProfile>>,
    pages: Mutex<VecDeque<(u32, Vec<u8>)>>,
    pub retained_bytes: u64,
}

#[derive(Debug)]
struct OfficeProfile {
    directory: tempfile::TempDir,
    conversion: Mutex<()>,
}

impl DocumentSession {
    fn new(
        temporary: tempfile::TempDir,
        pdf: PathBuf,
        page_count: u32,
        limits: PreviewLimits,
        office_profile: Option<Arc<OfficeProfile>>,
    ) -> Result<Self, String> {
        let retained_bytes = fs::metadata(&pdf).map_err(|error| error.to_string())?.len();
        Ok(Self {
            temporary,
            pdf,
            page_count,
            limits,
            _office_profile: office_profile,
            pages: Mutex::new(VecDeque::new()),
            retained_bytes,
        })
    }
}

#[derive(Clone, Debug)]
pub struct DocumentPreview {
    pub session: Arc<DocumentSession>,
    pub page_number: u32,
    pub pixbuf: gdk_pixbuf::Pixbuf,
}

fn document_extension(entry: &FileEntry) -> Option<&str> {
    let extension = entry.display_name.rsplit('.').next().unwrap_or_default();
    // Use a fixed extension rather than trusting filenames in staging paths.
    [
        "pdf", "doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "ppt", "pptx", "odp", "fodt",
        "fods", "fodp",
    ]
    .into_iter()
    .find(|known| extension.eq_ignore_ascii_case(known))
    .or_else(|| match entry.content_type.as_deref().unwrap_or_default() {
        "application/pdf" => Some("pdf"),
        "application/msword" => Some("doc"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some("docx"),
        "application/vnd.oasis.opendocument.text" => Some("odt"),
        "application/rtf" | "text/rtf" => Some("rtf"),
        "application/vnd.ms-excel" => Some("xls"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some("xlsx"),
        "application/vnd.oasis.opendocument.spreadsheet" => Some("ods"),
        "application/vnd.ms-powerpoint" => Some("ppt"),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => Some("pptx"),
        "application/vnd.oasis.opendocument.presentation" => Some("odp"),
        _ => None,
    })
}

pub fn is_document(entry: &FileEntry) -> bool {
    document_extension(entry).is_some()
}

pub(crate) fn load_document(
    entry: &FileEntry,
    generation: Generation,
    limits: PreviewLimits,
    cancellable: &gio::Cancellable,
    on_result: Rc<dyn Fn(PreviewResult)>,
) {
    let fail = |message: String| {
        on_result(PreviewResult {
            generation,
            content: Err(message),
        })
    };
    if entry.size.is_some_and(|size| size > MAX_DOCUMENT_BYTES) {
        fail(
            "Document preview is limited to 64 MiB. Open this file in its application.".to_owned(),
        );
        return;
    }
    let Some(extension) = document_extension(entry) else {
        return;
    };
    let temporary = match document_tempdir() {
        Ok(directory) => directory,
        Err(error) => {
            fail(error.to_string());
            return;
        }
    };
    let input = temporary.path().join(format!("input.{extension}"));
    let destination = gio::File::for_path(&input);
    let source = gio::File::for_uri(entry.location.uri());
    let is_pdf = extension == "pdf";
    let worker_cancellable = cancellable.clone();
    let copy_cancellable = cancellable.clone();
    let size_exceeded = Rc::new(Cell::new(false));
    let progress_size_exceeded = size_exceeded.clone();
    source.copy_async(
        &destination,
        gio::FileCopyFlags::NONE,
        glib::Priority::LOW,
        Some(cancellable),
        Some(Box::new(move |copied, _| {
            if copied > MAX_DOCUMENT_BYTES as i64 {
                progress_size_exceeded.set(true);
                copy_cancellable.cancel();
            }
        })),
        move |result| {
            if size_exceeded.get() {
                on_result(PreviewResult {
                    generation,
                    content: Err("Document preview is limited to 64 MiB".to_owned()),
                });
                return;
            }
            if worker_cancellable.is_cancelled() {
                return;
            }
            if let Err(error) = result {
                on_result(PreviewResult {
                    generation,
                    content: Err(error.to_string()),
                });
                return;
            }
            spawn_render(generation, &worker_cancellable, on_result, {
                let cancellable = worker_cancellable.clone();
                move || {
                    if fs::metadata(&input)
                        .map_err(|error| error.to_string())?
                        .len()
                        > MAX_DOCUMENT_BYTES
                    {
                        return Err("Document preview is limited to 64 MiB".to_owned());
                    }
                    let (pdf, office_profile) =
                        prepare_pdf(&temporary, &input, is_pdf, &cancellable)?;
                    if !is_pdf {
                        fs::remove_file(&input).map_err(|error| error.to_string())?;
                    }
                    let page_count = pdf_page_count(&temporary, &pdf, &cancellable)?;
                    let session = Arc::new(DocumentSession::new(
                        temporary,
                        pdf,
                        page_count,
                        limits,
                        office_profile,
                    )?);
                    let bytes = render_page(&session, 1, &cancellable)?;
                    Ok((session, 1, bytes))
                }
            });
        },
    );
}

pub fn load_document_page(
    session: Arc<DocumentSession>,
    page_number: u32,
    generation: Generation,
    on_result: impl Fn(PreviewResult) + 'static,
) -> gio::Cancellable {
    let cancellable = gio::Cancellable::new();
    let worker_cancellable = cancellable.clone();
    spawn_render(generation, &cancellable, Rc::new(on_result), move || {
        if page_number == 0 || page_number > session.page_count {
            return Err("Page is outside the document".to_owned());
        }
        let bytes = render_page(&session, page_number, &worker_cancellable)?;
        Ok((session, page_number, bytes))
    });
    cancellable
}

type RenderedPage = (Arc<DocumentSession>, u32, Vec<u8>);

fn spawn_render(
    generation: Generation,
    cancellable: &gio::Cancellable,
    on_result: Rc<dyn Fn(PreviewResult)>,
    work: impl FnOnce() -> Result<RenderedPage, String> + Send + 'static,
) {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    let cancellable = cancellable.clone();
    glib::timeout_add_local(Duration::from_millis(15), move || {
        if cancellable.is_cancelled() {
            return glib::ControlFlow::Break;
        }
        match receiver.try_recv() {
            Ok(result) => {
                let content = result.and_then(|(session, page_number, bytes)| {
                    gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(bytes))
                        .map(|pixbuf| {
                            PreviewContent::Document(DocumentPreview {
                                session,
                                page_number,
                                pixbuf,
                            })
                        })
                        .map_err(|error| error.to_string())
                });
                on_result(PreviewResult {
                    generation,
                    content,
                });
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                on_result(PreviewResult {
                    generation,
                    content: Err("Document renderer stopped unexpectedly".to_owned()),
                });
                glib::ControlFlow::Break
            }
        }
    });
}

fn document_tempdir() -> std::io::Result<tempfile::TempDir> {
    if Path::new("/.flatpak-info").exists() {
        // The application cache has the same absolute path on the host. Flatpak's
        // private /tmp cannot be passed to host tools through flatpak-spawn.
        let cache = glib::user_cache_dir().join("document-preview");
        fs::create_dir_all(&cache)?;
        tempfile::Builder::new()
            .prefix("document-")
            .tempdir_in(cache)
    } else {
        tempfile::Builder::new()
            .prefix("pathpilot-document-")
            .tempdir()
    }
}

fn office_profile() -> Result<Arc<OfficeProfile>, String> {
    // A Weak reference permits cleanup as soon as the last cached document is
    // evicted; static TempDirs would otherwise be left on disk at process exit.
    static PROFILE: OnceLock<Mutex<Weak<OfficeProfile>>> = OnceLock::new();
    let mut shared = PROFILE
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .map_err(|error| error.to_string())?;
    if let Some(profile) = shared.upgrade() {
        return Ok(profile);
    }
    let directory = document_tempdir().map_err(|error| error.to_string())?;
    let profile_path = directory.path().join("profile");
    fs::create_dir_all(profile_path.join("user")).map_err(|error| error.to_string())?;
    // Disable document macros even if the user's regular profile allows them.
    fs::write(profile_path.join("user/registrymodifications.xcu"),
        r#"<?xml version="1.0" encoding="UTF-8"?><oor:items xmlns:oor="http://openoffice.org/2001/registry"><item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item><item oor:path="/org.openoffice.Office.Common/Load"><prop oor:name="UpdateDocMode" oor:op="fuse"><value>0</value></prop></item></oor:items>"#,
    ).map_err(|error| error.to_string())?;

    let profile = Arc::new(OfficeProfile {
        directory,
        conversion: Mutex::new(()),
    });
    *shared = Arc::downgrade(&profile);
    Ok(profile)
}

fn prepare_pdf(
    temporary: &tempfile::TempDir,
    input: &Path,
    is_pdf: bool,
    cancellable: &gio::Cancellable,
) -> Result<(PathBuf, Option<Arc<OfficeProfile>>), String> {
    if is_pdf {
        return Ok((input.to_owned(), None));
    }
    let profile = office_profile()?;
    let conversion = loop {
        match profile.conversion.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                if cancellable.is_cancelled() {
                    return Err("Document preview cancelled".to_owned());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    let profile_uri = gio::File::for_path(profile.directory.path().join("profile")).uri();
    // An isolated profile avoids reusing or interfering with an open LibreOffice session.
    run_tool(
        "libreoffice",
        &[
            format!("-env:UserInstallation={profile_uri}").into(),
            "--headless".into(),
            "--nologo".into(),
            "--nodefault".into(),
            "--norestore".into(),
            "--convert-to".into(),
            "pdf".into(),
            "--outdir".into(),
            temporary.path().into(),
            input.into(),
        ],
        temporary.path(),
        cancellable,
    )?;
    let pdf = temporary.path().join("input.pdf");
    if !pdf.is_file() {
        return Err(
            "LibreOffice could not convert this document (it may be damaged or password-protected)"
                .to_owned(),
        );
    }
    drop(conversion);
    Ok((pdf, Some(profile)))
}

fn pdf_page_count(
    temporary: &tempfile::TempDir,
    pdf: &Path,
    cancellable: &gio::Cancellable,
) -> Result<u32, String> {
    let info = run_tool("pdfinfo", &[pdf.into()], temporary.path(), cancellable)?;
    parse_page_count(&info).ok_or_else(|| "Could not determine the document page count".to_owned())
}

fn parse_page_count(info: &str) -> Option<u32> {
    info.lines().find_map(|line| {
        line.strip_prefix("Pages:")
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|pages| *pages > 0)
    })
}

fn render_page(
    session: &DocumentSession,
    page: u32,
    cancellable: &gio::Cancellable,
) -> Result<Vec<u8>, String> {
    if cancellable.is_cancelled() {
        return Err("Document preview cancelled".to_owned());
    }
    {
        let mut pages = session.pages.lock().map_err(|error| error.to_string())?;
        if let Some(index) = pages.iter().position(|(number, _)| *number == page) {
            let cached = pages.remove(index).expect("page index exists");
            let bytes = cached.1.clone();
            pages.push_back(cached);
            return Ok(bytes);
        }
    }
    // Each request owns its output directory, including when a cancelled render is still exiting.
    let output = tempfile::Builder::new()
        .prefix("page-")
        .tempdir_in(session.temporary.path())
        .map_err(|error| error.to_string())?;
    let prefix = output.path().join("page");
    run_tool(
        "pdftoppm",
        &[
            "-f".into(),
            page.to_string().into(),
            "-l".into(),
            page.to_string().into(),
            "-singlefile".into(),
            "-scale-to".into(),
            session
                .limits
                .image_width
                .max(session.limits.image_height)
                .clamp(1, 2400)
                .to_string()
                .into(),
            "-png".into(),
            session.pdf.as_os_str().into(),
            prefix.into_os_string(),
        ],
        output.path(),
        cancellable,
    )?;
    let bytes = fs::read(output.path().join("page.png")).map_err(|error| error.to_string())?;
    let mut pages = session.pages.lock().map_err(|error| error.to_string())?;
    pages.push_back((page, bytes.clone()));
    while pages.len() > 4
        || pages.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() > 16 * 1024 * 1024
    {
        pages.pop_front();
    }
    Ok(bytes)
}

fn tool_command(program: &str) -> Command {
    if Path::new("/.flatpak-info").exists() {
        let mut host = Command::new("flatpak-spawn");
        host.args(["--host", "--watch-bus", "--env=LC_ALL=C", program]);
        host
    } else {
        Command::new(program)
    }
}

fn run_tool(
    program: &str,
    args: &[OsString],
    working_directory: &Path,
    cancellable: &gio::Cancellable,
) -> Result<String, String> {
    if cancellable.is_cancelled() {
        return Err("Document preview cancelled".to_owned());
    }
    let stdout = tempfile::tempfile().map_err(|error| error.to_string())?;
    let stderr = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut command = tool_command(program);
    command
        .args(args)
        .current_dir(working_directory)
        .env("LC_ALL", "C")
        .stdout(stdout.try_clone().map_err(|error| error.to_string())?)
        .stderr(stderr.try_clone().map_err(|error| error.to_string())?)
        .stdin(Stdio::null());
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("Document preview requires {program}. Install poppler-utils for PDF and LibreOffice for office documents.")
        } else { format!("Could not start {program}: {error}") }
    })?;
    let started = Instant::now();
    loop {
        if cancellable.is_cancelled() || started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancellable.is_cancelled() {
                "Document preview cancelled"
            } else {
                "Document preview timed out"
            }
            .to_owned());
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => {
                let detail = read_tool_output(stderr).unwrap_or_default();
                return Err(format!(
                    "{program} could not read this document. {}",
                    detail.trim()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    }
    read_tool_output(stdout)
}

fn read_tool_output(mut output: fs::File) -> Result<String, String> {
    use std::io::{Read, Seek};
    output.rewind().map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    output
        .take(16 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_page_pdf() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Length 44 >>\nstream\nBT /F1 18 Tf 20 150 Td (Preview test) Tj ET\nendstream",
        ];
        let mut pdf = "%PDF-1.4\n".to_owned();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
        }
        let xref = pdf.len();
        pdf.push_str("xref\n0 7\n0000000000 65535 f \n");
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        ));
        pdf.into_bytes()
    }

    fn cached_document(index: usize, retained_bytes: u64) -> (FileEntry, PreviewContent, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().to_owned();
        let pdf = directory.join("input.pdf");
        fs::write(&pdf, b"fixture").unwrap();
        let mut session =
            DocumentSession::new(temporary, pdf, 1, PreviewLimits::default(), None).unwrap();
        session.retained_bytes = retained_bytes;
        let entry = FileEntry {
            location: crate::Location::new(format!("file:///document-{index}.docx")),
            display_name: format!("document-{index}.docx"),
            kind: pathpilot_core::FileKind::Regular,
            size: Some(7),
            modified: Some(std::time::SystemTime::UNIX_EPOCH),
            unix_mode: None,
            content_type: None,
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        };
        let content = PreviewContent::Document(DocumentPreview {
            session: Arc::new(session),
            page_number: 1,
            pixbuf: gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 1, 1).unwrap(),
        });
        (entry, content, directory)
    }

    #[test]
    fn document_cache_invalidates_changes_and_bounds_retained_files() {
        let mut cache = crate::PreviewCache::new(24);
        let mut directories = Vec::new();
        for index in 0..5 {
            let (entry, content, directory) = cached_document(index, 7);
            directories.push(directory);
            cache.insert(&entry, content);
        }
        assert!(
            !directories[0].exists(),
            "the fifth document evicts and cleans up the oldest"
        );
        assert_eq!(cache.entries.len(), 4);
        let (mut entry, content, directory) = cached_document(5, 7);
        cache.insert(&entry, content);
        assert!(cache.get(&entry).is_some());
        entry.modified = Some(std::time::SystemTime::now());
        assert!(cache.get(&entry).is_none());
        cache.invalidate(&entry.location);
        assert!(!directory.exists());
        let (entry, content, directory) = cached_document(6, 128 * 1024 * 1024);
        cache.insert(&entry, content);
        assert!(
            cache.get(&entry).is_none(),
            "oversized documents are not retained"
        );
        assert!(!directory.exists());
        assert_eq!(
            cache.entries.len(),
            3,
            "an oversized document must not evict useful cached previews"
        );
        let (mut entry, content, directory) = cached_document(7, 7);
        entry.modified = None;
        cache.insert(&entry, content);
        assert!(
            cache.get(&entry).is_none(),
            "unknown modification times cannot validate cached documents"
        );
        assert!(!directory.exists());
    }

    #[test]
    fn conversions_share_an_isolated_profile_without_sharing_user_settings() {
        let first = office_profile().unwrap();
        let second = office_profile().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let settings = fs::read_to_string(
            first
                .directory
                .path()
                .join("profile/user/registrymodifications.xcu"),
        )
        .unwrap();
        assert!(settings.contains("MacroSecurityLevel"));
        assert!(settings.contains("<value>3</value>"));
    }

    #[test]
    #[ignore = "requires Poppler tools and an image decoder; run with --ignored"]
    fn document_pipeline_stages_and_navigates_pages() {
        use std::cell::RefCell;
        let _guard = crate::tests::async_test_guard();
        let temporary = tempfile::tempdir().unwrap();
        let input = temporary.path().join("file with spaces.PDF");
        fs::write(&input, two_page_pdf()).unwrap();
        let entry = FileEntry {
            location: crate::Location::new(gio::File::for_path(&input).uri()),
            display_name: "file with spaces.PDF".to_owned(),
            kind: pathpilot_core::FileKind::Regular,
            size: None,
            modified: None,
            unix_mode: None,
            content_type: Some("application/octet-stream".to_owned()),
            is_hidden: false,
            is_symlink: false,
            archive_format: None,
        };
        let main_loop = glib::MainLoop::new(None, false);
        let result = Rc::new(RefCell::new(None));
        let generation = crate::GenerationTracker::default().advance();
        let _cancellable = crate::load_preview(
            &entry,
            generation,
            PreviewLimits::default(),
            false,
            crate::PreviewMode::Automatic,
            {
                let main_loop = main_loop.clone();
                let result = result.clone();
                move |preview| {
                    *result.borrow_mut() = Some(preview.content);
                    main_loop.quit();
                }
            },
        );
        main_loop.run();
        let PreviewContent::Document(first) = result.borrow_mut().take().unwrap().unwrap() else {
            panic!("expected document");
        };
        assert_eq!(first.page_number, 1);
        assert_eq!(first.session.page_count, 2);
        let staged_path = first.session.temporary.path().to_owned();
        assert!(staged_path.exists());
        let _cancellable = load_document_page(first.session.clone(), 2, generation, {
            let main_loop = main_loop.clone();
            let result = result.clone();
            move |preview| {
                *result.borrow_mut() = Some(preview.content);
                main_loop.quit();
            }
        });
        main_loop.run();
        let PreviewContent::Document(second) = result.borrow_mut().take().unwrap().unwrap() else {
            panic!("expected document");
        };
        assert_eq!(second.page_number, 2);
        assert_eq!((second.pixbuf.width(), second.pixbuf.height()), (1200, 900));
        drop(first);
        drop(second);
        assert!(
            !staged_path.exists(),
            "staged files are removed when the session is dropped"
        );
        assert!(input.exists(), "the source is never removed");
    }

    #[test]
    #[ignore = "requires Poppler tools; run with --ignored"]
    fn pdf_pages_render_lazily_and_invalid_documents_fail() {
        let temporary = tempfile::tempdir().unwrap();
        let input = temporary.path().join("input.pdf");
        fs::write(&input, two_page_pdf()).unwrap();
        let cancellable = gio::Cancellable::new();
        let pages = pdf_page_count(&temporary, &input, &cancellable).unwrap();
        assert_eq!(pages, 2);
        let session =
            DocumentSession::new(temporary, input, pages, PreviewLimits::default(), None).unwrap();
        let first = gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(
            render_page(&session, 1, &cancellable).unwrap(),
        ))
        .unwrap();
        let second = gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(
            render_page(&session, 2, &cancellable).unwrap(),
        ))
        .unwrap();
        assert_eq!((first.width(), first.height()), (800, 1200));
        assert_eq!((second.width(), second.height()), (1200, 900));
        fs::write(&session.pdf, b"%PDF-invalid").unwrap();
        assert!(
            render_page(&session, 1, &cancellable)
                .unwrap()
                .starts_with(b"\x89PNG"),
            "rendered pages are reused without reopening the PDF"
        );
        assert!(pdf_page_count(&session.temporary, &session.pdf, &cancellable).is_err());
    }

    #[test]
    #[ignore = "requires LibreOffice Writer/Calc and Poppler tools; run with --ignored"]
    fn office_documents_and_spreadsheets_keep_their_page_content() {
        for (extension, output_extension, source, expected) in [
            (
                "fodt",
                "docx",
                r#"<?xml version="1.0"?><office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2" office:mimetype="application/vnd.oasis.opendocument.text"><office:body><office:text><text:h text:outline-level="1">Document preview</text:h><text:p>Preserved content</text:p></office:text></office:body></office:document>"#,
                "Preserved content",
            ),
            (
                "fods",
                "xlsx",
                r#"<?xml version="1.0"?><office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2" office:mimetype="application/vnd.oasis.opendocument.spreadsheet"><office:body><office:spreadsheet><table:table table:name="Budget"><table:table-column table:number-columns-repeated="2"/><table:table-row><table:table-cell office:value-type="string"><text:p>Revenue</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="123"><text:p>123</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document>"#,
                "Revenue",
            ),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let source_path = temporary.path().join(format!("input.{extension}"));
            fs::write(&source_path, source).unwrap();
            let cancellable = gio::Cancellable::new();
            let profile = gio::File::for_path(temporary.path().join("fixture-profile")).uri();
            run_tool(
                "libreoffice",
                &[
                    format!("-env:UserInstallation={profile}").into(),
                    "--headless".into(),
                    "--convert-to".into(),
                    output_extension.into(),
                    "--outdir".into(),
                    temporary.path().into(),
                    source_path.into_os_string(),
                ],
                temporary.path(),
                &cancellable,
            )
            .unwrap();
            let input = temporary.path().join(format!("input.{output_extension}"));
            assert!(input.is_file());
            let started = Instant::now();
            let (pdf, profile) = prepare_pdf(&temporary, &input, false, &cancellable).unwrap();
            let conversion_time = started.elapsed();
            let pages = pdf_page_count(&temporary, &pdf, &cancellable).unwrap();
            assert!(pages >= 1);
            let text = run_tool(
                "pdftotext",
                &[pdf.as_os_str().into(), "-".into()],
                temporary.path(),
                &cancellable,
            )
            .unwrap();
            assert!(text.contains(expected), "missing {expected}: {text}");
            let session =
                DocumentSession::new(temporary, pdf, pages, PreviewLimits::default(), profile)
                    .unwrap();
            let page = render_page(&session, 1, &cancellable).unwrap();
            assert!(page.starts_with(b"\x89PNG"));
            let uncached = started.elapsed();
            let mut cache = crate::PreviewCache::new(24);
            let entry = FileEntry {
                location: crate::Location::new(gio::File::for_path(&input).uri()),
                display_name: format!("input.{output_extension}"),
                kind: pathpilot_core::FileKind::Regular,
                size: Some(fs::metadata(&input).unwrap().len()),
                modified: Some(fs::metadata(&input).unwrap().modified().unwrap()),
                unix_mode: None,
                content_type: None,
                is_hidden: false,
                is_symlink: false,
                archive_format: None,
            };
            let session = Arc::new(session);
            cache.insert(
                &entry,
                PreviewContent::Document(DocumentPreview {
                    session: session.clone(),
                    page_number: 1,
                    pixbuf: gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(page)).unwrap(),
                }),
            );
            let lookup = Instant::now();
            let cached = cache.get(&entry).unwrap();
            let lookup_time = lookup.elapsed();
            let PreviewContent::Document(cached) = cached else {
                panic!("expected document");
            };
            assert!(Arc::ptr_eq(&session, &cached.session));
            // Hold the first session while converting a different file through the initialized profile.
            let next = tempfile::tempdir().unwrap();
            let next_input = next.path().join(format!("input.{output_extension}"));
            fs::copy(&input, &next_input).unwrap();
            let warm_started = Instant::now();
            let (_, warm_profile) = prepare_pdf(&next, &next_input, false, &cancellable).unwrap();
            let warm_time = warm_started.elapsed();
            assert!(Arc::ptr_eq(
                session._office_profile.as_ref().unwrap(),
                warm_profile.as_ref().unwrap()
            ));
            eprintln!(
                "{output_extension}: conversion {conversion_time:?}, with first page {uncached:?}; cache lookup {lookup_time:?}; next conversion with reused profile {warm_time:?}"
            );
        }
    }

    #[test]
    fn cancelled_renderer_stops_the_process() {
        let temporary = tempfile::tempdir().unwrap();
        let cancellable = gio::Cancellable::new();
        let cancel = cancellable.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            cancel.cancel();
        });
        let started = Instant::now();
        let result = run_tool("sleep", &["10".into()], temporary.path(), &cancellable);
        thread.join().unwrap();
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn page_count_requires_a_positive_number() {
        assert_eq!(
            parse_page_count("Title: Test\nPages:  12\nEncrypted: no\n"),
            Some(12)
        );
        assert_eq!(parse_page_count("Pages: 0"), None);
        assert_eq!(parse_page_count("Pages: unknown"), None);
    }
}
