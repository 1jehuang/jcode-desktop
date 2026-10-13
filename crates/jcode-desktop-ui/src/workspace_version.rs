//! Workspace-wide version and updater status, kept out of individual chat footers.
use super::*;
use crate::updates::{ReleaseStatus, UpdateState};

fn status_label(
    release: &ReleaseStatus,
    update: &UpdateState,
    source: bool,
) -> (String, Option<&'static str>) {
    match update {
        UpdateState::Checking => ("Updating…".into(), None),
        UpdateState::Available { version } => (format!("Downloading {version}…"), None),
        UpdateState::ReadyToRestart { .. } => ("Update ready".into(), Some("Restart")),
        UpdateState::Finished { .. } => ("Update result".into(), None),
        UpdateState::Failed { .. } => ("Update failed".into(), Some("Retry")),
        UpdateState::Idle => match release {
            ReleaseStatus::Unknown | ReleaseStatus::Checking => {
                ("Checking for updates…".into(), None)
            }
            ReleaseStatus::Current { .. } => ("Latest version".into(), None),
            // A source checkout cannot install a release in place.
            ReleaseStatus::Newer { version } if source => (format!("{version} released"), None),
            ReleaseStatus::Newer { version } => (format!("{version} available"), Some("Update")),
            ReleaseStatus::Error { .. } => ("Unable to check".into(), None),
            ReleaseStatus::Source => ("Development build".into(), None),
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tone {
    Busy,
    Good,
    News,
    Bad,
}

/// What a check the user just asked for is doing, shown beside the version.
/// `None` when the Restart or Update action already says it.
fn check_feedback(release: &ReleaseStatus, update: &UpdateState) -> Option<(String, Tone)> {
    Some(match update {
        UpdateState::Checking => ("Checking…".into(), Tone::Busy),
        UpdateState::Available { version } => (format!("Downloading {version}…"), Tone::Busy),
        UpdateState::ReadyToRestart { .. } => return None,
        UpdateState::Failed { .. } => ("Update failed".into(), Tone::Bad),
        UpdateState::Finished { .. } | UpdateState::Idle => match release {
            ReleaseStatus::Unknown | ReleaseStatus::Checking => ("Checking…".into(), Tone::Busy),
            ReleaseStatus::Current { .. } => ("Up to date".into(), Tone::Good),
            ReleaseStatus::Newer { version } => (format!("v{version} out"), Tone::News),
            ReleaseStatus::Error { .. } => ("Couldn't check".into(), Tone::Bad),
            ReleaseStatus::Source => return None,
        },
    })
}

fn visible_feedback() -> Option<(String, Tone)> {
    updates::user_check_visible()
        .then(|| check_feedback(&updates::release_status(), &updates::current()))
        .flatten()
}

const CHAR_WIDTH: f32 = 5.4;

/// Size the pill to its short version string, with room for an optional action
/// and the result of a check the user asked for.
pub(super) fn pill_width() -> f32 {
    let (_, action) = status_label(
        &updates::release_status(),
        &updates::current(),
        updates::is_source_build(),
    );
    let version =
        ((crate::build_info::VERSION.len() + 1) as f32 * CHAR_WIDTH + 16.0).clamp(64.0, 132.0);
    let feedback = visible_feedback().map_or(0.0, |(text, _)| {
        text.chars().count() as f32 * CHAR_WIDTH + 8.0
    });
    (version + feedback + action.map_or(0.0, |label| label.len() as f32 * CHAR_WIDTH + 12.0)).ceil()
}

impl Workspace {
    /// Clicking the version pill: check for a newer release right away and
    /// ask the platform updater to check, download, or install whatever is
    /// next. The pill shows the check and its result.
    fn check_and_apply_update() {
        updates::note_user_check();
        updates::check_release_now();
        // A source checkout only reports releases. Ctrl+R or /update rebuild it.
        if updates::is_source_build() {
            return;
        }
        if updates::request_now() == updates::UpdateRequest::Unavailable {
            updates::set(UpdateState::Failed {
                message: "Automatic updates are unavailable in this build. Install the latest Desktop release manually.".into(),
            });
        }
    }

    pub(super) fn render_version_header(
        &mut self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        updates::ensure_release_check();
        let release = updates::release_status();
        let update = updates::current();
        let (label, action) = status_label(&release, &update, updates::is_source_build());
        let feedback = visible_feedback();
        let theme = Theme::global();
        let feedback = feedback.map(|(text, tone)| {
            let color = match tone {
                Tone::Busy => theme.ACCENT,
                Tone::Good => theme.OK,
                Tone::News => theme.ACCENT,
                Tone::Bad => theme.ERROR,
            };
            let content = if tone == Tone::Busy {
                let ticker = self.version_pulse.get(
                    "check",
                    crate::pulse_text::Kind::PulseText {
                        text: text.into(),
                        period: crate::pulse_text::TOOL_PULSE_PERIOD,
                        curve: version_check_pulse,
                    },
                    cx,
                );
                ticker.into_any_element()
            } else {
                div().child(text).into_any_element()
            };
            div()
                .id("workspace-version-check")
                .debug_selector(|| "workspace-version-check".into())
                .flex_none()
                .text_color(color)
                .font_family(theme.FONT_MONO)
                .text_size(px(9.0))
                .child(content)
        });
        if feedback.is_none() {
            self.version_pulse.retain(|_| false);
        }
        let detail = match &update {
            UpdateState::Finished { message } | UpdateState::Failed { message } => message.clone(),
            _ => match &release {
                ReleaseStatus::Error { message } => message.clone(),
                ReleaseStatus::Source => "Local source build. Release status cannot establish whether this checkout is current. Use Ctrl+R to rebuild local changes.".into(),
                ReleaseStatus::Current { latest } => format!("Latest release: v{latest}"),
                ReleaseStatus::Newer { version } if updates::is_source_build() => format!("v{version} is released. This source build updates through Git and Ctrl+R."),
                _ => label.clone(),
            },
        };
        div()
            .id("workspace-version")
            .debug_selector(|| "workspace-version".into())
            .absolute()
            .left(px(4.0))
            .top(px(5.0))
            .w(px(width - 4.0))
            .h(px(18.0))
            .rounded_full()
            .bg(Theme::global().PANEL_BG)
            .px(px(6.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .occlude()
            .cursor_pointer()
            .hover(|el| el.bg(Theme::global().ACCENT.opacity(0.08)))
            .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                cx.stop_propagation();
                window.prevent_default();
            })
            .on_click(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
                Self::check_and_apply_update();
                cx.notify();
            }))
            .tooltip(move |_, cx| {
                cx.new(|_| {
                    live_tabs::TabTooltip(
                        format!(
                            "{}\nClick to check for updates",
                            if detail == label {
                                label.clone()
                            } else {
                                format!("{label}\n{detail}")
                            }
                        )
                        .into(),
                    )
                })
                .into()
            })
            .child(
                div()
                    .id("workspace-build")
                    .debug_selector(|| "workspace-build".into())
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .font_family(Theme::global().FONT_MONO)
                    .text_size(px(9.0))
                    .text_color(Theme::global().TEXT_DIM)
                    .tooltip(|_, cx| cx.new(|_| crate::build_info::BuildTooltip).into())
                    .child(crate::build_info::version()),
            )
            .children(feedback)
            .when_some(action, |el, action| {
                el.child(
                    div()
                        .id("workspace-update-button")
                        .debug_selector(|| "workspace-update-button".into())
                        .flex_none()
                        .px_1()
                        .h(px(16.0))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .bg(Theme::global().ACCENT.opacity(0.12))
                        .text_color(Theme::global().ACCENT)
                        .font_family(Theme::global().FONT_MONO)
                        .text_size(px(9.0))
                        .cursor_pointer()
                        .hover(|el| el.bg(Theme::global().ACCENT.opacity(0.22)))
                        .child(action),
                )
            })
            .into_any_element()
    }
}

fn version_check_pulse(phase: f32) -> f32 {
    0.6 + 0.4 * (phase * std::f32::consts::TAU).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_status_never_calls_an_unchecked_build_latest() {
        for release in [
            ReleaseStatus::Unknown,
            ReleaseStatus::Checking,
            ReleaseStatus::Source,
            ReleaseStatus::Error {
                message: "offline".into(),
            },
        ] {
            let (label, action) = status_label(&release, &UpdateState::Idle, false);
            assert_ne!(label, "Latest version");
            assert!(action.is_none());
        }
        assert_eq!(
            status_label(
                &ReleaseStatus::Current {
                    latest: "1.0.0".into()
                },
                &UpdateState::Idle,
                false
            ),
            ("Latest version".into(), None)
        );
        let newer = ReleaseStatus::Newer {
            version: "1.1.0".into(),
        };
        assert_eq!(
            status_label(&newer, &UpdateState::Idle, false).1,
            Some("Update")
        );
        assert_eq!(
            status_label(&newer, &UpdateState::Idle, true).1,
            None,
            "a source checkout cannot install a release"
        );
        assert_eq!(status_label(&newer, &UpdateState::Checking, false).1, None);
        assert_eq!(
            status_label(
                &newer,
                &UpdateState::ReadyToRestart {
                    version: "1.1.0".into()
                },
                false
            )
            .1,
            Some("Restart")
        );
    }

    #[test]
    fn a_requested_check_reports_progress_then_its_result() {
        let current = ReleaseStatus::Current {
            latest: "1.0.0".into(),
        };
        assert_eq!(
            check_feedback(&ReleaseStatus::Checking, &UpdateState::Idle),
            Some(("Checking…".into(), Tone::Busy))
        );
        assert_eq!(
            check_feedback(&current, &UpdateState::Checking),
            Some(("Checking…".into(), Tone::Busy))
        );
        assert_eq!(
            check_feedback(&current, &UpdateState::Idle),
            Some(("Up to date".into(), Tone::Good))
        );
        assert_eq!(
            check_feedback(
                &current,
                &UpdateState::Finished {
                    message: "current".into()
                }
            ),
            Some(("Up to date".into(), Tone::Good))
        );
        assert_eq!(
            check_feedback(
                &ReleaseStatus::Newer {
                    version: "1.1.0".into()
                },
                &UpdateState::Idle
            ),
            Some(("v1.1.0 out".into(), Tone::News))
        );
        assert_eq!(
            check_feedback(
                &ReleaseStatus::Error {
                    message: "offline".into()
                },
                &UpdateState::Idle
            )
            .map(|(_, tone)| tone),
            Some(Tone::Bad)
        );
        assert_eq!(
            check_feedback(
                &current,
                &UpdateState::ReadyToRestart {
                    version: "1.1.0".into()
                }
            ),
            None,
            "Restart already says it"
        );
    }
    #[gpui::test]
    fn workspace_version_lives_in_header_and_update_button_runs_platform_action(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static REQUESTS: AtomicUsize = AtomicUsize::new(0);
        extern "C" fn check() {
            REQUESTS.fetch_add(1, Ordering::SeqCst);
        }
        let _guard = updates::test_lock();
        updates::clear_test_actions();
        updates::set_release_status(ReleaseStatus::Current {
            latest: "1.0.0".into(),
        });
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("version-header", cx);
            w
        });
        let draw = |vcx: &mut gpui::VisualTestContext| {
            workspace.update(vcx, |_, cx| cx.notify());
            vcx.draw(
                gpui::point(px(0.), px(0.)),
                gpui::size(px(1200.), px(800.)),
                |_, _| div(),
            );
            vcx.run_until_parked();
        };
        draw(vcx);
        let header = vcx
            .debug_bounds("workspace-version")
            .expect("version in header");
        assert!(header.origin.y < px(100.));
        assert_eq!(header.size.height, px(18.0));
        assert!(header.size.width < px(132.0), "idle version stays compact");
        let fps = vcx.debug_bounds("fps-counter").unwrap();
        let build = vcx.debug_bounds("workspace-build").unwrap();
        assert!(header.right() <= fps.left());
        assert!(build.size.height <= px(16.0));
        assert!(build.top() >= header.top() && build.bottom() <= header.bottom());
        assert!(vcx.debug_bounds("workspace-version-status").is_none());
        assert!(
            vcx.debug_bounds("panel-build").is_none(),
            "workspace footer must not duplicate version"
        );
        assert!(vcx.debug_bounds("workspace-update-button").is_none());
        updates::set_release_status(ReleaseStatus::Newer {
            version: "1.1.0".into(),
        });
        unsafe {
            updates::jcode_update_register_actions(check, check);
        }
        draw(vcx);
        let header = vcx.debug_bounds("workspace-version").unwrap();
        let button = vcx
            .debug_bounds("workspace-update-button")
            .expect("new release offers Update");
        let build = vcx.debug_bounds("workspace-build").unwrap();
        assert!(build.right() <= button.left());
        assert!((build.center().y - button.center().y).abs() < px(1.0));
        assert!(button.right() <= header.right());
        assert!(button.origin.y >= header.origin.y);
        assert!(button.bottom() <= header.bottom());
        let before = REQUESTS.load(Ordering::SeqCst);
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        draw(vcx);
        assert_eq!(REQUESTS.load(Ordering::SeqCst), before + 1);
        assert!(
            vcx.debug_bounds("workspace-update-button").is_none(),
            "busy update cannot be clicked twice"
        );
        assert!(
            vcx.debug_bounds("update-chip").is_none(),
            "workspace uses header, not bottom overlay"
        );
        // Clicking the version text itself also checks for and applies updates.
        updates::clear_test_actions();
        updates::set_release_status(ReleaseStatus::Current {
            latest: "1.0.0".into(),
        });
        unsafe {
            updates::jcode_update_register_actions(check, check);
        }
        draw(vcx);
        let build = vcx.debug_bounds("workspace-build").unwrap();
        let compact = vcx.debug_bounds("workspace-version").unwrap().size.width;
        assert!(vcx.debug_bounds("workspace-version-check").is_none());
        let before = REQUESTS.load(Ordering::SeqCst);
        vcx.simulate_click(build.center(), gpui::Modifiers::default());
        draw(vcx);
        assert_eq!(REQUESTS.load(Ordering::SeqCst), before + 1);
        assert_eq!(updates::current(), UpdateState::Checking);
        assert_eq!(updates::release_status(), ReleaseStatus::Checking);
        // The click is visible: the pill grows to show the running check.
        let check = vcx
            .debug_bounds("workspace-version-check")
            .expect("clicking shows the check in progress");
        let header = vcx.debug_bounds("workspace-version").unwrap();
        assert!(header.size.width > compact);
        assert!(check.right() <= header.right());
        // Its result stays visible once the check settles.
        updates::set(UpdateState::Idle);
        updates::set_release_status(ReleaseStatus::Current {
            latest: "1.0.0".into(),
        });
        draw(vcx);
        assert!(vcx.debug_bounds("workspace-version-check").is_some());
        assert_eq!(visible_feedback(), Some(("Up to date".into(), Tone::Good)));
        updates::clear_test_actions();
        updates::set_release_status(ReleaseStatus::Source);
    }
}
