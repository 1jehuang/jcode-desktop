//! Process/app-owned texture identities shared by the host and UI libraries.
//!
//! This Global must live in a shared dependency, not the UI crate: Cargo builds
//! the linked UI and its cdylib with different crate identities, so a UI-local
//! Global has different TypeIds and silently creates two counters in one App.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use gpui::{App, Global, ImageId};

// Leave both GPUI's low IDs and the old UI-local allocator's 0x8000... IDs
// untouched. This also fixes an already-running host without reusing textures
// painted before the first reload that installs the shared allocator.
const FIRST_IMAGE_ID: usize = 3usize << (usize::BITS - 2);

#[derive(Clone)]
pub struct ImageIds(Arc<AtomicUsize>);

impl Global for ImageIds {}

impl Default for ImageIds {
    fn default() -> Self {
        Self(Arc::new(AtomicUsize::new(FIRST_IMAGE_ID)))
    }
}

impl ImageIds {
    pub fn get(cx: &mut App) -> Self {
        // Avoid enqueuing global notifications on every animated icon render.
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
        cx.global::<Self>().clone()
    }

    pub fn next(&self) -> ImageId {
        ImageId(
            self.0
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("desktop image IDs exhausted"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_ids_outside_both_legacy_namespaces() {
        let ids = ImageIds::default();
        let worker = ids.clone();
        assert_eq!(ids.next().0, FIRST_IMAGE_ID);
        assert_eq!(worker.next().0, FIRST_IMAGE_ID + 1);
        assert_eq!(ids.next().0, FIRST_IMAGE_ID + 2);
        assert!(FIRST_IMAGE_ID > (1usize << (usize::BITS - 1)));
    }

    #[test]
    fn exhaustion_never_wraps_into_another_namespace() {
        let ids = ImageIds(Arc::new(AtomicUsize::new(usize::MAX - 1)));
        assert_eq!(ids.next().0, usize::MAX - 1);
        assert!(std::panic::catch_unwind(|| ids.next()).is_err());
        assert_eq!(ids.0.load(Ordering::Relaxed), usize::MAX);
    }
}
