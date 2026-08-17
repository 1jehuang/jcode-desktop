//! Delivery feedback for locally submitted prompts.

use std::time::{Duration, Instant};

pub const DURATION: Duration = Duration::from_millis(420);
pub const PENDING_TONE: f32 = 0.78;

pub fn motion(accepted_at: Instant, now: Instant) -> (f32, f32, bool) {
    let elapsed = now.saturating_duration_since(accepted_at);
    if elapsed >= DURATION {
        return (0.0, 1.0, false);
    }
    let t = elapsed.as_secs_f32() / DURATION.as_secs_f32();
    let decay = 1.0 - t;
    let offset = 5.0 * decay * decay * (t * 2.0 * std::f32::consts::TAU).sin();
    let opacity = PENDING_TONE + (1.0 - PENDING_TONE) * t;
    (offset, opacity, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgement_starts_and_ends_at_rest() {
        let at = Instant::now();
        assert_eq!(motion(at, at), (0.0, PENDING_TONE, true));
        assert_eq!(motion(at, at + DURATION), (0.0, 1.0, false));
        let during = motion(at, at + DURATION / 8);
        assert!(during.0.abs() > 0.5);
        assert!(during.1 > PENDING_TONE && during.1 < 1.0);
    }
}
