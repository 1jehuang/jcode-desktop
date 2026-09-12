//! On-demand, passive sampling of the real window. No redraws or focus changes.
//! A short-lived control file enables capture in an already-running UI generation.
use gpui::{
    App, Task, Window,
    profiler::{FrameDurationSnapshot, InputLatencySnapshot},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
struct Request {
    capture_id: String,
    until_unix_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn valid_request(request: &Request, now: u64) -> bool {
    !request.capture_id.is_empty()
        && request.capture_id.len() <= 64
        && request
            .capture_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && request.until_unix_ms > now
        && request.until_unix_ms - now <= 120_000
}

fn request_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|root| PathBuf::from(root).join("jcode-desktop-profile.json"))
}

fn read_request(path: &PathBuf) -> Option<Request> {
    if fs::metadata(path).ok()?.len() > 4096 {
        return None;
    }
    let request: Request = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    valid_request(&request, now_ms()).then_some(request)
}

#[derive(Serialize)]
struct Sample {
    capture_id: String,
    unix_ms: u64,
    pid: u32,
    window: String,
    interval_ms: f64,
    ui_wake_lag_ms: f64,
    draw_count: u64,
    draw_p95_ms: Option<f64>,
    draw_max_ms: Option<f64>,
    animation_present_count: u64,
    animation_present_p95_ms: Option<f64>,
    input_frame_count: u64,
    input_p95_ms: Option<f64>,
    input_max_ms: Option<f64>,
    coalesced_input_max: u64,
    mid_draw_inputs: u64,
}

fn delta(
    capture_id: String,
    window: String,
    interval: Duration,
    wake_lag: Duration,
    before: &(FrameDurationSnapshot, InputLatencySnapshot),
    after: &(FrameDurationSnapshot, InputLatencySnapshot),
) -> Sample {
    let mut draw = after.0.draw_duration_histogram.clone();
    let mut present = after.0.present_interval_histogram.clone();
    let mut input = after.1.latency_histogram.clone();
    let mut coalesced = after.1.events_per_frame_histogram.clone();
    let _ = draw.subtract(&before.0.draw_duration_histogram);
    let _ = present.subtract(&before.0.present_interval_histogram);
    let _ = input.subtract(&before.1.latency_histogram);
    let _ = coalesced.subtract(&before.1.events_per_frame_histogram);
    let ms = |ns: u64| ns as f64 / 1_000_000.0;
    Sample {
        capture_id,
        unix_ms: now_ms(),
        pid: std::process::id(),
        window,
        interval_ms: interval.as_secs_f64() * 1000.0,
        ui_wake_lag_ms: wake_lag.as_secs_f64() * 1000.0,
        draw_count: draw.len(),
        draw_p95_ms: (!draw.is_empty()).then(|| ms(draw.value_at_quantile(0.95))),
        draw_max_ms: (!draw.is_empty()).then(|| ms(draw.max())),
        animation_present_count: present.len(),
        animation_present_p95_ms: (!present.is_empty())
            .then(|| ms(present.value_at_quantile(0.95))),
        input_frame_count: input.len(),
        input_p95_ms: (!input.is_empty()).then(|| ms(input.value_at_quantile(0.95))),
        input_max_ms: (!input.is_empty()).then(|| ms(input.max())),
        coalesced_input_max: coalesced.max(),
        mid_draw_inputs: after
            .1
            .mid_draw_events_dropped
            .saturating_sub(before.1.mid_draw_events_dropped),
    }
}

fn append(path: PathBuf, sample: Sample) {
    // Multiple windows can sample the same process concurrently. Serialize
    // complete JSON lines, on background workers only.
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_guard) = WRITER.lock() else {
        return;
    };
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(path)
        && let Ok(line) = serde_json::to_string(&sample)
    {
        let _ = writeln!(file, "{line}");
    }
}

pub fn spawn(window: &Window, cx: &App) -> Task<()> {
    if let Some(gpu) = window.gpu_specs() {
        eprintln!("desktop graphics: {gpu:?}");
    }
    let handle = window.window_handle();
    let window_id = format!("{:?}", handle.window_id());
    cx.spawn(async move |cx| {
        let Some(path) = request_path() else {
            return;
        };
        let mut previous = None;
        let mut capture_id = String::new();
        let mut sampled_at = Instant::now();
        let mut active = false;
        loop {
            let delay = Duration::from_millis(if active { 100 } else { 1000 });
            let waiting_at = Instant::now();
            cx.background_executor().timer(delay).await;
            let wake_lag = waiting_at.elapsed().saturating_sub(delay);
            let read_path = path.clone();
            let request = cx
                .background_executor()
                .spawn(async move { read_request(&read_path) })
                .await;
            let Some(request) = request else {
                active = false;
                previous = None;
                continue;
            };
            active = true;
            let Ok(current) = handle.update(cx, |_, window, _| {
                (
                    window.frame_duration_snapshot(),
                    window.input_latency_snapshot(),
                )
            }) else {
                return;
            };
            let now = Instant::now();
            if capture_id != request.capture_id {
                capture_id = request.capture_id;
                previous = None;
            }
            if let Some(before) = previous.as_ref() {
                let sample = delta(
                    capture_id.clone(),
                    window_id.clone(),
                    now.duration_since(sampled_at),
                    wake_lag,
                    before,
                    &current,
                );
                let output = path.with_file_name(format!(
                    "jcode-desktop-profile-{}-{}.jsonl",
                    std::process::id(),
                    capture_id
                ));
                // Disk and serialization work never runs on the UI thread.
                cx.background_executor()
                    .spawn(async move {
                        append(output, sample);
                    })
                    .await;
            }
            previous = Some(current);
            sampled_at = now;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn samples_only_new_events_and_reports_idle_as_missing() {
        let histogram = || hdrhistogram::Histogram::<u64>::new(3).unwrap();
        let mut before = (
            FrameDurationSnapshot {
                draw_duration_histogram: histogram(),
                present_interval_histogram: histogram(),
            },
            InputLatencySnapshot {
                latency_histogram: histogram(),
                events_per_frame_histogram: histogram(),
                mid_draw_events_dropped: 7,
            },
        );
        before
            .0
            .draw_duration_histogram
            .record(100_000_000)
            .unwrap();
        before.1.latency_histogram.record(200_000_000).unwrap();
        let idle = delta(
            "test".into(),
            "window".into(),
            Duration::from_millis(100),
            Duration::ZERO,
            &before,
            &before,
        );
        assert_eq!(idle.draw_count, 0);
        assert_eq!(idle.input_max_ms, None);
        assert_eq!(idle.animation_present_p95_ms, None);
        let mut after = before.clone();
        after.0.draw_duration_histogram.record(2_000_000).unwrap();
        after.1.latency_histogram.record(8_000_000).unwrap();
        after.1.mid_draw_events_dropped = 9;
        let sample = delta(
            "test".into(),
            "window".into(),
            Duration::from_millis(100),
            Duration::from_millis(3),
            &before,
            &after,
        );
        assert_eq!(sample.draw_count, 1);
        assert_eq!(sample.input_frame_count, 1);
        assert!(sample.draw_max_ms.unwrap() < 2.01);
        assert!(sample.input_max_ms.unwrap() < 8.01);
        assert_eq!(sample.mid_draw_inputs, 2);
        assert_eq!(sample.ui_wake_lag_ms, 3.0);
    }

    #[test]
    fn control_is_bounded_and_rejects_paths_and_expiry() {
        let valid = Request {
            capture_id: "capture-1".into(),
            until_unix_ms: 2000,
        };
        assert!(valid_request(&valid, 1000));
        assert!(!valid_request(&valid, 2000));
        assert!(!valid_request(
            &Request {
                capture_id: "../escape".into(),
                until_unix_ms: 2000
            },
            1000
        ));
        assert!(!valid_request(
            &Request {
                capture_id: "x".into(),
                until_unix_ms: 122000
            },
            1000
        ));
    }
}
