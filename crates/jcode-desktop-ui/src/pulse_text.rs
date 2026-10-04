//! Decorations that change every few frames, each in its own small view.
//!
//! `with_animation` notifies the view whose render built the animated
//! element. Inside the workspace sidebar that rebuilt the whole workspace
//! (every strip, the header and the sidebar) 20 times a second for as long as
//! any session was working. Inside a chat panel, a running tool's pulse and
//! clock rebuilt the entire transcript and composer just as often. A
//! [`Ticker`] is a leaf view that notifies only itself on a shared grid, so
//! GPUI draws everything around it from the last frame and builds just the
//! leaf again.
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{Context, Entity, IntoElement, Render, Rgba, SharedString, Window, div, prelude::*};

/// 20 Hz on the shared decorative grid, the rate pulses always used.
const PULSE_TICK: Duration = Duration::from_millis(50);
/// Running tool rows breathe on this period, in step with sidebar titles.
pub(crate) const TOOL_PULSE_PERIOD: Duration = Duration::from_millis(1400);

#[derive(Clone, PartialEq)]
pub(crate) enum Kind {
    /// Text whose opacity breathes with `curve`.
    PulseText {
        text: SharedString,
        period: Duration,
        curve: fn(f32) -> f32,
    },
    /// A fill that fades in and out over its parent, so the content below
    /// appears to breathe without being rebuilt. `curve` is the opacity the
    /// content should appear to have, from 0 to 1.
    PulseVeil {
        color: Rgba,
        period: Duration,
        curve: fn(f32) -> f32,
    },
    /// Elapsed time since `since` in `format`, refreshed twice a second.
    Elapsed {
        since: Instant,
        format: fn(Duration) -> String,
    },
}

pub(crate) struct Ticker {
    kind: Kind,
    /// A tick is scheduled. A `Cell`, not entity state: clearing it through
    /// an entity update would count as a change for every view holding this
    /// one and rebuild them.
    tick_pending: Rc<Cell<bool>>,
}

/// Phase in `0..1`, from a process-wide epoch so every pulse sharing a period
/// breathes in step regardless of when it was created.
fn phase(period: Duration, cx: &gpui::App) -> f32 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let now = cx.background_executor().now();
    let epoch = if cfg!(test) {
        now
    } else {
        *EPOCH.get_or_init(|| now)
    };
    let period = period.as_nanos().max(1);
    (now.saturating_duration_since(epoch).as_nanos() % period) as f32 / period as f32
}

impl Ticker {
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            kind,
            tick_pending: Rc::default(),
        }
    }

    fn arm(&self, period: Duration, cx: &mut Context<Self>) {
        if self.tick_pending.replace(true) {
            return;
        }
        // One pending tick per render: a ticker that stops being drawn
        // (scrolled away, clipped, finished) stops ticking on its own.
        let pending = self.tick_pending.clone();
        let id = cx.entity_id();
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            crate::animation_clock::next_tick(cx.background_executor(), period).await;
            pending.set(false);
            if weak.upgrade().is_some() {
                cx.update(|cx| cx.notify(id));
            }
        })
        .detach();
    }
}

impl Render for Ticker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _render_scope = crate::render_stats::scope("Ticker");
        let reduce_motion = cx.reduce_motion() || crate::config::get().appearance.reduce_motion;
        match self.kind.clone() {
            Kind::PulseText {
                text,
                period,
                curve,
            } => {
                let opacity = if reduce_motion {
                    1.0
                } else {
                    self.arm(PULSE_TICK, cx);
                    curve(phase(period, cx))
                };
                div()
                    .size_full()
                    .min_w_0()
                    .truncate()
                    .opacity(opacity)
                    .child(text)
                    .into_any_element()
            }
            Kind::PulseVeil {
                color,
                period,
                curve,
            } => {
                let shown = if reduce_motion {
                    1.0
                } else {
                    self.arm(PULSE_TICK, cx);
                    curve(phase(period, cx))
                };
                div()
                    .absolute()
                    .inset_0()
                    .bg(Rgba {
                        a: color.a * (1.0 - shown).clamp(0.0, 1.0),
                        ..color
                    })
                    .into_any_element()
            }
            Kind::Elapsed { since, format } => {
                self.arm(Duration::from_millis(500), cx);
                div().child(format(since.elapsed())).into_any_element()
            }
        }
    }
}

/// One ticker per key, kept only while its key is live. The kind each was
/// last given is kept here, so the holder never reads the ticker view while
/// drawing, which would make it depend on every tick.
#[derive(Default)]
pub(crate) struct Tickers(HashMap<String, (Entity<Ticker>, Kind)>);

impl Tickers {
    pub(crate) fn retain(&mut self, mut live: impl FnMut(&str) -> bool) {
        self.0.retain(|key, _| live(key));
    }

    /// The ticker for `key` showing `kind`, created on first use.
    pub(crate) fn get<T: 'static>(
        &mut self,
        key: &str,
        kind: Kind,
        cx: &mut Context<T>,
    ) -> Entity<Ticker> {
        if let Some((ticker, shown)) = self.0.get_mut(key) {
            if *shown != kind {
                *shown = kind.clone();
                ticker.update(cx, |ticker, cx| {
                    ticker.kind = kind;
                    cx.notify();
                });
            }
            return ticker.clone();
        }
        let ticker = cx.new(|_| Ticker::new(kind.clone()));
        self.0.insert(key.to_owned(), (ticker.clone(), kind));
        ticker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tickers_hold_their_kind_and_have_no_pending_tick() {
        let period = Duration::from_millis(1400);
        let kind = Kind::PulseText {
            text: "x".into(),
            period,
            curve: |p| p,
        };
        let ticker = Ticker::new(kind.clone());
        assert!(ticker.kind == kind);
        assert!(!ticker.tick_pending.get());
    }
}
