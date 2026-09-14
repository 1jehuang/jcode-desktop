use super::*;
use gpui::EntityInputHandler;

#[gpui::test]
fn footer_model_click_preserves_draft_and_selects_without_sending(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new("footer-test".into(), None, None, bridge, cx));
    vcx.update(|_, cx| Panel::connect_input(&panel, cx));
    panel.update(vcx, |panel, cx| {
        panel.model = Some("openai:test".into());
        panel.provider = Some("openai".into());
        panel.auth_method = Some("oauth".into());
        panel.available_models = vec!["openai:test".into(), "anthropic:test".into()];
        panel.input.update(cx, |input, cx| {
            input.set_content("keep my draft".into(), cx)
        });
        cx.notify();
    });
    vcx.run_until_parked();
    let model = vcx.debug_bounds("panel-model").unwrap();
    let account = vcx.debug_bounds("panel-login").unwrap();
    assert!(
        model.right() <= account.left(),
        "account method follows model"
    );
    vcx.simulate_click(model.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("recovery-model-picker").is_some());
    assert!(commands.try_recv().is_err());
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("recovery-model-picker").is_none());
    let model = vcx.debug_bounds("panel-model").unwrap();
    vcx.simulate_click(model.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    let choice = vcx.debug_bounds("recovery-model-picker-1").unwrap();
    vcx.simulate_click(choice.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::SetModel { model, .. }) if model == "anthropic:test")
    );
    assert!(commands.try_recv().is_err());
    assert!(vcx.debug_bounds("recovery-model-picker").is_none());
    panel.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "keep my draft")
    });
    panel.update(vcx, |panel, cx| {
        panel.available_models.clear();
        cx.notify();
    });
    vcx.run_until_parked();
    let model = vcx.debug_bounds("panel-model").unwrap();
    vcx.simulate_click(model.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("recovery-connect-account").is_some());
    assert!(vcx.debug_bounds("recovery-refresh-models").is_some());
}

#[gpui::test]
fn narrow_short_panels_keep_composer_and_footer_controls_visible(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (bridge, _commands) = crate::harness::spawn_recording();
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new(
            "responsive-test".into(),
            None,
            Some("/a/very/long/project/path".into()),
            bridge,
            cx,
        )
    });
    let handle = vcx.update(|window, _| window.window_handle());
    for (width, height) in [(240., 240.), (320., 300.), (480., 360.), (800., 600.)] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        for fresh in [true, false] {
            panel.update(vcx, |panel, cx| {
                panel.startup_layout = None;
                panel.model = (!fresh).then(|| "a-very-long-model-name".repeat(4));
                panel.provider = (!fresh).then(|| "openai".into());
                panel.auth_method = (!fresh).then(|| "oauth".into());
                panel.reasoning_effort = (!fresh).then(|| "high".into());
                panel.items = if fresh {
                    vec![]
                } else {
                    vec![Item::User("Hello".into())]
                };
                panel
                    .input
                    .update(cx, |input, cx| input.set_content("".into(), cx));
                cx.notify();
            });
            vcx.run_until_parked();
            let input = vcx.debug_bounds("prompt-input").unwrap();
            let footer = vcx.debug_bounds("panel-meta").unwrap();
            assert!(
                input.top() >= px(0.) && input.bottom() <= px(height),
                "{width}x{height} fresh={fresh}: {input:?}"
            );
            assert!(input.left() >= px(0.) && input.right() <= px(width));
            assert!(footer.bottom() <= px(height));
            for selector in [
                "panel-model",
                "panel-login",
                "panel-build",
                "panel-status-badge",
            ] {
                let bounds = vcx.debug_bounds(selector).unwrap();
                assert!(
                    bounds.left() >= footer.left() && bounds.right() <= footer.right(),
                    "{selector} at {width}: {bounds:?} outside {footer:?}"
                );
                assert!(bounds.size.width > px(20.), "{selector} is squeezed away");
            }
            let identity = vcx.debug_bounds("panel-identity").unwrap();
            let model = vcx.debug_bounds("panel-model").unwrap();
            let login = vcx.debug_bounds("panel-login").unwrap();
            assert!(model.left() >= identity.left() && login.right() <= identity.right());
            assert!(model.right() <= login.left(), "identity controls must not overlap");
            assert!(login.right() <= vcx.debug_bounds("panel-build").unwrap().left());
            panel.update(vcx, |panel, cx| {
                panel.input.update(cx, |input, cx| {
                    input.set_content("long draft ".repeat(300).into(), cx)
                });
            });
            vcx.run_until_parked();
            let input = vcx.debug_bounds("prompt-input").unwrap();
            assert!(
                input.top() >= px(0.) && input.bottom() <= px(height),
                "long draft at {width}x{height}: {input:?}"
            );
            assert!(input.left() >= px(0.) && input.right() <= px(width));
            let editor = vcx.debug_bounds("prompt-editor").unwrap();
            let before = vcx
                .update(|window, cx| {
                    let input = panel.read(cx).input.clone();
                    input.update(cx, |input, cx| {
                        input.character_index_for_point(editor.center(), window, cx)
                    })
                })
                .unwrap();
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: editor.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.), px(40.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Started,
            });
            vcx.run_until_parked();
            let after = vcx
                .update(|window, cx| {
                    let input = panel.read(cx).input.clone();
                    input.update(cx, |input, cx| {
                        input.character_index_for_point(editor.center(), window, cx)
                    })
                })
                .unwrap();
            assert!(
                after < before,
                "embedded editor should scroll, fresh={fresh}"
            );
        }
    }
}
