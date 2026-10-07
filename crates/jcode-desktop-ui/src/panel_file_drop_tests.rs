use super::*;
use gpui::{ExternalPaths, FileDropEvent};

/// A real platform drop: files enter the window over the panel, then land.
#[gpui::test]
fn files_dropped_from_the_file_manager_attach_images_and_reference_paths(
    cx: &mut gpui::TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("screenshot.png");
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(4, 3)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    std::fs::write(&image, png.into_inner()).unwrap();
    let notes = dir.path().join("design notes.md");
    std::fs::write(&notes, "# notes").unwrap();

    let (bridge, _commands) = crate::harness::spawn_recording();
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        let mut panel = Panel::new("drop-test".into(), None, None, bridge, cx);
        panel.history_loaded = true;
        panel
    });
    vcx.update(|window, cx| {
        Panel::connect_input(&panel, cx);
        let handle = panel.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    });
    vcx.simulate_input("look at");
    vcx.run_until_parked();

    let target = vcx.debug_bounds("prompt-input").unwrap().center();
    // Typing first, then dragging a file in, is the common real sequence. It
    // used to leave the window in keyboard modality, which ignored the drop.
    vcx.simulate_event(FileDropEvent::Entered {
        position: target,
        paths: ExternalPaths(vec![image.clone(), notes.clone()].into()),
    });
    vcx.simulate_event(FileDropEvent::Pending { position: target });
    vcx.run_until_parked();
    let dragging = vcx.update(|_, cx| cx.has_active_drag());
    assert!(dragging, "the platform drag is active over the panel");
    vcx.simulate_event(FileDropEvent::Submit { position: target });
    vcx.run_until_parked();

    panel.read_with(vcx, |panel, cx| {
        let input = panel.input.read(cx).snapshot();
        assert_eq!(input.attachments.len(), 1, "the PNG attaches like a paste");
        assert_eq!(input.attachments[0].media_type, "image/png");
        assert_eq!(input.attachments[0].label, "4×3");
        assert_eq!(
            input.content,
            format!("look at \"{}\" ", notes.display()),
            "other files are referenced by quoted path"
        );
    });
}
