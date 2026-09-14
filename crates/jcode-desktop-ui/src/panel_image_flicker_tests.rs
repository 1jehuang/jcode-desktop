//! Image keys and fixed-height cards must not flicker when the transcript's
//! streaming ancestor changes. These tests use the real Panel and image loader.
use super::*;
use std::{cell::RefCell, rc::Rc};

fn panel_image(panel: &Panel) -> Arc<gpui::Image> {
    panel
        .items
        .iter()
        .find_map(|item| match item {
            Item::Image(image) => image.preview.clone(),
            _ => None,
        })
        .expect("fixture image exists")
}

struct ImageWitness {
    panel: Entity<Panel>,
    decoded: Rc<RefCell<Option<Arc<gpui::RenderImage>>>>,
}

impl gpui::Render for ImageWitness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Observe the same production decoder/cache used by the Panel image.
        // The callback runs during paint, where current_view() is valid. This
        // checks the decoded asset, not physical pixels presented by the GPU.
        let gpui::ImageSource::Custom(load) =
            crate::image_cache::source(panel_image(self.panel.read(cx)))
        else {
            unreachable!("desktop images use their shared custom decoder")
        };
        let decoded = self.decoded.clone();
        div()
            .relative()
            .size_full()
            .child(self.panel.clone())
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        *decoded.borrow_mut() = load(window, cx)
                            .map(|result| result.expect("valid fixture PNG decodes"));
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

async fn yield_once() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await;
}

#[gpui::test]
async fn encoded_image_asset_and_geometry_survive_streaming_and_history_reconstruction(
    cx: &mut gpui::TestAppContext,
) {
    for (width, height) in [(640., 480.), (900., 600.)] {
        for (image_width, image_height) in [(64, 256), (256, 64)] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(
                image_width,
                image_height,
                image::Rgba([240, 20, 60, 255]),
            )
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
            let rendered = jcode_sdk::RenderedImage {
                history_message_index: None,
                media_type: "image/png".into(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes.into_inner()),
                label: Some("Stable image fixture".into()),
                source: jcode_sdk::RenderedImageSource::UserInput,
                anchor: None,
            };
            let decoded = Rc::new(RefCell::new(None));
            let (host, vcx) = cx.add_window_view(|_, cx| {
                let panel = cx.new(|cx| {
                    let mut panel = Panel::new(
                        "image-stability".into(),
                        None,
                        None,
                        crate::harness::spawn_inert(),
                        cx,
                    );
                    panel.items.clear();
                    // Deliberately isolate images from expected tail-follow
                    // scrolling and pinned-prompt geometry changes.
                    panel.stick_to_bottom = false;
                    panel.load_history(Vec::new(), vec![rendered.clone()], cx);
                    panel
                });
                ImageWitness {
                    panel,
                    decoded: decoded.clone(),
                }
            });
            let panel = host.read_with(vcx, |host, _| host.panel.clone());
            let handle = vcx.update(|window, _| window.window_handle());
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
            // Background decoding and next-frame painting are separate. Yield
            // to the deterministic executor and explicitly repaint after each
            // turn rather than assuming run_until_parked alone loads an asset.
            for _ in 0..128 {
                host.update(vcx, |_, cx| cx.notify());
                vcx.run_until_parked();
                if decoded.borrow().is_some() {
                    break;
                }
                yield_once().await;
            }
            let baseline = decoded
                .borrow()
                .clone()
                .expect("PNG must decode within bounded executor turns");
            assert_eq!(&baseline.as_bytes(0).unwrap()[..4], &[60, 20, 240, 255]);
            assert_eq!(baseline.size(0).width.0, image_width as i32);
            assert_eq!(baseline.size(0).height.0, image_height as i32);
            let original_source = panel.read_with(vcx, |panel, _| panel_image(panel));
            let image_bounds = vcx.debug_bounds("transcript-image").unwrap();
            assert!(image_bounds.size.height >= px(320.));

            for _ in 0..3 {
                for streaming in [true, false] {
                    panel.update(vcx, |panel, cx| {
                        if streaming {
                            panel.apply(
                                &ApiEvent::TextDelta {
                                    session_id: "image-stability".into(),
                                    text: "Short answer.".into(),
                                },
                                cx,
                            );
                        } else {
                            panel.apply(
                                &ApiEvent::SessionStatus {
                                    session_id: "image-stability".into(),
                                    status: "idle".into(),
                                },
                                cx,
                            );
                        }
                    });
                    for _ in 0..4 {
                        host.update(vcx, |_, cx| cx.notify());
                        vcx.run_until_parked();
                        assert!(
                            vcx.debug_bounds(if streaming {
                                "transcript-with-response"
                            } else {
                                "transcript"
                            })
                            .is_some()
                        );
                        assert_eq!(vcx.debug_bounds("transcript-image"), Some(image_bounds));
                        let asset = decoded
                            .borrow()
                            .clone()
                            .expect("warm image must never become pending");
                        assert!(
                            Arc::ptr_eq(&baseline, &asset),
                            "ancestor ID change replaced the decoded asset"
                        );
                    }
                }
                panel.update(vcx, |panel, cx| {
                    // Exercise the actual initial-history reconstruction path.
                    // Ordinary reattachment with history_loaded=true only
                    // reconciles a response and does not rebuild image objects.
                    panel.items.clear();
                    panel.history_loaded = false;
                    panel.load_history(Vec::new(), vec![rendered.clone()], cx);
                });
                let rebuilt = panel.read_with(vcx, |panel, _| panel_image(panel));
                assert!(
                    !Arc::ptr_eq(&original_source, &rebuilt),
                    "history really reconstructs the encoded image"
                );
                assert_eq!(
                    rebuilt.id, original_source.id,
                    "reconstruction must retain the asset cache key"
                );
                host.update(vcx, |_, cx| cx.notify());
                vcx.run_until_parked();
                assert_eq!(vcx.debug_bounds("transcript-image"), Some(image_bounds));
                assert!(Arc::ptr_eq(
                    &baseline,
                    decoded
                        .borrow()
                        .as_ref()
                        .expect("reconstructed image stays cached")
                ));
            }
        }
    }
}

#[gpui::test]
fn settled_html_preview_keeps_its_instance_across_streaming_ancestor_changes(
    cx: &mut gpui::TestAppContext,
) {
    const BODY: &str = "<p>settled-html-image-identity-regression</p>\n";
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        let mut panel = Panel::new(
            "html-image-stability".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        );
        panel.items = vec![Item::Assistant(format!("```html-preview\n{BODY}```"))];
        panel.stick_to_bottom = false;
        panel
    });
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, gpui::size(px(900.), px(900.)));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("html-preview-card").is_some());
    let initial = vcx.read(|cx| crate::html_preview::test_instance_ids(BODY, cx));
    assert_eq!(initial.len(), 1);
    panel.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert_eq!(
        vcx.read(|cx| crate::html_preview::test_instance_ids(BODY, cx)),
        initial
    );
    let mut observations = Vec::new();
    for streaming in [true, false] {
        panel.update(vcx, |panel, cx| {
            if streaming {
                panel.apply(
                    &ApiEvent::TextDelta {
                        session_id: "html-image-stability".into(),
                        text: "Unrelated answer.".into(),
                    },
                    cx,
                );
            } else {
                panel.apply(
                    &ApiEvent::SessionStatus {
                        session_id: "html-image-stability".into(),
                        status: "idle".into(),
                    },
                    cx,
                );
            }
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("html-preview-card").is_some());
        observations.push(vcx.read(|cx| crate::html_preview::test_instance_ids(BODY, cx)));
    }
    // Collect both boundaries before asserting so a failure identifies whether
    // start, end, or both recreated a Preview (and its image/worker lifecycle).
    assert_eq!(
        observations,
        vec![initial.clone(), initial],
        "unchanged settled HTML preview remounted across streaming boundaries"
    );
}
