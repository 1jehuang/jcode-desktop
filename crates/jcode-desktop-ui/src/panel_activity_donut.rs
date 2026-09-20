//! Small-size rendering of the canonical jcode halftone torus.
//!
//! Geometry, camera, light and initial pose follow jcode-website/tools/gen-logo.mjs.
//! A size-specific 18-cell screen keeps the 14px mark legible without submitting
//! thousands of invisible subpixel contours. Each frame is raymarched and
//! screened lazily on its first paint in the initial cycle. Subsequent cycles
//! reuse the finite shared geometry cache without raymarching or screening.

use std::{
    f32::consts::{FRAC_1_SQRT_2, TAU},
    sync::OnceLock,
};

pub(super) const FRAME_COUNT: usize = 32;
const GRID: usize = 64;
const CELLS: f32 = 18.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Dot {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
}

pub(super) fn frame(step: usize) -> &'static [Dot] {
    static FRAMES: [OnceLock<Vec<Dot>>; FRAME_COUNT] = [const { OnceLock::new() }; FRAME_COUNT];
    let step = step % FRAME_COUNT;
    FRAMES[step].get_or_init(|| generate(step))
}

fn generate(step: usize) -> Vec<Dot> {
    let phase = (step % FRAME_COUNT) as f32 * TAU / FRAME_COUNT as f32;
    let tilt = 1.15 + 0.08 * phase.sin();
    let spin = 2.16 + phase;
    let (sa, ca) = tilt.sin_cos();
    let (sb, cb) = spin.sin_cos();
    let k1 = GRID as f32 * 5.0 / 8.0;
    let oy = -ca * 5.0;
    let oz = -sa * 5.0;
    let mut lum = [0.0; GRID * GRID];
    for y in 0..GRID {
        for x in 0..GRID {
            let wx = (x as f32 + 0.5 - GRID as f32 / 2.0) / k1;
            let wy = -(y as f32 + 0.5 - GRID as f32 / 2.0) / k1;
            let inv = (wx * wx + wy * wy + 1.0).sqrt().recip();
            let dx = (cb * wx + sb * wy) * inv;
            let dy = (sa * sb * wx - sa * cb * wy + ca) * inv;
            let dz = (-ca * sb * wx + ca * cb * wy + sa) * inv;
            let b = oy * dy + oz * dz;
            let disc = b * b - (25.0 - 3.02 * 3.02);
            if disc <= 0.0 {
                continue;
            }
            let exit = -b + disc.sqrt();
            let mut s = (-b - disc.sqrt()).max(0.0);
            let mut nearest = (f32::MAX, s);
            let mut hit = false;
            for _ in 0..96 {
                let px = dx * s;
                let py = oy + dy * s;
                let pz = oz + dz * s;
                let q = px.hypot(py) - 2.0;
                let d = q.hypot(pz) - 1.0;
                if d < nearest.0 {
                    nearest = (d, s);
                }
                if d < 0.0005 {
                    hit = true;
                    break;
                }
                s += d;
                if s > exit {
                    break;
                }
            }
            if !hit {
                s = nearest.1;
            }
            let aa = 1.5 * s / k1;
            if !hit && nearest.0 >= aa {
                continue;
            }
            let px = dx * s;
            let py = oy + dy * s;
            let pz = oz + dz * s;
            let k = 2.0 / px.hypot(py);
            let nx = px * (1.0 - k);
            let ny = py * (1.0 - k);
            let nl = (nx * nx + ny * ny + pz * pz).sqrt();
            let nwy = sb * nx - sa * cb * ny + ca * cb * pz;
            let nwz = ca * ny + sa * pz;
            let coverage = if hit { 1.0 } else { 1.0 - nearest.0 / aa };
            lum[x + y * GRID] = ((nwy - nwz) * FRAC_1_SQRT_2 / nl).max(0.0) * coverage;
        }
    }
    // The canonical 1-2-1 Gaussian screen prefilter.
    let mut smooth = [0.0; GRID * GRID];
    for y in 1..GRID - 1 {
        for x in 1..GRID - 1 {
            for (j, wj) in [1.0, 2.0, 1.0].iter().enumerate() {
                for (i, wi) in [1.0, 2.0, 1.0].iter().enumerate() {
                    smooth[x + y * GRID] += lum[x + i - 1 + (y + j - 1) * GRID] * wi * wj / 16.0;
                }
            }
        }
    }
    let mut dots = Vec::new();
    for j in -15..=15 {
        for i in -15..=15 {
            let x = 0.5 + (i - j) as f32 * FRAC_1_SQRT_2 / CELLS;
            let y = 0.5 + (i + j) as f32 * FRAC_1_SQRT_2 / CELLS;
            let gx = ((x - 0.5) / 1.12 + 0.5) * GRID as f32;
            let gy = ((y - 0.5) / 1.12 + 0.5) * GRID as f32;
            if gx < 0.0 || gy < 0.0 || gx > (GRID - 2) as f32 || gy > (GRID - 2) as f32 {
                continue;
            }
            let ix = gx as usize;
            let iy = gy as usize;
            let fx = gx.fract();
            let fy = gy.fract();
            let v = smooth[ix + iy * GRID] * (1.0 - fx) * (1.0 - fy)
                + smooth[ix + 1 + iy * GRID] * fx * (1.0 - fy)
                + smooth[ix + (iy + 1) * GRID] * (1.0 - fx) * fy
                + smooth[ix + 1 + (iy + 1) * GRID] * fx * fy;
            if v > 0.045 {
                dots.push(Dot {
                    x,
                    y,
                    radius: v.powf(0.85) * 0.62 / CELLS,
                });
            }
        }
    }
    // Center on ink, as the canonical generator does. Keep a half-pixel inset.
    let min_x = dots
        .iter()
        .map(|d| d.x - d.radius)
        .fold(f32::INFINITY, f32::min);
    let min_y = dots
        .iter()
        .map(|d| d.y - d.radius)
        .fold(f32::INFINITY, f32::min);
    let max_x = dots
        .iter()
        .map(|d| d.x + d.radius)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = dots
        .iter()
        .map(|d| d.y + d.radius)
        .fold(f32::NEG_INFINITY, f32::max);
    let scale = 13.0 / (max_x - min_x).max(max_y - min_y);
    for dot in &mut dots {
        dot.x = 7.0 + (dot.x - (min_x + max_x) / 2.0) * scale;
        dot.y = 7.0 + (dot.y - (min_y + max_y) / 2.0) * scale;
        dot.radius *= scale;
    }
    dots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_is_bounded_has_hole_and_shading() {
        for step in 0..FRAME_COUNT {
            let dots = frame(step);
            assert!((80..=300).contains(&dots.len()), "{} dots", dots.len());
            for dot in dots {
                assert!(dot.radius.is_finite() && dot.radius > 0.0);
                assert!(dot.x - dot.radius >= 0.49 && dot.x + dot.radius <= 13.51);
                assert!(dot.y - dot.radius >= 0.49 && dot.y + dot.radius <= 13.51);
                assert!(
                    (dot.x - 7.0).hypot(dot.y - 7.0) - dot.radius > 0.35,
                    "central hole at frame {step}: {dot:?}"
                );
            }
            let min = dots.iter().map(|d| d.radius).fold(f32::INFINITY, f32::min);
            let max = dots.iter().map(|d| d.radius).fold(0.0, f32::max);
            assert!(max > min * 3.0, "torus shading, not a uniform ring");
        }
    }

    #[test]
    fn frames_are_periodic_distinct_and_share_cached_storage() {
        assert_ne!(frame(0), frame(1));
        for step in 0..FRAME_COUNT {
            assert!(std::ptr::eq(frame(step), frame(step + FRAME_COUNT)));
            assert_eq!(generate(step), generate(step + FRAME_COUNT));
        }
    }

    #[test]
    #[ignore = "manual timing without machine-dependent assertions"]
    fn benchmark_cold_geometry() {
        let start = std::time::Instant::now();
        let mut max_frame = std::time::Duration::ZERO;
        let mut max_dots = 0;
        for step in 0..FRAME_COUNT {
            let frame_start = std::time::Instant::now();
            let dots = std::hint::black_box(generate(step));
            max_frame = max_frame.max(frame_start.elapsed());
            max_dots = max_dots.max(dots.len());
        }
        eprintln!(
            "cold all frames: {:?}, slowest frame {max_frame:?}, max {max_dots} dots",
            start.elapsed()
        );
    }

    #[test]
    fn cached_geometry_has_small_fixed_budget() {
        let total: usize = (0..FRAME_COUNT).map(|step| frame(step).len()).sum();
        assert!(total * std::mem::size_of::<Dot>() < 128 * 1024);
        let start = std::time::Instant::now();
        for step in 0..100_000 {
            std::hint::black_box(frame(step));
        }
        eprintln!(
            "100k cached lookups: {:?}, {total} dots cached",
            start.elapsed()
        );
    }
}
