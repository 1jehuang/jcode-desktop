//! Per-frame cost of keyboard navigation between panels.
//!
//! `loaded_animation_first_frame_profile` stops at the first frame after a
//! key. Navigation is a 150 ms camera pan, so what the user sees is every
//! frame of that pan. This drives the pan frame by frame against panels that
//! hold real conversations and reports what each frame cost and how much of
//! the window GPUI had to build again.
use super::*;

const PANELS: usize = 6;

fn conversation(panel: usize) -> Vec<crate::panel::Item> {
    use crate::panel::Item;
    (0..120)
        .map(|n| match n % 4 {
            0 => Item::User(format!("Panel {panel} request {n}: please look into the frame pacing")),
            1 => Item::Reasoning(format!("Thinking about step {n} in some detail before acting.")),
            2 => Item::Tool {
                call_id: format!("call-{panel}-{n}"),
                name: "bash".into(),
                input: format!("{{\"command\":\"justrust test -p crate-{n}\"}}"),
                output: "ok".into(),
                done: true,
                error: None,
            },
            _ => Item::Assistant(format!(
                "Result {n}: **formatted** text with `inline code` and a list.\n\n- first point about layout\n- second point about paint\n\n```rust\nfn frame_{n}() -> u32 {{ {n} }}\n```\n\nA closing paragraph long enough to wrap across more than one line in a half-width panel."
            )),
        })
        .collect()
}

fn loaded_workspace(
    cx: &mut gpui::TestAppContext,
) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
    cx.update(|cx| crate::bind_workspace_keys(cx));
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        for column in 0..PANELS {
            workspace.push_test_panel(&format!("nav-{column}"), cx);
            let items = conversation(column);
            workspace.slots[column].panel.update(cx, |panel, cx| {
                panel.test_settled_conversation(0, cx);
                *panel.items = items;
            });
        }
        workspace.active = 0;
        workspace
    });
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, gpui::size(px(1440.), px(900.)));
    vcx.update(|window, cx| window.focus(&workspace.read(cx).focus_handle.clone(), cx));
    vcx.run_until_parked();
    (workspace, vcx)
}

struct Frame {
    ms: f64,
    stats: gpui::LayoutStats,
}

fn sidebar_renders() -> u64 {
    crate::render_stats::snapshot()
        .into_iter()
        .find(|(name, _)| *name == "SidebarChrome")
        .map_or(0, |(_, stat)| stat.renders)
}

#[gpui::test]
fn sidebar_retention_tracks_models_not_camera_notifications(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = loaded_workspace(cx);
    let before = sidebar_renders();
    assert!(before > 0, "the real sidebar must have been rendered");
    // This is exactly what request_animation_frame does: notify the view by
    // id without updating its model. The camera is sampled during render.
    for _ in 0..3 {
        vcx.update(|_, cx| cx.notify(workspace.entity_id()));
        vcx.run_until_parked();
    }
    assert_eq!(
        sidebar_renders(),
        before,
        "camera-only frames rebuilt the sidebar"
    );

    workspace.update(vcx, |workspace, cx| {
        workspace.active = 1;
        cx.notify();
    });
    vcx.run_until_parked();
    let after_focus = sidebar_renders();
    assert!(
        after_focus > before,
        "focus changes must refresh sidebar selection"
    );

    let panel = workspace.read_with(vcx, |workspace, _| workspace.slots[1].panel.clone());
    panel.update(vcx, |panel, cx| {
        panel.session_id = "updated-sidebar-session".into();
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(
        sidebar_renders() > after_focus,
        "panel model changes must refresh sidebar metadata"
    );
}

#[gpui::test]
fn sidebar_retains_list_for_streaming_text_but_refreshes_metadata(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = loaded_workspace(cx);
    let panel = workspace.read_with(vcx, |workspace, _| workspace.slots[0].panel.clone());
    panel.update(vcx, |panel, cx| {
        panel.status = "running".into();
        panel.apply(
            &jcode_sdk::ApiEvent::TextDelta {
                session_id: panel.session_id.clone(),
                text: "first chunk".into(),
                message_id: None,
            },
            cx,
        );
        cx.notify();
    });
    vcx.run_until_parked();
    let before = sidebar_renders();
    let metadata_identity = workspace.read_with(vcx, |workspace, cx| {
        workspace.sidebar_panel_metadata.as_ref().unwrap().read(cx)[&panel.entity_id()]
            .session_id
            .as_ptr() as usize
    });
    for _ in 0..5 {
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &jcode_sdk::ApiEvent::TextDelta {
                    session_id: panel.session_id.clone(),
                    text: " more text".into(),
                    message_id: None,
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, cx| {
            assert_eq!(
                workspace.sidebar_panel_metadata.as_ref().unwrap().read(cx)[&panel.entity_id()]
                    .session_id
                    .as_ptr() as usize,
                metadata_identity,
                "unchanged metadata was replaced"
            );
        });
    }
    assert_eq!(
        sidebar_renders(),
        before,
        "text-only deltas rebuilt the session list"
    );
    panel.update(vcx, |panel, cx| {
        panel.title = "Updated live title".into();
        cx.notify();
    });
    vcx.run_until_parked();
    let after_title = sidebar_renders();
    assert!(
        after_title > before,
        "live title changes must invalidate sidebar metadata"
    );
    panel.update(vcx, |panel, cx| {
        panel.items.push(crate::panel::Item::Todos(
            serde_json::from_value(
                serde_json::json!({"todos": [{"content": "test task", "status": "pending"}]}),
            )
            .expect("valid sidebar todo fixture"),
        ));
        cx.notify();
    });
    vcx.run_until_parked();
    let after_todo = sidebar_renders();
    assert!(
        after_todo > after_title,
        "todo progress must refresh sidebar metadata"
    );
    panel.update(vcx, |panel, cx| {
        panel.working_dir = Some("/tmp/sidebar-metadata-test".into());
        cx.notify();
    });
    vcx.run_until_parked();
    let after_directory = sidebar_renders();
    assert!(
        after_directory > after_todo,
        "working directory must refresh sidebar grouping"
    );
    panel.update(vcx, |panel, cx| {
        panel.apply(
            &jcode_sdk::ApiEvent::TurnDone {
                session_id: panel.session_id.clone(),
                pending_soft_interrupts: None,
            },
            cx,
        );
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(
        sidebar_renders() > after_directory,
        "activity completion must refresh the sidebar"
    );
}

/// Press `key`, then deliver one animation frame every `step` until the pan
/// settles. Each sample is one frame: the key's own, then every pan frame.
fn pan_frames(
    workspace: &Entity<Workspace>,
    vcx: &mut gpui::VisualTestContext,
    key: &str,
    step: Duration,
) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut first = true;
    loop {
        vcx.update(|window, _| window.reset_layout_stats());
        let started = Instant::now();
        if std::mem::take(&mut first) {
            vcx.simulate_keystrokes(key);
        } else {
            // What the platform's frame callback does for a window that asked
            // for an animation frame.
            vcx.update(|window, cx| window.simulate_next_frame(cx));
        }
        vcx.run_until_parked();
        frames.push(Frame {
            ms: started.elapsed().as_secs_f64() * 1_000.0,
            stats: vcx.update(|window, _| window.layout_stats()),
        });
        let settled = workspace.read_with(vcx, |workspace, _| {
            workspace.camera_started[workspace.active_row].is_none()
        });
        assert!(
            frames.len() <= 200,
            "camera pan did not settle after 200 frames"
        );
        if settled {
            return frames;
        }
        std::thread::sleep(step);
    }
}

fn report(name: &str, frames: &[Frame]) {
    let mut ms: Vec<f64> = frames.iter().map(|frame| frame.ms).collect();
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q) as usize];
    let total = |f: fn(&gpui::LayoutStats) -> u64| frames.iter().map(|x| f(&x.stats)).sum::<u64>();
    let time = |f: fn(&gpui::LayoutStats) -> Duration| {
        frames
            .iter()
            .map(|x| f(&x.stats).as_secs_f64() * 1e3)
            .sum::<f64>()
            / frames.len() as f64
    };
    println!(
        "NAV {name:>12} frames={} p50={:.2}ms p95={:.2}ms max={:.2}ms over8.3={} | per frame: build={:.2} prepaint={:.2} paint={:.2} layout={:.2}ms views_built={:.1} reused={:.1} shaped={:.1} style_writes={:.1} measure={:.1}",
        frames.len(),
        at(0.5),
        at(0.95),
        at(1.0),
        ms.iter().filter(|ms| **ms > 8.33).count(),
        time(|s| s.build_time),
        time(|s| s.prepaint_time),
        time(|s| s.paint_time),
        time(|s| s.compute_layout_time),
        total(|s| s.views_built) as f64 / frames.len() as f64,
        total(|s| s.views_reused) as f64 / frames.len() as f64,
        total(|s| s.lines_shaped) as f64 / frames.len() as f64,
        total(|s| s.style_writes) as f64 / frames.len() as f64,
        total(|s| s.measure_calls) as f64 / frames.len() as f64,
    );
}

/// `justrust test --release -p jcode-desktop-ui navigation_frame_profile -- --ignored --nocapture`
#[gpui::test]
#[ignore = "manual navigation frame profiler"]
fn navigation_frame_profile(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = loaded_workspace(cx);
    let step = Duration::from_millis(8);
    // Warm caches both ways before measuring.
    for key in ["super-l", "super-l", "super-h", "super-h"] {
        pan_frames(&workspace, vcx, key, step);
    }
    let mut horizontal = Vec::new();
    for _ in 0..4 {
        for key in [
            "super-l", "super-l", "super-l", "super-h", "super-h", "super-h",
        ] {
            horizontal.extend(pan_frames(&workspace, vcx, key, step));
        }
    }
    report("horizontal", &horizontal);
    if std::env::var("NAV_PROFILE_FRAMES").is_ok() {
        for frame in &horizontal[..horizontal.len().min(40)] {
            println!("  {:.2}ms {:?}", frame.ms, frame.stats);
        }
    }
}
