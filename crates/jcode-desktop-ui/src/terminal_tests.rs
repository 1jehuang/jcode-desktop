use super::*;
use gpui::{AppContext, Keystroke};
use handterm_common::grid::{ATTR_BOLD, COLOR_FLAG_RGB};

#[test]
fn color_queries_follow_desktop_theme_and_changes() {
    let mut terminal = Terminal::new(80, 24);
    let mut theme = Theme::global().clone();
    for (foreground, background) in [(0x292724, 0xeeeae4), (0xe4ddd3, 0x1c1a18)] {
        theme.TEXT = gpui::rgb(foreground);
        theme.BG = gpui::rgb(background);
        sync_terminal_colors(&mut terminal, &theme);
        // Queries can cross PTY read boundaries, with either terminator.
        terminal.process(b"\x1b]10;?");
        terminal.process(b"\x07\x1b]11;?\x1b");
        terminal.process(b"\\");
        let expected = [(10, theme.TEXT), (11, theme.BG)]
            .into_iter()
            .map(|(command, color)| {
                let [r, g, b] = color_channels(color);
                format!("\x1b]{command};rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\")
            })
            .collect::<String>();
        assert_eq!(terminal.drain_responses().unwrap(), expected.as_bytes());
    }
}

#[test]
fn keys_follow_engine_modes_and_preserve_workspace_shortcuts() {
    let mut terminal = Terminal::new(80, 24);
    let up = Keystroke::parse("up").unwrap();
    assert_eq!(
        keys::key_bytes(&up, &terminal, false, false),
        Some(b"\x1b[A".to_vec())
    );
    terminal.process(b"\x1b[?1h");
    assert_eq!(
        keys::key_bytes(&up, &terminal, false, false),
        Some(b"\x1bOA".to_vec())
    );
    assert_eq!(
        keys::key_bytes(
            &Keystroke::parse("ctrl-c").unwrap(),
            &terminal,
            false,
            false
        ),
        Some(vec![3])
    );
    for key in [
        "super-t",
        "super-w",
        "ctrl-r",
        "ctrl-shift-c",
        "ctrl-shift-v",
        "shift-insert",
    ] {
        assert!(
            keys::key_bytes(&Keystroke::parse(key).unwrap(), &terminal, false, false).is_none(),
            "{key}"
        );
    }
    terminal.process(b"\x1b[>3u");
    assert_eq!(
        keys::key_bytes(
            &Keystroke::parse("escape").unwrap(),
            &terminal,
            false,
            false
        ),
        Some(b"\x1b[27u".to_vec())
    );
    assert!(
        keys::key_bytes(&Keystroke::parse("escape").unwrap(), &terminal, true, false)
            .unwrap()
            .ends_with(b":3u")
    );
}

#[test]
fn printable_text_uses_ime_without_duplicate_raw_input() {
    let terminal = Terminal::new(80, 24);
    let mut key = Keystroke::parse("a").unwrap();
    key.key_char = Some("a".into());
    assert!(keys::key_bytes(&key, &terminal, false, false).is_none());
    key.key_char = None;
    assert_eq!(
        keys::key_bytes(&key, &terminal, false, false),
        Some(b"a".to_vec())
    );
}

#[test]
fn bracketed_paste_preserves_newlines_and_blocks_escape_injection() {
    assert_eq!(
        keys::paste_bytes("a\r\nb\rc", true),
        b"\x1b[200~a\nb\nc\x1b[201~"
    );
    assert_eq!(keys::paste_bytes("a\r\nb", false), b"a\rb");
    let paste = keys::paste_bytes("\x1b[201~\nrm example", true);
    assert_eq!(paste.windows(6).filter(|s| *s == b"\x1b[201~").count(), 1);
}

#[test]
fn legacy_host_filter_keeps_cursor_reports_and_image_acknowledgments() {
    let responses = b"\x1b[?1;2c\x1b[?0u\x1bP>|handterm\x1b\\\x1b]11;rgb:0000/0000/0000\x1b\\\x1b[5;9R\x1b_Gi=7;OK\x1b\\\x1b[>1;0;0c";
    assert_eq!(
        keys::without_legacy_host_replies(responses),
        b"\x1b[5;9R\x1b_Gi=7;OK\x1b\\\x1b[>1;0;0c"
    );
}

#[test]
fn engine_cells_keep_styles_wide_unicode_and_image_data() {
    let mut terminal = Terminal::new(80, 24);
    terminal.process("\x1b[1;38;2;17;34;51mHi\x1b[0m 界".as_bytes());
    let cell = terminal.grid.cell_at(0, 0);
    assert_eq!(cell.fg, COLOR_FLAG_RGB | 0x112233);
    assert_ne!(cell.attrs & ATTR_BOLD, 0);
    assert!(terminal.grid.get_text(0, 1).contains("Hi 界"));
    terminal.process(b"\x1b_Ga=T,f=32,s=1,v=1,i=7,c=4,r=2;/wAA/w==\x1b\\");
    let image = terminal.kitty_image(7).unwrap();
    assert_eq!(image.data, [255, 0, 0, 255]);
    assert_eq!(
        paint::bgra_image(image).unwrap().as_raw(),
        &[0, 0, 255, 255]
    );
    assert_eq!(terminal.kitty_placements().len(), 1);
    terminal.process(b"\x1b_Ga=d,d=I,i=7\x1b\\");
    assert!(terminal.kitty_image(7).is_none());
}

#[gpui::test]
fn panel_replay_sets_theme_colors_without_sending_historical_replies(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        let panel = cx.new(|cx| TerminalPanel::new(None, None, None, HostHandle::inert(), cx));
        panel.update(cx, |panel, _| {
            panel.process_output(b"\x1b]11;?\x07", false);
            assert!(panel.terminal.drain_responses().is_none());
            panel.terminal.process(b"\x1b]11;?\x07");
            let [r, g, b] = color_channels(Theme::global().BG);
            assert_eq!(
                panel.terminal.drain_responses().unwrap(),
                format!("\x1b]11;rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\").as_bytes()
            );
        });
    });
}

#[gpui::test]
fn panel_renders_cells_and_keeps_image_texture_identity(cx: &mut gpui::TestAppContext) {
    cx.update(bind_keys);
    let (panel, vcx) =
        cx.add_window_view(|_, cx| TerminalPanel::new(None, None, None, HostHandle::inert(), cx));
    vcx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.status.clear();
            panel.focus(window, cx);
            panel.process_output(
                b"\x1b[1;31mred\x1b[0m\r\n\x1b_Ga=T,f=32,s=1,v=1,i=7,c=4,r=2;/wAA/w==\x1b\\",
                false,
            );
            cx.notify();
        })
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("plain-terminal").is_some());
    let id = panel.read_with(vcx, |panel, _| {
        assert!(panel.screen_contents().contains("red"));
        assert_eq!(panel.images.len(), 1);
        panel.images[&7].image.id
    });
    vcx.update(|_, cx| panel.update(cx, |_, cx| cx.notify()));
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, _| assert_eq!(panel.images[&7].image.id, id));
    vcx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.process_output(b"\x1b_Ga=d,d=I,i=7\x1b\\", false);
            cx.notify();
        })
    });
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, _| assert!(panel.images.is_empty()));
}

#[gpui::test]
fn panel_ime_commit_and_replay_drain_do_not_accumulate_protocol_events(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        let panel = cx.new(|cx| TerminalPanel::new(None, None, None, HostHandle::inert(), cx));
        panel.update(cx, |panel, _| {
            panel.process_output(b"\x1b[6n\x1b]2;title\x07", false);
            assert!(panel.terminal.drain_responses().is_none());
            assert!(panel.terminal.drain_osc().is_empty());
            panel.ime_key_text = Some(("a".into(), std::time::Instant::now()));
            panel.send_text("a");
            assert!(panel.ime_key_text.is_none());
        });
    });
}

#[gpui::test]
fn native_mouse_selection_copy_and_wheel_scroll(cx: &mut gpui::TestAppContext) {
    cx.update(bind_keys);
    let (panel, vcx) =
        cx.add_window_view(|_, cx| TerminalPanel::new(None, None, None, HostHandle::inert(), cx));
    vcx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.status.clear();
            panel.process_output(b"hello world", false);
            cx.notify();
        })
    });
    vcx.run_until_parked();
    let (start, end) = panel.read_with(vcx, |panel, _| {
        let start = panel.bounds.origin + gpui::point(panel.cell_width * 0.5, px(9.));
        (start, start + gpui::point(panel.cell_width * 4., px(0.)))
    });
    vcx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    vcx.simulate_mouse_move(end, MouseButton::Left, Default::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Default::default());
    panel.read_with(vcx, |panel, _| {
        assert_eq!(panel.terminal.grid.get_selection_text(), "hello")
    });
    vcx.simulate_keystrokes("ctrl-shift-c");
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("hello")
        )
    });
    vcx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.process_output("\r\nscrollback line".repeat(150).as_bytes(), false);
            cx.notify();
        })
    });
    vcx.run_until_parked();
    vcx.simulate_event(ScrollWheelEvent {
        position: start,
        delta: gpui::ScrollDelta::Lines(gpui::point(0., 3.)),
        ..Default::default()
    });
    panel.read_with(vcx, |panel, _| {
        assert_eq!(panel.terminal.grid.scroll_offset, 3);
        assert!(panel.terminal.grid.selection.is_none());
    });
}
