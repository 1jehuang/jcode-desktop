//! Opens the Desktop publish tracker session from a self-development panel.
use super::*;
use crate::publish;

impl Workspace {
    pub(super) fn publish_desktop(
        &mut self,
        request: &PublishDesktop,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self
            .slots
            .iter()
            .find(|slot| !slot.closing && slot.panel.entity_id() == request.source)
            .and_then(|slot| slot.panel.read(cx).working_dir.clone())
            .and_then(|dir| publish::checkout_root(&dir))
        else {
            return;
        };
        let directory = root.display().to_string();
        // One pipeline per checkout. A second press returns to the live
        // tracker instead of racing two release orchestrators.
        if let Some(index) = self.slots.iter().position(|slot| {
            let panel = slot.panel.read(cx);
            !slot.closing
                && panel.is_publish_tracker()
                && panel.working_dir.as_deref() == Some(directory.as_str())
                && (panel.is_pending_session() || panel.is_busy())
        }) {
            self.set_active(index, cx);
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let request_id =
            pending::next_draft_id().replacen("startup://draft/", publish::DRAFT_PREFIX, 1);
        self.mount_session_draft(
            request_id.clone(),
            Some(directory.clone()),
            publish::SESSION_TITLE.into(),
            cx,
        );
        let panel = self.slots[self.active].panel.clone();
        panel.update(cx, |panel, cx| {
            panel.set_publish_tracker(true);
            panel.status = "Starting publish…".into();
            cx.notify();
        });
        self.bridge.send(Command::CreateSession {
            working_dir: Some(directory),
            request_id: Some(request_id),
        });
    }

    /// Offline screenshot fixture: a self-development panel with the Publish
    /// button beside a tracker partway through the pipeline.
    pub(super) fn open_publish_fixture(&mut self, cx: &mut Context<Self>) {
        let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .display()
            .to_string();
        if let Some(slot) = self.slots.get(self.active) {
            slot.panel.update(cx, |panel, cx| {
                panel.working_dir = Some(checkout.clone());
                cx.notify();
            });
        }
        let mut session = self.sessions[0].clone();
        session.session_id = "screenshot-publish".into();
        session.title = Some(publish::SESSION_TITLE.into());
        session.working_dir = Some(checkout);
        self.sessions.push(session.clone());
        self.active = self.open_session(session, cx);
        self.slots[self.active].panel.update(cx, |panel, cx| {
            panel.set_publish_tracker(true);
            panel.seed_publish_fixture();
            cx.notify();
        });
    }

    /// Runtime creation finished for a publish draft. Name it, then start
    /// the staged pipeline in the new session.
    pub(super) fn start_publish_session(&mut self, session_id: String, root: Option<&str>) {
        let Some(root) = root.and_then(publish::checkout_root) else {
            return;
        };
        self.bridge.send(Command::SessionOperation {
            session_id: session_id.clone(),
            operation: harness::SessionOperation::Rename(Some(publish::SESSION_TITLE.into())),
        });
        self.bridge.send(Command::Send {
            session_id,
            content: publish::session_prompt(&root),
            images: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop_checkout() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("crates/jcode-desktop-ui/src")).unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = 'jcode-desktop'\nversion = '0.1.0'\n",
        )
        .unwrap();
        root
    }

    fn created(id: &str, dir: &str) -> jcode_sdk::SessionInfo {
        jcode_sdk::SessionInfo {
            session_id: id.into(),
            title: None,
            working_dir: Some(dir.into()),
            status: "idle".into(),
            transcript_bytes: None,
            saved: false,
            updated_at_ms: None,
            last_active_at_ms: None,
            archived: false,
            archived_at_ms: None,
            save_label: None,
            parent_session_id: None,
            agent_label: None,
            swarm_status: None,
            edit_stats: None,
        }
    }

    #[gpui::test]
    fn publish_button_needs_confirmation_and_opens_a_tracked_session(
        cx: &mut gpui::TestAppContext,
    ) {
        let checkout = desktop_checkout();
        let nested = checkout.path().join("crates/jcode-desktop-ui/src");
        let root = checkout
            .path()
            .canonicalize()
            .unwrap()
            .display()
            .to_string();
        cx.update(crate::bind_workspace_keys);
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.bridge = bridge;
            w.remotes.default_host = None;
            w.push_test_panel("selfdev", cx);
            w.restore_focus(window, cx);
            w
        });
        let source = workspace.update(vcx, |w, cx| {
            let panel = w.slots[0].panel.clone();
            panel.update(cx, |panel, cx| {
                panel.working_dir = Some(nested.display().to_string());
                cx.notify();
            });
            panel
        });
        vcx.run_until_parked();
        while commands.try_recv().is_ok() {}

        // First click only arms. Nothing is created or sent.
        let button = vcx
            .debug_bounds("publish-desktop")
            .expect("selfdev panels show the Publish button");
        vcx.simulate_click(button.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        assert!(commands.try_recv().is_err(), "one click must not publish");
        assert_eq!(workspace.read_with(vcx, |w, _| w.slots.len()), 1);

        // Second click opens the tracker draft in the checkout root.
        let button = vcx.debug_bounds("publish-desktop").unwrap();
        vcx.simulate_click(button.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        let request_id = match commands.try_recv() {
            Ok(Command::CreateSession {
                request_id: Some(id),
                working_dir,
            }) => {
                assert_eq!(working_dir.as_deref(), Some(root.as_str()));
                id
            }
            _ => panic!("expected publish session creation"),
        };
        assert!(crate::publish::is_publish_draft(&request_id));
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.len(), 2);
            let tracker = w.slots[w.active].panel.read(cx);
            assert!(tracker.is_publish_tracker());
            assert_ne!(w.slots[w.active].panel.entity_id(), source.entity_id());
        });
        assert!(vcx.debug_bounds("publish-tracker").is_some());
        for selector in ["publish-stage-0", "publish-stage-5", "publish-tracker-bar"] {
            assert!(vcx.debug_bounds(selector).is_some(), "{selector}");
        }

        // Pressing again while it starts refocuses instead of racing a second run.
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.set_active(0, cx);
                w.publish_desktop(
                    &PublishDesktop {
                        source: source.entity_id(),
                    },
                    window,
                    cx,
                );
                assert_eq!(w.slots.len(), 2);
                assert_eq!(w.active, 1);
            })
        });
        assert!(commands.try_recv().is_err());

        // Creation renames the session and sends the staged pipeline prompt.
        workspace.update(vcx, |w, cx| {
            w.apply(
                Update::SessionCreated {
                    session: created("publish", &root),
                    request_id: Some(request_id),
                },
                cx,
            );
        });
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::SessionOperation {
                session_id,
                operation: harness::SessionOperation::Rename(Some(title)),
            }) if session_id == "publish" && title == crate::publish::SESSION_TITLE
        ));
        match commands.try_recv() {
            Ok(Command::Send {
                session_id,
                content,
                ..
            }) => {
                assert_eq!(session_id, "publish");
                assert!(content.contains(&root));
                assert!(content.contains("release-desktop.py"));
            }
            _ => panic!("expected the publish prompt"),
        }
    }

    #[gpui::test]
    fn ordinary_projects_never_show_the_publish_button(cx: &mut gpui::TestAppContext) {
        let other = tempfile::tempdir().unwrap();
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("project", cx);
            w.restore_focus(window, cx);
            w
        });
        workspace.update(vcx, |w, cx| {
            w.slots[0].panel.update(cx, |panel, cx| {
                panel.working_dir = Some(other.path().display().to_string());
                cx.notify();
            });
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("composer").is_some());
        assert!(vcx.debug_bounds("publish-desktop").is_none());
    }
}
