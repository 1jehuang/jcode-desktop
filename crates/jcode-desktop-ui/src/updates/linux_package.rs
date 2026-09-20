//! Updates only the per-user, versioned Linux archive installation. Never relaunches.
use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use semver::Version;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
        },
    },
    path::{Path, PathBuf},
    time::Duration,
};

use super::release::{PUBLIC_RELEASES, parse_version};
const FILES: [&str; 5] = [
    "jcode-desktop",
    "jcode",
    "jcode-harness-api-bridge",
    "jcode.desktop",
    "jcode.png",
];
const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;
const MAX_EXTRACTED: u64 = 2 * MAX_ARCHIVE;

struct ReleaseTarget {
    architecture: &'static str,
    checksum_manifest: &'static str,
}

impl ReleaseTarget {
    fn for_architecture(architecture: &str) -> Result<Self> {
        let (architecture, checksum_manifest) = match architecture {
            "x86_64" => ("x86_64", "SHA256SUMS-linux"),
            "aarch64" => ("aarch64", "SHA256SUMS-linux-aarch64"),
            _ => bail!(
                "Managed archive updates support Linux x86_64 and aarch64 only (unsupported architecture: {architecture})"
            ),
        };
        Ok(Self {
            architecture,
            checksum_manifest,
        })
    }

    fn archive_names(&self, version: &Version) -> (String, String) {
        let root = format!("Jcode-{version}-linux-{}", self.architecture);
        let asset = format!("{root}.tar.gz");
        (root, asset)
    }
}

struct ManagedInstall {
    root: PathBuf,
    launcher: PathBuf,
    executable: PathBuf,
    version: Version,
}

fn secure_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Inspecting managed directory {}", path.display()))?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Managed directory must not be a symlink: {}",
        path.display()
    );
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
        "Managed directory must be owned by this user and not writable by others: {}",
        path.display()
    );
    Ok(())
}

fn detect(home: &Path, executable: &Path) -> Result<ManagedInstall> {
    let home = fs::canonicalize(home).context("Resolving home directory")?;
    let root = home.join(".local/opt/jcode-desktop");
    let executable =
        fs::canonicalize(executable).context("Resolving running desktop executable")?;
    let relative = executable.strip_prefix(&root)
        .context("Not a managed archive installation. System, .deb, and development installs must be updated with their original installer")?;
    for relative in [
        "",
        ".local",
        ".local/opt",
        ".local/opt/jcode-desktop",
        ".local/bin",
    ] {
        secure_directory(&home.join(relative))?;
    }
    let components: Vec<_> = relative.components().collect();
    ensure!(
        components.len() == 2 && components[1].as_os_str() == "jcode-desktop",
        "Executable is not in a managed version directory"
    );
    let version = parse_version(
        components[0]
            .as_os_str()
            .to_str()
            .context("Non-UTF8 version directory")?,
    )?;
    secure_directory(executable.parent().context("Missing version directory")?)?;
    let metadata = fs::symlink_metadata(&executable)?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "Managed executable has unsafe ownership or permissions"
    );
    let launcher = home.join(".local/bin/jcode-desktop");
    ensure!(
        fs::symlink_metadata(&launcher)?.file_type().is_symlink(),
        "Managed launcher must be a symlink"
    );
    ensure!(
        fs::canonicalize(&launcher)? == executable,
        "Launcher does not point to this running managed installation. Restart the desktop before updating again"
    );
    Ok(ManagedInstall {
        root,
        launcher,
        executable,
        version,
    })
}

fn lock(install: &ManagedInstall) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(install.root.join(".update.lock"))
        .context("Opening update lock")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0,
        "Unsafe desktop update lock file"
    );
    ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Another desktop update is already in progress"
    );
    Ok(file)
}

fn download(
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

fn checksum(manifest: &str, asset: &str) -> Result<String> {
    let mut found = None;
    for line in manifest.lines().filter(|line| !line.trim().is_empty()) {
        let split = line
            .find(char::is_whitespace)
            .context("Malformed SHA256SUMS-linux entry")?;
        let (hash, name) = line.split_at(split);
        ensure!(
            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid SHA256SUMS-linux digest"
        );
        // Historical published sums contain build-machine absolute paths. Only
        // compare basenames, never use checksum paths for filesystem access.
        let normalized = name.trim().trim_start_matches('*').replace('\\', "/");
        if normalized.rsplit('/').next() == Some(asset) {
            ensure!(found.is_none(), "Duplicate archive checksum");
            found = Some(hash.to_ascii_lowercase());
        }
    }
    found.context("Archive is missing from SHA256SUMS-linux")
}

fn verify(file: &mut File, expected: &str) -> Result<()> {
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

fn extract(reader: impl Read, destination: &Path, archive_root: &str) -> Result<()> {
    let mut archive = tar::Archive::new(GzDecoder::new(reader));
    let mut seen = HashSet::new();
    let mut root_seen = false;
    let mut total = 0u64;
    for entry in archive.entries().context("Reading desktop archive")? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        // Compare the raw spelling too: Path components normalize './', but no
        // alternate spelling, traversal, absolute path, or nested file is allowed.
        let raw = path.as_os_str().as_bytes();
        if raw == archive_root.as_bytes() || raw == format!("{archive_root}/").as_bytes() {
            ensure!(
                entry.header().entry_type().is_dir() && !root_seen,
                "Invalid or duplicate archive root"
            );
            root_seen = true;
            continue;
        }
        let name = FILES
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
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination.join(name))?;
        ensure!(
            std::io::copy(&mut entry, &mut output)? == size,
            "Truncated archive file"
        );
        output.set_permissions(fs::Permissions::from_mode(if FILES[..3].contains(name) {
            0o755
        } else {
            0o644
        }))?;
        output.sync_all()?;
    }
    ensure!(
        seen.len() == FILES.len(),
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
    File::open(destination)?.sync_all()?;
    Ok(())
}

fn activate(install: &ManagedInstall, staged: &Path, version: &Version) -> Result<()> {
    activate_with(install, staged, version, |source, target| {
        fs::rename(source, target)
    })
}

fn activate_with(
    install: &ManagedInstall,
    staged: &Path,
    version: &Version,
    switch_launcher: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    ensure!(
        version > &install.version,
        "Refusing to install an equal or older desktop version"
    );
    secure_directory(&install.root)?;
    secure_directory(
        install
            .launcher
            .parent()
            .context("Missing launcher parent")?,
    )?;
    ensure!(
        fs::symlink_metadata(&install.launcher)?
            .file_type()
            .is_symlink()
            && fs::canonicalize(&install.launcher)? == install.executable,
        "Launcher changed during update. Nothing was activated"
    );
    let destination = install.root.join(version.to_string());
    // Prepare all launcher-switch resources before publishing the version directory.
    let switch = tempfile::Builder::new()
        .prefix(".jcode-desktop-switch-")
        .tempdir_in(install.launcher.parent().unwrap())?;
    let link = switch.path().join("launcher");
    symlink(destination.join("jcode-desktop"), &link)?;
    let source_c = std::ffi::CString::new(staged.as_os_str().as_bytes())?;
    let destination_c = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
    // Unlike rename(), RENAME_NOREPLACE cannot overwrite an existing version,
    // even when another process creates that directory concurrently.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source_c.as_ptr(),
            libc::AT_FDCWD,
            destination_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!(
            "Could not install {} without replacing existing files. If this version directory already exists, inspect it and move it aside before retrying. Existing versions and launcher were preserved",
            destination.display()
        ));
    }
    let switch_result = (|| -> Result<()> {
        File::open(&install.root)?.sync_all()?;
        switch_launcher(&link, &install.launcher)?;
        Ok(())
    })();
    switch_result.with_context(|| format!(
        "Version {version} was saved at {}, but the launcher was not switched. Previous versions are preserved. Repair launcher permissions, then manually point {} to {}/jcode-desktop, or move the new version directory aside before retrying",
        destination.display(), install.launcher.display(), destination.display()
    ))?;
    File::open(install.launcher.parent().unwrap())?
        .sync_all()
        .context("The new desktop launcher is active, but syncing its directory failed")?;
    Ok(())
}

/// Download and activate a newer managed Linux archive. The running process and
/// old version remain untouched; the next normal launch uses the new version.
pub fn update() -> Result<String> {
    let target = ReleaseTarget::for_architecture(std::env::consts::ARCH)?;
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let install = detect(Path::new(&home), &std::env::current_exe()?)?;
    let _lock = lock(&install)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Jcode-Desktop-Managed-Updater")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    let version = super::release::latest()?;
    let tag = format!("desktop-v{version}");
    if version == install.version {
        return Ok(format!(
            "Jcode Desktop {} is already up to date.",
            install.version
        ));
    }
    if version < install.version {
        bail!(
            "Latest published desktop ({version}) is older than installed desktop ({}). Refusing to downgrade",
            install.version
        );
    }
    super::set(super::UpdateState::Available {
        version: version.to_string(),
    });
    let (archive_root, asset) = target.archive_names(&version);
    let base = format!("{PUBLIC_RELEASES}/{tag}");
    let mut sums = Vec::new();
    download(
        &client,
        &format!("{base}/{}", target.checksum_manifest),
        &mut sums,
        64 * 1024,
    )?;
    let expected = checksum(
        std::str::from_utf8(&sums).context("Non-UTF8 checksum manifest")?,
        &asset,
    )?;
    let stage = tempfile::Builder::new()
        .prefix(".update-")
        .tempdir_in(&install.root)?;
    let mut archive = tempfile::tempfile_in(stage.path())?;
    download(
        &client,
        &format!("{base}/{asset}"),
        &mut archive,
        MAX_ARCHIVE,
    )?;
    verify(&mut archive, &expected)?;
    let content = stage.path().join("content");
    fs::create_dir(&content)?;
    fs::set_permissions(&content, fs::Permissions::from_mode(0o755))?;
    extract(archive, &content, &archive_root)?;
    activate(&install, &content, &version)?;
    Ok(format!(
        "Jcode Desktop {version} installed. Quit and reopen Jcode Desktop to use it. Previous versions were preserved."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_names_match_linux_architecture() {
        let version = Version::parse("0.1.0-beta.10").unwrap();
        for (architecture, root, asset, manifest) in [
            (
                "x86_64",
                "Jcode-0.1.0-beta.10-linux-x86_64",
                "Jcode-0.1.0-beta.10-linux-x86_64.tar.gz",
                "SHA256SUMS-linux",
            ),
            (
                "aarch64",
                "Jcode-0.1.0-beta.10-linux-aarch64",
                "Jcode-0.1.0-beta.10-linux-aarch64.tar.gz",
                "SHA256SUMS-linux-aarch64",
            ),
        ] {
            let target = ReleaseTarget::for_architecture(architecture).unwrap();
            assert_eq!(target.archive_names(&version), (root.into(), asset.into()));
            assert_eq!(target.checksum_manifest, manifest);
        }
    }

    #[test]
    fn release_target_rejects_unsupported_architectures() {
        for architecture in ["", "x86", "arm", "arm64", "riscv64", "../aarch64"] {
            let error = ReleaseTarget::for_architecture(architecture)
                .err()
                .expect("unsupported architecture must be rejected");
            assert!(error.to_string().contains("unsupported architecture"));
        }
    }

    fn managed() -> (tempfile::TempDir, ManagedInstall) {
        let home = tempfile::tempdir().unwrap();
        let version_dir = home.path().join(".local/opt/jcode-desktop/0.1.0-beta.9");
        fs::create_dir_all(&version_dir).unwrap();
        fs::create_dir_all(home.path().join(".local/bin")).unwrap();
        let executable = version_dir.join("jcode-desktop");
        fs::write(&executable, "old desktop").unwrap();
        symlink(
            "../opt/jcode-desktop/0.1.0-beta.9/jcode-desktop",
            home.path().join(".local/bin/jcode-desktop"),
        )
        .unwrap();
        let install = detect(home.path(), &executable).unwrap();
        (home, install)
    }

    fn archive(entries: &[(&str, tar::EntryType)]) -> Vec<u8> {
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

    #[test]
    fn detects_relative_launcher_and_refuses_unmanaged() {
        let (home, install) = managed();
        assert_eq!(install.version, Version::parse("0.1.0-beta.9").unwrap());
        let system = home.path().join("system-desktop");
        fs::write(&system, "system").unwrap();
        assert!(detect(home.path(), &system).is_err());
        fs::remove_file(&install.launcher).unwrap();
        fs::write(&install.launcher, "not a symlink").unwrap();
        assert!(detect(home.path(), &install.executable).is_err());
    }

    #[test]
    fn refuses_foreign_launcher_and_writable_directories() {
        let (home, install) = managed();
        fs::set_permissions(&install.root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(detect(home.path(), &install.executable).is_err());
        fs::set_permissions(&install.root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_file(&install.launcher).unwrap();
        symlink("/usr/bin/true", &install.launcher).unwrap();
        assert!(detect(home.path(), &install.executable).is_err());
    }

    #[test]
    fn symlinked_managed_root_is_refused() {
        let (home, install) = managed();
        let moved = home.path().join("elsewhere");
        fs::rename(&install.root, &moved).unwrap();
        symlink(&moved, &install.root).unwrap();
        assert!(detect(home.path(), &install.executable).is_err());
    }

    #[test]
    fn checksums_are_exact_unique_and_support_historical_paths() {
        let hash = "ab".repeat(32);
        assert_eq!(
            checksum(
                &format!("{hash}  /build/archive.tar.gz\n"),
                "archive.tar.gz"
            )
            .unwrap(),
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
    fn extract_accepts_only_manifest_and_strips_dangerous_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = FILES.iter().map(|name| format!("release/{name}")).collect();
        let entries: Vec<_> = paths
            .iter()
            .map(|path| (path.as_str(), tar::EntryType::Regular))
            .collect();
        extract(archive(&entries).as_slice(), dir.path(), "release").unwrap();
        for name in FILES {
            assert_eq!(fs::read(dir.path().join(name)).unwrap(), b"test");
            let mode = fs::metadata(dir.path().join(name)).unwrap().mode() & 0o7777;
            assert_eq!(
                mode,
                if FILES[..3].contains(&name) {
                    0o755
                } else {
                    0o644
                }
            );
        }
    }

    #[test]
    fn extract_rejects_traversal_links_special_unknown_missing_and_duplicates() {
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
                extract(archive(&[(path, kind)]).as_slice(), dir.path(), "release").is_err(),
                "accepted {path} {kind:?}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(
            extract(
                archive(&[("release/jcode", tar::EntryType::Regular); 2]).as_slice(),
                dir.path(),
                "release"
            )
            .is_err()
        );
    }

    #[test]
    fn corrupt_gzip_is_rejected() {
        let paths: Vec<_> = FILES.iter().map(|name| format!("release/{name}")).collect();
        let entries: Vec<_> = paths
            .iter()
            .map(|path| (path.as_str(), tar::EntryType::Regular))
            .collect();
        let mut bytes = archive(&entries);
        let length = bytes.len();
        bytes[length - 8] ^= 0xff;
        let dir = tempfile::tempdir().unwrap();
        assert!(extract(bytes.as_slice(), dir.path(), "release").is_err());
    }

    #[test]
    fn atomic_activation_preserves_old_version_and_does_not_relaunch() {
        let (_home, install) = managed();
        let stage = tempfile::tempdir_in(&install.root).unwrap();
        let content = stage.path().join("content");
        fs::create_dir(&content).unwrap();
        fs::write(content.join("jcode-desktop"), "new desktop, not executable").unwrap();
        let version = Version::parse("0.1.0-beta.10").unwrap();
        activate(&install, &content, &version).unwrap();
        assert_eq!(
            fs::read_to_string(&install.executable).unwrap(),
            "old desktop"
        );
        assert_eq!(
            fs::read_to_string(&install.launcher).unwrap(),
            "new desktop, not executable"
        );
        assert_eq!(
            fs::read_link(&install.launcher).unwrap(),
            install.root.join("0.1.0-beta.10/jcode-desktop")
        );
    }

    #[test]
    fn refuses_downgrade_same_version_and_existing_destination() {
        let (_home, install) = managed();
        let stage = tempfile::tempdir_in(&install.root).unwrap();
        for version in ["0.1.0-beta.8", "0.1.0-beta.9"] {
            assert!(activate(&install, stage.path(), &Version::parse(version).unwrap()).is_err());
        }
        let existing = install.root.join("0.1.0-beta.10");
        fs::create_dir(&existing).unwrap();
        assert!(
            activate(
                &install,
                stage.path(),
                &Version::parse("0.1.0-beta.10").unwrap()
            )
            .is_err()
        );
        assert_eq!(
            fs::canonicalize(&install.launcher).unwrap(),
            install.executable
        );
        assert!(stage.path().exists());
    }

    #[test]
    fn refuses_launcher_changed_during_download_and_concurrent_updates() {
        let (_home, install) = managed();
        let held = lock(&install).unwrap();
        assert!(lock(&install).is_err());
        drop(held);
        lock(&install).unwrap();
        fs::remove_file(&install.launcher).unwrap();
        symlink("/usr/bin/true", &install.launcher).unwrap();
        let stage = tempfile::tempdir_in(&install.root).unwrap();
        assert!(activate(&install, stage.path(), &Version::parse("0.1.0").unwrap()).is_err());
        assert!(stage.path().exists());
    }

    #[test]
    fn unmanaged_install_reports_installer_guidance_without_managed_directories() {
        let home = tempfile::tempdir().unwrap();
        let executable = home.path().join("desktop");
        fs::write(&executable, "unmanaged").unwrap();
        let error = detect(home.path(), &executable).err().unwrap();
        assert!(format!("{error:#}").contains("original installer"));
    }

    #[test]
    fn semantic_versions_order_prereleases_numerically() {
        assert!(
            parse_version("0.1.0-beta.10").unwrap()
                > parse_version("desktop-v0.1.0-beta.9").unwrap()
        );
        assert!(parse_version("v0.1.0").unwrap() > parse_version("0.1.0-beta.99").unwrap());
        for version in ["../0.1.0", "0.1.0/evil", "0.1.0+ambiguous", "latest"] {
            assert!(parse_version(version).is_err());
        }
    }

    #[test]
    fn switch_failure_preserves_versions_and_explains_recovery() {
        let (_home, install) = managed();
        let stage = tempfile::tempdir_in(&install.root).unwrap();
        let content = stage.path().join("content");
        fs::create_dir(&content).unwrap();
        fs::write(content.join("jcode-desktop"), "new desktop").unwrap();
        let error = activate_with(
            &install,
            &content,
            &Version::parse("0.1.0").unwrap(),
            |_, _| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected switch failure",
                ))
            },
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("move the new version directory aside before retrying")
        );
        assert_eq!(
            fs::canonicalize(&install.launcher).unwrap(),
            install.executable
        );
        assert_eq!(
            fs::read_to_string(&install.executable).unwrap(),
            "old desktop"
        );
        assert_eq!(
            fs::read_to_string(install.root.join("0.1.0/jcode-desktop")).unwrap(),
            "new desktop"
        );
    }

    #[test]
    fn network_errors_timeouts_and_size_limits_are_bounded() {
        fn serve(
            response: &'static [u8],
            delay: Duration,
        ) -> (String, std::thread::JoinHandle<()>) {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/release", listener.local_addr().unwrap());
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
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
            (
                &b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n"[..],
                Duration::ZERO,
                100,
            ),
            (
                &b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ntest"[..],
                Duration::from_millis(250),
                100,
            ),
            (
                &b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ntest"[..],
                Duration::ZERO,
                3,
            ),
            (
                &b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\ntest"[..],
                Duration::ZERO,
                3,
            ),
            (
                &b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\nshort"[..],
                Duration::ZERO,
                100,
            ),
        ] {
            let (url, worker) = serve(response, delay);
            assert!(download(&client, &url, &mut Vec::new(), limit).is_err());
            worker.join().unwrap();
        }
        let (url, worker) = serve(
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ntest",
            Duration::ZERO,
        );
        let mut bytes = Vec::new();
        download(&client, &url, &mut bytes, 4).unwrap();
        worker.join().unwrap();
        assert_eq!(bytes, b"test");
    }
}
