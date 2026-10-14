//! Desktop-native commands. Declare this as a child of `panel`, not at crate root.
//! IO runs on the background executor. Nothing here submits an agent prompt.
use super::*;
use std::path::PathBuf;

type RecordingFlags =
    std::collections::HashMap<gpui::EntityId, std::sync::Arc<std::sync::atomic::AtomicBool>>;
static RECORDINGS: std::sync::OnceLock<std::sync::Mutex<RecordingFlags>> =
    std::sync::OnceLock::new();

fn split_command(content: &str) -> (&str, &str) {
    let content = content.trim();
    content
        .split_once(char::is_whitespace)
        .map_or((content, ""), |(name, args)| (name, args.trim()))
}

fn display_key(command: &str) -> Option<&'static str> {
    Some(match command {
        "/alignment" => "alignment",
        "/compact-notifications" => "compact_notifications",
        "/show-agentgrep-output" => "show_agentgrep_output",
        "/tool-call-details" => "tool_call_details",
        "/thinking-display" | "/thinking" | "/reasoning" => "thinking_display",
        "/debug-visual" => "debug_visual",
        "/screenshot-mode" => "screenshot_mode",
        "/diff" => "diff_display",
        _ => return None,
    })
}

fn setting_choices(key: &str) -> &'static str {
    match key {
        "alignment" => "left|center|right",
        "thinking_display" => "off|full|current",
        "diff_display" => "off|inline|full|pinned|file",
        _ => "on|off|toggle",
    }
}

impl Panel {
    pub(super) fn handle_desktop_command(&mut self, content: &str, cx: &mut Context<Self>) -> bool {
        let (command, args) = split_command(content);
        if let Some(key) = display_key(command) {
            let mut value = args.to_ascii_lowercase();
            if value.is_empty() && key == "diff_display" {
                value = "cycle".into();
            }
            if value.is_empty() && !matches!(key, "alignment" | "thinking_display" | "diff_display")
            {
                value = "toggle".into();
            }
            let result = if value.is_empty() || value == "status" {
                Ok(crate::config::display())
            } else {
                crate::config::set_display(key, &value)
            };
            match result {
                Ok(settings) => {
                    self.items.push(Item::Assistant(format!(
                        "{command}: `{}`. Use `{command} <{}>` to change it.",
                        settings.value(key).unwrap_or_default(),
                        setting_choices(key)
                    )));
                }
                Err(error) => self.items.push(Item::Error(format!(
                    "{error} Usage: `{command} <{}>`.",
                    setting_choices(key)
                ))),
            }
            self.transcript_measurements.dirty = true;
            cx.refresh_windows();
            cx.notify();
            return true;
        }
        match command {
            "/version" if args.is_empty() => self.items.push(Item::Assistant(format!(
                "Jcode Desktop {}\n\nRevision: `{}`\n\n{}\n\nPlatform: `{}/{}`",
                crate::build_info::version(), crate::build_info::revision(), crate::build_info::age(),
                std::env::consts::OS, std::env::consts::ARCH
            ))),
            "/git" | "/diff" => {
                let diff = command == "/diff";
                let staged = args == "--staged" || args == "--cached";
                if !args.is_empty() && !(diff && staged) {
                    self.items.push(Item::Error(format!("Usage: `{command}{}`.", if diff { " [--staged]" } else { "" })));
                } else if crate::harness::remote_host(&self.session_id).is_some() {
                    self.items.push(Item::Error(format!("{command} cannot inspect the remote checkout from this Desktop. Open the session's remote terminal to run `git {}` there.", if diff { "diff" } else { "status" })));
                } else if let Some(directory) = self.working_dir.clone().filter(|dir| !dir.trim().is_empty()) {
                    self.items.push(Item::Assistant(format!("Reading {} in `{directory}`…", if diff { "git diff" } else { "git status" })));
                    cx.spawn(async move |this, cx| {
                        let result = cx.background_executor().spawn(async move { git_read(&directory, diff, staged) }).await;
                        let _ = this.update(cx, |panel, cx| {
                            panel.items.push(match result {
                                Ok(text) => Item::Assistant(text),
                                Err(error) => Item::Error(error),
                            });
                            panel.transcript_measurements.dirty = true;
                            cx.notify();
                        });
                    }).detach();
                } else {
                    self.items.push(Item::Error("This session has no working directory to inspect.".into()));
                }
            }
            "/transcript" if args.is_empty() => {
                if crate::harness::remote_host(&self.session_id).is_some() {
                    self.items.push(Item::Error("The session transcript lives on the remote host, not this Desktop. Use the remote terminal to access its Jcode sessions directory.".into()));
                } else if valid_session_filename(&self.session_id) {
                    if let Some(home) = crate::harness::jcode_home() {
                        self.open_command_file(home.join("sessions").join(format!("{}.json", self.session_id)), "Session transcript", cx);
                    } else {
                        self.items.push(Item::Error("Could not locate the Jcode sessions directory.".into()));
                    }
                } else {
                    self.items.push(Item::Error("The session has not received a persisted session ID yet.".into()));
                }
            }
            "/config" if args.is_empty() || args == "show" => {
                let mut config = crate::config::get().clone();
                config.display = crate::config::display();
                config.appearance.theme = crate::theme::Theme::active_preset().id().into();
                self.items.push(Item::Assistant(format!("Desktop configuration: `{}`\n\n```toml\n{}\n```\n\nUse `/config open` to edit the file. Display commands apply live. Startup-only settings need a Desktop restart.", crate::config::path().display(), toml::to_string_pretty(&config).unwrap_or_default())));
            }
            "/config" if args == "open" => self.open_command_file(crate::config::path(), "Desktop configuration", cx),
            "/log" if args.is_empty() => {
                let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
                eprintln!("[desktop /log marker] unix_ms={timestamp} session={} transcript_items={}", self.session_id, self.items.len());
                let path = jcode_desktop_model::platform_dirs::state_dir()
                    .unwrap_or_else(|| std::env::temp_dir().join("jcode-desktop"))
                    .join("jcode-desktop.log");
                self.items.push(Item::Assistant(format!("Marked the current location in the Desktop log. Marker: `{timestamp}`. Log: `{}`. Use `/log open` to open it.", path.display())));
            }
            "/log" if args == "open" => self.open_command_file(jcode_desktop_model::platform_dirs::state_dir().unwrap_or_else(|| std::env::temp_dir().join("jcode-desktop")).join("jcode-desktop.log"), "Desktop log", cx),
            "/hotkeys" | "/keys" | "/keybindings" if args.is_empty() || args == "refresh" => {
                let keymap = cx.key_bindings();
                let keymap = keymap.borrow();
                let rows = keymap.bindings().map(|binding| {
                    let keys = binding.keystrokes().iter().map(|key| key.to_string()).collect::<Vec<_>>().join(" ");
                    let context = binding.predicate().map(|predicate| predicate.to_string()).unwrap_or_else(|| "Global".into());
                    format!("- `{keys}` → `{}` ({context})", binding.action().name())
                }).collect::<Vec<_>>();
                self.items.push(Item::Assistant(format!("## Desktop keybindings\n\nRegistered bindings in declaration order. Context determines availability, and later bindings take precedence. `/keys refresh` reads this live keymap again. Desktop uses GPUI rather than terminal key protocols. OS-reserved shortcuts may intercept a binding and are not detectable from this keymap.\n\n{}", rows.join("\n"))));
            }
            "/colors" | "/color" => {
                use crate::theme::{Theme, ThemePreset};
                if args.is_empty() || args == "list" {
                    let choices = ThemePreset::ALL.iter().map(|preset| format!("- `{}`: {}", preset.id(), preset.label())).collect::<Vec<_>>().join("\n");
                    self.items.push(Item::Assistant(format!("Current Desktop theme: `{}`.\n\nUse `/colors <theme-id>` to apply and save a theme.\n\n{choices}", Theme::active_preset().id())));
                } else if let Some(preset) = ThemePreset::ALL.iter().copied().find(|preset| preset.id() == args) {
                    match crate::config::persist_theme(preset.id()) {
                        Ok(()) => {
                            Theme::select(preset);
                            self.items.push(Item::Assistant(format!("Desktop theme set to **{}**.", preset.label())));
                            cx.refresh_windows();
                        }
                        Err(error) => self.items.push(Item::Error(format!("Could not save theme: {error}"))),
                    }
                } else {
                    self.items.push(Item::Error("Unknown Desktop theme. Use `/colors list` to see exact theme IDs.".into()));
                }
            }
            "/terminal-setup" if args.is_empty() => self.items.push(Item::Assistant(
                "Desktop uses its built-in Handterm terminal engine, so terminal escape-sequence setup for the TUI is unnecessary. Open a terminal panel using Desktop's terminal shortcut. Scrollback is configured with `terminal.scrollback_lines` in `/config`. No shell configuration was changed.".into()
            )),
            "/screenshot" => self.export_debug_snapshot(args, cx),
            "/record" => self.record_debug_demo(args, cx),
            "/version" | "/transcript" | "/config" | "/log" | "/hotkeys" | "/keys" | "/keybindings" | "/terminal-setup" => self.items.push(Item::Error(format!("Unexpected arguments for `{command}`."))),
            _ => return false,
        }
        self.transcript_measurements.dirty = true;
        cx.notify();
        true
    }

    fn record_debug_demo(&mut self, args: &str, cx: &mut Context<Self>) {
        use std::sync::atomic::Ordering;
        let id = cx.entity().entity_id();
        let recordings = RECORDINGS.get_or_init(Default::default);
        let mut recordings = recordings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let running = recordings
            .get(&id)
            .is_some_and(|flag| flag.load(Ordering::Relaxed));
        if args == "status" {
            self.items.push(Item::Assistant(format!("Desktop debug-demo recording: `{}`. This records JSON state, not video or audio. Use `/record start` or `/record stop`.", if running { "recording" } else { "stopped" })));
            return;
        }
        if args == "stop" || (args.is_empty() && running) {
            if let Some(flag) = recordings.get(&id) {
                flag.store(false, Ordering::Relaxed);
            }
            self.items.push(Item::Assistant(
                if running {
                    "Stopping debug-demo recording and flushing its JSONL file."
                } else {
                    "No debug-demo recording is running in this panel."
                }
                .into(),
            ));
            return;
        }
        let requested_path = args
            .strip_prefix("start ")
            .map(str::trim)
            .filter(|path| !path.is_empty());
        if !args.is_empty() && args != "start" && requested_path.is_none() {
            self.items.push(Item::Error("Usage: `/record [start|stop|status]`. Records a bounded JSON debug-state timeline, not video.".into()));
            return;
        }
        if running {
            self.items.push(Item::Assistant(
                "A debug-demo recording is already running in this panel. Use `/record stop`."
                    .into(),
            ));
            return;
        }
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        recordings.insert(id, flag.clone());
        drop(recordings);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let default_path = jcode_desktop_model::platform_dirs::state_dir()
            .unwrap_or_else(|| std::env::temp_dir().join("jcode-desktop"))
            .join("captures")
            .join(format!("desktop-demo-{timestamp}.jsonl"));
        let path = requested_path.map(PathBuf::from).unwrap_or(default_path);
        self.items.push(Item::Assistant(format!("Recording a Desktop debug-state demo to `{}`. This is JSONL, not video or audio, and includes transcript and draft content. Automatically stops after 60 seconds or 8 MiB. Use `/record stop`, and review the file before sharing.", path.display())));
        cx.spawn(async move |this, cx| {
            let result = async {
                let open_path = path.clone();
                let mut file = cx.background_executor().spawn(async move {
                    if let Some(parent) = open_path.parent() { std::fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
                    create_new_private_file(&open_path)
                }).await?;
                let mut bytes_written = 0usize;
                let mut samples = 0usize;
                let started = std::time::Instant::now();
                for _ in 0..120 {
                    if !flag.load(Ordering::Relaxed) || started.elapsed() >= std::time::Duration::from_secs(60) { break; }
                    let snapshot = match this.update(cx, |panel, cx| panel.snapshot_for_reload(cx)) {
                        Ok(snapshot) => snapshot,
                        Err(_) => break,
                    };
                    let remaining = 8 * 1024 * 1024 - bytes_written;
                    let elapsed_ms = started.elapsed().as_millis();
                    let result = cx.background_executor().spawn(async move {
                        use std::io::Write;
                        let sample = serde_json::json!({
                            "format": "jcode-desktop-debug-demo", "schema_version": 1,
                            "elapsed_ms": elapsed_ms.to_string(), "panel": snapshot,
                        });
                        let mut bytes = serde_json::to_vec(&sample).map_err(|error| error.to_string())?;
                        bytes.push(b'\n');
                        if bytes.len() > remaining { return Ok::<_, String>((file, 0)); }
                        file.write_all(&bytes).map_err(|error| error.to_string())?;
                        Ok((file, bytes.len()))
                    }).await?;
                    file = result.0;
                    if result.1 == 0 { break; }
                    bytes_written += result.1;
                    samples += 1;
                    cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                }
                cx.background_executor().spawn(async move { file.sync_all().map_err(|error| error.to_string()) }).await?;
                Ok::<_, String>((samples, bytes_written))
            }.await;
            flag.store(false, Ordering::Relaxed);
            if let Some(recordings) = RECORDINGS.get() {
                let mut recordings = recordings.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if recordings.get(&id).is_some_and(|current| std::sync::Arc::ptr_eq(current, &flag)) { recordings.remove(&id); }
            }
            let _ = this.update(cx, |panel, cx| {
                panel.items.push(match result {
                    Ok((samples, bytes)) => Item::Assistant(format!("Saved debug-demo recording: `{}` ({samples} snapshots, {bytes} bytes). This is JSONL debug state, not a screen recording.", path.display())),
                    Err(error) => Item::Error(format!("Debug-demo recording failed: {error}. Partial file: `{}`.", path.display())),
                });
                panel.transcript_measurements.dirty = true;
                cx.notify();
            });
        }).detach();
    }

    fn export_debug_snapshot(&mut self, args: &str, cx: &mut Context<Self>) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = if args.is_empty() {
            jcode_desktop_model::platform_dirs::state_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("jcode-desktop"))
                .join("captures")
                .join(format!("desktop-debug-{timestamp}.json"))
        } else {
            let requested = PathBuf::from(args);
            if requested.is_absolute() {
                requested
            } else {
                match std::env::current_dir() {
                    Ok(directory) => directory.join(requested),
                    Err(error) => {
                        self.items
                            .push(Item::Error(format!("Cannot resolve capture path: {error}")));
                        return;
                    }
                }
            }
        };
        let snapshot = self.snapshot_for_reload(cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                let value = serde_json::json!({
                    "format": "jcode-desktop-debug-state", "schema_version": 1,
                    "captured_unix_ns": timestamp.to_string(),
                    "desktop_version": crate::build_info::VERSION,
                    "display": crate::config::display(), "panel": snapshot,
                });
                let bytes = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
                if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
                write_new_private_file(&path, &bytes)?;
                Ok::<_, String>(path)
            }).await;
            let _ = this.update(cx, |panel, cx| {
                panel.items.push(match result {
                    Ok(path) => Item::Assistant(format!("Captured Desktop debug state: `{}`. This is a JSON snapshot, not a screen image. It contains the current session transcript and draft, so review it before sharing.", path.display())),
                    Err(error) => Item::Error(format!("Could not capture Desktop debug state: {error}")),
                });
                panel.transcript_measurements.dirty = true;
                cx.notify();
            });
        }).detach();
    }

    fn open_command_file(&mut self, path: PathBuf, label: &'static str, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let checked = cx
                .background_executor()
                .spawn(async move {
                    match std::fs::metadata(&path) {
                        Ok(metadata) if metadata.is_file() => Ok(path),
                        Ok(_) => Err(format!("{label} is not a file: `{}`.", path.display())),
                        Err(error) => Err(format!(
                            "Could not open {label}: `{}` ({error}).",
                            path.display()
                        )),
                    }
                })
                .await;
            let _ = this.update(cx, |panel, cx| {
                match checked {
                    Ok(path) => {
                        cx.open_with_system(&path);
                        panel.items.push(Item::Assistant(format!(
                            "Requested opening {label}: `{}`.",
                            path.display()
                        )));
                    }
                    Err(error) => panel.items.push(Item::Error(error)),
                }
                panel.transcript_measurements.dirty = true;
                cx.notify();
            });
        })
        .detach();
    }
}

fn write_new_private_file(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut file = create_new_private_file(path)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())
}

fn create_new_private_file(path: &std::path::Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|error| {
        format!(
            "`{}`: {error}. Existing files are never overwritten.",
            path.display()
        )
    })
}

fn valid_session_filename(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn git_read(directory: &str, diff: bool, staged: bool) -> Result<String, String> {
    let mut command = std::process::Command::new("git");
    command
        .current_dir(directory)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "--no-pager",
            "-c",
            "color.ui=false",
            "-c",
            "core.fsmonitor=false",
        ]);
    if diff {
        command.args(["diff", "--no-ext-diff", "--no-textconv"]);
        if staged {
            command.arg("--cached");
        }
    } else {
        command.args(["status", "--short", "--branch", "--untracked-files=normal"]);
    }
    jcode_desktop_harness::platform::no_console_window(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("Could not run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let output = String::from_utf8_lossy(&output.stdout);
    if output.trim().is_empty() {
        return Ok(if diff {
            "No tracked-file changes in this diff. Untracked files appear in `/git`."
        } else {
            "Working tree is clean."
        }
        .into());
    }
    // Bound the transcript payload without truncating inside a UTF-8 character.
    let clipped = output.chars().take(64_000).collect::<String>();
    let truncated = clipped.len() < output.len();
    // Indented code safely displays content containing Markdown fence characters.
    let code = clipped
        .lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "## {}\n\n{code}{}",
        if diff { "Git diff" } else { "Git status" },
        if truncated {
            "\n\nOutput truncated to 64,000 characters."
        } else {
            ""
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn debug_demo_records_and_stops_without_agent_commands(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("demo.jsonl");
        let (bridge, commands) = crate::harness::spawn_recording();
        let panel = cx.new(|cx| Panel::new("demo-test".into(), None, None, bridge, cx));
        panel.update(cx, |panel, cx| {
            panel.items.clear();
            assert!(panel.handle_desktop_command(&format!("/record start {}", path.display()), cx));
        });
        cx.run_until_parked();
        let first = std::fs::read_to_string(&path).unwrap();
        let sample: serde_json::Value =
            serde_json::from_str(first.lines().next().unwrap()).unwrap();
        assert_eq!(sample["format"], "jcode-desktop-debug-demo");
        assert_eq!(sample["panel"]["session_id"], "demo-test");
        panel.update(cx, |panel, cx| {
            assert!(panel.handle_desktop_command("/record stop", cx))
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        panel.read_with(cx, |panel, _| assert!(matches!(panel.items.last(), Some(Item::Assistant(text)) if text.contains("Saved debug-demo recording"))));
        assert!(
            !commands
                .try_iter()
                .any(|command| matches!(command, Command::Send { .. }))
        );
    }

    #[gpui::test]
    fn screenshot_exports_real_debug_state_without_overwriting(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("debug.json");
        let (bridge, commands) = crate::harness::spawn_recording();
        let panel = cx.new(|cx| Panel::new("debug-capture-test".into(), None, None, bridge, cx));
        panel.update(cx, |panel, cx| {
            panel.items.clear();
            panel.items.push(Item::User("capture fixture".into()));
            assert!(panel.handle_desktop_command(&format!("/screenshot {}", path.display()), cx));
        });
        cx.run_until_parked();
        let original = std::fs::read(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        assert_eq!(value["format"], "jcode-desktop-debug-state");
        assert_eq!(value["panel"]["session_id"], "debug-capture-test");
        assert!(String::from_utf8_lossy(&original).contains("capture fixture"));
        panel.update(cx, |panel, cx| {
            assert!(panel.handle_desktop_command(&format!("/screenshot {}", path.display()), cx));
        });
        cx.run_until_parked();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        panel.read_with(cx, |panel, _| {
            assert!(matches!(panel.items.last(), Some(Item::Error(_))))
        });
        assert!(
            !commands
                .try_iter()
                .any(|command| matches!(command, Command::Send { .. }))
        );
    }

    #[test]
    fn debug_capture_files_are_private_and_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.json");
        write_new_private_file(&path, b"original").unwrap();
        assert!(write_new_private_file(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[gpui::test]
    fn informational_commands_are_native_and_validate_arguments(cx: &mut gpui::TestAppContext) {
        let (bridge, commands) = crate::harness::spawn_recording();
        let panel = cx.new(|cx| Panel::new("desktop-native-test".into(), None, None, bridge, cx));
        panel.update(cx, |panel, cx| {
            panel.items.clear();
            for command in [
                "/version",
                "/config show",
                "/keys refresh",
                "/hotkeys",
                "/keybindings",
                "/colors list",
                "/terminal-setup",
                "/diff status",
                "/thinking status",
                "/reasoning status",
                "/alignment status",
                "/debug-visual status",
                "/compact-notifications status",
                "/show-agentgrep-output status",
                "/tool-call-details status",
                "/screenshot-mode status",
                "/record status",
            ] {
                assert!(panel.handle_desktop_command(command, cx), "{command}");
                assert!(
                    matches!(panel.items.last(), Some(Item::Assistant(_))),
                    "{command}"
                );
            }
            assert!(!panel.handle_desktop_command("/version-extra", cx));
            assert!(!panel.handle_desktop_command("ordinary user text", cx));
            for command in [
                "/thinking-display invalid",
                "/color no-such-theme",
                "/version extra",
            ] {
                assert!(panel.handle_desktop_command(command, cx));
                assert!(matches!(panel.items.last(), Some(Item::Error(_))));
            }
        });
        assert!(
            !commands
                .try_iter()
                .any(|command| matches!(command, Command::Send { .. }))
        );
    }

    #[test]
    fn command_names_require_exact_token_boundaries() {
        assert_eq!(
            split_command("  /thinking\tcurrent  "),
            ("/thinking", "current")
        );
        assert_eq!(split_command("/version-extra"), ("/version-extra", ""));
        assert!(display_key("/thinking-extra").is_none());
        for alias in ["/thinking-display", "/thinking", "/reasoning"] {
            assert_eq!(display_key(alias), Some("thinking_display"));
        }
    }

    #[test]
    fn session_paths_cannot_escape_the_sessions_directory() {
        for id in ["", ".", "..", "../secret", "/secret", "remote:abc", "a\\b"] {
            assert!(!valid_session_filename(id), "{id}");
        }
        assert!(valid_session_filename("session_abc-123"));
    }

    #[test]
    fn git_status_and_diff_read_a_real_repository_without_modifying_it() {
        let directory = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(directory.path())
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(directory.path().join("untracked.txt"), "hello").unwrap();
        let path = directory.path().to_str().unwrap();
        assert!(
            git_read(path, false, false)
                .unwrap()
                .contains("untracked.txt")
        );
        assert!(
            git_read(path, true, false)
                .unwrap()
                .contains("No tracked-file changes")
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("untracked.txt")).unwrap(),
            "hello"
        );
    }
}
