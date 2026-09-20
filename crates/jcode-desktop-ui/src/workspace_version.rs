//! Workspace-wide version and updater status, kept out of individual chat footers.
use super::*;
use crate::updates::{ReleaseStatus, UpdateState};

fn status_label(release: &ReleaseStatus, update: &UpdateState) -> (String, Option<&'static str>) {
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
            ReleaseStatus::Newer { version } => (format!("{version} available"), Some("Update")),
            ReleaseStatus::Error { .. } => ("Unable to check".into(), None),
            ReleaseStatus::Source => ("Development build".into(), None),
        },
    }
}

impl Workspace {
    pub(super) fn render_version_header(
        &self,
        width: f32,
        right: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        updates::ensure_release_check();
        let release = updates::release_status();
        let update = updates::current();
        let (mut label, action) = status_label(&release, &update);
        let detail = match &update {
            UpdateState::Finished { message } | UpdateState::Failed { message } => message.clone(),
            _ => match &release {
                ReleaseStatus::Error { message } => message.clone(),
                ReleaseStatus::Source => "Local source build. Release status cannot establish whether this checkout is current. Use Ctrl+R to rebuild local changes.".into(),
                _ => label.clone(),
            },
        };
        if width < 200.0
            && matches!(release, ReleaseStatus::Newer { .. })
            && update == UpdateState::Idle
        {
            label = "Update available".into();
        }
        div()
            .id("workspace-version")
            .debug_selector(|| "workspace-version".into())
            .absolute()
            .right(px(right))
            .top_0()
            .w(px(width))
            .h(px(FOLDER_CONTENT_INSET - 8.0))
            .px_2()
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .occlude()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .text_size(px(10.0))
                    .text_color(Theme::global().TEXT_DIM)
                    .child(
                        div()
                            .id("workspace-build")
                            .debug_selector(|| "workspace-build".into())
                            .truncate()
                            .tooltip(|_, cx| cx.new(|_| crate::build_info::BuildTooltip).into())
                            .child(if width < 200.0 { crate::build_info::version() } else { format!("Desktop {}", crate::build_info::version()) }),
                    )
                    .child(
                        div()
                            .id("workspace-version-status")
                            .debug_selector(|| "workspace-version-status".into())
                            .truncate()
                            .when(action.is_some(), |el| el.text_color(Theme::global().ACCENT))
                            .tooltip(move |_, cx| {
                                cx.new(|_| live_tabs::TabTooltip(detail.clone().into()))
                                    .into()
                            })
                            .child(label),
                    ),
            )
            .when_some(action, |el, action| {
                el.child(
                    div()
                        .id("workspace-update-button")
                        .debug_selector(|| "workspace-update-button".into())
                        .flex_none()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(Theme::global().ACCENT.opacity(0.12))
                        .text_color(Theme::global().ACCENT)
                        .text_size(px(11.0))
                        .cursor_pointer()
                        .hover(|el| el.bg(Theme::global().ACCENT.opacity(0.22)))
                        .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                            cx.stop_propagation();
                            window.prevent_default();
                        })
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.stop_propagation();
                            if updates::request_now() == updates::UpdateRequest::Unavailable {
                                updates::set(UpdateState::Failed {
                                    message: "Automatic updates are unavailable in this build. Install the latest Desktop release manually.".into(),
                                });
                            }
                            cx.notify();
                        }))
                        .child(action),
                )
            })
            .into_any_element()
    }
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
            let (label, action) = status_label(&release, &UpdateState::Idle);
            assert_ne!(label, "Latest version");
            assert!(action.is_none());
        }
        assert_eq!(
            status_label(
                &ReleaseStatus::Current {
                    latest: "1.0.0".into()
                },
                &UpdateState::Idle
            ),
            ("Latest version".into(), None)
        );
        let newer = ReleaseStatus::Newer {
            version: "1.1.0".into(),
        };
        assert_eq!(status_label(&newer, &UpdateState::Idle).1, Some("Update"));
        assert_eq!(status_label(&newer, &UpdateState::Checking).1, None);
        assert_eq!(
            status_label(
                &newer,
                &UpdateState::ReadyToRestart {
                    version: "1.1.0".into()
                }
            )
            .1,
            Some("Restart")
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
        let button = vcx
            .debug_bounds("workspace-update-button")
            .expect("new release offers Update");
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
        updates::clear_test_actions();
        updates::set_release_status(ReleaseStatus::Source);
    }
}
