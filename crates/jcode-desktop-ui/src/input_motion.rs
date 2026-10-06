//! Composer motion: a typewriter placeholder that shows two example prompts and
//! a caret that glides between positions and breathes instead of blinking.
//!
//! Both are time functions evaluated at paint, driven by one bounded ticker.
//! Motion settles after inactivity (a solid caret and a fully typed example)
//! so an idle window returns to zero draws, and reduced motion disables it.
use std::time::{Duration, Instant};

use gpui::{Pixels, Point, point, px};

/// Generic example prompts. Shown when there is no personalized work to
/// suggest (see `crate::example_prompts`), so they must fit any project.
pub(super) const FALLBACK_PROMPTS: &[&str] = &[
    "Find bugs in this repo",
    "Explain how this codebase is structured",
    "Write tests for the most fragile code",
    "Fix the failing tests",
    "Review my uncommitted changes",
    "Refactor the messiest file here",
    "Document the public API",
    "Speed up the slowest part of this app",
    "Find dead code and remove it",
    "Make the error messages clearer",
    "Summarize what changed this week",
    "Commit and push my changes",
];

const TYPE_PER_CHAR: Duration = Duration::from_millis(42);
const HOLD: Duration = Duration::from_millis(2300);
const DELETE_PER_CHAR: Duration = Duration::from_millis(16);
const GAP: Duration = Duration::from_millis(420);
/// Reduced motion swaps to the second prompt after this long.
const STILL_ROTATE: Duration = Duration::from_secs(5);
/// How many catalog prompts one idle composer shows before settling.
const SHOWN_PROMPTS: usize = 2;

/// Caret stays fully solid this long after it moves, so typing never flickers.
const CARET_SOLID: Duration = Duration::from_millis(550);
const CARET_PERIOD: f32 = 1.25;
/// Breathing stops (solid caret) after this long without input.
pub(super) const CARET_ACTIVE: Duration = Duration::from_secs(20);
/// Short enough that the caret never trails the character just typed.
const GLIDE: Duration = Duration::from_millis(55);

/// Tick interval while any motion is live. Matches the orb and tab ring
/// period so all decorative ticks share one frame on the animation grid.
pub(super) const TICK: Duration = Duration::from_nanos(33_333_334);

/// The prompts one idle composer shows: personalized ones first, then the
/// generic fallback catalog for any remaining slot. `seed` rotates both.
pub(super) fn pick_prompts<S: AsRef<str>>(personal: &[S], seed: usize) -> [&str; SHOWN_PROMPTS] {
    std::array::from_fn(|step| {
        if step < personal.len() {
            personal[(seed + step) % personal.len()].as_ref()
        } else {
            FALLBACK_PROMPTS[(seed + step) % FALLBACK_PROMPTS.len()]
        }
    })
}

/// The first prompt is typed, held and deleted, then the second is typed
/// and stays. Returns the visible prefix, the prompt it belongs to, and
/// whether the placeholder is still animating.
pub(super) fn placeholder_at<'a>(
    elapsed: Duration,
    prompts: [&'a str; SHOWN_PROMPTS],
    reduce_motion: bool,
) -> (&'a str, &'a str, bool) {
    let pick = |step: usize| prompts[step];
    let last = pick(SHOWN_PROMPTS - 1);
    if reduce_motion {
        return if elapsed < STILL_ROTATE {
            (pick(0), pick(0), true)
        } else {
            (last, last, false)
        };
    }
    let mut t = elapsed;
    for step in 0..SHOWN_PROMPTS {
        let prompt = pick(step);
        let chars = prompt.chars().count() as u32;
        let typing = TYPE_PER_CHAR * chars;
        let final_prompt = step + 1 == SHOWN_PROMPTS;
        if final_prompt && t >= typing {
            return (prompt, prompt, false);
        }
        let total = typing + HOLD + DELETE_PER_CHAR * chars + GAP;
        if !final_prompt && t >= total {
            t -= total;
            continue;
        }
        let shown = if t < typing {
            (t.as_millis() / TYPE_PER_CHAR.as_millis()) as usize + 1
        } else if t < typing + HOLD {
            chars as usize
        } else if t < total - GAP {
            let gone = ((t - typing - HOLD).as_millis() / DELETE_PER_CHAR.as_millis()) as usize;
            (chars as usize).saturating_sub(gone + 1)
        } else {
            0
        };
        let end = prompt
            .char_indices()
            .nth(shown)
            .map_or(prompt.len(), |(byte, _)| byte);
        return (&prompt[..end], prompt, true);
    }
    (last, last, false)
}

/// Caret opacity at `since` the caret last moved, and how long until it next
/// changes. Smooth breathing, not an on/off blink, and fully solid while the
/// user is actively typing. The solid phase reports its remaining length
/// rather than asking for frames, because every composer frame re-renders the
/// whole chat panel and those redundant frames compete with keystrokes.
pub(super) fn caret_alpha(since: Duration, reduce_motion: bool) -> (f32, Option<Duration>) {
    if reduce_motion || since >= CARET_ACTIVE {
        return (1.0, None);
    }
    if since < CARET_SOLID {
        return (1.0, Some(CARET_SOLID - since));
    }
    let phase = (since - CARET_SOLID).as_secs_f32() / CARET_PERIOD;
    let wave = 0.5 + 0.5 * (phase * std::f32::consts::TAU).cos();
    // Ease toward the extremes so the caret lingers visible, then dips.
    let eased = wave * wave * (3. - 2. * wave);
    (0.18 + 0.82 * eased, Some(Duration::ZERO))
}

/// Caret glide from its previous position to the new one.
#[derive(Clone, Copy, Debug)]
pub(super) struct Glide {
    from: Point<Pixels>,
    to: Point<Pixels>,
    start: Instant,
}

impl Glide {
    pub(super) fn new(at: Point<Pixels>, now: Instant) -> Self {
        Self {
            from: at,
            to: at,
            start: now - GLIDE,
        }
    }

    /// Retarget toward `to`, starting from wherever the caret is drawn now.
    /// Large jumps (new line, click far away) snap rather than streak.
    pub(super) fn retarget(
        &mut self,
        to: Point<Pixels>,
        line_height: Pixels,
        now: Instant,
        reduce_motion: bool,
    ) {
        if to == self.to {
            return;
        }
        let current = self.position(now).0;
        let far =
            (to.y - current.y).abs() > line_height * 0.5 || (to.x - current.x).abs() > px(240.);
        self.from = if reduce_motion || far { to } else { current };
        self.to = to;
        self.start = now;
    }

    /// Jump straight to `to`. Typing uses this so the caret never trails the
    /// character just inserted.
    pub(super) fn snap(&mut self, to: Point<Pixels>, now: Instant) {
        *self = Self::new(to, now);
    }

    pub(super) fn position(&self, now: Instant) -> (Point<Pixels>, bool) {
        let t = (now.saturating_duration_since(self.start).as_secs_f32() / GLIDE.as_secs_f32())
            .clamp(0., 1.);
        if t >= 1. {
            return (self.to, false);
        }
        // Quartic ease-out: most of the travel lands in the first frame or two.
        let eased = 1. - (1. - t).powi(4);
        (
            point(
                self.from.x + (self.to.x - self.from.x) * eased,
                self.from.y + (self.to.y - self.from.y) * eased,
            ),
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: &[&str] = &[];

    #[test]
    fn placeholder_types_holds_deletes_and_advances() {
        let prompts = pick_prompts(NONE, 0);
        let first = FALLBACK_PROMPTS[0];
        let (start, full, live) = placeholder_at(Duration::ZERO, prompts, false);
        assert!(live);
        assert_eq!((start, full), (&first[..1], first));
        let typed = TYPE_PER_CHAR * first.chars().count() as u32;
        assert_eq!(placeholder_at(typed + HOLD / 2, prompts, false).0, first);
        let deleted = typed + HOLD + DELETE_PER_CHAR * first.chars().count() as u32;
        assert_eq!(placeholder_at(deleted + GAP / 2, prompts, false).0, "");
        let (second, full, _) = placeholder_at(deleted + GAP + TYPE_PER_CHAR * 3, prompts, false);
        assert_eq!(full, FALLBACK_PROMPTS[1]);
        assert!(full.starts_with(second) && second.chars().count() == 4);
    }

    #[test]
    fn placeholder_settles_on_the_second_prompt_and_stops_ticking() {
        let prompts = pick_prompts(NONE, 3);
        let first = FALLBACK_PROMPTS[3].chars().count() as u32;
        let second = FALLBACK_PROMPTS[4].chars().count() as u32;
        let settled =
            TYPE_PER_CHAR * first + HOLD + DELETE_PER_CHAR * first + GAP + TYPE_PER_CHAR * second;
        let done = (FALLBACK_PROMPTS[4], FALLBACK_PROMPTS[4], false);
        assert_eq!(placeholder_at(settled, prompts, false), done);
        assert_eq!(
            placeholder_at(settled * 10, prompts, false),
            done,
            "never rotates to a third prompt"
        );
        let prompts = pick_prompts(NONE, 0);
        assert_eq!(
            placeholder_at(Duration::from_secs(1), prompts, true),
            (FALLBACK_PROMPTS[0], FALLBACK_PROMPTS[0], true)
        );
        assert_eq!(
            placeholder_at(Duration::from_secs(6), prompts, true),
            (FALLBACK_PROMPTS[1], FALLBACK_PROMPTS[1], false)
        );
    }

    #[test]
    fn personalized_prompts_lead_and_fallbacks_fill_the_rest() {
        let one = ["Finish the login flow"];
        assert_eq!(
            pick_prompts(&one, 5),
            ["Finish the login flow", FALLBACK_PROMPTS[6]]
        );
        let two = ["Alpha task", "Beta task"];
        assert_eq!(pick_prompts(&two, 0), ["Alpha task", "Beta task"]);
        assert_eq!(pick_prompts(&two, 1), ["Beta task", "Alpha task"]);
    }

    #[test]
    fn fallback_prompts_are_single_line_and_short() {
        for prompt in FALLBACK_PROMPTS {
            assert!(!prompt.contains('\n'));
            assert!(prompt.chars().count() <= 48, "{prompt}");
        }
    }

    #[test]
    fn caret_breathes_smoothly_then_settles_solid() {
        assert_eq!(
            caret_alpha(Duration::from_millis(100), false),
            (1.0, Some(CARET_SOLID - Duration::from_millis(100))),
            "the solid phase sleeps until breathing starts instead of ticking"
        );
        assert_eq!(
            caret_alpha(CARET_SOLID + Duration::from_millis(1), false).1,
            Some(Duration::ZERO)
        );
        let dim = caret_alpha(
            CARET_SOLID + Duration::from_secs_f32(CARET_PERIOD / 2.),
            false,
        )
        .0;
        assert!(dim < 0.25, "caret dips at mid-period: {dim}");
        // Continuous: neighbouring samples never jump like a hard blink.
        let mut last = 1.0;
        for ms in (550..3000).step_by(33) {
            let alpha = caret_alpha(Duration::from_millis(ms), false).0;
            assert!((alpha - last).abs() < 0.2, "jump at {ms}ms");
            last = alpha;
        }
        assert_eq!(caret_alpha(CARET_ACTIVE, false), (1.0, None));
        assert_eq!(caret_alpha(Duration::from_secs(2), true), (1.0, None));
    }

    #[test]
    fn caret_glides_short_moves_and_snaps_line_changes() {
        let now = Instant::now();
        let mut glide = Glide::new(point(px(0.), px(0.)), now);
        glide.retarget(point(px(20.), px(0.)), px(18.), now, false);
        let (mid, live) = glide.position(now + GLIDE / 2);
        assert!(live && mid.x > px(0.) && mid.x < px(20.));
        assert_eq!(glide.position(now + GLIDE).0, point(px(20.), px(0.)));
        glide.retarget(point(px(4.), px(36.)), px(18.), now + GLIDE, false);
        assert_eq!(glide.position(now + GLIDE).0, point(px(4.), px(36.)));
        glide.retarget(point(px(40.), px(36.)), px(18.), now + GLIDE, false);
        glide.snap(point(px(48.), px(36.)), now + GLIDE);
        assert_eq!(
            glide.position(now + GLIDE),
            (point(px(48.), px(36.)), false)
        );
    }
}
