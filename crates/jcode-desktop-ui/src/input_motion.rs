//! Composer motion: a typewriter placeholder that cycles example prompts and
//! a caret that glides between positions and breathes instead of blinking.
//!
//! Both are time functions evaluated at paint, driven by one bounded ticker.
//! Motion settles after inactivity (a solid caret and a fully typed example)
//! so an idle window returns to zero draws, and reduced motion disables it.
use std::time::{Duration, Instant};

use gpui::{Pixels, Point, point, px};

/// Example prompts shown behind an empty chat composer.
pub(super) const EXAMPLE_PROMPTS: &[&str] = &[
    "Find bugs in this repo",
    "Fix the flaky test in the Jcode TUI",
    "Make Jcode Desktop hot reload faster",
    "Spin up a swarm to review this PR",
    "Compare Claude and Codex on this diff",
    "Profile GPUI frame times on Wayland",
    "Why does niri drop this window's focus?",
    "Fix Handterm scrollback after a resize",
    "Wire Nari dictation into the composer",
    "Ask Jev to route my voice notes",
    "Make the harness API TypeSafe end to end",
    "Add a Copilot login route",
    "Switch this session from OpenAI to Anthropic",
    "Explain how hot reload swaps the UI plugin",
    "Write tests for the swarm task graph",
    "Commit and push my changes",
];

const TYPE_PER_CHAR: Duration = Duration::from_millis(42);
const HOLD: Duration = Duration::from_millis(2300);
const DELETE_PER_CHAR: Duration = Duration::from_millis(16);
const GAP: Duration = Duration::from_millis(420);
/// Typewriter cycling stops this long after the composer was last touched.
pub(super) const PLACEHOLDER_ACTIVE: Duration = Duration::from_secs(60);
/// Reduced motion swaps whole prompts at this interval instead of typing.
const STILL_ROTATE: Duration = Duration::from_secs(5);

/// Caret stays fully solid this long after it moves, so typing never flickers.
const CARET_SOLID: Duration = Duration::from_millis(550);
const CARET_PERIOD: f32 = 1.25;
/// Breathing stops (solid caret) after this long without input.
pub(super) const CARET_ACTIVE: Duration = Duration::from_secs(20);
const GLIDE: Duration = Duration::from_millis(90);

/// Tick interval while any motion is live.
pub(super) const TICK: Duration = Duration::from_millis(33);

/// Placeholder text at `elapsed` since the cycle began. Returns the visible
/// prefix and whether the cycle is still animating.
pub(super) fn placeholder_at(
    elapsed: Duration,
    seed: usize,
    reduce_motion: bool,
) -> (&'static str, bool) {
    let count = EXAMPLE_PROMPTS.len();
    if reduce_motion {
        let index = (seed + (elapsed.as_millis() / STILL_ROTATE.as_millis()) as usize) % count;
        return (EXAMPLE_PROMPTS[index], elapsed < PLACEHOLDER_ACTIVE);
    }
    if elapsed >= PLACEHOLDER_ACTIVE {
        // Settle on the prompt the cycle would show fully typed.
        return (EXAMPLE_PROMPTS[seed % count], false);
    }
    let mut t = elapsed;
    let mut index = seed;
    loop {
        let prompt = EXAMPLE_PROMPTS[index % count];
        let chars = prompt.chars().count() as u32;
        let typing = TYPE_PER_CHAR * chars;
        let deleting = DELETE_PER_CHAR * chars;
        let total = typing + HOLD + deleting + GAP;
        if t >= total {
            t -= total;
            index += 1;
            continue;
        }
        let shown = if t < typing {
            (t.as_millis() / TYPE_PER_CHAR.as_millis()) as usize + 1
        } else if t < typing + HOLD {
            chars as usize
        } else if t < typing + HOLD + deleting {
            let gone = ((t - typing - HOLD).as_millis() / DELETE_PER_CHAR.as_millis()) as usize;
            (chars as usize).saturating_sub(gone + 1)
        } else {
            0
        };
        let end = prompt
            .char_indices()
            .nth(shown)
            .map_or(prompt.len(), |(byte, _)| byte);
        return (&prompt[..end], true);
    }
}

/// Caret opacity at `since` the caret last moved. Smooth breathing, not an
/// on/off blink, and fully solid while the user is actively typing.
pub(super) fn caret_alpha(since: Duration, reduce_motion: bool) -> (f32, bool) {
    if reduce_motion || since >= CARET_ACTIVE {
        return (1.0, false);
    }
    if since < CARET_SOLID {
        return (1.0, true);
    }
    let phase = (since - CARET_SOLID).as_secs_f32() / CARET_PERIOD;
    let wave = 0.5 + 0.5 * (phase * std::f32::consts::TAU).cos();
    // Ease toward the extremes so the caret lingers visible, then dips.
    let eased = wave * wave * (3. - 2. * wave);
    (0.18 + 0.82 * eased, true)
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
    pub(super) fn retarget(&mut self, to: Point<Pixels>, line_height: Pixels, now: Instant, reduce_motion: bool) {
        if to == self.to {
            return;
        }
        let current = self.position(now).0;
        let far = (to.y - current.y).abs() > line_height * 0.5
            || (to.x - current.x).abs() > px(240.);
        self.from = if reduce_motion || far { to } else { current };
        self.to = to;
        self.start = now;
    }

    pub(super) fn position(&self, now: Instant) -> (Point<Pixels>, bool) {
        let t = (now.saturating_duration_since(self.start).as_secs_f32() / GLIDE.as_secs_f32())
            .clamp(0., 1.);
        if t >= 1. {
            return (self.to, false);
        }
        let eased = 1. - (1. - t).powi(3);
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

    #[test]
    fn placeholder_types_holds_deletes_and_advances() {
        let first = EXAMPLE_PROMPTS[0];
        let (start, live) = placeholder_at(Duration::ZERO, 0, false);
        assert!(live);
        assert_eq!(start, &first[..1]);
        let typed = TYPE_PER_CHAR * first.chars().count() as u32;
        assert_eq!(placeholder_at(typed + HOLD / 2, 0, false).0, first);
        let deleted = typed + HOLD + DELETE_PER_CHAR * first.chars().count() as u32;
        assert_eq!(placeholder_at(deleted + GAP / 2, 0, false).0, "");
        let second = placeholder_at(deleted + GAP + TYPE_PER_CHAR * 3, 0, false).0;
        assert!(EXAMPLE_PROMPTS[1].starts_with(second) && second.chars().count() == 4);
    }

    #[test]
    fn placeholder_settles_on_a_full_prompt_and_stops_ticking() {
        let (text, live) = placeholder_at(PLACEHOLDER_ACTIVE, 3, false);
        assert!(!live);
        assert_eq!(text, EXAMPLE_PROMPTS[3]);
        let (text, _) = placeholder_at(Duration::from_secs(6), 0, true);
        assert_eq!(text, EXAMPLE_PROMPTS[1], "reduced motion swaps whole prompts");
    }

    #[test]
    fn example_prompts_are_single_line_and_short() {
        for prompt in EXAMPLE_PROMPTS {
            assert!(!prompt.contains('\n'));
            assert!(prompt.chars().count() <= 48, "{prompt}");
        }
    }

    #[test]
    fn caret_breathes_smoothly_then_settles_solid() {
        assert_eq!(caret_alpha(Duration::from_millis(100), false), (1.0, true));
        let dim = caret_alpha(CARET_SOLID + Duration::from_secs_f32(CARET_PERIOD / 2.), false).0;
        assert!(dim < 0.25, "caret dips at mid-period: {dim}");
        // Continuous: neighbouring samples never jump like a hard blink.
        let mut last = 1.0;
        for ms in (550..3000).step_by(33) {
            let alpha = caret_alpha(Duration::from_millis(ms), false).0;
            assert!((alpha - last).abs() < 0.2, "jump at {ms}ms");
            last = alpha;
        }
        assert_eq!(caret_alpha(CARET_ACTIVE, false), (1.0, false));
        assert_eq!(caret_alpha(Duration::from_secs(2), true), (1.0, false));
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
    }
}
