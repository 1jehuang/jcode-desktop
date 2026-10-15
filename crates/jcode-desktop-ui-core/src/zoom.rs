use std::cell::Cell;

pub const MIN: f32 = 0.5;
pub const MAX: f32 = 3.0;
pub const DEFAULT: f32 = 1.0;
pub const LEVELS: [f32; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

const EPSILON: f32 = 0.001;

thread_local! {
    static CURRENT: Cell<Option<f32>> = const { Cell::new(None) };
}

pub fn normalize(scale: f32) -> f32 {
    if scale.is_finite() && (MIN..=MAX).contains(&scale) {
        scale
    } else {
        DEFAULT
    }
}

pub fn zoomed_in(scale: f32) -> f32 {
    LEVELS
        .into_iter()
        .find(|level| *level > scale + EPSILON)
        .unwrap_or(MAX)
}

pub fn zoomed_out(scale: f32) -> f32 {
    LEVELS
        .into_iter()
        .rev()
        .find(|level| *level < scale - EPSILON)
        .unwrap_or(MIN)
}

pub fn percent(scale: f32) -> u32 {
    (scale * 100.0).round() as u32
}

pub fn current() -> f32 {
    CURRENT
        .get()
        .unwrap_or_else(|| crate::config::get().appearance.ui_scale)
}

pub fn set_current(scale: f32) -> f32 {
    let scale = normalize(scale);
    CURRENT.set(Some(scale));
    scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_walk_the_browser_style_levels_and_stop_at_the_bounds() {
        let mut scale = DEFAULT;
        let mut ups = Vec::new();
        while scale < MAX {
            scale = zoomed_in(scale);
            ups.push(scale);
        }
        assert_eq!(ups, [1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0]);
        assert_eq!(zoomed_in(MAX), MAX);

        let mut downs = Vec::new();
        while scale > MIN {
            scale = zoomed_out(scale);
            downs.push(scale);
        }
        assert_eq!(
            downs,
            [
                2.5, 2.0, 1.75, 1.5, 1.25, 1.1, 1.0, 0.9, 0.8, 0.75, 0.67, 0.5
            ]
        );
        assert_eq!(zoomed_out(MIN), MIN);
    }

    #[test]
    fn a_hand_edited_scale_between_levels_steps_to_the_neighbouring_level() {
        assert_eq!(zoomed_in(1.3), 1.5);
        assert_eq!(zoomed_out(1.3), 1.25);
        assert_eq!(zoomed_in(1.2499), 1.5);
        assert_eq!(zoomed_out(1.2501), 1.1);
    }

    #[test]
    fn scales_outside_the_supported_range_fall_back_to_the_default() {
        for scale in [0.0, 0.49, 3.01, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(normalize(scale), DEFAULT, "{scale}");
        }
        for scale in [MIN, 1.5, MAX] {
            assert_eq!(normalize(scale), scale);
        }
    }

    #[test]
    fn the_current_level_is_per_thread() {
        set_current(2.0);
        let other = std::thread::spawn(current).join().unwrap();
        assert_eq!(other, DEFAULT);
        assert_eq!(current(), 2.0);
        set_current(DEFAULT);
    }

    #[test]
    fn percent_rounds_to_whole_numbers() {
        assert_eq!(percent(0.67), 67);
        assert_eq!(percent(1.1), 110);
        assert_eq!(percent(1.25), 125);
    }
}
