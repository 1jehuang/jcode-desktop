//! Learning: a model of which workspace actions the user does not yet know,
//! inferred from how they actually work, plus a coach that teaches at the
//! moment the knowledge would have paid off.
//!
//! The design rests on four ideas that a static cheat sheet cannot express.
//!
//! 1. **Knowing is not permanent.** Each skill carries a memory trace with a
//!    stability that decays over time, so a shortcut used once in March is not
//!    assumed known in August. Reinforcement follows the spacing effect: a
//!    successful use when recall was already weak strengthens the trace far
//!    more than a use while it was still fresh.
//!
//! 2. **Ignorance is observable.** A user who does not know a key does the same
//!    work the long way: clicking a panel instead of focusing it, or pressing
//!    "focus right" five times instead of jumping to the end. These slow paths
//!    are direct evidence of *not* knowing, and they are what drives the model
//!    down, rather than mere absence of use.
//!
//! 3. **Being told is not learning.** Using a key seconds after being shown it
//!    earns only a fraction of the credit of recalling it unaided. Otherwise
//!    the coach teaches once, sees a use, and wrongly concludes mastery.
//!
//! 4. **Teaching has a cost.** Hints interrupt. The coach spends a limited
//!    budget on the highest-value skill that is currently relevant and whose
//!    prerequisites are already mastered, and it backs off permanently from
//!    advice the user has repeatedly declined to take.
//!
//! The module is deliberately free of UI and clock dependencies: every entry
//! point takes the current time, so the whole system is directly testable.

use std::collections::HashMap;

/// Unix timestamp in seconds. Passed in rather than read from the clock so the
/// model is deterministic under test and can be persisted across restarts.
pub type Seconds = u64;

const SECONDS_PER_DAY: f32 = 86_400.0;

// --- Skill catalog ------------------------------------------------------

/// How a skill is grouped in the coach view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    Navigation,
    Arrangement,
    Sizing,
    Sessions,
}

impl Area {
    pub fn label(self) -> &'static str {
        match self {
            Area::Navigation => "navigation",
            Area::Arrangement => "arrangement",
            Area::Sizing => "sizing",
            Area::Sessions => "sessions",
        }
    }
}

/// One teachable action.
pub struct Skill {
    /// Stable identifier, also the persistence key.
    pub id: &'static str,
    /// The keys that invoke it, as shown to the user.
    pub keys: &'static str,
    /// What it does, in the user's terms.
    pub label: &'static str,
    /// How the same work gets done without it. Shown when correcting a slow
    /// path, so the hint explains what was just observed.
    pub instead_of: &'static str,
    pub area: Area,
    /// Value of knowing this, in [0, 1]. Weights which hint is worth an
    /// interruption when several are due.
    pub importance: f32,
    /// Skills that should be mastered first. A hint is withheld until its
    /// prerequisites are known, so the curriculum stays ordered.
    pub prerequisites: &'static [&'static str],
    /// Actions saved each time it is used instead of the slow path. Drives the
    /// wasted-effort accounting.
    pub effort_saved: u32,
}

/// The full curriculum. Ordering is irrelevant; prerequisites define the shape.
pub const SKILLS: &[Skill] = &[
    Skill {
        id: "focus_left_right",
        keys: "super-h / super-l",
        label: "focus the panel left or right",
        instead_of: "clicking a panel to focus it",
        area: Area::Navigation,
        importance: 1.0,
        prerequisites: &[],
        effort_saved: 1,
    },
    Skill {
        id: "focus_up_down",
        keys: "super-j / super-k",
        label: "move between strips",
        instead_of: "hunting for the strip in the overview",
        area: Area::Navigation,
        importance: 0.8,
        prerequisites: &["focus_left_right"],
        effort_saved: 2,
    },
    Skill {
        id: "focus_first_last",
        keys: "super-home / super-end",
        label: "jump to the first or last panel",
        instead_of: "pressing super-h or super-l repeatedly",
        area: Area::Navigation,
        importance: 0.6,
        prerequisites: &["focus_left_right"],
        effort_saved: 3,
    },
    Skill {
        id: "focus_previous",
        keys: "super-tab",
        label: "flip back to the panel you came from",
        instead_of: "navigating back by hand",
        area: Area::Navigation,
        importance: 0.9,
        prerequisites: &["focus_left_right"],
        effort_saved: 2,
    },
    Skill {
        id: "overview",
        keys: "super-o",
        label: "see every session at once",
        instead_of: "walking the strips to find a session",
        area: Area::Navigation,
        importance: 0.7,
        prerequisites: &[],
        effort_saved: 3,
    },
    Skill {
        id: "move_panel",
        keys: "super-shift-h / super-shift-l",
        label: "reorder the focused panel",
        instead_of: "leaving panels where they landed",
        area: Area::Arrangement,
        importance: 0.5,
        prerequisites: &["focus_left_right"],
        effort_saved: 2,
    },
    Skill {
        id: "move_panel_strip",
        keys: "super-shift-j / super-shift-k",
        label: "send the panel to another strip",
        instead_of: "keeping unrelated work on one strip",
        area: Area::Arrangement,
        importance: 0.5,
        prerequisites: &["focus_up_down"],
        effort_saved: 2,
    },
    Skill {
        id: "move_panel_end",
        keys: "super-shift-home / super-shift-end",
        label: "send the panel to the far end",
        instead_of: "pressing super-shift-l repeatedly",
        area: Area::Arrangement,
        importance: 0.35,
        prerequisites: &["move_panel"],
        effort_saved: 3,
    },
    Skill {
        id: "cycle_width",
        keys: "super-r",
        label: "cycle the panel width preset",
        instead_of: "reaching for a specific width key",
        area: Area::Sizing,
        importance: 0.6,
        prerequisites: &[],
        effort_saved: 1,
    },
    Skill {
        id: "maximize",
        keys: "super-f",
        label: "fill the window, then restore",
        instead_of: "setting 100% and sizing back by hand",
        area: Area::Sizing,
        importance: 0.7,
        prerequisites: &[],
        effort_saved: 2,
    },
    Skill {
        id: "width_presets",
        keys: "super-1 .. super-4",
        label: "set an exact panel width",
        instead_of: "cycling past the width you wanted",
        area: Area::Sizing,
        importance: 0.4,
        prerequisites: &["cycle_width"],
        effort_saved: 1,
    },
    Skill {
        id: "new_panel",
        keys: "super-n",
        label: "open a session right of this one",
        instead_of: "clicking the new session card",
        area: Area::Sessions,
        importance: 0.9,
        prerequisites: &[],
        effort_saved: 2,
    },
    Skill {
        id: "close_panel",
        keys: "super-q",
        label: "close the focused panel",
        instead_of: "leaving finished sessions open",
        area: Area::Sessions,
        importance: 0.6,
        prerequisites: &[],
        effort_saved: 1,
    },
];

pub fn skill(id: &str) -> Option<&'static Skill> {
    SKILLS.iter().find(|skill| skill.id == id)
}

// --- Evidence -----------------------------------------------------------

/// An observation about one skill. The variants are deliberately distinct
/// because they carry very different amounts of information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// Used the key unaided. The strongest evidence of real knowledge.
    Recalled,
    /// Used the key shortly after being shown it. Counts for little: the
    /// answer was on screen.
    Copied,
    /// Did the work the long way while the key existed. Direct evidence of
    /// not knowing, or of having forgotten.
    SlowPath,
    /// A hint was shown and the user carried on without taking it up.
    HintDeclined,
}

// --- Memory trace -------------------------------------------------------

/// Minimum stability in days. Keeps a lapsed trace from decaying to nothing
/// and makes reinforcement well-behaved.
const MIN_STABILITY: f32 = 0.25;
const MAX_STABILITY: f32 = 365.0;
const MIN_DIFFICULTY: f32 = 1.0;
const MAX_DIFFICULTY: f32 = 10.0;
const START_DIFFICULTY: f32 = 5.0;

/// The memory trace for a single skill.
#[derive(Debug, Clone)]
pub struct Trace {
    /// Belief in [0, 1] that the user has ever actually learned this,
    /// independent of whether they would recall it right now.
    pub belief: f32,
    /// Stability in days: how slowly recall decays. Grows with successful
    /// unaided use, collapses on a lapse.
    pub stability: f32,
    /// Intrinsic difficulty in [1, 10], in the manner of FSRS. Rises when the
    /// user keeps falling back to the slow path.
    pub difficulty: f32,
    pub last_evidence_at: Option<Seconds>,
    pub recalled: u32,
    pub copied: u32,
    pub slow_paths: u32,
    pub hints_shown: u32,
    pub hints_declined: u32,
    pub last_hint_at: Option<Seconds>,
}

impl Default for Trace {
    fn default() -> Self {
        Self {
            // Unknown until shown otherwise: the coach assumes nothing.
            belief: 0.0,
            stability: MIN_STABILITY,
            difficulty: START_DIFFICULTY,
            last_evidence_at: None,
            recalled: 0,
            copied: 0,
            slow_paths: 0,
            hints_shown: 0,
            hints_declined: 0,
            last_hint_at: None,
        }
    }
}

impl Trace {
    /// Probability of recalling the skill unaided right now, given how long it
    /// has been since the trace was last reinforced.
    ///
    /// This is the FSRS power-law forgetting curve, `R = (1 + t/(9S))^-1`,
    /// rather than a simple exponential: real forgetting has a long tail, so a
    /// well-established shortcut stays available for months while a shaky one
    /// fades within days.
    pub fn retrievability(&self, now: Seconds) -> f32 {
        let Some(last) = self.last_evidence_at else {
            return 0.0;
        };
        let days = now.saturating_sub(last) as f32 / SECONDS_PER_DAY;
        1.0 / (1.0 + days / (9.0 * self.stability))
    }

    /// How well the user knows this skill right now, in [0, 1]: they must both
    /// have learned it and still be able to recall it.
    pub fn mastery(&self, now: Seconds) -> f32 {
        self.belief * self.retrievability(now)
    }

    /// True once the user has stopped taking this advice. The coach then leaves
    /// them alone: declining three hints without ever using the key is a
    /// decision, not a gap.
    pub fn retired(&self) -> bool {
        self.hints_declined >= 3 && self.recalled == 0
    }

    fn record(&mut self, evidence: Evidence, now: Seconds) {
        match evidence {
            Evidence::Recalled => {
                let retrievability = self.retrievability(now);
                self.belief += (1.0 - self.belief) * 0.55;
                // The spacing effect: reinforcing a trace that had already
                // faded teaches far more than restating a fresh one. At high
                // retrievability the gain approaches nothing.
                let surprise = 1.0 - retrievability;
                let ease = 11.0 - self.difficulty; // 1 (hard) .. 10 (easy)
                let growth = 1.0 + (ease / 10.0) * (0.4 + 2.6 * surprise);
                self.stability = (self.stability * growth).clamp(MIN_STABILITY, MAX_STABILITY);
                self.difficulty = (self.difficulty - 0.15).clamp(MIN_DIFFICULTY, MAX_DIFFICULTY);
                self.recalled += 1;
                self.last_evidence_at = Some(now);
            }
            Evidence::Copied => {
                // Reading a hint and typing what it said is weak evidence:
                // enough to stop nagging immediately, not enough to claim the
                // skill is learned.
                self.belief += (1.0 - self.belief) * 0.12;
                self.stability = (self.stability * 1.15).clamp(MIN_STABILITY, MAX_STABILITY);
                self.copied += 1;
                self.last_evidence_at = Some(now);
            }
            Evidence::SlowPath => {
                // A lapse. How much it counts against the user depends on how
                // well established the skill was: someone who has used a key
                // unaided for weeks and then reaches for the mouse once is
                // probably choosing convenience, not revealing ignorance, so a
                // single slip must not undo a demonstrated habit. A shaky
                // skill, by contrast, is likely genuinely unknown.
                let established = (self.recalled as f32 / 5.0).clamp(0.0, 1.0);
                let belief_penalty = 0.5 + 0.42 * established;
                let stability_penalty = 0.35 + 0.5 * established;
                self.belief *= belief_penalty;
                self.stability =
                    (self.stability * stability_penalty).clamp(MIN_STABILITY, MAX_STABILITY);
                self.difficulty = (self.difficulty + 0.4).clamp(MIN_DIFFICULTY, MAX_DIFFICULTY);
                self.slow_paths += 1;
                self.last_evidence_at = Some(now);
            }
            Evidence::HintDeclined => {
                self.hints_declined += 1;
            }
        }
    }
}

// --- Hints --------------------------------------------------------------

/// A teachable moment the coach has decided is worth an interruption.
#[derive(Debug, Clone)]
pub struct Hint {
    pub skill_id: &'static str,
    pub keys: &'static str,
    pub label: &'static str,
    /// Why it is being shown now, phrased from the observed behavior.
    pub because: String,
    pub shown_at: Seconds,
}

/// A hint is only credited as "taken" if the key is used soon after it appears;
/// past that, using the key counts as unaided recall again.
const CREDIT_WINDOW: Seconds = 25;
/// A hint stays on screen this long.
const HINT_LIFETIME: Seconds = 9;
/// Minimum quiet period between any two hints, so the coach never chatters.
const HINT_COOLDOWN: Seconds = 45;
/// Minimum gap before the same skill may be taught again.
const PER_SKILL_COOLDOWN: Seconds = 600;
/// Hints per session. Teaching more than this in one sitting is nagging.
const SESSION_HINT_BUDGET: u32 = 6;
/// Above this mastery a skill is considered known and is not taught.
const KNOWN_THRESHOLD: f32 = 0.7;

// --- Slow-path detection ------------------------------------------------

/// Repeats of one action within this window count as a single "grinding" run.
const REPEAT_WINDOW: Seconds = 6;
/// Repeats of a single-step action before the compound shortcut is treated as
/// the thing they should have known.
const REPEAT_THRESHOLD: u32 = 3;

/// A single-step action and the compound skill that replaces grinding on it.
struct Compound {
    step: &'static str,
    compound: &'static str,
}

const COMPOUNDS: &[Compound] = &[
    Compound {
        step: "focus_left_right",
        compound: "focus_first_last",
    },
    Compound {
        step: "move_panel",
        compound: "move_panel_end",
    },
    Compound {
        step: "cycle_width",
        compound: "width_presets",
    },
];

// --- The coach ----------------------------------------------------------

/// Tracks what the user knows and decides what, if anything, to teach.
pub struct Coach {
    traces: HashMap<String, Trace>,
    /// The hint currently on screen.
    active: Option<Hint>,
    /// Skill whose hint was shown most recently, awaiting a use or a decline.
    pending_credit: Option<(&'static str, Seconds)>,
    last_hint_at: Option<Seconds>,
    hints_this_session: u32,
    /// Consecutive repeats of one skill's action, for grinding detection.
    repeat: Option<(&'static str, u32, Seconds)>,
    /// Actions the user has spent on slow paths that a shortcut would have
    /// saved. Concrete and cumulative, so progress is legible.
    pub effort_wasted: u32,
    /// Actions saved by using shortcuts, for the same reason.
    pub effort_saved: u32,
    dirty: bool,
}

impl Default for Coach {
    fn default() -> Self {
        Self::new()
    }
}

impl Coach {
    pub fn new() -> Self {
        Self {
            traces: HashMap::new(),
            active: None,
            pending_credit: None,
            last_hint_at: None,
            hints_this_session: 0,
            repeat: None,
            effort_wasted: 0,
            effort_saved: 0,
            dirty: false,
        }
    }

    pub fn trace(&self, id: &str) -> Trace {
        self.traces.get(id).cloned().unwrap_or_default()
    }

    fn trace_mut(&mut self, id: &str) -> &mut Trace {
        self.dirty = true;
        self.traces.entry(id.to_string()).or_default()
    }

    /// Whether the model has changed since it was last persisted.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    pub fn mastery(&self, id: &str, now: Seconds) -> f32 {
        self.traces
            .get(id)
            .map(|trace| trace.mastery(now))
            .unwrap_or(0.0)
    }

    /// Overall command of the workspace, weighted by how much each skill
    /// matters. Reported to the user as a single figure.
    pub fn overall_mastery(&self, now: Seconds) -> f32 {
        let total: f32 = SKILLS.iter().map(|skill| skill.importance).sum();
        if total <= 0.0 {
            return 0.0;
        }
        let earned: f32 = SKILLS
            .iter()
            .map(|skill| skill.importance * self.mastery(skill.id, now))
            .sum();
        earned / total
    }

    // --- Observation ---------------------------------------------------

    /// The user invoked `skill_id` by its keys. Credit depends on whether they
    /// were just told: recalling unaided is what proves knowledge.
    pub fn used_shortcut(&mut self, skill_id: &str, now: Seconds) {
        let prompted = self
            .pending_credit
            .is_some_and(|(id, at)| id == skill_id && now.saturating_sub(at) <= CREDIT_WINDOW);
        if prompted {
            self.pending_credit = None;
            // Taking the advice retires the hint without counting as a decline.
            if self.active.as_ref().is_some_and(|h| h.skill_id == skill_id) {
                self.active = None;
            }
        }
        let evidence = if prompted {
            Evidence::Copied
        } else {
            Evidence::Recalled
        };
        self.trace_mut(skill_id).record(evidence, now);
        if let Some(skill) = skill(skill_id) {
            self.effort_saved += skill.effort_saved;
        }
        self.note_repeat(skill_id, now);
    }

    /// The user achieved `skill_id`'s outcome the long way. This is the signal
    /// that actually drives teaching, since it proves both the gap and that the
    /// skill is relevant to what they are doing right now.
    pub fn used_slow_path(&mut self, skill_id: &str, now: Seconds) {
        self.trace_mut(skill_id).record(Evidence::SlowPath, now);
        if let Some(skill) = skill(skill_id) {
            self.effort_wasted += skill.effort_saved;
        }
        self.repeat = None;
        self.consider_hint(skill_id, now);
    }

    /// Track runs of one action so that grinding on a single-step key is read
    /// as not knowing the compound one. Pressing "focus right" five times is
    /// not evidence of skill; it is evidence of a missing shortcut.
    fn note_repeat(&mut self, skill_id: &str, now: Seconds) {
        let Some(compound) = COMPOUNDS.iter().find(|entry| entry.step == skill_id) else {
            self.repeat = None;
            return;
        };
        let step = compound.step;
        let count = match self.repeat {
            Some((id, count, at)) if id == step && now.saturating_sub(at) <= REPEAT_WINDOW => {
                count + 1
            }
            _ => 1,
        };
        self.repeat = Some((step, count, now));
        if count >= REPEAT_THRESHOLD {
            self.repeat = None;
            let target = compound.compound;
            self.trace_mut(target).record(Evidence::SlowPath, now);
            if let Some(skill) = skill(target) {
                self.effort_wasted += skill.effort_saved;
            }
            self.consider_hint(target, now);
        }
    }

    // --- Teaching ------------------------------------------------------

    /// True when every prerequisite is known. Teaching "send the panel to
    /// another strip" to someone who cannot yet move between strips would not
    /// land, so the curriculum stays ordered.
    fn prerequisites_met(&self, skill: &Skill, now: Seconds) -> bool {
        skill
            .prerequisites
            .iter()
            .all(|id| self.mastery(id, now) >= KNOWN_THRESHOLD)
    }

    /// Value of teaching `skill` right now. `relevant` marks the skill the user
    /// just needed, which is what makes a hint land rather than annoy.
    fn hint_value(&self, skill: &Skill, relevant: bool, now: Seconds) -> f32 {
        let trace = self.trace(skill.id);
        if trace.retired() || !self.prerequisites_met(skill, now) {
            return 0.0;
        }
        let mastery = trace.mastery(now);
        if mastery >= KNOWN_THRESHOLD {
            return 0.0;
        }
        // Repeatedly declining without retiring still dampens eagerness.
        let patience = 1.0 / (1.0 + trace.hints_declined as f32);
        let urgency = if relevant { 1.0 } else { 0.35 };
        skill.importance * (1.0 - mastery) * urgency * patience
    }

    /// Decide whether to teach `skill_id` now, given the interruption budget.
    fn consider_hint(&mut self, skill_id: &str, now: Seconds) {
        if self.hints_this_session >= SESSION_HINT_BUDGET {
            return;
        }
        if self
            .last_hint_at
            .is_some_and(|at| now.saturating_sub(at) < HINT_COOLDOWN)
        {
            return;
        }
        let Some(skill) = skill(skill_id) else {
            return;
        };
        let trace = self.trace(skill.id);
        if trace
            .last_hint_at
            .is_some_and(|at| now.saturating_sub(at) < PER_SKILL_COOLDOWN)
        {
            return;
        }
        if self.hint_value(skill, true, now) <= 0.0 {
            return;
        }
        self.show(skill, now);
    }

    fn show(&mut self, skill: &'static Skill, now: Seconds) {
        // An unanswered previous hint was not taken up.
        self.decline_pending(now);
        self.active = Some(Hint {
            skill_id: skill.id,
            keys: skill.keys,
            label: skill.label,
            because: format!("instead of {}", skill.instead_of),
            shown_at: now,
        });
        self.pending_credit = Some((skill.id, now));
        self.last_hint_at = Some(now);
        self.hints_this_session += 1;
        let trace = self.trace_mut(skill.id);
        trace.hints_shown += 1;
        trace.last_hint_at = Some(now);
    }

    fn decline_pending(&mut self, now: Seconds) {
        if let Some((id, at)) = self.pending_credit.take()
            && now.saturating_sub(at) > CREDIT_WINDOW
        {
            self.trace_mut(id).record(Evidence::HintDeclined, now);
        }
    }

    /// The hint to draw, if any. Also expires stale hints and records that a
    /// hint whose window has closed was not acted on.
    pub fn active_hint(&mut self, now: Seconds) -> Option<Hint> {
        if let Some(hint) = &self.active
            && now.saturating_sub(hint.shown_at) > HINT_LIFETIME
        {
            self.active = None;
        }
        if self
            .pending_credit
            .is_some_and(|(_, at)| now.saturating_sub(at) > CREDIT_WINDOW)
        {
            self.decline_pending(now);
        }
        self.active.clone()
    }

    /// Dismiss the visible hint without judging it either way.
    pub fn dismiss_hint(&mut self) {
        self.active = None;
    }

    /// The skill currently being taught, without touching expiry. For
    /// diagnostics and automated observation of the running app.
    pub fn active_hint_id(&self) -> Option<&'static str> {
        self.active.as_ref().map(|hint| hint.skill_id)
    }

    /// The next thing worth learning, ignoring immediate relevance. Drives the
    /// coach view so the user can see where they are headed.
    pub fn next_lesson(&self, now: Seconds) -> Option<&'static Skill> {
        SKILLS
            .iter()
            .filter(|skill| self.hint_value(skill, false, now) > 0.0)
            .max_by(|a, b| {
                self.hint_value(a, false, now)
                    .total_cmp(&self.hint_value(b, false, now))
            })
    }

    /// Skills grouped for display, ordered weakest first within each area so
    /// the gaps are what stand out.
    pub fn report(&self, now: Seconds) -> Vec<(Area, Vec<(&'static Skill, f32)>)> {
        let areas = [
            Area::Navigation,
            Area::Arrangement,
            Area::Sizing,
            Area::Sessions,
        ];
        areas
            .into_iter()
            .map(|area| {
                let mut rows: Vec<_> = SKILLS
                    .iter()
                    .filter(|skill| skill.area == area)
                    .map(|skill| (skill, self.mastery(skill.id, now)))
                    .collect();
                rows.sort_by(|a, b| a.1.total_cmp(&b.1));
                (area, rows)
            })
            .collect()
    }

    // --- Persistence ---------------------------------------------------

    /// Serialize to a compact line-based form. One skill per line keeps the
    /// file readable and lets unknown ids be dropped harmlessly when the
    /// catalog changes.
    pub fn serialize(&self) -> String {
        let mut lines = vec![format!(
            "v1 effort {} {}",
            self.effort_saved, self.effort_wasted
        )];
        let mut ids: Vec<_> = self.traces.keys().collect();
        ids.sort();
        for id in ids {
            let trace = &self.traces[id];
            lines.push(format!(
                "skill {} {:.4} {:.4} {:.4} {} {} {} {} {} {}",
                id,
                trace.belief,
                trace.stability,
                trace.difficulty,
                trace.last_evidence_at.unwrap_or(0),
                trace.recalled,
                trace.copied,
                trace.slow_paths,
                trace.hints_shown,
                trace.hints_declined,
            ));
        }
        lines.join("\n")
    }

    /// Parse what `serialize` wrote, ignoring anything unrecognized so a
    /// forward-incompatible or hand-edited file degrades to a fresh model
    /// rather than failing to start.
    pub fn deserialize(text: &str) -> Self {
        let mut coach = Coach::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            match fields.as_slice() {
                ["v1", "effort", saved, wasted] => {
                    coach.effort_saved = saved.parse().unwrap_or(0);
                    coach.effort_wasted = wasted.parse().unwrap_or(0);
                }
                [
                    "skill",
                    id,
                    belief,
                    stability,
                    difficulty,
                    last,
                    recalled,
                    copied,
                    slow,
                    shown,
                    declined,
                ] => {
                    // Drop skills that have left the catalog.
                    if skill(id).is_none() {
                        continue;
                    }
                    let last: Seconds = last.parse().unwrap_or(0);
                    let trace = Trace {
                        belief: belief.parse().unwrap_or(0.0),
                        stability: stability
                            .parse()
                            .unwrap_or(MIN_STABILITY)
                            .clamp(MIN_STABILITY, MAX_STABILITY),
                        difficulty: difficulty
                            .parse()
                            .unwrap_or(START_DIFFICULTY)
                            .clamp(MIN_DIFFICULTY, MAX_DIFFICULTY),
                        last_evidence_at: (last > 0).then_some(last),
                        recalled: recalled.parse().unwrap_or(0),
                        copied: copied.parse().unwrap_or(0),
                        slow_paths: slow.parse().unwrap_or(0),
                        hints_shown: shown.parse().unwrap_or(0),
                        hints_declined: declined.parse().unwrap_or(0),
                        last_hint_at: None,
                    };
                    coach.traces.insert(id.to_string(), trace);
                }
                _ => {}
            }
        }
        coach.dirty = false;
        coach
    }
}

/// Where the model lives between runs.
pub fn state_path() -> Option<std::path::PathBuf> {
    if let Some(base) = std::env::var_os("XDG_STATE_HOME") {
        return Some(
            std::path::PathBuf::from(base)
                .join("jcode-desktop")
                .join("learning"),
        );
    }
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    #[cfg(target_os = "macos")]
    let base = home.join("Library/Application Support/Jcode");
    #[cfg(not(target_os = "macos"))]
    let base = home.join(".local/state/jcode-desktop");
    Some(base.join("learning"))
}

pub fn load() -> Coach {
    state_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| Coach::deserialize(&text))
        .unwrap_or_default()
}

pub fn save(coach: &Coach) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, coach.serialize());
}

/// The wall clock, as the model wants it.
pub fn now() -> Seconds {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Seconds = 86_400;

    #[test]
    fn catalog_prerequisites_all_resolve() {
        for entry in SKILLS {
            for prerequisite in entry.prerequisites {
                assert!(
                    skill(prerequisite).is_some(),
                    "{} requires unknown skill {prerequisite}",
                    entry.id
                );
                assert_ne!(*prerequisite, entry.id, "{} requires itself", entry.id);
            }
        }
        for entry in COMPOUNDS {
            assert!(skill(entry.step).is_some(), "unknown step {}", entry.step);
            assert!(
                skill(entry.compound).is_some(),
                "unknown compound {}",
                entry.compound
            );
        }
    }

    #[test]
    fn an_unseen_skill_is_assumed_unknown() {
        let coach = Coach::new();
        assert_eq!(coach.mastery("maximize", 0), 0.0);
        assert_eq!(coach.overall_mastery(0), 0.0);
    }

    #[test]
    fn recall_builds_mastery_and_time_erodes_it() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_shortcut("maximize", start);
        let fresh = coach.mastery("maximize", start);
        assert!(fresh > 0.5, "a use should establish the skill: {fresh}");

        // Same skill, unused for a month: recall should have faded materially.
        let later = coach.mastery("maximize", start + 30 * DAY);
        assert!(
            later < fresh * 0.5,
            "an unused shortcut must decay: {fresh} -> {later}"
        );
    }

    #[test]
    fn repeated_spaced_use_outlasts_repeated_immediate_use() {
        // The spacing effect: the same number of uses spread out must leave a
        // more durable trace than the same uses in quick succession.
        //
        // Comparing mastery at a fixed date would not show this, because the
        // spaced learner's last use is also more recent, so recency alone would
        // carry the assertion. Two things are compared instead: the stability
        // the practice built, and mastery measured the same distance *after
        // each learner's final use*, which holds recency equal.
        let start = 1_000_000;
        let mut crammed = Coach::new();
        for step in 0..4 {
            crammed.used_shortcut("maximize", start + step * 10);
        }
        let crammed_last = start + 3 * 10;

        let mut spaced = Coach::new();
        for step in 0..4 {
            spaced.used_shortcut("maximize", start + step * 5 * DAY);
        }
        let spaced_last = start + 3 * 5 * DAY;

        let crammed_stability = crammed.trace("maximize").stability;
        let spaced_stability = spaced.trace("maximize").stability;
        assert!(
            spaced_stability > crammed_stability * 1.5,
            "spacing should build substantially more stability: \
             {spaced_stability} vs {crammed_stability}"
        );

        // Equal time since each learner's last use: only durability differs.
        let elapsed = 30 * DAY;
        let spaced_retained = spaced.mastery("maximize", spaced_last + elapsed);
        let crammed_retained = crammed.mastery("maximize", crammed_last + elapsed);
        assert!(
            spaced_retained > crammed_retained,
            "at equal recency, spaced practice should be better retained: \
             {spaced_retained} vs {crammed_retained}"
        );
    }

    #[test]
    fn being_told_earns_less_credit_than_recalling() {
        let start = 1_000_000;

        // Prompted: the hint fires from a slow path, then the key is used.
        let mut prompted = Coach::new();
        prompted.used_slow_path("maximize", start);
        assert!(prompted.active_hint(start).is_some(), "hint should show");
        prompted.used_shortcut("maximize", start + 3);

        // Unaided: the same single use with no hint on screen.
        let mut unaided = Coach::new();
        unaided.used_shortcut("maximize", start + 3);

        assert!(
            unaided.mastery("maximize", start + 3) > prompted.mastery("maximize", start + 3),
            "copying a hint must not count as knowing it"
        );
        assert_eq!(prompted.trace("maximize").copied, 1);
        assert_eq!(prompted.trace("maximize").recalled, 0);
        assert_eq!(unaided.trace("maximize").recalled, 1);
    }

    #[test]
    fn using_the_key_long_after_a_hint_counts_as_recall() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_slow_path("maximize", start);
        coach.used_shortcut("maximize", start + CREDIT_WINDOW + 5);
        assert_eq!(coach.trace("maximize").recalled, 1);
        assert_eq!(coach.trace("maximize").copied, 0);
    }

    #[test]
    fn slow_paths_teach_and_lower_mastery() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_shortcut("new_panel", start);
        let known = coach.mastery("new_panel", start);

        // A month later they click the card instead: evidence of forgetting.
        let later = start + 30 * DAY;
        coach.used_slow_path("new_panel", later);
        assert!(
            coach.mastery("new_panel", later) < known,
            "a slow path must reduce mastery"
        );
        let hint = coach.active_hint(later).expect("slow path should teach");
        assert_eq!(hint.skill_id, "new_panel");
        assert!(hint.because.contains("instead of"));
    }

    #[test]
    fn grinding_a_single_step_teaches_the_compound_shortcut() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        // Establish the prerequisite so the lesson is allowed to surface.
        for step in 0..6 {
            coach.used_shortcut("focus_left_right", start + step * 3 * DAY);
        }
        let run = start + 25 * DAY;
        assert!(coach.mastery("focus_left_right", run) >= KNOWN_THRESHOLD);

        // Walking the strip one panel at a time.
        coach.used_shortcut("focus_left_right", run);
        coach.used_shortcut("focus_left_right", run + 1);
        coach.used_shortcut("focus_left_right", run + 2);

        let hint = coach.active_hint(run + 2).expect("grinding should teach");
        assert_eq!(hint.skill_id, "focus_first_last");
        assert!(coach.effort_wasted > 0, "grinding should count as waste");
    }

    #[test]
    fn slow_but_deliberate_single_steps_are_not_grinding() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        for step in 0..6 {
            coach.used_shortcut("focus_left_right", start + step * 3 * DAY);
        }
        let run = start + 25 * DAY;
        // Spread beyond the repeat window: ordinary navigation, not a run.
        coach.used_shortcut("focus_left_right", run);
        coach.used_shortcut("focus_left_right", run + REPEAT_WINDOW + 1);
        coach.used_shortcut("focus_left_right", run + 2 * (REPEAT_WINDOW + 1));
        assert!(coach.active_hint(run + 30).is_none(), "should not nag");
    }

    #[test]
    fn lessons_wait_for_their_prerequisites() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        // Moving a panel between strips needs strip navigation first.
        coach.used_slow_path("move_panel_strip", start);
        assert!(
            coach.active_hint(start).is_none(),
            "should not teach past a missing prerequisite"
        );

        // Learn the prerequisites, and the lesson becomes available.
        for step in 0..5 {
            coach.used_shortcut("focus_left_right", start + step * 2 * DAY);
        }
        for step in 0..5 {
            coach.used_shortcut("focus_up_down", start + step * 2 * DAY);
        }
        let later = start + 20 * DAY;
        coach.used_slow_path("move_panel_strip", later);
        assert_eq!(
            coach.active_hint(later).map(|hint| hint.skill_id),
            Some("move_panel_strip")
        );
    }

    #[test]
    fn hints_respect_a_cooldown_and_a_session_budget() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_slow_path("new_panel", start);
        assert!(coach.active_hint(start).is_some());
        coach.dismiss_hint();

        // A different gap immediately afterwards must stay quiet.
        coach.used_slow_path("close_panel", start + 1);
        assert!(
            coach.active_hint(start + 1).is_none(),
            "hints must not chatter"
        );

        // Well past the cooldown it may teach again.
        let later = start + HINT_COOLDOWN + 1;
        coach.used_slow_path("close_panel", later);
        assert!(coach.active_hint(later).is_some());
    }

    #[test]
    fn the_same_skill_is_not_retaught_immediately() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_slow_path("new_panel", start);
        assert!(coach.active_hint(start).is_some());
        coach.dismiss_hint();

        let soon = start + HINT_COOLDOWN + 1;
        coach.used_slow_path("new_panel", soon);
        assert!(
            coach.active_hint(soon).is_none(),
            "the same lesson should wait out its own cooldown"
        );
    }

    #[test]
    fn declined_advice_is_eventually_dropped() {
        let mut coach = Coach::new();
        let mut when = 1_000_000;
        for _ in 0..3 {
            coach.used_slow_path("new_panel", when);
            assert!(
                coach.active_hint(when).is_some(),
                "expected a hint at {when}"
            );
            // Walk past the credit window without using the key: declined.
            when += CREDIT_WINDOW + 5;
            let _ = coach.active_hint(when);
            when += PER_SKILL_COOLDOWN + 1;
        }
        assert!(
            coach.trace("new_panel").retired(),
            "three refusals should retire the lesson"
        );
        coach.used_slow_path("new_panel", when);
        assert!(
            coach.active_hint(when).is_none(),
            "a retired lesson must stay silent"
        );
    }

    #[test]
    fn taking_the_advice_is_not_recorded_as_a_refusal() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_slow_path("new_panel", start);
        assert!(coach.active_hint(start).is_some());
        coach.used_shortcut("new_panel", start + 2);
        // Long after, the pending credit must not turn into a decline.
        let _ = coach.active_hint(start + 10 * 60);
        assert_eq!(coach.trace("new_panel").hints_declined, 0);
        assert!(coach.active_hint(start + 10 * 60).is_none());
    }

    #[test]
    fn a_hint_expires_on_its_own() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_slow_path("new_panel", start);
        assert!(coach.active_hint(start + 1).is_some());
        assert!(coach.active_hint(start + HINT_LIFETIME + 1).is_none());
    }

    #[test]
    fn known_skills_are_not_taught() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        for step in 0..5 {
            coach.used_shortcut("new_panel", start + step * 2 * DAY);
        }
        let now = start + 10 * DAY;
        assert!(coach.mastery("new_panel", now) >= KNOWN_THRESHOLD);
        // A stray click on a well-known action should not trigger a lecture.
        assert!(coach.hint_value(skill("new_panel").unwrap(), true, now) <= 0.0);
    }

    #[test]
    fn the_next_lesson_follows_importance_and_ordering() {
        let coach = Coach::new();
        let lesson = coach.next_lesson(1_000_000).expect("something to learn");
        // Nothing is known yet, so the first lesson must be one with no
        // prerequisites, and the most important such skill.
        assert!(lesson.prerequisites.is_empty());
        let best = SKILLS
            .iter()
            .filter(|skill| skill.prerequisites.is_empty())
            .map(|skill| skill.importance)
            .fold(0.0f32, f32::max);
        assert!((lesson.importance - best).abs() < f32::EPSILON);
    }

    #[test]
    fn effort_is_accounted_on_both_sides() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_slow_path("new_panel", start);
        coach.used_shortcut("new_panel", start + 1);
        assert_eq!(
            coach.effort_wasted,
            skill("new_panel").unwrap().effort_saved
        );
        assert_eq!(coach.effort_saved, skill("new_panel").unwrap().effort_saved);
    }

    #[test]
    fn a_hint_is_only_credited_once_even_if_the_key_repeats() {
        // Taking the advice should read as one prompted use, not as a run of
        // confident recalls that would fake mastery.
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_slow_path("maximize", start);
        coach.used_shortcut("maximize", start + 2);
        assert_eq!(coach.trace("maximize").copied, 1);
        assert_eq!(coach.trace("maximize").recalled, 0);
        // A second press, still inside the window, is now unaided: the toast
        // was retired by the first use.
        coach.used_shortcut("maximize", start + 4);
        assert_eq!(coach.trace("maximize").recalled, 1);
    }

    #[test]
    fn mastery_stays_within_bounds_under_heavy_use() {
        // The model is a belief, so it must never exceed certainty however much
        // evidence accumulates, nor go negative after repeated failures.
        let mut coach = Coach::new();
        let mut when = 1_000_000;
        for _ in 0..200 {
            coach.used_shortcut("maximize", when);
            when += DAY;
        }
        let mastery = coach.mastery("maximize", when);
        assert!(
            (0.0..=1.0).contains(&mastery),
            "mastery out of range: {mastery}"
        );
        for _ in 0..50 {
            coach.used_slow_path("maximize", when);
            when += 60;
        }
        let mastery = coach.mastery("maximize", when);
        assert!(
            (0.0..=1.0).contains(&mastery),
            "mastery out of range after lapses: {mastery}"
        );
        assert!(coach.overall_mastery(when) <= 1.0);
    }

    #[test]
    fn a_long_lapse_makes_a_known_skill_teachable_again() {
        // The point of modelling decay: a shortcut learned and then abandoned
        // should re-enter the curriculum rather than count as known forever.
        let mut coach = Coach::new();
        let start = 1_000_000;
        for step in 0..4 {
            coach.used_shortcut("maximize", start + step * 2 * DAY);
        }
        let known_at = start + 8 * DAY;
        assert!(coach.mastery("maximize", known_at) >= KNOWN_THRESHOLD);
        let entry = skill("maximize").unwrap();
        assert_eq!(coach.hint_value(entry, true, known_at), 0.0);

        let much_later = start + 400 * DAY;
        assert!(coach.mastery("maximize", much_later) < KNOWN_THRESHOLD);
        assert!(coach.hint_value(entry, true, much_later) > 0.0);
    }

    #[test]
    fn the_model_survives_a_round_trip() {
        let mut coach = Coach::new();
        let start = 1_000_000;
        coach.used_shortcut("maximize", start);
        coach.used_slow_path("new_panel", start + 100);
        coach.used_shortcut("focus_left_right", start + 200);

        let restored = Coach::deserialize(&coach.serialize());
        let now = start + 5 * DAY;
        for entry in SKILLS {
            let before = coach.mastery(entry.id, now);
            let after = restored.mastery(entry.id, now);
            assert!(
                (before - after).abs() < 0.001,
                "{} drifted across persistence: {before} vs {after}",
                entry.id
            );
        }
        assert_eq!(restored.effort_saved, coach.effort_saved);
        assert_eq!(restored.effort_wasted, coach.effort_wasted);
    }

    /// `load` and `save` are the only paths that touch the disk, and were
    /// previously exercised only by running the desktop app. This drives them
    /// directly against a real temporary state directory, so persistence is
    /// verified without needing a compositor.
    #[test]
    fn save_and_load_round_trip_through_the_real_filesystem() {
        // state_path() reads XDG_STATE_HOME, so point it at a scratch dir.
        // Tests share a process, so use a unique directory and restore the env.
        let scratch = std::env::temp_dir().join(format!(
            "jcode-desktop-learning-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("create scratch dir");

        // SAFETY: single-threaded within this test's scope; the variable is
        // restored before returning.
        let previous = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", &scratch) };

        let outcome = std::panic::catch_unwind(|| {
            // Nothing saved yet: a fresh model, and no file on disk.
            let path = state_path().expect("a state path");
            assert!(!path.exists(), "no state should exist yet");
            assert_eq!(load().overall_mastery(1_000_000), 0.0);

            // Record real activity, save it, and read it back.
            let now = 1_700_000_000;
            let mut coach = Coach::new();
            coach.used_shortcut("maximize", now);
            coach.used_slow_path("new_panel", now + 10);
            coach.used_shortcut("focus_left_right", now + 20);
            save(&coach);

            assert!(path.exists(), "save should have written the state file");
            let restored = load();
            for skill in SKILLS {
                let before = coach.mastery(skill.id, now + 100);
                let after = restored.mastery(skill.id, now + 100);
                assert!(
                    (before - after).abs() < 0.001,
                    "{} drifted through the filesystem: {before} vs {after}",
                    skill.id
                );
            }
            assert_eq!(restored.effort_saved, coach.effort_saved);
            assert_eq!(restored.effort_wasted, coach.effort_wasted);

            // A corrupted file must not prevent startup.
            std::fs::write(&path, "this is not a model").expect("write garbage");
            assert_eq!(load().overall_mastery(now), 0.0, "damaged state resets");
        });

        unsafe {
            match previous {
                Some(value) => std::env::set_var("XDG_STATE_HOME", value),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&scratch);
        if let Err(payload) = outcome {
            std::panic::resume_unwind(payload);
        }
    }

    #[test]
    fn a_damaged_state_file_yields_a_fresh_model() {
        let coach = Coach::deserialize("garbage\nskill nonexistent 1 1 1 1 1 1 1 1 1\nv9 ?");
        assert_eq!(coach.overall_mastery(1_000_000), 0.0);
        assert_eq!(coach.effort_saved, 0);
    }

    #[test]
    fn skills_dropped_from_the_catalog_do_not_linger_in_the_model() {
        // A skill removed from the catalog (renamed, retired) must not be
        // carried forward: it would accumulate in the state file forever and be
        // written back out on every save. Checking overall mastery cannot see
        // this, since that only sums catalog skills, so inspect what survives a
        // load-then-save cycle directly.
        let saved = "v1 effort 5 2\n\
                     skill maximize 0.9000 4.0000 4.0000 1700000000 3 0 0 0 0\n\
                     skill retired_skill 0.9000 4.0000 4.0000 1700000000 3 0 0 0 0";
        let coach = Coach::deserialize(saved);

        assert!(
            coach.trace("maximize").recalled > 0,
            "a known skill should survive the round trip"
        );
        let rewritten = coach.serialize();
        assert!(
            rewritten.contains("maximize"),
            "known skills should be written back"
        );
        assert!(
            !rewritten.contains("retired_skill"),
            "a skill no longer in the catalog must be dropped, got: {rewritten}"
        );
    }

    #[test]
    fn a_lapse_costs_a_shaky_skill_more_than_an_established_one() {
        // The penalty is graded by evidence, so occasional mouse use by an
        // expert is not read the same way as a beginner's fallback.
        let start = 1_000_000;

        let mut beginner = Coach::new();
        beginner.used_shortcut("maximize", start);
        let beginner_before = beginner.mastery("maximize", start);
        beginner.used_slow_path("maximize", start + 60);
        let beginner_loss = beginner_before - beginner.mastery("maximize", start + 60);

        let mut expert = Coach::new();
        for step in 0..6 {
            expert.used_shortcut("maximize", start + step * 2 * DAY);
        }
        let expert_at = start + 12 * DAY;
        let expert_before = expert.mastery("maximize", expert_at);
        expert.used_slow_path("maximize", expert_at + 60);
        let expert_loss = expert_before - expert.mastery("maximize", expert_at + 60);

        assert!(
            beginner_loss > expert_loss,
            "a beginner's lapse should count for more: {beginner_loss} vs {expert_loss}"
        );
        // The expert is still considered to know it.
        assert!(expert.mastery("maximize", expert_at + 60) >= KNOWN_THRESHOLD);
    }

    /// A full arc: a new user clicks their way around, gets taught, adopts the
    /// keys, and the model's picture of them changes accordingly. This is the
    /// behavior the feature exists to produce, tested end to end.
    #[test]
    fn a_beginner_is_taught_and_then_recognized_as_competent() {
        let mut coach = Coach::new();
        let mut when = 1_000_000;

        // Day one: they do not know super-n, so they click the new-session
        // card. The coach notices and teaches.
        coach.used_slow_path("new_panel", when);
        let hint = coach.active_hint(when).expect("should teach super-n");
        assert_eq!(hint.skill_id, "new_panel");
        assert_eq!(hint.keys, "super-n");

        // They take the advice immediately. That earns weak credit only, so the
        // coach must not yet consider the skill learned.
        when += 3;
        coach.used_shortcut("new_panel", when);
        assert!(
            coach.mastery("new_panel", when) < 0.7,
            "one prompted use is not mastery"
        );

        // Over the following weeks they use it unaided and spaced out.
        for _ in 0..4 {
            when += 3 * DAY;
            coach.used_shortcut("new_panel", when);
        }
        assert!(
            coach.mastery("new_panel", when) >= 0.7,
            "repeated unaided use should establish the skill"
        );

        // Now it is known, a stray click no longer produces a lecture, though
        // it is still recorded as wasted effort.
        let wasted_before = coach.effort_wasted;
        when += 60;
        coach.used_slow_path("new_panel", when);
        assert!(coach.effort_wasted > wasted_before);
        assert!(
            coach.active_hint(when).is_none(),
            "a known skill should not be retaught after one slip"
        );

        // Their overall fluency has risen from nothing, and the coach has moved
        // on to a different lesson.
        assert!(coach.overall_mastery(when) > 0.0);
        let next = coach.next_lesson(when).expect("more to learn");
        assert_ne!(next.id, "new_panel");
    }

    #[test]
    fn the_report_covers_every_skill_once() {
        let coach = Coach::new();
        let report = coach.report(1_000_000);
        let listed: usize = report.iter().map(|(_, rows)| rows.len()).sum();
        assert_eq!(listed, SKILLS.len(), "every skill must appear exactly once");
    }
}
