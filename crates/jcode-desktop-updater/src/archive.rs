//! Downloading, verifying and extracting published desktop packages.
//!
//! Every package is a single top-level directory holding a fixed set of files.
//! Anything else (links, traversal, extra or missing files) is rejected before
//! a byte of it can reach an installation.
use anyhow::{Context, Result, ensure};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use crate::release::Target;

pub const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;
const MAX_EXTRACTED: u64 = 2 * MAX_ARCHIVE;

/// Files in a Linux or FreeBSD tarball. The first three are executables.
pub const UNIX_FILES: [&str; 5] = [
    "jcode-desktop",
    "jcode",
    "jcode-harness-api-bridge",
    "jcode.desktop",
    "jcode.png",
];
/// Files in a Windows zip. The first three are executables.
pub const WINDOWS_FILES: [&str; 5] = [
    "jcode-desktop.exe",
    "jcode.exe",
    "jcode-harness-api-bridge.exe",
    "jcode-desktop.exe.manifest",
    "Jcode.png",
];

pub fn files(target: &Target) -> &'static [&'static str; 5] {
    if target.zip {
        &WINDOWS_FILES
    } else {
        &UNIX_FILES
    }
}

pub fn is_executable(name: &str) -> bool {
    UNIX_FILES[..3].contains(&name) || WINDOWS_FILES[..3].contains(&name)
}

pub fn download(
    client: &reqwest::blocking::Client,
    url: &str,
    writer: &mut impl Write,
    limit: u64,
) -> Result<()> {
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("Downloading {url}"))?
        .error_for_status()
        .with_context(|| format!("Download failed: {url}"))?;
    ensure!(
        response.content_length().is_none_or(|size| size <= limit),
        "Download exceeds safety size limit"
    );
    let size = std::io::copy(&mut response.take(limit + 1), writer)
        .context("Reading release download (network timeout or incomplete response)")?;
    ensure!(size <= limit, "Download exceeds safety size limit");
    Ok(())
}

pub fn checksum(manifest: &str, asset: &str) -> Result<String> {
    let mut found = None;
    for line in manifest.lines().filter(|line| !line.trim().is_empty()) {
        let split = line
            .find(char::is_whitespace)
            .context("Malformed SHA256SUMS entry")?;
        let (hash, name) = line.split_at(split);
        ensure!(
            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid SHA256SUMS digest"
        );
        // Historical published sums contain build-machine absolute paths. Only
        // compare basenames, never use checksum paths for filesystem access.
        let normalized = name.trim().trim_start_matches('*').replace('\\', "/");
        if normalized.rsplit('/').next() == Some(asset) {
            ensure!(found.is_none(), "Duplicate archive checksum");
            found = Some(hash.to_ascii_lowercase());
        }
    }
    found.context("Archive is missing from SHA256SUMS")
}

pub fn verify(file: &mut File, expected: &str) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    ensure!(
        format!("{:x}", hash.finalize()) == expected,
        "Desktop archive SHA-256 mismatch. Nothing was installed"
    );
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}

fn create(destination: &Path, name: &str) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(destination.join(name))?)
}

fn finish(output: File, name: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        output.set_permissions(fs::Permissions::from_mode(if is_executable(name) {
            0o755
        } else {
            0o644
        }))?;
    }
    #[cfg(not(unix))]
    let _ = name;
    output.sync_all()?;
    Ok(())
}

pub fn extract(target: &Target, file: File, destination: &Path, archive_root: &str) -> Result<()> {
    if target.zip {
        extract_zip(file, destination, archive_root, files(target))
    } else {
        extract_tar(file, destination, archive_root, files(target))
    }
}

pub fn extract_tar(
    reader: impl Read,
    destination: &Path,
    archive_root: &str,
    expected: &[&'static str],
) -> Result<()> {
    let mut archive = tar::Archive::new(GzDecoder::new(reader));
    let mut seen = HashSet::new();
    let mut root_seen = false;
    let mut total = 0u64;
    for entry in archive.entries().context("Reading desktop archive")? {
        let mut entry = entry?;
        // Compare the raw bytes: Path components normalize './', but no
        // alternate spelling, traversal, absolute path, or nested file is allowed.
        let raw = entry.path_bytes().into_owned();
        if raw == archive_root.as_bytes() || raw == format!("{archive_root}/").as_bytes() {
            ensure!(
                entry.header().entry_type().is_dir() && !root_seen,
                "Invalid or duplicate archive root"
            );
            root_seen = true;
            continue;
        }
        let name = expected
            .iter()
            .find(|name| raw == format!("{archive_root}/{name}").as_bytes())
            .context("Archive contains an unexpected path")?;
        ensure!(
            entry.header().entry_type().is_file(),
            "Archive links and special files are forbidden"
        );
        ensure!(seen.insert(*name), "Archive contains duplicate files");
        let size = entry.size();
        total = total.checked_add(size).context("Archive size overflow")?;
        ensure!(
            size > 0 && total <= MAX_EXTRACTED,
            "Archive file size exceeds safety limits or is empty"
        );
        let mut output = create(destination, name)?;
        ensure!(
            std::io::copy(&mut entry, &mut output)? == size,
            "Truncated archive file"
        );
        finish(output, name)?;
    }
    ensure!(
        seen.len() == expected.len(),
        "Desktop archive is missing required files"
    );
    // Consume the decoder to validate the gzip trailer/CRC even when tar stops
    // at its end marker. Bound padding to prevent decompression bombs.
    let mut decoder = archive.into_inner();
    let trailing = std::io::copy(
        &mut decoder.by_ref().take(1024 * 1024 + 1),
        &mut std::io::sink(),
    )?;
    ensure!(trailing <= 1024 * 1024, "Excessive archive padding");
    Ok(())
}

pub fn extract_zip(
    file: File,
    destination: &Path,
    archive_root: &str,
    expected: &[&'static str],
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(file).context("Reading desktop archive")?;
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        // PowerShell versions differ on separators. Normalize, then demand an
        // exact match, so traversal and nested paths can never be spelled.
        let raw = entry.name().replace('\\', "/");
        if entry.is_dir() {
            ensure!(
                raw == format!("{archive_root}/"),
                "Archive contains an unexpected directory"
            );
            continue;
        }
        let name = expected
            .iter()
            .find(|name| raw == format!("{archive_root}/{name}"))
            .context("Archive contains an unexpected path")?;
        ensure!(
            !entry.is_symlink() && entry.is_file(),
            "Archive links and special files are forbidden"
        );
        ensure!(seen.insert(*name), "Archive contains duplicate files");
        let size = entry.size();
        total = total.checked_add(size).context("Archive size overflow")?;
        ensure!(
            size > 0 && total <= MAX_EXTRACTED,
            "Archive file size exceeds safety limits or is empty"
        );
        let mut output = create(destination, name)?;
        // The zip reader validates each entry's CRC once it is fully read.
        let copied = std::io::copy(&mut (&mut entry).take(size + 1), &mut output)?;
        ensure!(copied == size, "Truncated or oversized archive file");
        finish(output, name)?;
    }
    ensure!(
        seen.len() == expected.len(),
        "Desktop archive is missing required files"
    );
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::time::Duration;

    pub(crate) fn tarball(entries: &[(&str, tar::EntryType)]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, kind) in entries {
            let mut header = tar::Header::new_gnu();
            // Raw name bytes deliberately allow malformed paths for security tests.
            header.as_mut_bytes()[..100].fill(0);
            header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
            header.set_entry_type(*kind);
            header.set_mode(0o7777);
            header.set_size(if kind.is_file() { 4 } else { 0 });
            if kind.is_symlink() || kind.is_hard_link() {
                header.set_link_name("/etc/passwd").unwrap();
            }
            header.set_cksum();
            builder
                .append(
                    &header,
                    if kind.is_file() {
                        &b"test"[..]
                    } else {
                        &[][..]
                    },
                )
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    pub(crate) fn zipfile(entries: &[(&str, &[u8])]) -> File {
        let mut file = tempfile::tempfile().unwrap();
        {
            let mut writer = zip::ZipWriter::new(&mut file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in entries {
                if name.ends_with('/') {
                    writer.add_directory(*name, options).unwrap();
                } else {
                    writer.start_file(*name, options).unwrap();
                    writer.write_all(bytes).unwrap();
                }
            }
            writer.finish().unwrap();
        }
        file.seek(SeekFrom::Start(0)).unwrap();
        file
    }

    fn reader(bytes: Vec<u8>) -> File {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&bytes).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file
    }

    #[test]
    fn checksums_are_exact_unique_and_support_historical_paths() {
        let hash = "ab".repeat(32);
        assert_eq!(
            checksum(&format!("{hash}  /build/archive.tar.gz\n"), "archive.tar.gz").unwrap(),
            hash
        );
        assert!(checksum(&format!("{hash}  other.tar.gz"), "archive.tar.gz").is_err());
        assert!(
            checksum(
                &format!("{hash}  archive.tar.gz\n{hash}  archive.tar.gz"),
                "archive.tar.gz"
            )
            .is_err()
        );
        assert!(checksum("invalid  archive.tar.gz", "archive.tar.gz").is_err());
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"verified payload").unwrap();
        let expected = format!("{:x}", Sha256::digest(b"verified payload"));
        verify(&mut file, &expected).unwrap();
        assert_eq!(file.stream_position().unwrap(), 0);
        assert!(verify(&mut file, &hash).is_err());
    }

    #[test]
    fn tar_accepts_only_manifest_and_strips_dangerous_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = UNIX_FILES.iter().map(|name| format!("release/{name}")).collect();
        let entries: Vec<_> = paths
            .iter()
            .map(|path| (path.as_str(), tar::EntryType::Regular))
            .collect();
        extract_tar(reader(tarball(&entries)), dir.path(), "release", &UNIX_FILES).unwrap();
        for name in UNIX_FILES {
            assert_eq!(fs::read(dir.path().join(name)).unwrap(), b"test");
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let mode = fs::metadata(dir.path().join(name)).unwrap().mode() & 0o7777;
                assert_eq!(mode, if is_executable(name) { 0o755 } else { 0o644 });
            }
        }
    }

    #[test]
    fn tar_rejects_traversal_links_special_unknown_missing_and_duplicates() {
        for (path, kind) in [
            ("../escape", tar::EntryType::Regular),
            ("/absolute", tar::EntryType::Regular),
            ("release/../escape", tar::EntryType::Regular),
            ("release/./jcode", tar::EntryType::Regular),
            ("release/unknown", tar::EntryType::Regular),
            ("release/jcode", tar::EntryType::Symlink),
            ("release/jcode", tar::EntryType::Link),
            ("release/jcode", tar::EntryType::Fifo),
            ("release/jcode", tar::EntryType::Directory),
            ("release/jcode", tar::EntryType::Regular), // Missing the other files.
        ] {
            let dir = tempfile::tempdir().unwrap();
            assert!(
                extract_tar(reader(tarball(&[(path, kind)])), dir.path(), "release", &UNIX_FILES)
                    .is_err(),
                "accepted {path} {kind:?}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(
            extract_tar(
                reader(tarball(&[("release/jcode", tar::EntryType::Regular); 2])),
                dir.path(),
                "release",
                &UNIX_FILES
            )
            .is_err()
        );
    }

    #[test]
    fn corrupt_gzip_is_rejected() {
        let paths: Vec<_> = UNIX_FILES.iter().map(|name| format!("release/{name}")).collect();
        let entries: Vec<_> = paths
            .iter()
            .map(|path| (path.as_str(), tar::EntryType::Regular))
            .collect();
        let mut bytes = tarball(&entries);
        let length = bytes.len();
        bytes[length - 8] ^= 0xff;
        let dir = tempfile::tempdir().unwrap();
        assert!(extract_tar(reader(bytes), dir.path(), "release", &UNIX_FILES).is_err());
    }

    #[test]
    fn zip_accepts_published_layout_with_either_separator() {
        for separator in ["/", "\\"] {
            let names: Vec<_> = WINDOWS_FILES
                .iter()
                .map(|name| format!("release{separator}{name}"))
                .collect();
            let mut entries: Vec<(&str, &[u8])> =
                names.iter().map(|name| (name.as_str(), &b"test"[..])).collect();
            entries.insert(0, ("release/", b""));
            let dir = tempfile::tempdir().unwrap();
            extract_zip(zipfile(&entries), dir.path(), "release", &WINDOWS_FILES).unwrap();
            for name in WINDOWS_FILES {
                assert_eq!(fs::read(dir.path().join(name)).unwrap(), b"test");
            }
        }
    }

    #[test]
    fn zip_rejects_unexpected_traversal_missing_and_duplicates() {
        let full: Vec<_> = WINDOWS_FILES.iter().map(|name| format!("release/{name}")).collect();
        let valid: Vec<(&str, &[u8])> = full.iter().map(|name| (name.as_str(), &b"x"[..])).collect();
        for extra in ["release/evil.dll", "../release/jcode.exe", "release/sub/jcode.exe", "other/"] {
            let mut entries = valid.clone();
            entries.push((extra, b"x"));
            let dir = tempfile::tempdir().unwrap();
            assert!(
                extract_zip(zipfile(&entries), dir.path(), "release", &WINDOWS_FILES).is_err(),
                "accepted {extra}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(extract_zip(zipfile(&valid[..4]), dir.path(), "release", &WINDOWS_FILES).is_err());
        let mut empty = valid.clone();
        empty[0].1 = b"";
        let dir = tempfile::tempdir().unwrap();
        assert!(extract_zip(zipfile(&empty), dir.path(), "release", &WINDOWS_FILES).is_err());
    }

    #[test]
    fn network_errors_timeouts_and_size_limits_are_bounded() {
        fn serve(response: &'static [u8], delay: Duration) -> (String, std::thread::JoinHandle<()>) {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/release", listener.local_addr().unwrap());
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                std::thread::sleep(delay);
                let _ = stream.write_all(response);
            });
            (url, worker)
        }
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        for (response, delay, limit) in [
            (&b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n"[..], Duration::ZERO, 100),
            (&b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ntest"[..], Duration::from_millis(250), 100),
            (&b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ntest"[..], Duration::ZERO, 3),
            (&b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\ntest"[..], Duration::ZERO, 3),
            (&b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\nshort"[..], Duration::ZERO, 100),
        ] {
            let (url, worker) = serve(response, delay);
            assert!(download(&client, &url, &mut Vec::new(), limit).is_err());
            worker.join().unwrap();
        }
        let (url, worker) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ntest", Duration::ZERO);
        let mut bytes = Vec::new();
        download(&client, &url, &mut bytes, 4).unwrap();
        worker.join().unwrap();
        assert_eq!(bytes, b"test");
    }
}
