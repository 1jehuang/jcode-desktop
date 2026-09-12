//! Image identities belong to the long-lived app, not a reloadable library.
//!
//! GPUI's RenderImage::new uses a library-local static counter. The host SVG
//! renderer, linked UI, and each loaded UI generation have separate counters,
//! but share one GPU atlas. Reusing an ID can paint an unrelated old texture.
use gpui::{App, Asset, Global, Image, ImageCacheError, ImageSource, RenderImage};
use std::sync::Arc;

/// Encoded assets are keyed by `Image::id`, not by byte equality. Independent
/// counters for composer and transcript images (or a restarted UI generation)
/// alias the same decoder cache entry. Content identity is stable across all
/// these paths and also avoids decoding the same attachment again on submit.
pub(crate) fn encoded(format: gpui::ImageFormat, bytes: Vec<u8>) -> Arc<Image> {
    Arc::new(Image::from_bytes(format, bytes))
}

#[derive(Clone, Default)]
pub(crate) struct ImageIds(jcode_desktop_api::ImageIds);
impl ImageIds {
    pub(crate) fn get(cx: &mut App) -> Self {
        Self(jcode_desktop_api::ImageIds::get(cx))
    }

    fn next(&self) -> gpui::ImageId {
        self.0.next()
    }

    pub(crate) fn render(&self, frames: Vec<image::Frame>) -> Arc<RenderImage> {
        let mut image = RenderImage::new(frames);
        image.id = self.next();
        Arc::new(image)
    }
}

struct DesktopImageDecoder;
impl Asset for DesktopImageDecoder {
    type Source = Arc<Image>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let id = ImageIds::get(cx).next();
        let source_id = source.id;
        let started = std::time::Instant::now();
        let renderer = cx.svg_renderer();
        eprintln!(
            "desktop-image decode-start source={source_id:016x} texture={}",
            id.0
        );
        async move {
            let mut image = match source.to_image_data(renderer) {
                Ok(image) => image,
                Err(error) => {
                    // Never log source bytes, labels, paths, or decoder errors
                    // that could embed private image metadata.
                    eprintln!(
                        "desktop-image decode-failed source={source_id:016x} elapsed_ms={}",
                        started.elapsed().as_millis()
                    );
                    return Err(ImageCacheError::from(error));
                }
            };
            // GPUI returns a freshly decoded image. Assign before publishing to
            // the asset cache, preserving BGRA, animation and SVG scale data.
            Arc::get_mut(&mut image)
                .expect("freshly decoded image is uniquely owned")
                .id = id;
            eprintln!(
                "desktop-image decode-ready source={source_id:016x} texture={} elapsed_ms={}",
                id.0,
                started.elapsed().as_millis()
            );
            Ok(image)
        }
    }
}

const MAX_PAINT_TRACE_ENTRIES: usize = 128;
const MAX_PAINT_TRACE_EVENTS: usize = 32;
const PAINT_TRACE_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImagePaintState {
    Pending,
    Ready(usize),
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ImagePaintEvent {
    reason: &'static str,
    previous: Option<ImagePaintState>,
    previous_texture: Option<usize>,
    current: ImagePaintState,
}

struct ImagePaintEntry {
    key: (u64, u64),
    state: ImagePaintState,
    texture: Option<usize>,
    last_anomaly: Option<std::time::Instant>,
}

/// Only identifiers and state, never image data, errors, or private metadata.
/// The least recently observed entry is evicted when the fixed cap is reached.
#[derive(Default)]
struct ImagePaintTrace {
    entries: std::collections::VecDeque<ImagePaintEntry>,
    event_window: Option<std::time::Instant>,
    emitted_in_window: usize,
}

impl ImagePaintTrace {
    fn observe(
        &mut self,
        key: (u64, u64),
        state: ImagePaintState,
        now: std::time::Instant,
    ) -> Option<ImagePaintEvent> {
        // Always update the LRU/state even when logs are suppressed. More than
        // 128 visible sources may otherwise churn first associations every
        // frame, bypassing any per-key cooldown after eviction.
        let event = self.observe_unthrottled(key, state, now)?;
        if self
            .event_window
            .is_none_or(|start| now.saturating_duration_since(start) >= PAINT_TRACE_COOLDOWN)
        {
            self.event_window = Some(now);
            self.emitted_in_window = 0;
        }
        if self.emitted_in_window >= MAX_PAINT_TRACE_EVENTS {
            return None;
        }
        self.emitted_in_window += 1;
        Some(event)
    }

    fn observe_unthrottled(
        &mut self,
        key: (u64, u64),
        state: ImagePaintState,
        now: std::time::Instant,
    ) -> Option<ImagePaintEvent> {
        let Some(index) = self.entries.iter().position(|entry| entry.key == key) else {
            if self.entries.len() == MAX_PAINT_TRACE_ENTRIES {
                self.entries.pop_front();
            }
            self.entries.push_back(ImagePaintEntry {
                key,
                state,
                texture: match state {
                    ImagePaintState::Ready(texture) => Some(texture),
                    _ => None,
                },
                last_anomaly: None,
            });
            return Some(ImagePaintEvent {
                reason: "first-observed",
                previous: None,
                previous_texture: None,
                current: state,
            });
        };
        let mut entry = self.entries.remove(index).expect("existing trace entry");
        let previous = entry.state;
        let previous_texture = entry.texture;
        let reason = match state {
            ImagePaintState::Ready(texture) => match previous_texture {
                None => Some("first-ready"),
                Some(previous) if previous != texture => Some("texture-changed"),
                _ => None,
            },
            ImagePaintState::Pending if matches!(previous, ImagePaintState::Ready(_)) => {
                Some("ready-to-pending")
            }
            ImagePaintState::Error if previous != ImagePaintState::Error => Some("became-error"),
            _ => None,
        };
        let event = reason.and_then(|reason| {
            if reason != "first-ready" {
                if entry
                    .last_anomaly
                    .is_some_and(|last| now.saturating_duration_since(last) < PAINT_TRACE_COOLDOWN)
                {
                    return None;
                }
                entry.last_anomaly = Some(now);
            }
            Some(ImagePaintEvent {
                reason,
                previous: Some(previous),
                previous_texture,
                current: state,
            })
        });
        entry.state = state;
        if let ImagePaintState::Ready(texture) = state {
            entry.texture = Some(texture);
        }
        self.entries.push_back(entry);
        event
    }
}

#[derive(Default)]
struct ImagePaintTraceGlobal(std::cell::RefCell<ImagePaintTrace>);
impl Global for ImagePaintTraceGlobal {}

/// Keep asynchronous decoding/caching, but never publish library-local IDs.
pub(crate) fn source(image: Arc<Image>) -> ImageSource {
    ImageSource::Custom(Arc::new(move |window, cx| {
        let result = window.use_asset::<DesktopImageDecoder>(&image, cx);
        let state = match &result {
            None => ImagePaintState::Pending,
            Some(Ok(decoded)) => ImagePaintState::Ready(decoded.id.0),
            Some(Err(_)) => ImagePaintState::Error,
        };
        let view = window.current_view().as_u64();
        if !cx.has_global::<ImagePaintTraceGlobal>() {
            cx.set_global(ImagePaintTraceGlobal::default());
        }
        // Read the global immutably: global_mut/default_global would enqueue a
        // NotifyGlobalObservers effect on every image lookup, including paint.
        let event = cx.global::<ImagePaintTraceGlobal>().0.borrow_mut().observe(
            (view, image.id),
            state,
            std::time::Instant::now(),
        );
        if let Some(event) = event {
            let timestamp_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            eprintln!(
                "desktop-image paint-state timestamp_ms={timestamp_ms} view={view} source={:016x} reason={} previous={:?} previous_texture={:?} current={:?}",
                image.id, event.reason, event.previous, event.previous_texture, event.current
            );
        }
        result
    }))
}

#[cfg(test)]
mod paint_trace_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[gpui::test]
    async fn render_callbacks_detect_cached_image_eviction(cx: &mut gpui::TestAppContext) {
        use gpui::prelude::*;
        struct ImageProbe(Arc<Image>);
        impl gpui::Render for ImageProbe {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                gpui::img(source(self.0.clone())).size(gpui::px(100.))
            }
        }
        async fn settle(
            view: &gpui::Entity<ImageProbe>,
            vcx: &mut gpui::VisualTestContext,
            image: &Arc<Image>,
        ) {
            vcx.run_until_parked();
            // A parked foreground executor does not mean decoding has finished.
            // Await the already-started shared asset, then paint its result.
            let _ = vcx
                .update(|_, cx| cx.fetch_asset::<DesktopImageDecoder>(image).0)
                .await;
            view.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
        let image = encoded(
            gpui::ImageFormat::Png,
            include_bytes!("../../../assets/previews/image-preview.png").to_vec(),
        );
        let (view, vcx) = cx.add_window_view(|_, _| ImageProbe(image.clone()));
        settle(&view, vcx, &image).await;
        // Asset completion schedules a later frame. The test dispatcher does
        // not advance the native frame clock just by draining decoder tasks.
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let key = (view.entity_id().as_u64(), image.id);
        let before = vcx.update(|_, cx| {
            let trace = cx.global::<ImagePaintTraceGlobal>().0.borrow();
            let entry = trace.entries.iter().find(|entry| entry.key == key).unwrap();
            assert!(matches!(entry.state, ImagePaintState::Ready(_)));
            assert!(entry.last_anomaly.is_none());
            entry.texture.unwrap()
        });
        // Fault injection validates the real source callback/logging path. It
        // is not evidence that normal operation evicts this asset unexpectedly.
        vcx.update(|_, cx| cx.remove_asset::<DesktopImageDecoder>(&image));
        view.update(vcx, |_, cx| cx.notify());
        settle(&view, vcx, &image).await;
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let trace = cx.global::<ImagePaintTraceGlobal>().0.borrow();
            let entry = trace.entries.iter().find(|entry| entry.key == key).unwrap();
            assert!(matches!(entry.state, ImagePaintState::Ready(_)));
            assert_ne!(entry.texture, Some(before));
            assert!(
                entry.last_anomaly.is_some(),
                "real image redraw must report the unexpected reload"
            );
        });
        let invalid = encoded(gpui::ImageFormat::Png, b"invalid png fixture".to_vec());
        let invalid_key = (view.entity_id().as_u64(), invalid.id);
        view.update(vcx, |view, cx| {
            view.0 = invalid.clone();
            cx.notify();
        });
        settle(&view, vcx, &invalid).await;
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let trace = cx.global::<ImagePaintTraceGlobal>().0.borrow();
            let entry = trace
                .entries
                .iter()
                .find(|entry| entry.key == invalid_key)
                .unwrap();
            assert_eq!(entry.state, ImagePaintState::Error);
            assert!(
                entry.last_anomaly.is_some(),
                "failed decoding must reach paint-state diagnostics"
            );
        });
    }

    #[test]
    fn unique_source_churn_is_globally_rate_limited_and_recovers_after_ten_seconds() {
        let mut trace = ImagePaintTrace::default();
        let now = Instant::now();
        let mut emitted = 0;
        for source in 0..1000 {
            emitted += usize::from(
                trace
                    .observe((1, source), ImagePaintState::Ready(30), now)
                    .is_some(),
            );
        }
        assert_eq!(emitted, MAX_PAINT_TRACE_EVENTS);
        assert_eq!(trace.entries.len(), MAX_PAINT_TRACE_ENTRIES);
        assert_eq!(trace.entries.back().unwrap().key, (1, 999));
        assert!(
            trace
                .observe(
                    (1, 1000),
                    ImagePaintState::Pending,
                    now + Duration::from_secs(9)
                )
                .is_none()
        );
        assert!(
            trace
                .observe(
                    (1, 1001),
                    ImagePaintState::Pending,
                    now + PAINT_TRACE_COOLDOWN
                )
                .is_some()
        );
        assert_eq!(trace.emitted_in_window, 1);
    }

    #[test]
    fn first_association_and_ready_are_reported_without_steady_frame_spam() {
        let mut trace = ImagePaintTrace::default();
        let now = Instant::now();
        let key = (1, 20);
        let event = trace.observe(key, ImagePaintState::Pending, now).unwrap();
        assert_eq!(event.reason, "first-observed");
        assert_eq!(event.previous, None);
        assert!(trace.observe(key, ImagePaintState::Pending, now).is_none());
        let event = trace.observe(key, ImagePaintState::Ready(30), now).unwrap();
        assert_eq!(event.reason, "first-ready");
        for _ in 0..100 {
            assert!(
                trace
                    .observe(key, ImagePaintState::Ready(30), now)
                    .is_none()
            );
        }
    }

    #[test]
    fn ready_regression_is_immediate_then_rate_limited() {
        let mut trace = ImagePaintTrace::default();
        let now = Instant::now();
        let key = (1, 20);
        trace.observe(key, ImagePaintState::Ready(30), now);
        let event = trace.observe(key, ImagePaintState::Pending, now).unwrap();
        assert_eq!(event.reason, "ready-to-pending");
        assert_eq!(event.previous_texture, Some(30));
        assert!(
            trace
                .observe(key, ImagePaintState::Ready(31), now)
                .is_none()
        );
        let after = now + PAINT_TRACE_COOLDOWN;
        let event = trace
            .observe(key, ImagePaintState::Ready(32), after)
            .unwrap();
        assert_eq!(event.reason, "texture-changed");
        assert_eq!(event.previous_texture, Some(31));
        assert!(
            trace
                .observe(key, ImagePaintState::Ready(33), after)
                .is_none()
        );
    }

    #[test]
    fn texture_identity_is_remembered_across_pending_and_error_states() {
        let mut trace = ImagePaintTrace::default();
        let now = Instant::now();
        let key = (1, 20);
        trace.observe(key, ImagePaintState::Ready(30), now);
        assert_eq!(
            trace
                .observe(key, ImagePaintState::Error, now)
                .unwrap()
                .reason,
            "became-error"
        );
        assert!(trace.observe(key, ImagePaintState::Error, now).is_none());
        trace.observe(key, ImagePaintState::Pending, now);
        let event = trace
            .observe(key, ImagePaintState::Ready(31), now + PAINT_TRACE_COOLDOWN)
            .unwrap();
        assert_eq!(event.reason, "texture-changed");
        assert_eq!(event.previous_texture, Some(30));
        assert_eq!(event.previous, Some(ImagePaintState::Pending));
    }

    #[test]
    fn keys_are_scoped_by_both_view_and_source() {
        let mut trace = ImagePaintTrace::default();
        let now = Instant::now();
        for key in [(1, 20), (2, 20), (1, 21)] {
            assert_eq!(
                trace
                    .observe(key, ImagePaintState::Ready(30), now)
                    .unwrap()
                    .reason,
                "first-observed"
            );
            assert!(trace.observe(key, ImagePaintState::Pending, now).is_some());
        }
        assert_eq!(trace.entries.len(), 3);
    }

    #[test]
    fn trace_is_bounded_and_recently_used_entries_survive_eviction() {
        let mut trace = ImagePaintTrace::default();
        let now = Instant::now();
        for source in 0..MAX_PAINT_TRACE_ENTRIES as u64 {
            trace.observe((1, source), ImagePaintState::Pending, now);
        }
        trace.observe((1, 0), ImagePaintState::Pending, now);
        trace.observe(
            (1, 128),
            ImagePaintState::Pending,
            now + Duration::from_secs(1),
        );
        assert_eq!(trace.entries.len(), MAX_PAINT_TRACE_ENTRIES);
        assert!(trace.entries.iter().any(|entry| entry.key == (1, 0)));
        assert!(!trace.entries.iter().any(|entry| entry.key == (1, 1)));
        for source in 129..1000 {
            trace.observe((1, source), ImagePaintState::Pending, now);
            assert_eq!(trace.entries.len(), MAX_PAINT_TRACE_ENTRIES);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    async fn encoded_images_do_not_alias_across_surfaces_or_reload(cx: &mut gpui::TestAppContext) {
        fn png(pixel: [u8; 4]) -> Vec<u8> {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(2, 3, image::Rgba(pixel))
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            bytes.into_inner()
        }
        let bytes = png([240, 20, 60, 255]);
        let attachment = encoded(gpui::ImageFormat::Png, bytes.clone());
        let transcript = encoded(gpui::ImageFormat::Png, png([10, 220, 90, 255]));
        assert_ne!(attachment.id, transcript.id);
        let first = cx
            .update(|cx| cx.fetch_asset::<DesktopImageDecoder>(&attachment).0)
            .await
            .unwrap();
        let second = cx
            .update(|cx| cx.fetch_asset::<DesktopImageDecoder>(&transcript).0)
            .await
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(&first.as_bytes(0).unwrap()[..4], &[60, 20, 240, 255]);
        assert_eq!(&second.as_bytes(0).unwrap()[..4], &[90, 220, 10, 255]);
        // Reconstructing after submission/reload reuses the correct cached
        // pixels rather than restarting a library-local source counter.
        let restored = encoded(gpui::ImageFormat::Png, bytes);
        let (task, is_first) = cx.update(|cx| cx.fetch_asset::<DesktopImageDecoder>(&restored));
        assert!(!is_first);
        assert_eq!(task.await.unwrap().id, first.id);
    }

    #[gpui::test]
    fn allocator_survives_ui_generation_handles(cx: &mut gpui::TestAppContext) {
        let old = cx.update(ImageIds::get);
        let first = old.next();
        let worker = old.clone();
        drop(old);
        let next_generation = cx.update(ImageIds::get);
        assert_eq!(worker.next().0, first.0 + 1);
        assert_eq!(next_generation.next().0, first.0 + 2);
        // The host/shared dependency and UI wrapper must use the same Global.
        let host = cx.update(jcode_desktop_api::ImageIds::get);
        assert_eq!(host.next().0, first.0 + 3);
        assert_eq!(next_generation.next().0, first.0 + 4);
        assert!(first.0 >= 3usize << (usize::BITS - 2));
        assert!(RenderImage::new(vec![]).id.0 < 1usize << (usize::BITS - 1));
    }

    #[test]
    fn raw_frames_preserve_pixels_and_have_distinct_ids() {
        let ids = ImageIds::default();
        let pixels = image::RgbaImage::from_pixel(2, 3, image::Rgba([10, 20, 30, 255]));
        let first = ids.render(vec![image::Frame::new(pixels.clone())]);
        let second = ids.render(vec![image::Frame::new(pixels.clone())]);
        assert_ne!(first.id, second.id);
        assert_eq!(first.as_bytes(0), Some(pixels.as_raw().as_slice()));
        assert_eq!(first.size(0).width.0, 2);
        assert_eq!(first.size(0).height.0, 3);
    }
}
