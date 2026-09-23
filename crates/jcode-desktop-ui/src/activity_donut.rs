//! Jcode's tilted donut translated into the thinking-orbs dotted vocabulary.
//! The canonical SVG/app icon stays unchanged. This compact torus shares the
//! upstream Frame/Dot paint path and can morph to any dot-only upstream preset.

use std::f32::consts::TAU;

use gpui_thinking_orbs::{Dot, Frame, sanitize_size, sort_dots};

const AROUND: usize = 18;
const TUBE: usize = 4;

pub(super) fn draw_donut_into(size: f32, t: f32, out: &mut Frame) {
    out.clear();
    let size = sanitize_size(size);
    let t = if t.is_finite() { t } else { 0.0 };
    // Keep the hole open, including at the 20px inline size. A diagonal tilt
    // echoes the canonical Jcode mark rather than a face-on loading ring.
    let (tilt_sin, tilt_cos) = 0.65_f32.sin_cos();
    let (roll_sin, roll_cos) = (-0.48_f32).sin_cos();
    for around in 0..AROUND {
        for tube in 0..TUBE {
            let u = TAU * around as f32 / AROUND as f32 + t * 0.55;
            let v = TAU * tube as f32 / TUBE as f32 + t * 0.8;
            let (su, cu) = u.sin_cos();
            let (sv, cv) = v.sin_cos();
            let radius = 0.30 + 0.105 * cv;
            let x = radius * cu;
            let y = radius * su;
            let z = 0.105 * sv;
            let tilted_y = y * tilt_cos - z * tilt_sin;
            let depth = y * tilt_sin + z * tilt_cos;
            let px = x * roll_cos - tilted_y * roll_sin;
            let py = x * roll_sin + tilted_y * roll_cos;
            // Surface-normal lighting defines the tube, while depth attenuates
            // the back side. Both use the upstream theme-relative ink scale.
            let nx = cv * cu;
            let ny = cv * su * tilt_cos - sv * tilt_sin;
            let nz = cv * su * tilt_sin + sv * tilt_cos;
            let light = (-0.35 * nx - 0.45 * ny + 0.82 * nz).clamp(-1.0, 1.0);
            let near = ((depth + 0.32) / 0.64).clamp(0.0, 1.0);
            out.dots.push(
                Dot::new(
                    size * (0.5 + px),
                    size * (0.5 + py),
                    depth,
                    size * (0.017 + 0.009 * near),
                    (0.28 - 0.22 * light).clamp(0.0, 1.0),
                )
                .with_a(0.5 + 0.5 * near),
            );
        }
    }
    sort_dots(&mut out.dots);
}

/// Interpolate geometry, not whole-image opacity. Extra particles shrink/fade
/// at an existing endpoint rather than popping in or flying from the origin.
/// The caller captures the displayed frame on retargeting, so reversals begin
/// from exactly what was last painted. Buffers retain their high-water capacity.
pub(super) fn morph_into(from: &Frame, to: &Frame, progress: f32, out: &mut Frame) {
    out.clear();
    let progress = if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if progress <= 0.0 {
        out.dots.extend_from_slice(&from.dots);
        return;
    }
    if progress >= 1.0 {
        out.dots.extend_from_slice(&to.dots);
        return;
    }
    let p = progress * progress * (3.0 - 2.0 * progress);
    let count = from.dots.len().max(to.dots.len());
    for index in 0..count {
        let fallback = |dots: &[Dot]| {
            dots.get(index % dots.len().max(1))
                .copied()
                .unwrap_or_else(|| Dot::new(0.0, 0.0, 0.0, 0.0, 0.0))
        };
        let a = from.dots.get(index).copied().unwrap_or_else(|| {
            let mut dot = fallback(&to.dots);
            dot.r = 0.0;
            dot.a = 0.0;
            dot
        });
        let b = to.dots.get(index).copied().unwrap_or_else(|| {
            let mut dot = fallback(&from.dots);
            dot.r = 0.0;
            dot.a = 0.0;
            dot
        });
        let mix = |a: f32, b: f32| a + (b - a) * p;
        out.dots.push(
            Dot::new(
                mix(a.x, b.x),
                mix(a.y, b.y),
                mix(a.z, b.z),
                mix(a.r, b.r),
                mix(a.white, b.white),
            )
            .with_a(mix(a.a, b.a)),
        );
    }
    sort_dots(&mut out.dots);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_thinking_orbs::{OrbSize, OrbState, draw_mode_into, resolve_preset};

    fn signature(frame: &Frame) -> Vec<[f32; 6]> {
        frame
            .dots
            .iter()
            .map(|d| [d.x, d.y, d.z, d.r, d.white, d.a])
            .collect()
    }

    #[test]
    fn donut_stays_bounded_sorted_and_has_an_open_center() {
        let mut frame = Frame::new();
        draw_donut_into(20.0, 0.6, &mut frame);
        let capacity = frame.dots.capacity();
        let buffer = frame.dots.as_ptr();
        let first = signature(&frame);
        for i in 0..600 {
            draw_donut_into(20.0, i as f32 / 30.0, &mut frame);
            assert_eq!(frame.dots.len(), AROUND * TUBE);
            assert_eq!(frame.dots.capacity(), capacity);
            assert_eq!(frame.dots.as_ptr(), buffer);
            assert!(frame.lines.is_empty());
            assert!(frame.dots.windows(2).all(|pair| pair[0].z <= pair[1].z));
            for dot in &frame.dots {
                assert!(
                    [dot.x, dot.y, dot.z, dot.r, dot.white, dot.a]
                        .iter()
                        .all(|n| n.is_finite())
                );
                assert!(dot.x - dot.r >= 0.0 && dot.x + dot.r <= 20.0);
                assert!(dot.y - dot.r >= 0.0 && dot.y + dot.r <= 20.0);
                assert!(
                    (dot.x - 10.0).hypot(dot.y - 10.0) - dot.r > 1.8,
                    "donut must retain its central hole: {dot:?}"
                );
            }
        }
        assert_ne!(first, signature(&frame));
        assert!(capacity * std::mem::size_of::<Dot>() <= 4096);
    }

    #[test]
    fn morph_endpoints_are_exact_and_reversal_preserves_displayed_geometry() {
        let mut donut = Frame::new();
        draw_donut_into(20.0, 0.6, &mut donut);
        for state in [
            OrbState::Working,
            OrbState::Reasoning,
            OrbState::Composing,
            OrbState::Solving,
        ] {
            let preset = resolve_preset(state, OrbSize::Inline);
            let mut orb = Frame::new();
            draw_mode_into(preset.mode, 20.0, 0.6, &preset.opts, &mut orb);
            let mut out = Frame::new();
            morph_into(&donut, &orb, 0.0, &mut out);
            assert_eq!(signature(&out), signature(&donut));
            morph_into(&donut, &orb, 1.0, &mut out);
            assert_eq!(signature(&out), signature(&orb));
            morph_into(&donut, &orb, 0.4, &mut out);
            let interrupted = out.clone();
            morph_into(&interrupted, &donut, 0.0, &mut out);
            assert_eq!(signature(&out), signature(&interrupted));
            morph_into(&interrupted, &donut, 1.0, &mut out);
            assert_eq!(signature(&out), signature(&donut));
            for i in 1..20 {
                morph_into(&donut, &orb, i as f32 / 20.0, &mut out);
                assert!(out.lines.is_empty());
                assert!(out.dots.windows(2).all(|d| d[0].z <= d[1].z));
                assert!(out.dots.iter().all(|d| d.x.is_finite()
                    && d.y.is_finite()
                    && d.r >= 0.0
                    && (0.0..=1.0).contains(&d.a)));
            }
        }
    }

    #[test]
    fn morph_handles_empty_frames_and_reuses_its_buffer() {
        let empty = Frame::new();
        let mut donut = Frame::new();
        let mut out = Frame::new();
        draw_donut_into(20.0, 0.6, &mut donut);
        morph_into(&empty, &empty, 0.5, &mut out);
        assert!(out.dots.is_empty());
        morph_into(&empty, &donut, 0.5, &mut out);
        let buffer = out.dots.as_ptr();
        for i in 0..100 {
            morph_into(&donut, &empty, i as f32 / 100.0, &mut out);
            assert_eq!(out.dots.as_ptr(), buffer);
        }
    }
}
