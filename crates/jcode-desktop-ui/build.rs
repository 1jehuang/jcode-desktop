use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs, path::Path, process::Command};

fn main() {
    // The footer is also the hot-reload generation indicator. Re-run this
    // script for every UI source change so a rebuilt cdylib carries a fresh
    // timestamp instead of inheriting the first build's metadata.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=JCODE_DESKTOP_VERSION");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-env-changed=JCODE_DESKTOP_BUILD_EPOCH");

    let version = env::var("JCODE_DESKTOP_VERSION").unwrap_or_else(|_| {
        env::var("CARGO_PKG_VERSION").expect("Cargo package version is missing")
    });
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch");
    let requested_at = env::var("JCODE_DESKTOP_BUILD_EPOCH").ok().map(|millis| {
        millis
            .parse::<u128>()
            .expect("JCODE_DESKTOP_BUILD_EPOCH must be a non-negative Unix timestamp")
    });
    let built_at = env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| {
        requested_at
            .map(|millis| (millis / 1_000).to_string())
            .unwrap_or_else(|| now.as_secs().to_string())
    });
    built_at
        .parse::<u64>()
        .expect("SOURCE_DATE_EPOCH must be a non-negative Unix timestamp");

    println!("cargo:rustc-env=JCODE_DESKTOP_VERSION={version}");
    println!("cargo:rustc-env=JCODE_DESKTOP_BUILT_AT={built_at}");
    // Include sub-second precision so two quick hot reloads still have visibly
    // different identities. Reproducible builds retain a stable identifier.
    let build_id = env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| {
        requested_at
            .map(|millis| millis.to_string())
            .unwrap_or_else(|| now.as_millis().to_string())
    });
    println!("cargo:rustc-env=JCODE_DESKTOP_BUILD_ID={build_id}");
    generate_changelog(&version, &build_id);
}

// Git is consulted only by Cargo, never by the running desktop application.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn generate_changelog(version: &str, build_id: &str) {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").expect("missing manifest directory");
    let root = Path::new(&manifest).join("../..");
    for path in [
        "CHANGELOG.md",
        "src",
        "crates",
        "assets",
        "scripts",
        "Cargo.toml",
        "Cargo.lock",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    // Resolve git paths rather than assuming .git is a directory (worktrees).
    for name in ["HEAD", "index", "packed-refs", "refs", "logs/HEAD"] {
        if let Some(path) = git(&root, &["rev-parse", "--git-path", name]) {
            let path = root.join(path.trim());
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    if let Some(files) = git(&root, &["ls-files", "-z"]) {
        for file in files.split('\0').filter(|file| !file.is_empty()) {
            println!("cargo:rerun-if-changed={}", root.join(file).display());
        }
    }
    let curated = fs::read_to_string(root.join("CHANGELOG.md"))
        .expect("root CHANGELOG.md must be available for offline release notes");
    let mut notes = format!("# Jcode Desktop v{version}\n\nBuild `{build_id}`\n\n{curated}");
    notes.push_str("\n## Recent commits\n\nBuild-time history, not a complete list of changes since your last update.\n\n");
    match git(&root, &["log", "-8", "--format=%s"]) {
        Some(log) if !log.trim().is_empty() => {
            for subject in log.lines() {
                notes.push_str(&format!("- {}\n", escape_markdown(subject)));
            }
        }
        _ => notes
            .push_str("Git history was unavailable for this build. See the curated notes above.\n"),
    }
    let mut debug = String::from(
        "\n## Development build snapshot\n\nUncommitted paths captured at build time, not release guarantees.\n\n### Uncommitted files\n\n",
    );
    match git(
        &root,
        &[
            "-c",
            "core.quotePath=true",
            "status",
            "--short",
            "--untracked-files=normal",
        ],
    ) {
        Some(status) if status.trim().is_empty() => debug.push_str("Working tree was clean.\n"),
        Some(status) => {
            let lines: Vec<_> = status.lines().collect();
            debug.push_str(&format!(
                "{} changed path entries (Git status):\n\n",
                lines.len()
            ));
            for line in lines.iter().take(40) {
                debug.push_str(&format!("- {}\n", escape_markdown(line)));
            }
            if lines.len() > 40 {
                debug.push_str(&format!("- … and {} more entries.\n", lines.len() - 40));
            }
        }
        None => debug.push_str("Working-tree status was unavailable for this build.\n"),
    }
    let out = env::var_os("OUT_DIR").expect("missing output directory");
    // Match the TUI's newest-first, version-boundary history. Keep this
    // structured so the UI can distinguish unseen updates from all releases.
    let history = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(&root)
        .env("TZ", "UTC")
        .args([
            "log",
            "--first-parent",
            "--decorate=short",
            "--decorate-refs=refs/tags/*",
            "-120",
            "--date=format-local:%Y-%m-%d %H:%M UTC",
            "--format=%H%x1e%D%x1e%cd%x1e%s%x1f",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
        .unwrap_or_default();
    fs::write(Path::new(&out).join("update-history.txt"), history)
        .expect("write embedded update history");
    fs::write(Path::new(&out).join("changelog.md"), notes).expect("write embedded changelog");
    fs::write(Path::new(&out).join("changelog-debug.md"), debug).expect("write debug changelog");
}

fn escape_markdown(text: &str) -> String {
    let mut escaped = String::new();
    for ch in text.chars() {
        if "\\`*_{}[]<>()#!|".contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}
