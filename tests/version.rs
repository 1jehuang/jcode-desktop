//! Version inspection must not open a window or contact an existing Desktop.
#[test]
fn version_flags_work_without_a_display_or_runtime() {
    let scratch = tempfile::tempdir().unwrap();
    let expected = format!("Jcode Desktop {}\n", jcode_desktop_ui::build_version());
    for flag in ["--version", "-V"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_jcode-desktop"))
            .env_clear()
            .env("HOME", scratch.path())
            .env("XDG_RUNTIME_DIR", scratch.path())
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert!(output.stderr.is_empty());
        assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
    }
}
