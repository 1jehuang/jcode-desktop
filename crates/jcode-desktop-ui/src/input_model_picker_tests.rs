//! Regression coverage for cached, viewport-only model suggestions.
use super::*;

struct Fixture(Entity<PromptInput>);

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .pt(px(500.))
            .px(px(20.))
            .child(self.0.clone())
    }
}

fn routes(count: usize) -> Vec<jcode_sdk::ModelRouteInfo> {
    (0..count)
        .map(|index| jcode_sdk::ModelRouteInfo {
            model: format!("atlas-{index:04}"),
            provider: "OpenAI".into(),
            api_method: "openai-api-key".into(),
            available: true,
            detail: String::new(),
            usage: None,
        })
        .collect()
}

#[gpui::test]
fn model_suggestions_cache_invalidates_for_query_routes_current_and_groups(
    cx: &mut gpui::TestAppContext,
) {
    let input = cx.new(|cx| PromptInput::new(cx, "test", |_, _, _, _| {}));
    input.update(cx, |input, cx| {
        let mut routes = routes(8);
        input.set_model_routes(Vec::new(), &routes, None, cx);
        input.set_content("/model ".into(), cx);
        let collapsed = input.command_suggestions();
        assert_eq!(collapsed.len(), 4);
        assert!(Arc::ptr_eq(&collapsed, &input.command_suggestions()));

        // Changing only highlight must not rebuild the metadata vector.
        input.command_selection = 1;
        assert!(Arc::ptr_eq(&collapsed, &input.command_suggestions()));

        input.set_content("/model atlas-0007".into(), cx);
        let filtered = input.command_suggestions();
        assert!(!Arc::ptr_eq(&collapsed, &filtered));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].value, "/model openai-api:atlas-0007");

        routes[7].available = false;
        input.set_model_routes(Vec::new(), &routes, None, cx);
        let unavailable = input.command_suggestions();
        assert!(!Arc::ptr_eq(&filtered, &unavailable));
        assert!(unavailable.is_empty());

        input.set_content("/model ".into(), cx);
        let before_current = input.command_suggestions();
        input.set_current_model(Some("atlas-0006".into()), cx);
        let current = input.command_suggestions();
        assert!(!Arc::ptr_eq(&before_current, &current));
        assert_eq!(current[0].value, "/model openai-api:atlas-0006");
        assert_eq!(current[0].help, "Current");

        let group = current.last().unwrap().toggle.clone().unwrap();
        input.toggle_model_group(group.clone(), cx);
        let expanded = input.command_suggestions();
        assert!(!Arc::ptr_eq(&current, &expanded));
        assert_eq!(expanded.len(), 8); // Seven available models and disclosure.
        assert_eq!(expanded.last().unwrap().value, "Show fewer models");
        input.toggle_model_group(group, cx);
        let recollapsed = input.command_suggestions();
        assert!(!Arc::ptr_eq(&expanded, &recollapsed));
        assert_eq!(recollapsed.as_ref(), current.as_ref());
    });
}

#[gpui::test]
fn model_suggestions_cache_survives_real_hover_selection(cx: &mut gpui::TestAppContext) {
    let (fixture, vcx) = cx.add_window_view(|_, cx| {
        Fixture(cx.new(|cx| PromptInput::new(cx, "test", |_, _, _, _| {})))
    });
    let input = fixture.read_with(vcx, |fixture, _| fixture.0.clone());
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, size(px(1000.), px(800.)));
    input.update(vcx, |input, cx| {
        input.set_model_routes(Vec::new(), &routes(40), None, cx);
        input.set_content("/model atlas".into(), cx);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
    for (selector, selected) in [("slash-command-row-1", 1), ("slash-command-row-0", 0)] {
        let position = vcx.debug_bounds(selector).unwrap().center();
        assert!(
            vcx.debug_bounds("slash-command-scroll")
                .unwrap()
                .contains(&position)
        );
        // Compare in one synchronous update to avoid dependence on a frame's
        // elapsed time crossing the cache's relative-label minute boundary.
        vcx.update(|window, cx| {
            let before = input.read(cx).command_suggestions();
            window.simulate_mouse_move(position, cx);
            assert_eq!(input.read(cx).command_selection, selected);
            assert!(Arc::ptr_eq(&before, &input.read(cx).command_suggestions()));
            window.draw(cx).clear(cx);
            assert!(Arc::ptr_eq(&before, &input.read(cx).command_suggestions()));
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds(selector)
                .unwrap()
                .contains(&vcx.debug_bounds("slash-command-selected").unwrap().center())
        );
    }
}

#[gpui::test]
fn model_picker_virtualizes_thousand_routes_and_clicks_exact_last_route(
    cx: &mut gpui::TestAppContext,
) {
    let submitted = std::rc::Rc::new(RefCell::new(Vec::<String>::new()));
    let sink = submitted.clone();
    let (fixture, vcx) = cx.add_window_view(|_, cx| {
        Fixture(cx.new(|cx| {
            PromptInput::new(cx, "test", move |text, _, _, _| {
                sink.borrow_mut().push(text)
            })
        }))
    });
    let input = fixture.read_with(vcx, |fixture, _| fixture.0.clone());
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, size(px(1000.), px(800.)));
    let mut catalog = routes(1000);
    // The current longer name matches the final short name's query and is
    // ranked first. Clicking the short name must not submit that first match.
    catalog[998].model = "atlas-0999-long".into();
    input.update(vcx, |input, cx| {
        input.set_model_routes(Vec::new(), &catalog, Some("atlas-0999-long".into()), cx);
        input.set_content("/model atlas".into(), cx);
        let rows = input.command_suggestions();
        assert_eq!(rows.len(), 1000);
        assert_eq!(rows[0].value, "/model openai-api:atlas-0999-long");
        assert_eq!(rows[999].value, "/model openai-api:atlas-0999");
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
    // debug_bounds accepts static selectors. These test-only names are leaked
    // once for the process lifetime, never from production rendering.
    let selectors: Vec<&'static str> = (0..1000)
        .map(|index| {
            Box::leak(format!("slash-command-row-{index}").into_boxed_str()) as &'static str
        })
        .collect();
    let visible = selectors
        .iter()
        .filter(|selector| vcx.debug_bounds(selector).is_some())
        .count();
    assert!((2..50).contains(&visible), "painted {visible} of 1000 rows");
    assert!(vcx.debug_bounds("slash-command-row-999").is_none());

    input.update_in(vcx, |input, window, cx| {
        input.command_selection = 998;
        input.history_next(&HistoryNext, window, cx);
        assert_eq!(input.command_selection, 999);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
    let last = vcx
        .debug_bounds("slash-command-row-999")
        .expect("last route is reachable");
    assert!(
        vcx.debug_bounds("slash-command-scroll")
            .unwrap()
            .contains(&last.center())
    );
    assert!(vcx.debug_bounds("slash-command-row-0").is_none());
    let visible = selectors
        .iter()
        .filter(|selector| vcx.debug_bounds(selector).is_some())
        .count();
    assert!((1..50).contains(&visible), "painted {visible} rows at end");
    vcx.simulate_click(last.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        submitted.borrow().as_slice(),
        ["/model openai-api:atlas-0999"]
    );
    input.read_with(vcx, |input, _| {
        assert!(input.content.is_empty());
        assert_eq!(
            input.history.last().unwrap(),
            "/model openai-api:atlas-0999"
        );
    });
}
