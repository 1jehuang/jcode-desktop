//! Payload-only PDF previews. Poppler never receives a caller-supplied path.
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;
const MAX_EDGE: u32 = 4096;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_DIAGNOSTIC_BYTES: u64 = 64 * 1024;
const MAX_PNG_BYTES: u64 = 4 * MAX_EDGE as u64 * MAX_EDGE as u64 + 1024 * 1024;

// Shared across all documents, including metadata probes. Queued callers wait at
// most 15 seconds, independently of the active command's 15-second deadline.
static POPPLER_SLOTS: CommandSlots = CommandSlots {
    active: Mutex::new(0),
    available: Condvar::new(),
};

struct CommandSlots {
    active: Mutex<usize>,
    available: Condvar,
}

impl CommandSlots {
    fn acquire(&self, timeout: Duration) -> Result<CommandPermit<'_>> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (mut active, _) = self
            .available
            .wait_timeout_while(active, timeout, |active| *active >= 2)
            .unwrap_or_else(|error| error.into_inner());
        ensure!(
            *active < 2,
            "PDF renderer is busy: timed out waiting for a rendering slot"
        );
        *active += 1;
        Ok(CommandPermit(self))
    }
}

struct CommandPermit<'a>(&'a CommandSlots);
impl Drop for CommandPermit<'_> {
    fn drop(&mut self) {
        let mut active = self
            .0
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *active -= 1;
        self.0.available.notify_one();
    }
}

/// An immutable document whose private payload survives until the last Arc drops.
/// Rendering is concurrency-safe: every call owns a separate output directory.
#[derive(Debug)]
pub(crate) struct PdfDocument {
    directory: TempDir,
    pages: usize,
}

impl PdfDocument {
    pub(crate) fn from_base64(payload: &str) -> Result<Self> {
        ensure!(!payload.is_empty(), "PDF payload is empty");
        // Check encoded length before allocating the decoded buffer.
        ensure!(
            payload.len() <= MAX_PDF_BYTES.div_ceil(3) * 4,
            "PDF exceeds the 20 MiB limit"
        );
        let bytes = STANDARD
            .decode(payload)
            .context("Invalid PDF base64 payload")?;
        ensure!(bytes.len() <= MAX_PDF_BYTES, "PDF exceeds the 20 MiB limit");
        ensure!(
            bytes.starts_with(b"%PDF-"),
            "PDF payload has no %PDF signature"
        );
        let mut builder = tempfile::Builder::new();
        builder.prefix("jcode-pdf-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        let directory = builder
            .tempdir()
            .context("Could not create private PDF storage")?;
        // NamedTempFile is mode 0600 on Unix, inside a mode 0700 directory.
        let mut file = tempfile::NamedTempFile::new_in(directory.path())?;
        file.write_all(&bytes)?;
        file.persist(directory.path().join("document.pdf"))
            .context("Could not store private PDF payload")?;
        let mut command = poppler_command("pdfinfo");
        command.arg(directory.path().join("document.pdf"));
        let metadata = run_poppler(&mut command, directory.path(), None, COMMAND_TIMEOUT)?;
        let metadata = std::str::from_utf8(&metadata).context("Invalid PDF metadata")?;
        let field = |name: &str| {
            metadata
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .map(str::trim)
        };
        ensure!(
            field("Encrypted:") == Some("no"),
            "Encrypted PDFs are not supported"
        );
        let pages: usize = field("Pages:")
            .context("PDF has no page count")?
            .parse()
            .context("Invalid PDF page count")?;
        ensure!(pages > 0, "PDF contains no pages");
        Ok(Self { directory, pages })
    }

    pub(crate) fn page_count(&self) -> usize {
        self.pages
    }

    /// Render exactly one 1-indexed page. `longest_edge` must be in 1..=4096.
    pub(crate) fn render_page(&self, page: usize, longest_edge: u32) -> Result<Vec<u8>> {
        ensure!(
            (1..=self.pages).contains(&page),
            "PDF page is outside 1..={}",
            self.pages
        );
        ensure!(
            (1..=MAX_EDGE).contains(&longest_edge),
            "PDF raster edge must be in 1..=4096"
        );
        let output = tempfile::Builder::new()
            .prefix("render-")
            .tempdir_in(self.directory.path())?;
        let prefix = output.path().join("page");
        let png = prefix.with_extension("png");
        let mut command = poppler_command("pdftoppm");
        command
            .args([
                "-f",
                &page.to_string(),
                "-l",
                &page.to_string(),
                "-singlefile",
                "-scale-to",
                &longest_edge.to_string(),
                "-png",
            ])
            .arg(self.directory.path().join("document.pdf"))
            .arg(prefix);
        run_poppler(&mut command, output.path(), Some(&png), COMMAND_TIMEOUT)?;
        let bytes =
            read_bounded(&png, MAX_PNG_BYTES).context("Could not read rendered PDF page")?;
        ensure!(
            bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "PDF renderer did not produce a PNG"
        );
        Ok(bytes)
    }
}

/// GUI launches on macOS often omit Homebrew from PATH. Honor an existing
/// executable on PATH first, then try the standard Apple Silicon / Intel roots.
fn poppler_command(program: &str) -> Command {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = |path: &Path| {
            fs::metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        };
        let on_path = std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|directory| executable(&directory.join(program)))
        });
        if !on_path {
            for directory in ["/opt/homebrew/bin", "/usr/local/bin"] {
                let path = Path::new(directory).join(program);
                if executable(&path) {
                    return Command::new(path);
                }
            }
        }
    }
    Command::new(program)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "PDF renderer output exceeds its size limit"
    );
    Ok(bytes)
}

/// File-backed stdio avoids pipe deadlocks. Polling bounds time and disk output,
/// including when malformed input makes Poppler emit excessive diagnostics.
fn run_poppler(
    command: &mut Command,
    directory: &Path,
    raster: Option<&Path>,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let _permit = POPPLER_SLOTS.acquire(COMMAND_TIMEOUT)?;
    let stdout_path = directory.join("stdout");
    let stderr_path = directory.join("stderr");
    let stdout = File::create(&stdout_path)?;
    let stderr = File::create(&stderr_path)?;
    let program = command.get_program().to_string_lossy().into_owned();
    command
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr.try_clone()?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: background previews must not flash a console.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("PDF preview requires Poppler ({program}). Linux: install poppler-utils (Debian/Ubuntu) or poppler (Fedora/Arch). macOS: brew install poppler.")
        } else { anyhow::anyhow!("Could not start PDF renderer {program}: {error}") }
    })?;
    let start = Instant::now();
    let result = (|| -> Result<_> {
        loop {
            ensure!(
                start.elapsed() < timeout,
                "PDF renderer {program} timed out after {} seconds",
                timeout.as_secs()
            );
            for (path, limit) in [
                (Some(stdout_path.as_path()), MAX_DIAGNOSTIC_BYTES),
                (Some(stderr_path.as_path()), MAX_DIAGNOSTIC_BYTES),
                (raster, MAX_PNG_BYTES),
            ] {
                if let Some(path) = path {
                    match fs::metadata(path) {
                        Ok(metadata) => ensure!(
                            metadata.len() <= limit,
                            "PDF renderer output exceeds its size limit"
                        ),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(20));
        }
    })();
    // Reap on every failure, not only on timeout.
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let status = result?;
    {
        // Diagnostics may contain payload text and private paths. Do not surface
        // them in UI errors; only classify password failures from a bounded read.
        let mut diagnostic = String::new();
        // Reopen for reading: the child's stderr handle was write-only.
        File::open(&stderr_path)?
            .take(MAX_DIAGNOSTIC_BYTES)
            .read_to_string(&mut diagnostic)
            .ok();
        if diagnostic.to_ascii_lowercase().contains("password") {
            bail!("Encrypted PDFs are not supported");
        }
        ensure!(
            status.success() && !diagnostic.contains("Syntax Error"),
            "PDF renderer {program} rejected the document (corrupt or unsupported PDF)"
        );
    }
    read_bounded(&stdout_path, MAX_DIAGNOSTIC_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    const FIXTURE: &[u8] = include_bytes!("../../../assets/previews/pdf-preview.pdf");

    #[test]
    fn rejects_zero_page_and_password_protected_documents() {
        // Build structurally complete PDFs with a real xref, so rejection tests
        // exercise page/encryption handling rather than a missing trailer.
        fn pdf(objects: &[&str], trailer_fields: &str) -> String {
            let mut pdf = String::from("%PDF-1.4\n");
            let mut offsets = vec![0];
            for (index, object) in objects.iter().enumerate() {
                offsets.push(pdf.len());
                pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
            }
            let xref = pdf.len();
            pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()));
            for offset in &offsets[1..] {
                pdf.push_str(&format!("{offset:010} 00000 n \n"));
            }
            pdf.push_str(&format!(
                "trailer\n<< /Size {} /Root 1 0 R {trailer_fields} >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len()
            ));
            pdf
        }
        let empty = pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [] /Count 0 >>",
            ],
            "",
        );
        assert!(PdfDocument::from_base64(&STANDARD.encode(empty)).is_err());
        let encrypted = pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] >>",
                "<< /Filter /Standard /V 1 /R 2 /O <0000000000000000000000000000000000000000000000000000000000000000> /U <0000000000000000000000000000000000000000000000000000000000000000> /P -4 >>",
            ],
            "/Encrypt 4 0 R /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>]",
        );
        let error = PdfDocument::from_base64(&STANDARD.encode(encrypted)).unwrap_err();
        assert!(error.to_string().contains("Encrypted"), "{error}");
    }

    #[test]
    fn concurrency_slots_queue_timeout_and_release() {
        let slots = CommandSlots {
            active: Mutex::new(0),
            available: Condvar::new(),
        };
        let one = slots.acquire(Duration::ZERO).unwrap();
        let two = slots.acquire(Duration::ZERO).unwrap();
        assert!(slots.acquire(Duration::from_millis(10)).is_err());
        drop(one);
        let three = slots.acquire(Duration::ZERO).unwrap();
        drop((two, three));
        assert_eq!(*slots.active.lock().unwrap(), 0);
        let one = slots.acquire(Duration::ZERO).unwrap();
        let two = slots.acquire(Duration::ZERO).unwrap();
        thread::scope(|scope| {
            let waiter = scope.spawn(|| slots.acquire(Duration::from_secs(1)).is_ok());
            drop(one);
            assert!(waiter.join().unwrap());
        });
        drop(two);
    }

    #[cfg(unix)]
    #[test]
    fn private_permissions_and_output_limits() {
        use std::os::unix::fs::PermissionsExt;
        let document = document();
        assert_eq!(
            fs::metadata(document.directory.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(document.directory.path().join("document.pdf"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let directory = tempfile::tempdir().unwrap();
        let mut command = Command::new("head");
        command.args(["-c", "1000000", "/dev/zero"]);
        assert!(
            run_poppler(&mut command, directory.path(), None, COMMAND_TIMEOUT)
                .unwrap_err()
                .to_string()
                .contains("size limit")
        );
    }

    fn document() -> PdfDocument {
        PdfDocument::from_base64(&STANDARD.encode(FIXTURE)).unwrap()
    }

    #[test]
    fn real_two_page_pdf_renders_portrait_and_landscape() {
        let document = Arc::new(document());
        assert_eq!(document.page_count(), 2);
        let workers: Vec<_> = [(1, (513, 683)), (2, (683, 513))]
            .into_iter()
            .map(|(page, expected)| {
                let document = document.clone();
                thread::spawn(move || {
                    let png = document.render_page(page, 683).unwrap();
                    let image = image::load_from_memory(&png).unwrap();
                    assert_eq!((image.width(), image.height()), expected);
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn private_payload_lives_until_last_owner_drops() {
        let document = Arc::new(document());
        let path = document.directory.path().to_owned();
        let other = document.clone();
        drop(document);
        assert!(path.join("document.pdf").exists());
        drop(other);
        assert!(!path.exists());
    }

    #[test]
    fn rejects_invalid_payloads_and_render_bounds() {
        for payload in [
            String::new(),
            "!invalid".into(),
            STANDARD.encode(b"not a PDF"),
            STANDARD.encode(b"%PDF-1.7\ncorrupt"),
            "A".repeat(MAX_PDF_BYTES.div_ceil(3) * 4 + 1),
            STANDARD.encode(vec![0; MAX_PDF_BYTES + 1]),
        ] {
            assert!(PdfDocument::from_base64(&payload).is_err());
        }
        let document = document();
        for (page, edge) in [(0, 512), (3, 512), (usize::MAX, 512), (1, 0), (1, 4097)] {
            assert!(document.render_page(page, edge).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn command_timeout_and_missing_poppler_errors() {
        let directory = tempfile::tempdir().unwrap();
        let mut command = Command::new("sleep");
        command.arg("5");
        let start = Instant::now();
        let error = run_poppler(
            &mut command,
            directory.path(),
            None,
            Duration::from_millis(50),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
        let error = run_poppler(
            &mut Command::new("jcode-nonexistent-poppler-test"),
            directory.path(),
            None,
            COMMAND_TIMEOUT,
        )
        .unwrap_err();
        assert!(error.to_string().contains("poppler-utils"));
        assert!(error.to_string().contains("brew install poppler"));
    }
}
