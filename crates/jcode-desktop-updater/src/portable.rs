//! Extracted tarball or Windows zip installs, updated in place.
//!
//! Each published file is swapped by rename. Linux and NTFS both allow
//! renaming an executable that is running, but Windows forbids deleting or
//! overwriting it, so the previous files are set aside under a unique name and
//! removed by a later launch. A failure part way through renames every file
//! back, leaving the previous version intact.
use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    archive,
    release::{Target, parse_version},
};

/// Infix of a file set aside by an update: `<name>.jcode-old-<token>`.
pub(crate) const OLD_INFIX: &str = ".jcode-old-";
const VERSION_FILE: &str = ".jcode-desktop-version";

pub(crate) struct Portable {
    pub dir: PathBuf,
}

/// The published name of a file an update set aside, if `name` is one.
pub(crate) fn original_name(name: &str) -> Option<&str> {
    name.split_once(OLD_INFIX).map(|(original, _)| original)
}

impl Portable {
    pub fn detect(target: &Target, executable: &Path) -> Result<Self> {
        let name = executable
            .file_name()
            .and_then(|name| name.to_str())
            .context("Unexpected desktop executable name")?;
        let name = original_name(name).unwrap_or(name);
        ensure!(
            name.eq_ignore_ascii_case(target.executable()),
            "Not a packaged desktop executable"
        );
        let dir = executable
            .parent()
            .context("The desktop executable has no directory")?
            .to_path_buf();
        // A portable bundle carries every published file. A system package
        // spreads them across /usr/bin and /usr/share instead.
        for name in archive::files(target) {
            ensure!(
                dir.join(name).is_file(),
                "Not an extracted desktop package (missing {name})"
            );
        }
        ensure!(
            writable(&dir),
            "Jcode Desktop's folder {} is not writable. Move the Jcode folder somewhere you own, then update again",
            dir.display()
        );
        Ok(Self { dir })
    }

    pub fn lock_path(&self) -> PathBuf {
        self.dir.join(".jcode-update.lock")
    }

    /// What is on disk now, which an earlier update may have made newer than
    /// the running process.
    pub fn installed_version(&self, running: &Version) -> Result<Version> {
        let recorded = fs::read_to_string(self.dir.join(VERSION_FILE))
            .ok()
            .and_then(|text| parse_version(text.trim()).ok());
        Ok(match recorded {
            Some(recorded) if crate::is_newer(&recorded, running) => recorded,
            _ => running.clone(),
        })
    }

    pub fn activate(&self, target: &Target, staged: &Path, version: &Version) -> Result<()> {
        let token = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut placed: Vec<PathBuf> = Vec::new();
        let result = (|| -> Result<()> {
            for name in archive::files(target) {
                let live = self.dir.join(name);
                let aside = self.dir.join(format!("{name}{OLD_INFIX}{token}"));
                fs::rename(&live, &aside)
                    .with_context(|| format!("Setting aside {}", live.display()))?;
                moved.push((aside, live.clone()));
                fs::rename(staged.join(name), &live)
                    .with_context(|| format!("Installing {}", live.display()))?;
                placed.push(live);
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Put the previous version back exactly as it was.
            let mut restored = true;
            for live in placed.iter().rev() {
                let name = live.file_name().unwrap_or_default();
                restored &= fs::rename(live, staged.join(name)).is_ok();
            }
            for (aside, live) in moved.iter().rev() {
                restored &= fs::rename(aside, live).is_ok();
            }
            if !restored {
                bail!(
                    "{error:#}. Some files could not be restored. Reinstall Jcode Desktop from https://jcode.sh/desktop"
                );
            }
            return Err(error.context("The previous version was kept"));
        }
        let _ = fs::write(self.dir.join(VERSION_FILE), format!("{version}\n"));
        Ok(())
    }

    /// Delete files set aside by earlier updates. A file still in use by a
    /// process from before the update stays until a later launch.
    pub fn cleanup(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if original_name(name).is_some() {
                let _ = fs::remove_file(entry.path());
            } else if name.starts_with(".jcode-update-") {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

#[cfg(unix)]
fn writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(unix))]
fn writable(dir: &Path) -> bool {
    // Windows ACLs are only reliably answered by trying.
    tempfile::Builder::new()
        .prefix(".jcode-write-test-")
        .tempfile_in(dir)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{environment, fixture_fetch};

    fn bundle(target: Target, contents: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("Jcode");
        fs::create_dir(&dir).unwrap();
        for name in archive::files(&target) {
            fs::write(dir.join(name), contents).unwrap();
        }
        let executable = dir.join(target.executable());
        (root, executable)
    }

    #[test]
    fn swaps_every_file_in_place_and_reports_the_installed_version() {
        for target in [
            Target::for_platform("linux", "x86_64").unwrap(),
            Target::for_platform("windows", "x86_64").unwrap(),
        ] {
            let (_root, executable) = bundle(target, b"old");
            let running = Version::parse("0.5.1").unwrap();
            let latest = Version::parse("0.6.0").unwrap();
            let fetch = fixture_fetch(target, &latest, b"new");
            let mut announced = Vec::new();
            let outcome = crate::install_with(
                &environment(target, executable.clone(), None),
                &running,
                &latest,
                &fetch,
                &mut |version| announced.push(version.clone()),
            )
            .unwrap()
            .unwrap();
            assert_eq!(announced, [latest.clone()]);
            let crate::Outcome::Installed { version, relaunch, kind, fresh } = outcome else {
                panic!("expected an install");
            };
            assert_eq!((version, kind, fresh), (latest.clone(), crate::InstallKind::Portable, true));
            assert_eq!(relaunch, executable);
            let dir = executable.parent().unwrap();
            for name in archive::files(&target) {
                assert_eq!(fs::read(dir.join(name)).unwrap(), b"new", "{name}");
            }
            // The previous files are set aside, not lost, until cleanup.
            let aside = fs::read_dir(dir)
                .unwrap()
                .filter(|entry| {
                    original_name(entry.as_ref().unwrap().file_name().to_str().unwrap()).is_some()
                })
                .count();
            assert_eq!(aside, 5);

            // The same process asking again learns the update is already on disk.
            let again = crate::install_with(
                &environment(target, executable.clone(), None),
                &running,
                &latest,
                &fetch,
                &mut |_| panic!("must not download twice"),
            )
            .unwrap()
            .unwrap();
            assert!(matches!(again, crate::Outcome::Installed { fresh: false, .. }));

            Portable { dir: dir.to_path_buf() }.cleanup();
            assert_eq!(fs::read_dir(dir).unwrap().count(), 7); // 5 files, version, lock
        }
    }

    #[test]
    fn current_release_is_a_no_op() {
        let target = Target::for_platform("linux", "x86_64").unwrap();
        let (_root, executable) = bundle(target, b"same");
        let version = Version::parse("0.5.1").unwrap();
        let outcome = crate::install_with(
            &environment(target, executable.clone(), None),
            &version,
            &version,
            &|_: &str, _: &mut dyn std::io::Write| panic!("no download when current"),
            &mut |_| panic!("no download when current"),
        )
        .unwrap();
        assert_eq!(outcome, None);
        assert_eq!(fs::read(&executable).unwrap(), b"same");
    }

    #[test]
    fn a_corrupt_download_changes_nothing() {
        let target = Target::for_platform("windows", "x86_64").unwrap();
        let (_root, executable) = bundle(target, b"old");
        let latest = Version::parse("0.6.0").unwrap();
        let good = fixture_fetch(target, &latest, b"new");
        let corrupt = |asset: &str, writer: &mut dyn std::io::Write| -> Result<()> {
            if asset.starts_with("SHA256SUMS") {
                good(asset, writer)
            } else {
                writer.write_all(b"not the published archive")?;
                Ok(())
            }
        };
        let failure = crate::install_with(
            &environment(target, executable.clone(), None),
            &Version::parse("0.5.1").unwrap(),
            &latest,
            &corrupt,
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(failure.stage, "verify");
        for name in archive::files(&target) {
            assert_eq!(fs::read(executable.parent().unwrap().join(name)).unwrap(), b"old");
        }
    }

    #[test]
    fn a_failed_swap_restores_the_previous_version() {
        let target = Target::for_platform("linux", "x86_64").unwrap();
        let (_root, executable) = bundle(target, b"old");
        let dir = executable.parent().unwrap();
        let staged = tempfile::tempdir().unwrap();
        // Only some of the new files exist, so the swap fails part way.
        for name in &archive::UNIX_FILES[..2] {
            fs::write(staged.path().join(name), b"new").unwrap();
        }
        let portable = Portable { dir: dir.to_path_buf() };
        let error = portable
            .activate(&target, staged.path(), &Version::parse("0.6.0").unwrap())
            .unwrap_err();
        assert!(format!("{error:#}").contains("previous version was kept"));
        for name in archive::files(&target) {
            assert_eq!(fs::read(dir.join(name)).unwrap(), b"old", "{name}");
        }
        assert_eq!(fs::read_dir(dir).unwrap().count(), 5);
    }

    #[test]
    fn detection_requires_a_complete_writable_bundle() {
        let target = Target::for_platform("linux", "x86_64").unwrap();
        let (_root, executable) = bundle(target, b"x");
        assert!(Portable::detect(&target, &executable).is_ok());
        let renamed = executable.with_file_name(format!("jcode-desktop{OLD_INFIX}123"));
        assert!(Portable::detect(&target, &renamed).is_ok());
        fs::remove_file(executable.with_file_name("jcode.png")).unwrap();
        assert!(Portable::detect(&target, &executable).is_err());
        assert!(Portable::detect(&target, Path::new("/usr/bin/jcode-desktop")).is_err());
    }
}
