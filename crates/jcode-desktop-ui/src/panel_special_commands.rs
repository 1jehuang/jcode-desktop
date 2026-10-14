//! Specialist slash commands. Runtime state always belongs to the selected session.
use super::*;
use jcode_sdk::api::{RuntimeCommand, RuntimeCommandOutput};

#[derive(Debug, PartialEq)]
enum SpecialCommand {
    Runtime(RuntimeCommand, Vec<String>),
    Support,
    Subscribe,
    SwarmPrompt,
    Dictation,
    Usage(&'static str),
}

fn parse_special_command(content: &str) -> Option<SpecialCommand> {
    use SpecialCommand::*;
    let mut words = content.split_whitespace();
    let name = words.next()?;
    let args: Vec<String> = words.map(str::to_owned).collect();
    let exact = |command, usage| {
        if args.is_empty() {
            command
        } else {
            Usage(usage)
        }
    };
    Some(match name {
        "/overnight" => Runtime(RuntimeCommand::Overnight, args),
        "/initiatives" | "/goals" => Runtime(RuntimeCommand::Initiatives, args),
        "/productivity" | "/wrapped" => Runtime(RuntimeCommand::Productivity, args),
        "/z" | "/zz" | "/zzz" | "/zstatus" => {
            if !args.is_empty() {
                Usage("Premium commands take no arguments: `/z`, `/zz`, `/zzz`, `/zstatus`.")
            } else {
                Runtime(
                    RuntimeCommand::Premium,
                    vec![
                        match name {
                            "/z" => "normal",
                            "/zz" => "toggle-one",
                            "/zzz" => "toggle-zero",
                            _ => "status",
                        }
                        .into(),
                    ],
                )
            }
        }
        "/support" => exact(
            Support,
            "Usage: `/support`. Opens a diagnostic email draft, never sends it.",
        ),
        "/subscribe" => exact(
            Subscribe,
            "Usage: `/subscribe`. Opens the native account dialog.",
        ),
        "/swarm-prompt" => exact(SwarmPrompt, "Usage: `/swarm-prompt`."),
        "/dictate" | "/dictation" => exact(Dictation, "Usage: `/dictate` or `/dictation`."),
        _ => return None,
    })
}

fn percent_encode(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            result.push(char::from(byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

fn support_draft(provider: &str, model: &str) -> (String, String) {
    // Deliberately excludes transcripts, credentials, account identity and file paths.
    let body = format!(
        "Describe your issue:\n\n\n--- Desktop diagnostics ---\nVersion: {}\nBuild: {}\nOS/Arch: {}/{}\nProvider: {}\nModel: {}\n\nPlease review this draft before sending. No conversation text or credentials were included.\n",
        env!("JCODE_DESKTOP_VERSION"),
        env!("JCODE_DESKTOP_BUILD_ID"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        provider,
        model
    );
    let url = format!(
        "mailto:support@jcode.sh?subject=jcode%20Desktop%20support&body={}",
        percent_encode(&body)
    );
    (body, url)
}

/// Materialize the selected prompt only when the user explicitly requests editing.
fn editable_swarm_prompt(working_dir: Option<&str>) -> anyhow::Result<std::path::PathBuf> {
    let global = jcode_base::storage::jcode_dir()?.join("swarm-prompt.md");
    let project = working_dir.map(|dir| std::path::Path::new(dir).join(".jcode/swarm-prompt.md"));
    let path = project
        .filter(|path| std::fs::read_to_string(path).is_ok_and(|text| !text.trim().is_empty()))
        .unwrap_or(global);
    if !path.exists() {
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("Prompt has no parent directory"))?,
        )?;
        // Never overwrite a prompt another worker/editor created concurrently.
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => file.write_all(jcode_base::prompt::DEFAULT_SWARM_PROMPT.as_bytes())?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

fn persist_review_html(
    html: &str,
    directory: &std::path::Path,
) -> anyhow::Result<std::path::PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(directory)?;
    let mut file = tempfile::Builder::new()
        .prefix("overnight-review-")
        .suffix(".html")
        .tempfile_in(directory)?;
    file.write_all(html.as_bytes())?;
    let (_, path) = file.keep()?;
    Ok(path)
}

impl Panel {
    pub(super) fn handle_special_command(&mut self, content: &str, cx: &mut Context<Self>) -> bool {
        let Some(command) = parse_special_command(content) else {
            return false;
        };
        if self.is_side_document() || self.preview_state.is_some() {
            self.items.push(Item::Error(
                "Special commands require a regular Desktop session.".into(),
            ));
            cx.notify();
            return true;
        }
        match command {
            SpecialCommand::Runtime(command, args) => self.bridge.send(Command::RuntimeCommand {
                session_id: self.session_id.clone(),
                command,
                args,
            }),
            SpecialCommand::Usage(usage) => self.items.push(Item::Error(usage.into())),
            SpecialCommand::Support => {
                let (body, url) = support_draft(
                    self.provider.as_deref().unwrap_or("unknown"),
                    self.model.as_deref().unwrap_or("unknown"),
                );
                cx.open_url(&url);
                self.items.push(Item::Assistant(format!("Opened a support email draft for `support@jcode.sh`. Nothing was sent. If your mail app did not open, copy this draft:\n\n{body}")));
            }
            SpecialCommand::Subscribe => {
                // Composer dispatches OpenAccounts before pending-session handling.
                // Direct callers still get useful subscription information, never a charge.
                self.items.push(Item::Assistant("Subscribe through the native Accounts dialog: choose Jcode, review the plan, then explicitly confirm checkout. `/account` opens that dialog. No checkout or payment was started.".into()));
            }
            SpecialCommand::SwarmPrompt => {
                if crate::harness::remote_host(&self.session_id).is_some() {
                    self.items.push(Item::Error("The active swarm prompt belongs to the remote runtime. Edit `.jcode/swarm-prompt.md` in that remote checkout or `~/.jcode/swarm-prompt.md` on that host. Desktop will not open or change a local file on its behalf.".into()));
                } else {
                    let directory = self.working_dir.clone();
                    let task = cx
                        .background_executor()
                        .spawn(async move { editable_swarm_prompt(directory.as_deref()) });
                    cx.spawn(async move |this, cx| {
                        let result = task.await;
                        let _ = this.update(cx, |panel, cx| {
                            match result {
                                Ok(path) => {
                                    cx.open_url(&format!("file://{}", percent_encode(&path.to_string_lossy())));
                                    panel.items.push(Item::Assistant(format!("Opened the active swarm prompt: `{}`. Changes apply when new agents load their routing prompt.", path.display())));
                                }
                                Err(error) => panel.items.push(Item::Error(format!("Could not open swarm prompt: {error:#}"))),
                            }
                            cx.notify();
                        });
                    }).detach();
                }
            }
            SpecialCommand::Dictation => {
                let task = cx.background_executor().spawn(async {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?
                        .block_on(jcode_voice::dictation::run_configured())
                });
                self.items.push(Item::Assistant("Running the configured local dictation command. The transcript will be inserted into this composer's draft for review, not submitted automatically.".into()));
                cx.spawn(async move |this, cx| {
                    let result: anyhow::Result<jcode_voice::dictation::DictationRun> = task.await;
                    let _ = this.update(cx, |panel, cx| {
                        match result {
                            Ok(run) => panel
                                .input
                                .update(cx, |input, cx| input.append_dictation(&run.text, cx)),
                            Err(error) => panel
                                .items
                                .push(Item::Error(format!("Dictation failed: {error:#}"))),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        self.stick_to_bottom = true;
        cx.notify();
        true
    }

    /// Extra artifacts are explicit and session-scoped, never ignored or loaded
    /// from a path on the local machine when the result came from a remote host.
    pub(super) fn apply_special_command_artifacts(
        &mut self,
        output: &RuntimeCommandOutput,
        cx: &mut Context<Self>,
    ) {
        if output.session_id != self.session_id {
            return;
        }
        if output.command == RuntimeCommand::Initiatives
            && let Some(payload) = self.latest_todo_payload()
        {
            self.items.push(Item::Todos(payload));
        }
        if output.command == RuntimeCommand::Overnight
            && let Some(html) = output.data.get("html").and_then(serde_json::Value::as_str)
        {
            let html = html.to_owned();
            let task = cx.background_executor().spawn(async move {
                let root = jcode_base::storage::jcode_dir()?.join("generated-reviews");
                persist_review_html(&html, &root)
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |panel, cx| {
                    match result {
                        Ok(path) => panel.items.push(Item::Assistant(format!("Saved the full HTML review as a private local artifact: `{}`. It was not opened or executed automatically. Review the source before opening external HTML.", path.display()))),
                        Err(error) => panel.items.push(Item::Error(format!("Could not save the full HTML review: {error:#}. The task review remains visible above."))),
                    }
                    cx.notify();
                });
            }).detach();
        }
        if output.command == RuntimeCommand::Productivity
            && let Some(data) = output
                .data
                .get("image")
                .and_then(|image| image.get("base64"))
                .and_then(serde_json::Value::as_str)
        {
            self.items.push(Item::Image(TranscriptImage::new(
                "image/png".into(),
                data.into(),
                Some("Productivity dashboard".into()),
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn specialist_runtime_commands_use_the_source_connection_not_prompt_send(
        cx: &mut gpui::TestAppContext,
    ) {
        let (bridge, commands) = crate::harness::spawn_recording();
        let panel = cx.new(|cx| Panel::new("special-source".into(), None, None, bridge, cx));
        panel.update(cx, |panel, cx| {
            for input in [
                "/overnight status",
                "/productivity",
                "/wrapped",
                "/initiatives",
                "/goals",
                "/z",
                "/zz",
                "/zzz",
                "/zstatus",
            ] {
                assert!(panel.handle_special_command(input, cx), "{input}");
            }
            assert!(panel.handle_special_command("/support unexpected", cx));
            assert!(matches!(panel.items.last(), Some(Item::Error(_))));
        });
        let captured: Vec<_> = commands.try_iter().collect();
        assert_eq!(captured.len(), 9);
        for command in captured {
            assert!(
                matches!(command, Command::RuntimeCommand { session_id, .. } if session_id == "special-source")
            );
        }
    }

    #[gpui::test]
    fn dashboard_artifacts_are_scoped_to_the_source_session(cx: &mut gpui::TestAppContext) {
        let (bridge, _) = crate::harness::spawn_recording();
        let panel = cx.new(|cx| Panel::new("special-source".into(), None, None, bridge, cx));
        let mut output = RuntimeCommandOutput {
            session_id: "different-session".into(),
            command: RuntimeCommand::Productivity,
            data: serde_json::json!({"image": {"base64": ""}}),
            message: "report".into(),
        };
        panel.update(cx, |panel, cx| {
            panel.items.clear();
            panel.apply_special_command_artifacts(&output, cx);
            assert!(panel.items.is_empty());
            output.session_id = "special-source".into();
            panel.apply_special_command_artifacts(&output, cx);
            assert!(matches!(panel.items.last(), Some(Item::Image(_))));
        });
    }

    #[gpui::test]
    fn legacy_initiatives_reuses_current_session_todo_payload(cx: &mut gpui::TestAppContext) {
        let (bridge, _) = crate::harness::spawn_recording();
        let panel = cx.new(|cx| Panel::new("special-source".into(), None, None, bridge, cx));
        let output = RuntimeCommandOutput {
            session_id: "special-source".into(),
            command: RuntimeCommand::Initiatives,
            data: serde_json::json!({"retired": true}),
            message: "retired".into(),
        };
        panel.update(cx, |panel, cx| {
            panel.items.clear();
            panel.items.push(Item::Todos(TodoCardPayload::default()));
            panel.apply_special_command_artifacts(&output, cx);
            assert_eq!(panel.items.len(), 2);
            assert!(matches!(panel.items.last(), Some(Item::Todos(_))));
        });
    }

    #[test]
    fn reviews_are_private_collision_free_and_not_opened() {
        let directory = tempfile::tempdir().unwrap();
        let first = persist_review_html("<html>first</html>", directory.path()).unwrap();
        let second = persist_review_html("<script>second</script>", directory.path()).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "<html>first</html>"
        );
        assert_eq!(
            std::fs::read_to_string(&second).unwrap(),
            "<script>second</script>"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(first).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn aliases_and_exact_names() {
        for (left, right) in [
            ("/goals", "/initiatives"),
            ("/wrapped", "/productivity"),
            ("/dictation", "/dictate"),
        ] {
            assert_eq!(parse_special_command(left), parse_special_command(right));
        }
        for value in [
            "/overnightly",
            "/zebra",
            "/supported",
            "/subscribe-now",
            "dictate",
            "",
        ] {
            assert_eq!(parse_special_command(value), None);
        }
        assert!(matches!(
            parse_special_command("/support extra"),
            Some(SpecialCommand::Usage(_))
        ));
        assert!(matches!(
            parse_special_command("/zzz extra"),
            Some(SpecialCommand::Usage(_))
        ));
    }
    #[test]
    fn runtime_arguments_and_premium_toggles_are_preserved() {
        assert_eq!(
            parse_special_command(" /overnight 8h fix tests "),
            Some(SpecialCommand::Runtime(
                RuntimeCommand::Overnight,
                vec!["8h".into(), "fix".into(), "tests".into()]
            ))
        );
        assert_eq!(
            parse_special_command("/zz"),
            Some(SpecialCommand::Runtime(
                RuntimeCommand::Premium,
                vec!["toggle-one".into()]
            ))
        );
        assert_eq!(
            parse_special_command("/zzz"),
            Some(SpecialCommand::Runtime(
                RuntimeCommand::Premium,
                vec!["toggle-zero".into()]
            ))
        );
    }
    #[test]
    fn mailto_encoding_does_not_allow_header_injection() {
        let (body, url) = support_draft("provider&bcc=bad", "model\r\nBcc: bad");
        assert!(body.contains("Version:"));
        assert!(!url.contains("&bcc="));
        assert!(!url.contains('\n'));
        assert!(url.contains("%26bcc%3Dbad"));
        assert_eq!(percent_encode("a b?#%"), "a%20b%3F%23%25");
    }
}
