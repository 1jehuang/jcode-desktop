//! End-to-end sizing checks through streamed events and the real virtualized
//! transcript. These must find the native canvas, not merely the fallback fence.

use gpui::{Bounds, Pixels, px};
use jcode_sdk::ApiEvent;

const IDEA_FLOWCHART: &str = "flowchart TD\n\
    A[Start with an idea] --> B[Make a plan] --> C[Build it] --> D{Does it work?}\n\
    D -->|Not yet| E[Improve it]\n\
    E --> C\n\
    D -->|Yes| F[Ship it]";

#[derive(Debug, Clone, Copy)]
struct DiagramBounds {
    frame: Bounds<Pixels>,
    canvas: Bounds<Pixels>,
}

fn assert_contains(outer: Bounds<Pixels>, inner: Bounds<Pixels>, description: &str) {
    let tolerance = px(1.);
    assert!(
        inner.left() >= outer.left() - tolerance
            && inner.right() <= outer.right() + tolerance
            && inner.top() >= outer.top() - tolerance
            && inner.bottom() <= outer.bottom() + tolerance,
        "{description}: outer={outer:?}, inner={inner:?}"
    );
}

fn stream_and_resize(
    cx: &mut gpui::TestAppContext,
    source: &str,
    widths: &[f32],
) -> Vec<DiagramBounds> {
    cx.update(crate::input::bind_keys);
    let (bridge, _commands) = crate::harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.push_test_panel("session-mermaid-sizing", cx);
        workspace
    });
    let panel = workspace
        .read_with(vcx, |workspace, _| workspace.test_panel(0))
        .expect("test transcript exists");
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, gpui::size(px(widths[0]), px(1400.)));

    // Separate deltas exercise the production streaming markdown path, including
    // a fence which is initially incomplete, rather than constructing an element.
    for text in ["```mermaid\n", source, "\n```"] {
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    session_id: "session-mermaid-sizing".into(),
                    text: text.into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();
    }

    let (natural_width, natural_height) = crate::markdown::mermaid_natural_size(source);
    widths
        .iter()
        .map(|&width| {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(1400.)));
            vcx.run_until_parked();
            let frame = vcx
                .debug_bounds("md-mermaid")
                .expect("the streamed fence paints in the transcript");
            let canvas = vcx
                .debug_bounds("md-mermaid-native")
                .expect("Mermaid paints a native canvas, not a source fallback or image");
            let response = vcx
                .debug_bounds("assistant-response")
                .expect("the assistant response remains in the transcript");
            assert!(canvas.size.width > px(0.) && canvas.size.height > px(0.));
            assert!(
                frame.size.width <= px(width + 1.),
                "Mermaid must not force the transcript wider than its viewport: {frame:?} at {width}"
            );
            assert_contains(frame, canvas, "all native content fits the Mermaid frame");
            assert_contains(response, frame, "Mermaid fits its assistant response");

            // Independent layout oracle: shrink for available width or the chat
            // height ceiling, but never stretch a naturally small diagram.
            let scale = (f32::from(frame.size.width) / natural_width)
                .min(520. / natural_height)
                .min(1.);
            assert!(
                (canvas.size.width - px(natural_width * scale)).abs() <= px(1.),
                "unexpected canvas width at window {width}: {canvas:?}, natural={natural_width}x{natural_height}"
            );
            assert!(
                (canvas.size.height - px(natural_height * scale)).abs() <= px(1.),
                "aspect ratio or height changed at window {width}: {canvas:?}, natural={natural_width}x{natural_height}"
            );
            assert!(canvas.size.width <= px(natural_width + 1.));
            assert!(canvas.size.height <= px(natural_height.min(520.) + 1.));
            assert!(
                (frame.size.height - canvas.size.height).abs() <= px(1.),
                "the transcript must reserve content height, not a forced giant display: {frame:?} / {canvas:?}"
            );
            eprintln!(
                "native Mermaid at window {width}: natural={natural_width}x{natural_height}, frame={:?}, canvas={:?}",
                frame.size, canvas.size
            );
            DiagramBounds { frame, canvas }
        })
        .collect()
}

#[gpui::test]
fn native_mermaid_original_flowchart_fits_narrow_and_wide_transcripts(
    cx: &mut gpui::TestAppContext,
) {
    let sizes = stream_and_resize(cx, IDEA_FLOWCHART, &[480., 1600., 640., 480.]);
    assert!(
        sizes[1].frame.size.width > sizes[0].frame.size.width + px(100.),
        "the test must exercise genuinely different transcript widths: {sizes:?}"
    );
    assert_eq!(
        sizes[0].canvas.size, sizes[3].canvas.size,
        "resizing back must restore the original canvas size without stale layout"
    );
}

#[gpui::test]
fn native_mermaid_small_diagram_keeps_natural_size_without_giant_frame(
    cx: &mut gpui::TestAppContext,
) {
    let source = "flowchart TD\nA[Hi]";
    let sizes = stream_and_resize(cx, source, &[800., 1600., 1000.]);
    let (natural_width, natural_height) = crate::markdown::mermaid_natural_size(source);
    assert!(natural_height < 300., "fixture must be naturally compact");
    for size in sizes {
        assert!(size.frame.size.width > px(natural_width + 10.));
        assert!((size.canvas.size.width - px(natural_width)).abs() <= px(1.));
        assert!((size.canvas.size.height - px(natural_height)).abs() <= px(1.));
        assert!(
            size.frame.size.height < px(300.),
            "small diagram must not become a 520px card"
        );
    }
}

#[gpui::test]
fn native_mermaid_wide_diagram_scales_both_axes_with_transcript_width(
    cx: &mut gpui::TestAppContext,
) {
    let sizes = stream_and_resize(
        cx,
        "flowchart LR\nA[Start with an idea] --> B[Make a plan] --> C[Build it] --> D[Verify the result] --> E[Ship it]",
        &[480., 1600., 480.],
    );
    assert!(sizes[1].canvas.size.width > sizes[0].canvas.size.width + px(100.));
    assert!(sizes[1].canvas.size.height > sizes[0].canvas.size.height);
    assert_eq!(sizes[0].canvas.size, sizes[2].canvas.size);
}
