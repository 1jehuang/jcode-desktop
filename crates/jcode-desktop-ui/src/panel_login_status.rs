//! Read-only connection health for the native account picker.
//! The CLI's structured doctor report distinguishes saved keys from working logins.
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConnectionStatus {
    Checking,
    Testing,
    Connected,
    Unverified,
    Expired,
    Failed,
    NotConnected,
    Unknown,
}

impl ConnectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking…",
            Self::Testing => "Testing…",
            Self::Connected => "Connected",
            Self::Unverified => "Untested",
            Self::Expired => "Expired",
            Self::Failed => "Needs attention",
            Self::NotConnected => "Not connected",
            Self::Unknown => "Status unavailable",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Checking => "Reading account status on this computer.",
            Self::Testing => "Sending a small live request through this account.",
            Self::Connected => "A live request through this account succeeded in the last 7 days.",
            Self::Unverified => {
                "Credentials are saved, but no live request has been sent in the last 7 days. Press Test to send one."
            }
            Self::Expired => "Your sign-in has expired. Connect again to continue.",
            Self::Failed => "The latest live request failed. Test again or reconnect.",
            Self::NotConnected => "Connect this account to make its models available.",
            Self::Unknown => "Could not read account health. Reopen Accounts to try again.",
        }
    }

    pub fn color(self) -> gpui::Rgba {
        let theme = crate::theme::Theme::global();
        match self {
            Self::Connected => theme.OK,
            Self::Testing => theme.ACCENT,
            Self::Unverified | Self::Unknown => theme.WARN,
            Self::Expired | Self::Failed => theme.ERROR,
            Self::NotConnected | Self::Checking => theme.TEXT_DIM,
        }
    }
}

pub(super) type ConnectionStatuses = HashMap<String, ConnectionStatus>;

pub(super) fn status_for(
    statuses: Option<&ConnectionStatuses>,
    id: &str,
    loading: bool,
) -> ConnectionStatus {
    if loading {
        return ConnectionStatus::Checking;
    }
    // With no provider argument, doctor includes every configured provider.
    // An omitted provider is unconfigured, not a failed lookup. A failed report
    // is None and must remain unknown rather than pretending everyone logged out.
    statuses
        .map(|statuses| {
            statuses
                .get(id)
                .copied()
                .unwrap_or(ConnectionStatus::NotConnected)
        })
        .unwrap_or(ConnectionStatus::Unknown)
}

pub(super) fn parse_connection_statuses(json: &str) -> Option<ConnectionStatuses> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let providers = value.get("providers")?.as_array()?;
    Some(
        providers
            .iter()
            .filter_map(|provider| {
                let id = provider.get("id")?.as_str()?;
                let validation = &provider["validation_detail"];
                let recent = validation["stale"].as_bool() == Some(false);
                let success = validation["success"].as_bool();
                let refresh_failed = provider["last_refresh_detail"]["last_error"]
                    .as_str()
                    .is_some();
                let state = match provider["status"].as_str() {
                    Some("not_configured") => ConnectionStatus::NotConnected,
                    Some("expired") => ConnectionStatus::Expired,
                    Some("available") if refresh_failed => ConnectionStatus::Failed,
                    Some("available") if recent && success == Some(false) => {
                        ConnectionStatus::Failed
                    }
                    Some("available") if recent && success == Some(true) => {
                        ConnectionStatus::Connected
                    }
                    Some("available") => ConnectionStatus::Unverified,
                    _ => ConnectionStatus::Unknown,
                };
                Some((id.to_owned(), state))
            })
            .collect(),
    )
}

/// No --validate: opening a panel never issues paid model requests or alters logins.
/// Bound process lifetime and output without blocking the UI or filling a stdout pipe.
pub(super) fn fetch_connection_statuses() -> Option<ConnectionStatuses> {
    if crate::harness::screenshot_mode() {
        return Some(
            [
                ("openai".into(), ConnectionStatus::Connected),
                ("claude".into(), ConnectionStatus::Expired),
                ("gemini".into(), ConnectionStatus::Unverified),
                ("copilot".into(), ConnectionStatus::Failed),
                ("openai-api".into(), ConnectionStatus::NotConnected),
            ]
            .into(),
        );
    }
    let mut output = tempfile::tempfile().ok()?;
    let mut child = Command::new(crate::platform::companion_executable("jcode"))
        .args(["auth", "doctor", "--json"])
        .stdin(Stdio::null())
        .stdout(output.try_clone().ok()?)
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    output.seek(SeekFrom::Start(0)).ok()?;
    let mut json = String::new();
    output.take(1024 * 1024).read_to_string(&mut json).ok()?;
    parse_connection_statuses(&json)
}

/// Outcome of a live smoke test for one provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LiveTestResult {
    pub status: ConnectionStatus,
    /// Why the test failed, from the doctor's validation result.
    pub error: Option<String>,
}

fn parse_live_results(json: &str) -> Option<HashMap<String, LiveTestResult>> {
    let statuses = parse_connection_statuses(json)?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let errors: HashMap<&str, String> = value["providers"]
        .as_array()?
        .iter()
        .filter_map(|provider| {
            let result = provider["validation_result"].as_str()?;
            let id = provider["id"].as_str()?;
            (result != "validation passed").then(|| (id, result.trim().to_owned()))
        })
        .collect();
    Some(
        statuses
            .into_iter()
            .map(|(id, status)| {
                let error = errors.get(id.as_str()).cloned();
                (id, LiveTestResult { status, error })
            })
            .collect(),
    )
}

/// Send a real request through each configured provider (or just `provider`)
/// and record the result. This spends a small amount of quota or credit, so
/// it only runs when the user presses Test.
pub(super) fn run_live_test(provider: Option<&str>) -> Option<HashMap<String, LiveTestResult>> {
    if crate::harness::screenshot_mode() {
        return None;
    }
    let mut args = vec!["auth", "doctor"];
    args.extend(provider);
    args.extend(["--validate", "--json"]);
    let mut output = tempfile::tempfile().ok()?;
    let mut child = Command::new(crate::platform::companion_executable("jcode"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(output.try_clone().ok()?)
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // The doctor bounds each provider at 120s. Leave room for several.
    let deadline = Instant::now() + Duration::from_secs(if provider.is_some() { 150 } else { 600 });
    loop {
        match child.try_wait() {
            // The doctor exits non-zero when any provider has an issue, and
            // still prints the full report.
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    output.seek(SeekFrom::Start(0)).ok()?;
    let mut json = String::new();
    output
        .take(4 * 1024 * 1024)
        .read_to_string(&mut json)
        .ok()?;
    parse_live_results(&json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_health_does_not_confuse_saved_credentials_with_working_accounts() {
        let statuses = parse_connection_statuses(r#"{"providers":[
            {"id":"working","status":"available","validation_detail":{"success":true,"stale":false}},
            {"id":"failed","status":"available","validation_detail":{"success":false,"stale":false}},
            {"id":"refresh-failed","status":"available","validation_detail":{"success":true,"stale":false},"last_refresh_detail":{"last_error":"expired refresh token"}},
            {"id":"saved","status":"available"},
            {"id":"old-pass","status":"available","validation_detail":{"success":true,"stale":true}},
            {"id":"old-fail","status":"available","validation_detail":{"success":false,"stale":true}},
            {"id":"expired","status":"expired","validation_detail":{"success":true,"stale":false}},
            {"id":"absent","status":"not_configured"},
            {"id":"future","status":"other"}
        ]}"#).unwrap();
        assert_eq!(statuses["working"], ConnectionStatus::Connected);
        assert_eq!(statuses["failed"], ConnectionStatus::Failed);
        assert_eq!(statuses["refresh-failed"], ConnectionStatus::Failed);
        for id in ["saved", "old-pass", "old-fail"] {
            assert_eq!(statuses[id], ConnectionStatus::Unverified);
        }
        assert_eq!(statuses["expired"], ConnectionStatus::Expired);
        assert_eq!(statuses["absent"], ConnectionStatus::NotConnected);
        assert_eq!(statuses["future"], ConnectionStatus::Unknown);
        assert!(parse_connection_statuses("not json").is_none());
        assert!(parse_connection_statuses("{}").is_none());
        assert_eq!(
            status_for(Some(&statuses), "omitted", false),
            ConnectionStatus::NotConnected
        );
        assert_eq!(
            status_for(None, "omitted", false),
            ConnectionStatus::Unknown
        );
        assert_eq!(
            status_for(Some(&statuses), "working", true),
            ConnectionStatus::Checking
        );
    }

    #[test]
    fn live_results_carry_the_failure_reason() {
        let results = parse_live_results(r#"{"providers":[
            {"id":"ok","status":"available","validation_result":"validation passed","validation_detail":{"success":true,"stale":false}},
            {"id":"bad","status":"available","validation_result":"401 Unauthorized","validation_detail":{"success":false,"stale":false}}
        ]}"#).unwrap();
        assert_eq!(results["ok"].status, ConnectionStatus::Connected);
        assert_eq!(results["ok"].error, None);
        assert_eq!(results["bad"].status, ConnectionStatus::Failed);
        assert_eq!(results["bad"].error.as_deref(), Some("401 Unauthorized"));
    }
}
