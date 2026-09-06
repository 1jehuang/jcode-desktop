//! One record per creation attempt, with no prompts, paths, credentials or errors.
//! Timings isolate SDK connection/handshake from runtime session initialization.
use super::*;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(super) fn retry_startup(request_id: Option<&str>) -> bool {
    // Retain the original startup recovery policy. Ordinary pending panels
    // have unique correlation IDs too, but retrying an ambiguous timeout can
    // create a second session and is not safe for those user actions.
    request_id == Some("startup://draft")
}

pub(super) fn create(
    connector: impl FnOnce() -> jcode_sdk::Result<JcodeClient>,
    working_dir: Option<String>,
) -> jcode_sdk::Result<(SessionInfo, JcodeClient)> {
    let started = Instant::now();
    let connection = connector();
    let connect_elapsed = started.elapsed();
    let connected = connection.is_ok();
    let result = connection.and_then(|client| {
        let session = client.create_session(working_dir)?;
        Ok((session, client))
    });
    let total = started.elapsed();
    // Use the existing local diagnostic log rather than opening a file on the
    // UI thread or exporting telemetry. The public native profiler joins these
    // records to rendered panel IDs. Failed connects have no create duration.
    eprintln!(
        "jcode desktop spawn: {}",
        record(
            connect_elapsed,
            connected.then(|| total.saturating_sub(connect_elapsed)),
            result
                .as_ref()
                .ok()
                .map(|(session, _)| session.session_id.as_str()),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default(),
        )
    );
    result
}

fn record(
    connect: Duration,
    create: Option<Duration>,
    session_id: Option<&str>,
    timestamp: Duration,
) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "stage": "create_session",
        "timestamp_us": timestamp.as_micros() as u64,
        "connect_ms": connect.as_secs_f64() * 1000.0,
        "create_ms": create.map(|elapsed| elapsed.as_secs_f64() * 1000.0),
        "total_ms": (connect + create.unwrap_or_default()).as_secs_f64() * 1000.0,
        "success": session_id.is_some(),
        "session_id": session_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_profile_only_retries_the_original_startup_request() {
        assert!(retry_startup(Some("startup://draft")));
        assert!(!retry_startup(None));
        assert!(!retry_startup(Some("startup://draft/123")));
        assert!(!retry_startup(Some("other-request")));
    }

    #[test]
    fn spawn_profile_separates_handshake_and_runtime_initialization() {
        let value = record(
            Duration::from_millis(2),
            Some(Duration::from_millis(1800)),
            Some("session_test"),
            Duration::from_secs(1),
        );
        assert_eq!(value["connect_ms"], 2.0);
        assert_eq!(value["create_ms"], 1800.0);
        assert_eq!(value["total_ms"], 1802.0);
        assert_eq!(value["timestamp_us"], 1_000_000);
        assert_eq!(value["success"], true);
    }

    #[test]
    fn spawn_profile_distinguishes_connect_and_create_failures() {
        let connect_failure = record(Duration::from_millis(3), None, None, Duration::ZERO);
        assert!(connect_failure["create_ms"].is_null());
        assert_eq!(connect_failure["total_ms"], 3.0);
        assert_eq!(connect_failure["success"], false);
        let create_failure = record(
            Duration::ZERO,
            Some(Duration::from_millis(5)),
            None,
            Duration::ZERO,
        );
        assert_eq!(create_failure["create_ms"], 5.0);
        assert_eq!(create_failure["success"], false);
        assert!(create_failure["session_id"].is_null());
    }
}
