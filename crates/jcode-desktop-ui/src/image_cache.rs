//! Image identities belong to the long-lived app, not a reloadable library.
//!
//! GPUI's RenderImage::new uses a library-local static counter. The host SVG
//! renderer, linked UI, and each loaded UI generation have separate counters,
//! but share one GPU atlas. Reusing an ID can paint an unrelated old texture.
use gpui::{App, Asset, Global, Image, ImageCacheError, ImageId, ImageSource, RenderImage};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const FIRST_DESKTOP_IMAGE_ID: usize = 1usize << (usize::BITS - 1);

#[derive(Clone)]
pub(crate) struct ImageIds(Arc<AtomicUsize>);
impl Global for ImageIds {}
impl Default for ImageIds {
    fn default() -> Self {
        Self(Arc::new(AtomicUsize::new(FIRST_DESKTOP_IMAGE_ID)))
    }
}
impl ImageIds {
    pub(crate) fn get(cx: &mut App) -> Self {
        cx.default_global::<Self>().clone()
    }

    fn next(&self) -> ImageId {
        // Reserve the high half for desktop images, away from GPUI's host
        // counter. Never reset on activation or reuse an ID after eviction.
        ImageId(
            self.0
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("desktop image IDs exhausted"),
        )
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
        let renderer = cx.svg_renderer();
        async move {
            let mut image = source
                .to_image_data(renderer)
                .map_err(ImageCacheError::from)?;
            // GPUI returns a freshly decoded image. Assign before publishing to
            // the asset cache, preserving BGRA, animation and SVG scale data.
            Arc::get_mut(&mut image)
                .expect("freshly decoded image is uniquely owned")
                .id = id;
            Ok(image)
        }
    }
}

/// Keep asynchronous decoding/caching, but never publish library-local IDs.
pub(crate) fn source(image: Arc<Image>) -> ImageSource {
    ImageSource::Custom(Arc::new(move |window, cx| {
        window.use_asset::<DesktopImageDecoder>(&image, cx)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn allocator_survives_ui_generation_handles(cx: &mut gpui::TestAppContext) {
        let old = cx.update(ImageIds::get);
        let first = old.next();
        let worker = old.clone();
        drop(old);
        let next_generation = cx.update(ImageIds::get);
        assert_eq!(worker.next().0, first.0 + 1);
        assert_eq!(next_generation.next().0, first.0 + 2);
        assert!(first.0 >= FIRST_DESKTOP_IMAGE_ID);
        assert!(RenderImage::new(vec![]).id.0 < FIRST_DESKTOP_IMAGE_ID);
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
