//! Per-user versioned installs under `~/.local/opt/jcode-desktop` (Linux).
//!
//! Each release is a complete directory, published with one atomic rename, and
//! `~/.local/bin/jcode-desktop` is switched to it with another. Copies that
//! cannot update themselves (a .deb in /usr/bin, a root-owned extraction) are
//! adopted: the first update installs a managed copy, and launching the system
//! copy redirects to the newest managed version while it is newer.
use anyhow::{Context, Result, ensure};
use semver::Version;
use std::{
    ffi::CString,
    fs::{self, File},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
};

use crate::{InstallKind, release::parse_version};

const EXECUTABLE: &str = "jcode-desktop";
/// Versions kept beside the newest, for windows still running an older build.
const KEEP_OLDER: usize = 2;

pub(crate) struct Managed {
    pub root: PathBuf,
    launcher: PathBuf,
    kind: InstallKind,
}

fn root_of(home: &Path) -> PathBuf {
    home.join(".local/opt/jcode-desktop")
}

fn launcher_of(home: &Path) -> PathBuf {
    home.join(".local/bin/jcode-desktop")
}

/// Owned by this user and not writable by anyone else. Group write is allowed
/// only for this user's own group, the default with per-user private groups.
fn secure_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Inspecting managed directory {}", path.display()))?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Managed directory must not be a symlink: {}",
        path.display()
    );
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    let mode = metadata.mode();
    ensure!(
        metadata.uid() == uid
            && mode & 0o002 == 0
            && (mode & 0o020 == 0 || metadata.gid() == gid),
        "Managed directory must be owned by this user and not writable by others: {}",
        path.display()
    );
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path).with_context(|| format!("Creating {}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    secure_directory(path)
}

/// Every complete managed version, newest first.
fn versions(root: &Path) -> Vec<Version> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<Version> = entries
        .flatten()
        .filter_map(|entry| {
            let version = parse_version(entry.file_name().to_str()?).ok()?;
            let executable = entry.path().join(EXECUTABLE);
            let metadata = fs::symlink_metadata(&executable).ok()?;
            (metadata.is_file() && metadata.mode() & 0o111 != 0).then_some(version)
        })
        .collect();
    versions.sort_by(|a, b| b.cmp_precedence(a));
    versions
}

impl Managed {
    /// The managed install this executable runs from, if any.
    pub fn detect(home: &Path, executable: &Path) -> Result<Option<Self>> {
        let root = root_of(home);
        let (Ok(root_real), Ok(executable)) = (fs::canonicalize(&root), fs::canonicalize(executable))
        else {
            return Ok(None);
        };
        let Ok(relative) = executable.strip_prefix(&root_real) else {
            return Ok(None);
        };
        ensure!(
            root_real == root,
            "The managed install directory must not be reached through a symlink"
        );
        let components: Vec<_> = relative.components().collect();
        ensure!(
            components.len() == 2 && components[1].as_os_str() == EXECUTABLE,
            "Executable is not in a managed version directory"
        );
        parse_version(
            components[0]
                .as_os_str()
                .to_str()
                .context("Non-UTF8 version directory")?,
        )?;
        for directory in [Path::new(""), Path::new(".local"), Path::new(".local/opt")] {
            secure_directory(&home.join(directory))?;
        }
        secure_directory(&root)?;
        Ok(Some(Self {
            root,
            launcher: launcher_of(home),
            kind: InstallKind::Managed,
        }))
    }

    /// Prepare a managed install for a copy that cannot update in place.
    pub fn adopt(home: &Path) -> Result<Self> {
        let home = fs::canonicalize(home).context("Resolving home directory")?;
        secure_directory(&home)?;
        let root = root_of(&home);
        ensure_directory(&home.join(".local"))?;
        ensure_directory(&home.join(".local/opt"))?;
        ensure_directory(&root)?;
        Ok(Self {
            root,
            launcher: launcher_of(&home),
            kind: InstallKind::System,
        })
    }

    pub fn kind(&self) -> InstallKind {
        self.kind
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join(".update.lock")
    }

    pub fn installed_version(&self, running: &Version) -> Result<Version> {
        Ok(match versions(&self.root).into_iter().next() {
            Some(newest) if crate::is_newer(&newest, running) => newest,
            _ => running.clone(),
        })
    }

    pub fn relaunch_executable(&self) -> Result<PathBuf> {
        let newest = versions(&self.root)
            .into_iter()
            .next()
            .context("No managed desktop version is installed")?;
        Ok(self.root.join(newest.to_string()).join(EXECUTABLE))
    }

    /// The launcher is ours to switch when it is absent or already managed.
    fn owns_launcher(&self) -> bool {
        match fs::symlink_metadata(&self.launcher) {
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
            Ok(metadata) => {
                metadata.file_type().is_symlink()
                    && fs::read_link(&self.launcher).is_ok_and(|target| {
                        let target = self
                            .launcher
                            .parent()
                            .map(|parent| parent.join(&target))
                            .unwrap_or(target);
                        // Compare lexically: the target may already be pruned.
                        normalize(&target).starts_with(&self.root)
                    })
            }
        }
    }

    pub fn activate(&self, staged: &Path, version: &Version) -> Result<()> {
        secure_directory(&self.root)?;
        fs::set_permissions(staged, fs::Permissions::from_mode(0o755))?;
        let destination = self.root.join(version.to_string());
        let source = CString::new(staged.as_os_str().as_bytes())?;
        let target = CString::new(destination.as_os_str().as_bytes())?;
        // Unlike rename(), RENAME_NOREPLACE never overwrites an existing
        // version, even when another process creates it concurrently.
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "Could not install {} without replacing existing files",
                    destination.display()
                )
            });
        }
        File::open(&self.root)?.sync_all()?;
        if self.owns_launcher() {
            // A launcher is a convenience. The version directory is complete
            // and redirects already find it, so a failure here is not fatal.
            if let Err(error) = self.switch_launcher(&destination) {
                eprintln!("jcode-desktop update: could not update the launcher: {error:#}");
            }
        }
        Ok(())
    }

    fn switch_launcher(&self, destination: &Path) -> Result<()> {
        let bin = self.launcher.parent().context("Launcher has no directory")?;
        ensure_directory(bin)?;
        let switch = tempfile::Builder::new()
            .prefix(".jcode-desktop-switch-")
            .tempdir_in(bin)?;
        let link = switch.path().join("launcher");
        symlink(destination.join(EXECUTABLE), &link)?;
        fs::rename(&link, &self.launcher)?;
        File::open(bin)?.sync_all()?;
        Ok(())
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

/// The newest managed build when it is newer than the one running.
pub(crate) fn redirect_target(home: &Path, executable: &Path, running: &Version) -> Option<PathBuf> {
    let root = root_of(home);
    let newest = versions(&root).into_iter().next()?;
    if !crate::is_newer(&newest, running) {
        return None;
    }
    let target = root.join(newest.to_string()).join(EXECUTABLE);
    // Never redirect to yourself, whatever the version files claim.
    let same = fs::canonicalize(&target).ok()? == fs::canonicalize(executable).ok()?;
    (!same && secure_directory(&root).is_ok()).then_some(target)
}

/// Remove managed versions nothing should run any more.
pub(crate) fn prune(home: &Path, running: &Version) {
    let root = root_of(home);
    if secure_directory(&root).is_err() {
        return;
    }
    let launcher = fs::canonicalize(launcher_of(home)).ok();
    for version in versions(&root).into_iter().skip(1 + KEEP_OLDER) {
        let directory = root.join(version.to_string());
        let in_use = version == *running
            || launcher
                .as_ref()
                .is_some_and(|launcher| launcher.starts_with(&directory));
        if !in_use {
            let _ = fs::remove_dir_all(directory);
        }
    }
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            if entry.file_name().to_str().is_some_and(|name| name.starts_with(".jcode-update-")) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Target,
        tests::{environment, fixture_fetch},
    };

    fn target() -> Target {
        Target::for_platform("linux", "x86_64").unwrap()
    }

    fn home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        home
    }

    fn install_version(home: &Path, version: &str, contents: &str) -> PathBuf {
        let directory = root_of(home).join(version);
        fs::create_dir_all(&directory).unwrap();
        for directory in [".local", ".local/opt", ".local/opt/jcode-desktop", ".local/bin"] {
            let path = home.join(directory);
            fs::create_dir_all(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let executable = directory.join(EXECUTABLE);
        fs::write(&executable, contents).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        executable
    }

    fn update(
        home: &Path,
        executable: &Path,
        running: &str,
        latest: &str,
    ) -> Result<Option<crate::Outcome>, crate::Failure> {
        let latest = Version::parse(latest).unwrap();
        crate::install_with(
            &environment(target(), executable.to_path_buf(), Some(home.to_path_buf())),
            &Version::parse(running).unwrap(),
            &latest,
            &fixture_fetch(target(), &latest, b"new desktop"),
            &mut |_| {},
        )
    }

    #[test]
    fn managed_install_adds_a_version_and_switches_the_launcher() {
        let home = home();
        let executable = install_version(home.path(), "0.5.0", "old desktop");
        let launcher = launcher_of(home.path());
        symlink("../opt/jcode-desktop/0.5.0/jcode-desktop", &launcher).unwrap();

        let outcome = update(home.path(), &executable, "0.5.0", "0.6.0").unwrap().unwrap();
        let new = root_of(home.path()).join("0.6.0/jcode-desktop");
        assert_eq!(
            outcome,
            crate::Outcome::Installed {
                version: Version::parse("0.6.0").unwrap(),
                relaunch: new.clone(),
                kind: InstallKind::Managed,
                fresh: true,
            }
        );
        assert_eq!(fs::read_link(&launcher).unwrap(), new);
        assert_eq!(fs::read(&new).unwrap(), b"new desktop");
        assert_eq!(fs::read(&executable).unwrap(), b"old desktop");
        assert_eq!(fs::metadata(&new).unwrap().mode() & 0o777, 0o755);

        // A second request from the same old process does not download again.
        let again = update(home.path(), &executable, "0.5.0", "0.6.0").unwrap().unwrap();
        assert!(matches!(again, crate::Outcome::Installed { fresh: false, .. }));
        assert_eq!(redirect_target(home.path(), &executable, &Version::parse("0.5.0").unwrap()), Some(new.clone()));
        assert_eq!(redirect_target(home.path(), &new, &Version::parse("0.6.0").unwrap()), None);
    }

    #[test]
    fn a_system_install_is_adopted_and_launches_redirect_to_it() {
        let home = home();
        let system = tempfile::tempdir().unwrap();
        let executable = system.path().join("jcode-desktop");
        fs::write(&executable, "system desktop").unwrap();

        let outcome = update(home.path(), &executable, "0.5.0", "0.6.0").unwrap().unwrap();
        let managed = root_of(home.path()).join("0.6.0/jcode-desktop");
        assert!(matches!(
            &outcome,
            crate::Outcome::Installed { kind: InstallKind::System, relaunch, .. } if *relaunch == managed
        ));
        assert_eq!(fs::read(&executable).unwrap(), b"system desktop");
        assert_eq!(fs::read_link(launcher_of(home.path())).unwrap(), managed);
        let running = Version::parse("0.5.0").unwrap();
        assert_eq!(redirect_target(home.path(), &executable, &running), Some(managed));
        // A newer system package wins over an older managed copy.
        assert_eq!(redirect_target(home.path(), &executable, &Version::parse("0.7.0").unwrap()), None);
    }

    #[test]
    fn a_foreign_launcher_is_left_alone() {
        let home = home();
        let system = tempfile::tempdir().unwrap();
        let executable = system.path().join("jcode-desktop");
        fs::write(&executable, "system desktop").unwrap();
        install_version(home.path(), "0.1.0", "unused");
        let launcher = launcher_of(home.path());
        fs::write(&launcher, "#!/bin/sh\nexec my-own-wrapper\n").unwrap();
        update(home.path(), &executable, "0.5.0", "0.6.0").unwrap().unwrap();
        assert_eq!(fs::read_to_string(&launcher).unwrap(), "#!/bin/sh\nexec my-own-wrapper\n");
    }

    #[test]
    fn unsafe_directories_are_refused() {
        let home = home();
        let executable = install_version(home.path(), "0.5.0", "old");
        fs::set_permissions(root_of(home.path()), fs::Permissions::from_mode(0o777)).unwrap();
        let failure = update(home.path(), &executable, "0.5.0", "0.6.0").unwrap_err();
        assert_eq!(failure.stage, "detect");
        fs::set_permissions(root_of(home.path()), fs::Permissions::from_mode(0o755)).unwrap();
        let moved = home.path().join("elsewhere");
        fs::rename(root_of(home.path()), &moved).unwrap();
        symlink(&moved, root_of(home.path())).unwrap();
        let executable = root_of(home.path()).join("0.5.0/jcode-desktop");
        assert!(update(home.path(), &executable, "0.5.0", "0.6.0").is_err());
    }

    #[test]
    fn private_group_write_is_accepted_but_world_write_is_not() {
        let home = home();
        let directory = home.path().join("dir");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o775)).unwrap();
        // Our own group: what umask 002 with per-user groups produces.
        assert!(secure_directory(&directory).is_ok());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o757)).unwrap();
        assert!(secure_directory(&directory).is_err());
    }

    #[test]
    fn pruning_keeps_the_running_launcher_and_recent_versions() {
        let home = home();
        for version in ["0.1.0", "0.2.0", "0.3.0", "0.4.0", "0.5.0"] {
            install_version(home.path(), version, version);
        }
        symlink(
            root_of(home.path()).join("0.1.0/jcode-desktop"),
            launcher_of(home.path()),
        )
        .unwrap();
        prune(home.path(), &Version::parse("0.2.0").unwrap());
        let kept: Vec<_> = versions(&root_of(home.path()))
            .into_iter()
            .map(|version| version.to_string())
            .collect();
        assert_eq!(kept, ["0.5.0", "0.4.0", "0.3.0", "0.2.0", "0.1.0"]);
        fs::remove_file(launcher_of(home.path())).unwrap();
        prune(home.path(), &Version::parse("0.5.0").unwrap());
        let kept: Vec<_> = versions(&root_of(home.path()))
            .into_iter()
            .map(|version| version.to_string())
            .collect();
        assert_eq!(kept, ["0.5.0", "0.4.0", "0.3.0"]);
    }
}
