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

/// Real GPUI dispatch plus completed frames, without GPU/compositor presentation.
#[gpui::test]
#[ignore = "manual model selector CPU frame profile"]
fn model_selector_frame_profile(cx: &mut gpui::TestAppContext) {
    let (fixture, vcx) = cx.add_window_view(|_, cx| {
        Fixture(cx.new(|cx| PromptInput::new(cx, "test", |_, _, _, _| {})))
    });
    let input = fixture.read_with(vcx, |fixture, _| fixture.0.clone());
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, size(px(1000.), px(800.)));
    for count in [40, 200, 1000] {
        let routes: Vec<_> = (0..count)
            .map(|i| jcode_sdk::ModelRouteInfo {
                model: format!("atlas-{i:04}"),
                provider: "OpenAI".into(),
                api_method: "openai-api-key".into(),
                available: true,
                detail: String::new(),
                usage: None,
            })
            .collect();
        for search in [false, true] {
            input.update(vcx, |input, cx| {
                input.set_model_routes(
                    Vec::new(),
                    &routes,
                    Some(format!("atlas-{:04}", count - 1)),
                    cx,
                );
                input.set_content(if search { "/model atlas" } else { "/model " }.into(), cx);
            });
            vcx.run_until_parked();
            let positions = [
                vcx.debug_bounds("slash-command-row-0").unwrap().center(),
                vcx.debug_bounds("slash-command-row-1").unwrap().center(),
            ];
            let scroll = vcx.debug_bounds("slash-command-scroll").unwrap();
            assert!(positions.iter().all(|p| scroll.contains(p)));
            let mut samples = Vec::new();
            for i in 0..60 {
                let start = Instant::now();
                vcx.update(|window, cx| window.simulate_mouse_move(positions[i % 2], cx));
                vcx.run_until_parked();
                vcx.update(|window, cx| window.draw(cx).clear(cx));
                vcx.run_until_parked();
                input.read_with(vcx, |input, _| assert_eq!(input.command_selection, i % 2));
                if i >= 10 {
                    samples.push(start.elapsed().as_micros());
                }
            }
            samples.sort_unstable();
            println!(
                "MODEL_HOVER count={count} search={search} p50={}us p95={}us",
                samples[24], samples[47]
            );
        }
    }
}
