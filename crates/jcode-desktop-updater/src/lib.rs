//! Jcode Desktop's self-updater for Linux, FreeBSD and Windows.
//!
//! macOS updates through Sparkle. Every other packaged build updates through
//! [`update`], which the background updater, `/update`, and the headless
//! `jcode-desktop --update` share, so CI exercises exactly what users run.
//!
//! Three installation shapes are supported:
//! - Portable: an extracted tarball or Windows zip in a user-writable
//!   directory. Files are swapped in place, the running executable included
//!   (both Linux and NTFS allow renaming a running image).
//! - Managed (Linux): versioned bundles under `~/.local/opt/jcode-desktop`
//!   with an atomically switched `~/.local/bin/jcode-desktop` symlink.
//! - System (Linux .deb or root-owned directory): the first update adopts a
//!   managed per-user install, and launches of the system copy redirect to it
//!   while it is newer. A later system package upgrade that is newer wins.
use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub mod archive;
#[cfg(target_os = "linux")]
mod managed;
mod portable;
pub mod release;

pub use release::{Target, is_newer, latest, parse_version};

/// How this copy of Jcode Desktop was installed, as a telemetry-safe token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallKind {
    Portable,
    Managed,
    /// A system package adopted into a managed per-user install.
    System,
}

impl InstallKind {
    pub fn as_str(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Self::Portable, true) => "windows_portable",
            (Self::Portable, false) => "portable",
            (Self::Managed, _) => "linux_managed",
            (Self::System, _) => "linux_system",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The running build is the newest published release.
    UpToDate { version: Version },
    /// A newer release is installed on disk. `relaunch` starts it.
    Installed {
        version: Version,
        relaunch: PathBuf,
        kind: InstallKind,
        /// Whether this call installed it, or an earlier one already had.
        fresh: bool,
    },
}

/// What a failure interrupted, as a short telemetry-safe token.
#[derive(Debug)]
pub struct Failure {
    pub stage: &'static str,
    pub kind: Option<InstallKind>,
    pub error: anyhow::Error,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.error)
    }
}

trait Stage<T> {
    fn stage(self, stage: &'static str, kind: Option<InstallKind>) -> Result<T, Failure>;
}

impl<T> Stage<T> for Result<T> {
    fn stage(self, stage: &'static str, kind: Option<InstallKind>) -> Result<T, Failure> {
        self.map_err(|error| Failure { stage, kind, error })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Progress {
    Downloading { version: Version },
}

/// A build run from a Cargo checkout, which must never update or redirect.
///
/// Only Cargo's own output layout counts: `target/<profile>/<binary>` or
/// `target/<triple>/<profile>/<binary>`. An install that merely lives somewhere
/// under a checkout's `target` (acceptance tests stage them there) is not one.
pub fn is_source_build(executable: &Path) -> bool {
    let components: Vec<_> = executable.iter().collect();
    let Some(binary) = components.last().and_then(|name| name.to_str()) else {
        return false;
    };
    if binary != "jcode-desktop" && binary != "jcode-desktop.exe" {
        return false;
    }
    for depth in [2, 3] {
        let Some(target_index) = components.len().checked_sub(depth + 1) else {
            continue;
        };
        let profile = components[components.len() - 2];
        if components[target_index] == "target"
            && (profile == "debug" || profile == "release")
            && components[..target_index]
                .iter()
                .collect::<PathBuf>()
                .join("Cargo.toml")
                .is_file()
        {
            return true;
        }
    }
    false
}

fn current_executable() -> Result<PathBuf> {
    let executable = std::env::current_exe().context("Locating the running desktop")?;
    // A Linux image replaced in place reports "<path> (deleted)".
    if !executable.exists()
        && let Some(path) = executable
            .to_str()
            .and_then(|path| path.strip_suffix(" (deleted)"))
    {
        return Ok(PathBuf::from(path));
    }
    Ok(executable)
}

/// Where this platform's installs live, and how to reach the internet.
pub(crate) struct Environment {
    pub target: Target,
    pub executable: PathBuf,
    pub home: Option<PathBuf>,
    /// Run each staged executable with `--version` before activating it.
    pub smoke_test: bool,
}

impl Environment {
    fn current() -> Result<Self> {
        Ok(Self {
            target: Target::current()?,
            executable: current_executable()?,
            home: std::env::var_os("HOME").map(PathBuf::from),
            smoke_test: true,
        })
    }
}

/// Download, verify, and install the newest release next to this one.
///
/// The running process and its files keep working. The new build takes effect
/// at the next launch, or through a restart onto the returned executable.
pub fn update(running: &Version, progress: &mut dyn FnMut(Progress)) -> Result<Outcome, Failure> {
    let environment = Environment::current().stage("platform", None)?;
    ensure_not_source(&environment.executable).stage("source_build", None)?;
    let latest = latest().stage("check", None)?;
    let client = release::client("Jcode-Desktop-Updater", Duration::from_secs(600))
        .stage("platform", None)?;
    let base = format!("{}/desktop-v{latest}", release::PUBLIC_RELEASES);
    let fetch = |asset: &str, writer: &mut dyn std::io::Write| -> Result<()> {
        let limit = if asset.starts_with("SHA256SUMS") {
            64 * 1024
        } else {
            archive::MAX_ARCHIVE
        };
        archive::download(&client, &format!("{base}/{asset}"), &mut WriteRef(writer), limit)
    };
    let outcome = install_with(&environment, running, &latest, &fetch, &mut |version| {
        progress(Progress::Downloading {
            version: version.clone(),
        })
    })?;
    Ok(outcome.unwrap_or(Outcome::UpToDate { version: latest }))
}

fn ensure_not_source(executable: &Path) -> Result<()> {
    ensure!(
        !is_source_build(executable),
        "This is a source build. Rebuild the checkout instead of updating it"
    );
    Ok(())
}

/// A function that writes the published bytes for `asset` of a release. The
/// real implementation downloads from the public channel. Tests serve fixtures.
pub(crate) type Fetch<'a> = dyn Fn(&str, &mut dyn std::io::Write) -> Result<()> + 'a;

struct WriteRef<'a>(&'a mut dyn std::io::Write);

impl std::io::Write for WriteRef<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// The platform-independent update transaction. `None` means up to date.
pub(crate) fn install_with(
    environment: &Environment,
    running: &Version,
    latest: &Version,
    fetch: &Fetch<'_>,
    downloading: &mut dyn FnMut(&Version),
) -> Result<Option<Outcome>, Failure> {
    let install = Install::detect(environment).stage("detect", None)?;
    let kind = Some(install.kind());
    let _lock = install.lock().stage("lock", kind)?;
    // Another process may have finished this update while we waited.
    let installed = install.installed_version(running).stage("detect", kind)?;
    if !is_newer(latest, &installed) {
        return Ok(if is_newer(&installed, running) {
            Some(Outcome::Installed {
                version: installed,
                relaunch: install.relaunch_executable(environment).stage("detect", kind)?,
                kind: install.kind(),
                fresh: false,
            })
        } else {
            None
        });
    }
    downloading(latest);
    let staging = install.staging_parent();
    fs::create_dir_all(&staging)
        .context("Creating the update staging directory")
        .stage("stage", kind)?;
    let stage = tempfile::Builder::new()
        .prefix(".jcode-update-")
        .tempdir_in(&staging)
        .context("The install directory is not writable")
        .stage("stage", kind)?;
    let (archive_root, asset) = environment.target.archive_names(latest);
    let mut sums = Vec::new();
    fetch(environment.target.checksum_manifest, &mut sums).stage("download", kind)?;
    let expected = archive::checksum(
        std::str::from_utf8(&sums)
            .context("Non-UTF8 checksum manifest")
            .stage("verify", kind)?,
        &asset,
    )
    .stage("verify", kind)?;
    let mut file = tempfile::tempfile_in(stage.path())
        .context("Creating the download file")
        .stage("stage", kind)?;
    fetch(&asset, &mut file).stage("download", kind)?;
    archive::verify(&mut file, &expected).stage("verify", kind)?;
    let content = stage.path().join("content");
    fs::create_dir(&content).context("Creating staged bundle").stage("stage", kind)?;
    archive::extract(&environment.target, file, &content, &archive_root).stage("extract", kind)?;
    if environment.smoke_test {
        smoke_test(&content.join(environment.target.executable()), latest)
            .stage("smoke_test", kind)?;
    }
    install
        .activate(environment, &content, latest)
        .stage("activate", kind)?;
    Ok(Some(Outcome::Installed {
        version: latest.clone(),
        relaunch: install.relaunch_executable(environment).stage("activate", kind)?,
        kind: install.kind(),
        fresh: true,
    }))
}

/// Refuse a bundle whose desktop cannot even report its own version.
fn smoke_test(executable: &Path, version: &Version) -> Result<()> {
    let mut child = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("The downloaded desktop could not start")?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            bail!("The downloaded desktop did not report its version within 60s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    ensure!(
        output.status.success() && text.contains(&version.to_string()),
        "The downloaded desktop reported an unexpected version: {}",
        text.trim()
    );
    Ok(())
}

pub(crate) enum Install {
    Portable(portable::Portable),
    #[cfg(target_os = "linux")]
    Managed(managed::Managed),
}

impl Install {
    fn detect(environment: &Environment) -> Result<Self> {
        #[cfg(target_os = "linux")]
        if let Some(home) = &environment.home
            && let Some(managed) = managed::Managed::detect(home, &environment.executable)?
        {
            return Ok(Self::Managed(managed));
        }
        match portable::Portable::detect(&environment.target, &environment.executable) {
            Ok(portable) => Ok(Self::Portable(portable)),
            #[cfg(target_os = "linux")]
            Err(error) => {
                let Some(home) = &environment.home else {
                    return Err(error);
                };
                Ok(Self::Managed(managed::Managed::adopt(home)?))
            }
            #[cfg(not(target_os = "linux"))]
            Err(error) => Err(error),
        }
    }

    fn kind(&self) -> InstallKind {
        match self {
            Self::Portable(_) => InstallKind::Portable,
            #[cfg(target_os = "linux")]
            Self::Managed(managed) => managed.kind(),
        }
    }

    fn lock(&self) -> Result<File> {
        let path = match self {
            Self::Portable(portable) => portable.lock_path(),
            #[cfg(target_os = "linux")]
            Self::Managed(managed) => managed.lock_path(),
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .context("Opening the update lock")?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(250))
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    bail!("Another desktop update is still in progress")
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }

    fn installed_version(&self, running: &Version) -> Result<Version> {
        match self {
            Self::Portable(portable) => portable.installed_version(running),
            #[cfg(target_os = "linux")]
            Self::Managed(managed) => managed.installed_version(running),
        }
    }

    fn staging_parent(&self) -> PathBuf {
        match self {
            Self::Portable(portable) => portable.dir.clone(),
            #[cfg(target_os = "linux")]
            Self::Managed(managed) => managed.root.clone(),
        }
    }

    fn activate(&self, environment: &Environment, staged: &Path, version: &Version) -> Result<()> {
        match self {
            Self::Portable(portable) => portable.activate(&environment.target, staged, version),
            #[cfg(target_os = "linux")]
            Self::Managed(managed) => managed.activate(staged, version),
        }
    }

    fn relaunch_executable(&self, environment: &Environment) -> Result<PathBuf> {
        match self {
            Self::Portable(portable) => Ok(portable.dir.join(environment.target.executable())),
            #[cfg(target_os = "linux")]
            Self::Managed(managed) => managed.relaunch_executable(),
        }
    }
}

/// The newer installed build a launch of this copy should run instead.
///
/// Only a managed per-user install can be newer than the running executable
/// without replacing it: a system package adopted by an earlier update, or a
/// stale shortcut naming an older managed version directory.
pub fn redirect_target(running: &Version) -> Option<PathBuf> {
    if std::env::var_os("JCODE_DESKTOP_NO_REDIRECT").is_some() {
        return None;
    }
    let executable = current_executable().ok()?;
    if is_source_build(&executable) {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        managed::redirect_target(&home, &executable, running)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = running;
        None
    }
}

/// Remove files an earlier update set aside. Best effort and quick.
pub fn cleanup_after_update(running: &Version) {
    let Ok(environment) = Environment::current() else {
        return;
    };
    if is_source_build(&environment.executable) {
        return;
    }
    if let Ok(portable) = portable::Portable::detect(&environment.target, &environment.executable) {
        portable.cleanup();
    }
    #[cfg(target_os = "linux")]
    if let Some(home) = &environment.home {
        managed::prune(home, running);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = running;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn environment(target: Target, executable: PathBuf, home: Option<PathBuf>) -> Environment {
        Environment {
            target,
            executable,
            home,
            smoke_test: false,
        }
    }

    /// Serve a release built from fixture files through the real transaction.
    pub(crate) fn fixture_fetch(
        target: Target,
        version: &Version,
        contents: &'static [u8],
    ) -> impl Fn(&str, &mut dyn std::io::Write) -> Result<()> {
        use sha2::Digest;
        let (root, asset) = target.archive_names(version);
        let names = archive::files(&target);
        let bytes = if target.zip {
            let entries: Vec<(String, &[u8])> = names
                .iter()
                .map(|name| (format!("{root}/{name}"), contents))
                .collect();
            let borrowed: Vec<(&str, &[u8])> =
                entries.iter().map(|(name, bytes)| (name.as_str(), *bytes)).collect();
            let mut file = archive::tests::zipfile(&borrowed);
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
            bytes
        } else {
            let encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            builder.append_data(&mut header, format!("{root}/"), &[][..]).unwrap();
            for name in names {
                let mut header = tar::Header::new_gnu();
                header.set_mode(0o755);
                header.set_size(contents.len() as u64);
                builder
                    .append_data(&mut header, format!("{root}/{name}"), contents)
                    .unwrap();
            }
            builder.into_inner().unwrap().finish().unwrap()
        };
        let sums = format!("{:x}  {asset}\n", sha2::Sha256::digest(&bytes));
        let manifest = target.checksum_manifest;
        move |requested: &str, writer: &mut dyn std::io::Write| {
            if requested == manifest {
                writer.write_all(sums.as_bytes())?;
            } else {
                ensure!(requested == asset, "unexpected asset {requested}");
                writer.write_all(&bytes)?;
            }
            Ok(())
        }
    }

    #[test]
    fn source_checkouts_are_detected_by_their_cargo_target() {
        let checkout = tempfile::tempdir().unwrap();
        fs::write(checkout.path().join("Cargo.toml"), "").unwrap();
        let binary = checkout.path().join("target/release/jcode-desktop");
        assert!(is_source_build(&binary));
        assert!(is_source_build(
            &checkout.path().join("target/x86_64-pc-windows-msvc/debug/jcode-desktop.exe")
        ));
        assert!(!is_source_build(&checkout.path().join("dist/jcode-desktop")));
        assert!(!is_source_build(Path::new("/usr/bin/jcode-desktop")));
        // A managed install staged by an acceptance test under target/.
        assert!(!is_source_build(&checkout.path().join(
            "target/ui-review/home/.local/opt/jcode-desktop/0.5.0/jcode-desktop"
        )));
        assert!(!is_source_build(&checkout.path().join("target/release/jcode")));
    }
}
