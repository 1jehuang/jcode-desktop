//! The highlighted panel must always be the one that receives typing.
use super::*;

fn two_panels(cx: &mut gpui::TestAppContext) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.show_sidebar = false;
        for name in ["first", "second"] {
            workspace.push_test_panel(name, cx);
        }
        for slot in &mut workspace.slots {
            slot.width_fraction = 0.5;
            slot.animated_width =
                AnimatedValue::new(0.5, transition::policy(Transition::PanelWidth).duration);
        }
        workspace.active = 0;
        workspace
    });
    vcx.update(|window, cx| {
        workspace
            .read(cx)
            .test_panel(0)
            .unwrap()
            .read(cx)
            .input_focus_handle(cx)
            .focus(window, cx)
    });
    vcx.run_until_parked();
    (workspace, vcx)
}

#[gpui::test]
fn keyboard_focus_entering_another_panel_activates_it(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = two_panels(cx);
    let second = workspace.read_with(vcx, |w, _| w.test_panel(1).unwrap());
    // Many paths focus a composer directly: panel buttons, restored drafts,
    // voice, and pickers. None of them go through `set_active`.
    vcx.update(|window, cx| second.read(cx).input_focus_handle(cx).focus(window, cx));
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert_eq!(
            w.active, 1,
            "the panel that takes typing must be highlighted"
        );
    });
    vcx.simulate_input("hello");
    vcx.run_until_parked();
    second.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "hello");
    });
}

#[gpui::test]
fn pressing_in_another_composer_and_releasing_elsewhere_activates_it(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = two_panels(cx);
    let first = vcx.debug_bounds("panel-0").unwrap();
    let second = vcx.debug_bounds("panel-1").unwrap();
    let composer = gpui::point(second.center().x, second.bottom() - px(40.));
    vcx.simulate_mouse_down(composer, gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_up(first.center(), gpui::MouseButton::Left, Default::default());
    vcx.run_until_parked();
    let focused = workspace.read_with(vcx, |w, _| w.test_panel(1).unwrap());
    let typing_in_second =
        vcx.update(|window, cx| focused.read(cx).input_focus_handle(cx).is_focused(window));
    workspace.read_with(vcx, |w, _| {
        assert_eq!(
            typing_in_second,
            w.active == 1,
            "highlight and typing target disagree"
        );
    });
}

#[gpui::test]
fn activating_another_panel_moves_typing_to_it(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = two_panels(cx);
    // Sidebar rows, tabs and commands activate a panel without touching focus.
    workspace.update(vcx, |w, cx| w.set_active(1, cx));
    vcx.run_until_parked();
    vcx.simulate_input("hello");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.active, 1);
        let text = |index| {
            w.test_panel(index)
                .unwrap()
                .read(cx)
                .input
                .read(cx)
                .content
                .clone()
        };
        assert_eq!(text(1).as_ref(), "hello");
        assert_eq!(
            text(0).as_ref(),
            "",
            "the outgoing session must not receive the prompt"
        );
    });
}
