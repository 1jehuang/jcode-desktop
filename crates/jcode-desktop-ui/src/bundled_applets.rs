//! Applets that ship inside Desktop. Their sources are embedded at build time
//! and installed into `~/.jcode/applets/<id>` on first use, so built-in
//! surfaces (such as the Super+Shift+G inbox) work without the repository.
//!
//! A `.bundled` marker records the hash of what Desktop wrote. Later builds
//! refresh the copy only while it still matches that hash, so a user's local
//! edits are never overwritten.
use std::path::Path;

struct Bundled {
    id: &'static str,
    files: &'static [(&'static str, &'static str)],
}

const BUNDLED: &[Bundled] = &[Bundled {
    id: "gmail",
    files: &[
        (
            "applet.json",
            include_str!("../../../applets/gmail/applet.json"),
        ),
        (
            "provider.py",
            include_str!("../../../applets/gmail/provider.py"),
        ),
        (
            "README.md",
            include_str!("../../../applets/gmail/README.md"),
        ),
    ],
}];

const MARKER: &str = ".bundled";

fn fnv(bytes: &[u8], mut hash: u64) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn digest(files: impl Iterator<Item = (String, Vec<u8>)>) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for (name, body) in files {
        hash = fnv(name.as_bytes(), hash);
        hash = fnv(&[0], hash);
        hash = fnv(&body, hash);
        hash = fnv(&[0], hash);
    }
    format!("{hash:016x}")
}

fn embedded_digest(bundled: &Bundled) -> String {
    digest(
        bundled
            .files
            .iter()
            .map(|(name, body)| ((*name).to_owned(), body.as_bytes().to_vec())),
    )
}

fn installed_digest(bundled: &Bundled, dir: &Path) -> Option<String> {
    let mut files = Vec::new();
    for (name, _) in bundled.files {
        files.push(((*name).to_owned(), std::fs::read(dir.join(name)).ok()?));
    }
    Some(digest(files.into_iter()))
}

/// What `ensure` did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    NotBundled,
    Installed,
    Updated,
    Current,
    /// The user changed the installed copy. It is left alone.
    UserModified,
}

/// Install or refresh one bundled applet under `root`.
pub(crate) fn ensure(root: &Path, id: &str) -> std::io::Result<Outcome> {
    let Some(bundled) = BUNDLED.iter().find(|b| b.id == id) else {
        return Ok(Outcome::NotBundled);
    };
    let dir = root.join(id);
    let wanted = embedded_digest(bundled);
    let outcome = if !dir.join("applet.json").exists() {
        Outcome::Installed
    } else {
        let current = installed_digest(bundled, &dir);
        if current.as_deref() == Some(wanted.as_str()) {
            // Adopt an identical copy (for example one from install-applet.sh)
            // so later builds can refresh it.
            if !dir.join(MARKER).exists() {
                std::fs::write(dir.join(MARKER), format!("{wanted}\n"))?;
            }
            return Ok(Outcome::Current);
        }
        let marker = std::fs::read_to_string(dir.join(MARKER)).ok();
        if marker.is_none() || marker.map(|m| m.trim().to_owned()) != current {
            return Ok(Outcome::UserModified);
        }
        Outcome::Updated
    };
    std::fs::create_dir_all(&dir)?;
    for (name, body) in bundled.files {
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, dir.join(name))?;
    }
    std::fs::write(dir.join(MARKER), format!("{wanted}\n"))?;
    Ok(outcome)
}

/// Refresh every bundled applet the user already has, without installing new
/// ones. Called on activation so updates reach unmodified copies.
pub(crate) fn refresh_installed(root: &Path) {
    for bundled in BUNDLED {
        if root.join(bundled.id).join(MARKER).exists()
            && let Err(error) = ensure(root, bundled.id)
        {
            eprintln!(
                "jcode desktop: could not update applet {}: {error}",
                bundled.id
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_updates_and_never_clobbers_user_edits() {
        let root = std::env::temp_dir().join(format!("jcode-bundled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(ensure(&root, "nope").unwrap(), Outcome::NotBundled);
        assert_eq!(ensure(&root, "gmail").unwrap(), Outcome::Installed);
        let applets = crate::applet_runtime::discover(&root);
        assert_eq!(applets.len(), 1);
        assert_eq!(applets[0].id, "gmail");
        assert!(applets[0].autostart);
        assert_eq!(ensure(&root, "gmail").unwrap(), Outcome::Current);

        // An older bundled copy (marker matches its contents) is refreshed.
        let provider = root.join("gmail/provider.py");
        std::fs::write(&provider, "# old build\n").unwrap();
        let old = installed_digest(&BUNDLED[0], &root.join("gmail")).unwrap();
        std::fs::write(root.join("gmail").join(MARKER), &old).unwrap();
        assert_eq!(ensure(&root, "gmail").unwrap(), Outcome::Updated);
        assert!(
            std::fs::read_to_string(&provider)
                .unwrap()
                .contains("jcode.applet/1")
        );

        // A user edit makes the marker stale, so the copy is left alone.
        std::fs::write(&provider, "# mine\n").unwrap();
        assert_eq!(ensure(&root, "gmail").unwrap(), Outcome::UserModified);
        refresh_installed(&root);
        assert_eq!(std::fs::read_to_string(&provider).unwrap(), "# mine\n");
        let _ = std::fs::remove_dir_all(&root);
    }
}
