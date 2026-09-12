//! Connected accounts: which OAuth logins and API keys the runtime can use,
//! fetched from `jcode auth status --json` and shown with provider logos.
//!
//! The desktop shows only configured credentials (available or expired), not
//! the full catalog of possible providers: the question this surface answers
//! is "what am I logged into", not "what could I log into".

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static AUTH_REFRESH: (Mutex<u64>, std::sync::Condvar) = (Mutex::new(0), std::sync::Condvar::new());

/// Wake account feeds after a native login without waiting for the slow poll.
pub fn request_refresh() {
    if let Ok(mut generation) = AUTH_REFRESH.0.lock() {
        *generation = generation.wrapping_add(1);
        AUTH_REFRESH.1.notify_all();
    }
}

/// One configured credential, as reported by the CLI's canonical auth report.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    /// Stable provider id (`claude`, `openai-api`, ...): keys the logo lookup.
    pub id: String,
    pub display_name: String,
    /// `available` or `expired`. Unconfigured providers are filtered out.
    pub status: String,
    /// `OAuth`, `API key`, `CLI`, `device code`, ...
    pub auth_kind: String,
    /// Human method line, e.g. "API key (`OPENAI_API_KEY`)".
    pub method: String,
    /// Every rolling or model-specific limit reported by `jcode usage`.
    pub limits: Vec<UsageLimit>,
    /// Keep each OAuth account's report separate, never sum unrelated logins.
    pub usage_reports: Vec<UsageReport>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageReport {
    pub provider_name: String,
    pub account_label: Option<String>,
    pub limits: Vec<UsageLimit>,
    pub extra_info: Vec<(String, String)>,
}

pub const USAGE_ESTIMATE_NOTE: &str = "Recorded by Jcode. API-equivalent estimates, not your ChatGPT bill. Today starts at local midnight. Lifetime covers recorded history only.";

impl UsageReport {
    pub fn title(&self) -> String {
        match &self.account_label {
            Some(label) => format!("{} · {label}", self.provider_name),
            None => self.provider_name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageLimit {
    pub name: String,
    /// Percentage consumed, clamped by the UI when painted.
    pub usage_percent: f32,
    pub reset_in: Option<String>,
}

impl Account {
    /// The CLI marks its active OAuth login with ✦. Never blend unrelated logins.
    pub fn active_limits(&self) -> Option<&[UsageLimit]> {
        let active: Vec<_> = self
            .usage_reports
            .iter()
            .filter(|report| report.provider_name.trim_end().ends_with('✦'))
            .collect();
        match active.as_slice() {
            [report] => Some(&report.limits),
            [] => match self.usage_reports.as_slice() {
                [report] => Some(&report.limits),
                [] => Some(&self.limits),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn available(&self) -> bool {
        self.status == "available"
    }

    pub fn shows_oauth_history(&self) -> bool {
        self.id == "openai"
    }
}

/// Resolve the credential, not the model family (Claude can use OpenRouter).
pub fn credential_id(provider: &str, auth: Option<&str>) -> String {
    let api_key = auth == Some("api key");
    match provider {
        "anthropic" if api_key => "anthropic-api",
        "anthropic" | "claude-cli" => "claude",
        "openai" if api_key => "openai-api",
        "gemini" if api_key => "gemini-api",
        other => other,
    }
    .to_owned()
}

/// Bounded MRU, updated only on completed turns, never on tokens or quota polls.
pub fn record_use(recent: &mut Vec<String>, id: String) {
    if recent.first() == Some(&id) {
        return;
    }
    recent.retain(|previous| previous != &id);
    recent.insert(0, id);
    recent.truncate(3);
}

pub fn ordered<'a>(
    accounts: &'a [Account],
    active: Option<&str>,
    recent: &[String],
) -> Vec<&'a Account> {
    let mut rows: Vec<_> = accounts.iter().collect();
    rows.sort_by_key(|account| {
        if active == Some(account.id.as_str()) {
            0
        } else if let Some(index) = recent.iter().position(|id| id == &account.id) {
            index + 1
        } else if account.available() {
            4
        } else {
            5
        }
    });
    rows
}

/// Background feed of account snapshots. The UI polls `latest()`.
#[derive(Clone)]
pub struct Feed {
    updates: Arc<Mutex<Receiver<Vec<Account>>>>,
}

impl Feed {
    /// The most recent snapshot, if any arrived since the last poll.
    pub fn latest(&self) -> Option<Vec<Account>> {
        let receiver = self.updates.lock().ok()?;
        let mut latest = None;
        while let Ok(accounts) = receiver.try_recv() {
            latest = Some(accounts);
        }
        latest
    }
}

/// Fetch accounts now and then refresh periodically. Login state changes
/// rarely, so a slow poll keeps the surface honest without burning cycles.
pub fn spawn() -> Feed {
    let (tx, rx) = channel();
    if crate::harness::screenshot_mode() {
        let accounts = [
            ("openai", "OpenAI"),
            ("claude", "Claude"),
            ("jcode", "Jcode"),
            ("gemini", "Gemini"),
            ("openrouter", "OpenRouter"),
            ("copilot", "Copilot"),
        ]
        .into_iter()
        .map(|(id, name)| Account {
            id: id.into(),
            display_name: name.into(),
            status: "available".into(),
            auth_kind: "OAuth".into(),
            method: "Offline fixture".into(),
            usage_reports: if id == "openai" {
                vec![UsageReport {
                    provider_name: "OpenAI (ChatGPT)".into(),
                    account_label: Some("personal".into()),
                    limits: vec![UsageLimit { name: "5 hour".into(), usage_percent: 25., reset_in: Some("2h".into()) }],
                    extra_info: vec![
                        ("Today".into(), "120000 input / 8000 output tokens (90000 cached input), $0.4200 API-equivalent estimate, not a bill; recorded only; since local midnight".into()),
                        ("Lifetime".into(), "2400000 input / 160000 output tokens (1800000 cached input), $8.4000 known + unknown cost (2 unpriced responses) API-equivalent estimate, not a bill; recorded only; partial token counts; since 2026-09-01 10:00 -07:00".into()),
                    ],
                }]
            } else {
                Vec::new()
            },
            limits: vec![UsageLimit {
                name: "5 hour".into(),
                usage_percent: 25.0,
                reset_in: Some("2h".into()),
            }],
        })
        .collect();
        let _ = tx.send(accounts);
        return Feed {
            updates: Arc::new(Mutex::new(rx)),
        };
    }
    std::thread::Builder::new()
        .name("jcode-accounts".into())
        .spawn(move || {
            loop {
                let generation = *AUTH_REFRESH.0.lock().unwrap();
                if let Some(accounts) = fetch() {
                    if tx.send(accounts).is_err() {
                        return;
                    }
                }
                let _ = AUTH_REFRESH.1.wait_timeout_while(
                    AUTH_REFRESH.0.lock().unwrap(),
                    Duration::from_secs(60),
                    |current| *current == generation,
                );
            }
        })
        .expect("spawn accounts thread");
    Feed {
        updates: Arc::new(Mutex::new(rx)),
    }
}

fn fetch() -> Option<Vec<Account>> {
    let output = std::process::Command::new(crate::platform::companion_executable("jcode"))
        .args(["auth", "status", "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut accounts = parse(&String::from_utf8_lossy(&output.stdout))?;
    if let Ok(output) = std::process::Command::new(crate::platform::companion_executable("jcode"))
        .args(["usage", "--json"])
        .output()
        && output.status.success()
    {
        merge_usage(&mut accounts, &String::from_utf8_lossy(&output.stdout));
    }
    Some(accounts)
}

/// Parse the CLI report, keeping only configured credentials, available first.
pub fn parse(json: &str) -> Option<Vec<Account>> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let providers = value.get("providers")?.as_array()?;
    let mut accounts: Vec<Account> = providers
        .iter()
        .filter_map(|provider| {
            let text = |key: &str| {
                provider
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            let account = Account {
                id: text("id"),
                display_name: text("display_name"),
                status: text("status"),
                auth_kind: text("auth_kind"),
                method: text("method"),
                usage_reports: Vec::new(),
                limits: Vec::new(),
            };
            matches!(account.status.as_str(), "available" | "expired").then_some(account)
        })
        .collect();
    accounts.sort_by_key(|account| !account.available());
    Some(accounts)
}

/// Merge the usage command's provider reports into the canonical auth rows.
/// The usage schema predates stable provider ids, so names are normalized here.
fn merge_usage(accounts: &mut [Account], json: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    let Some(providers) = value.get("providers").and_then(|value| value.as_array()) else {
        return;
    };

    for provider in providers {
        let Some(provider_name) = provider
            .get("provider_name")
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        let Some(account) = accounts
            .iter_mut()
            .find(|account| usage_provider_matches(account, provider_name))
        else {
            continue;
        };
        let extra_info: Vec<(String, String)> = provider
            .get("extra_info")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let pair = entry.as_array()?;
                Some((
                    pair.first()?.as_str()?.to_owned(),
                    pair.get(1)?.as_str()?.to_owned(),
                ))
            })
            .collect();
        let account_label = extra_info
            .iter()
            .find(|(key, _)| key == "Account label")
            .map(|(_, value)| value.clone());
        account.usage_reports.push(UsageReport {
            provider_name: provider_name.to_owned(),
            account_label,
            limits: Vec::new(),
            extra_info: extra_info
                .into_iter()
                .filter(|(key, _)| key != "Account label")
                .collect(),
        });
        let limits: Vec<_> = provider
            .get("limits")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|limit| {
                Some(UsageLimit {
                    name: limit.get("name")?.as_str()?.to_owned(),
                    usage_percent: limit.get("usage_percent")?.as_f64()? as f32,
                    reset_in: limit
                        .get("reset_in")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned),
                })
            })
            .collect();
        account.usage_reports.last_mut().unwrap().limits = limits.clone();
        account.limits.extend(limits);
    }
}

fn usage_provider_matches(account: &Account, provider_name: &str) -> bool {
    let name = provider_name.to_ascii_lowercase();
    match account.id.as_str() {
        "claude" => name.starts_with("anthropic (claude)") || name.starts_with("anthropic - "),
        "anthropic-api" => name.starts_with("anthropic api"),
        "openai" => name.starts_with("openai (chatgpt)") || name.starts_with("openai - "),
        "openai-api" => name.starts_with("openai api"),
        "jcode" => name.starts_with("jcode subscription"),
        id => {
            let id = id.replace(['-', '_'], " ");
            name == id || name.starts_with(&format!("{id} "))
        }
    }
}

/// The provider's logo, tinted at paint time by the element's text color.
/// Logos are vendored from lobehub's MIT-licensed icon set (see
/// assets/icons/LICENSE); providers without a recognizable mark fall back to
/// a lettermark drawn by the caller.
pub fn logo(provider_id: &str) -> Option<&'static [u8]> {
    macro_rules! icon {
        ($name:literal) => {
            Some(include_bytes!(concat!("../../../assets/icons/", $name, ".svg")) as &'static [u8])
        };
    }
    match provider_id {
        "claude" => icon!("claude"),
        "jcode" => icon!("jcode"),
        "anthropic-api" => icon!("anthropic"),
        "openai" | "openai-api" | "openai-compatible" => icon!("openai"),
        "gemini" | "gemini-api" => icon!("gemini"),
        "google" => icon!("google"),
        "copilot" => icon!("githubcopilot"),
        "openrouter" => icon!("openrouter"),
        "bedrock" => icon!("bedrock"),
        "azure" => icon!("azure"),
        "cursor" => icon!("cursor"),
        "antigravity" => icon!("antigravity"),
        "grok-build" => icon!("grok"),
        "xai" => icon!("xai"),
        "mistral" => icon!("mistral"),
        "deepseek" => icon!("deepseek"),
        "moonshotai" | "kimi" => icon!("moonshot"),
        "zai" => icon!("zhipu"),
        "alibaba-coding-plan" => icon!("qwen"),
        "groq" => icon!("groq"),
        "perplexity" => icon!("perplexity"),
        "huggingface" => icon!("huggingface"),
        "togetherai" => icon!("together"),
        "deepinfra" => icon!("deepinfra"),
        "cerebras" => icon!("cerebras"),
        "minimax" => icon!("minimax"),
        "nvidia-nim" => icon!("nvidia"),
        "fireworks" => icon!("fireworks"),
        "baseten" => icon!("baseten"),
        "lmstudio" => icon!("lmstudio"),
        "ollama" => icon!("ollama"),
        _ => None,
    }
}

/// The lettermark for providers without a vendored logo.
pub fn lettermark(display_name: &str) -> String {
    display_name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_accounts_are_bounded_unique_and_stable() {
        let mut recent = Vec::new();
        for id in [
            "claude",
            "openai",
            "gemini",
            "openrouter",
            "gemini",
            "gemini",
        ] {
            record_use(&mut recent, id.into());
        }
        assert_eq!(recent, ["gemini", "openrouter", "openai"]);
    }

    #[test]
    fn credential_identity_distinguishes_oauth_from_api_keys() {
        assert_eq!(credential_id("anthropic", Some("oauth")), "claude");
        assert_eq!(credential_id("anthropic", Some("api key")), "anthropic-api");
        assert_eq!(credential_id("openai", Some("api key")), "openai-api");
        assert_eq!(credential_id("openai", Some("oauth")), "openai");
        assert_eq!(credential_id("openrouter", None), "openrouter");
    }

    #[test]
    fn active_then_recent_then_available_and_expired() {
        let rows: Vec<_> = ["other", "expired", "recent", "active", "other2"]
            .into_iter()
            .map(|id| Account {
                id: id.into(),
                display_name: id.into(),
                status: if id == "expired" {
                    "expired"
                } else {
                    "available"
                }
                .into(),
                auth_kind: String::new(),
                method: String::new(),
                usage_reports: Vec::new(),
                limits: Vec::new(),
            })
            .collect();
        let recent = vec!["recent".into(), "active".into()];
        let ids: Vec<_> = ordered(&rows, Some("active"), &recent)
            .into_iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(ids, ["active", "recent", "other", "other2", "expired"]);
        let ids: Vec<_> = ordered(&rows, Some("unknown"), &[])
            .into_iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(ids, ["other", "recent", "active", "other2", "expired"]);
    }

    const SAMPLE: &str = r#"{
        "any_available": true,
        "providers": [
            {"id": "claude", "display_name": "Anthropic/Claude", "status": "expired",
             "method": "OAuth (expired)", "auth_kind": "OAuth", "recommended": true},
            {"id": "anthropic-api", "display_name": "Anthropic API", "status": "available",
             "method": "API key (`ANTHROPIC_API_KEY`)", "auth_kind": "API key"},
            {"id": "openrouter", "display_name": "OpenRouter", "status": "not_configured",
             "method": "not configured", "auth_kind": "API key"},
            {"id": "openai", "display_name": "OpenAI", "status": "available",
             "method": "OAuth", "auth_kind": "OAuth"}
        ]
    }"#;

    #[test]
    fn parse_keeps_configured_credentials_and_puts_available_first() {
        let accounts = parse(SAMPLE).expect("sample parses");
        assert_eq!(
            accounts
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            vec!["anthropic-api", "openai", "claude"],
            "unconfigured providers are dropped, expired ones sink"
        );
        assert!(accounts[0].available());
        assert_eq!(accounts[2].status, "expired");
        assert_eq!(accounts[2].auth_kind, "OAuth");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse("not json").is_none());
        assert!(parse("{}").is_none());
    }

    #[test]
    fn oauth_history_preserves_each_label_and_report_without_summing() {
        let mut accounts = parse(SAMPLE).unwrap();
        accounts.extend(parse(r#"{"providers":[{"id":"openai-api","display_name":"OpenAI API","status":"available","auth_kind":"API key"}]}"#).unwrap());
        merge_usage(
            &mut accounts,
            r#"{"providers":[
            {"provider_name":"OpenAI - personal (p***l@example.com) ✦","limits":[],"extra_info":[
                ["Account label","personal"],["Today","10 input · ~$0.01"],["Lifetime","100 input · ~$0.10"]
            ]},
            {"provider_name":"OpenAI - work (w***k@example.com)","extra_info":[
                ["Account label","work"],["Today","20 input · ~$0.02"],["Lifetime","200 input · ~$0.20"],
                ["Coverage","Recorded history only"], ["invalid"], null, [123,"bad"]
            ]},
            {"provider_name":"OpenAI API","extra_info":[["Today","999 API-key tokens"]]}
        ]}"#,
        );
        let openai = accounts
            .iter()
            .find(|account| account.id == "openai")
            .unwrap();
        assert!(openai.shows_oauth_history());
        assert_eq!(openai.usage_reports.len(), 2);
        let personal = &openai.usage_reports[0];
        let work = &openai.usage_reports[1];
        assert_eq!(personal.account_label.as_deref(), Some("personal"));
        assert_eq!(
            personal.title(),
            "OpenAI - personal (p***l@example.com) ✦ · personal"
        );
        assert_eq!(
            personal.extra_info[0],
            ("Today".into(), "10 input · ~$0.01".into())
        );
        assert_eq!(work.account_label.as_deref(), Some("work"));
        assert_eq!(work.extra_info.len(), 3);
        assert_eq!(
            work.extra_info[1],
            ("Lifetime".into(), "200 input · ~$0.20".into())
        );
        assert!(
            accounts
                .iter()
                .filter(|account| !matches!(account.id.as_str(), "openai" | "openai-api"))
                .all(|account| account.usage_reports.is_empty())
        );
        let api = accounts
            .iter()
            .find(|account| account.id == "openai-api")
            .unwrap();
        assert!(!api.shows_oauth_history());
        assert_eq!(api.usage_reports.len(), 1);
        assert_eq!(api.usage_reports[0].extra_info[0].1, "999 API-key tokens");
    }

    #[test]
    fn oauth_history_retains_no_data_and_accepts_older_reports() {
        let mut accounts = parse(SAMPLE).unwrap();
        merge_usage(
            &mut accounts,
            r#"{"providers":[
            {"provider_name":"OpenAI (ChatGPT) empty","extra_info":[
                ["Today","No recorded usage"],["Lifetime","No recorded usage"]
            ]},
            {"provider_name":"OpenAI (ChatGPT) older","limits":[]}
        ]}"#,
        );
        let openai = accounts
            .iter()
            .find(|account| account.id == "openai")
            .unwrap();
        assert_eq!(openai.usage_reports[0].extra_info[0].1, "No recorded usage");
        assert_eq!(openai.usage_reports[0].title(), "OpenAI (ChatGPT) empty");
        assert!(openai.usage_reports[1].extra_info.is_empty());
        assert!(USAGE_ESTIMATE_NOTE.contains("not your ChatGPT bill"));
        assert!(USAGE_ESTIMATE_NOTE.contains("local midnight"));
    }

    #[test]
    fn usage_limits_merge_by_provider_and_preserve_every_limit() {
        let mut accounts = parse(SAMPLE).unwrap();
        merge_usage(
            &mut accounts,
            r#"{"providers":[{"provider_name":"OpenAI (ChatGPT) (a***@example.com)","limits":[
                {"name":"5 hour","usage_percent":24.5,"reset_in":"2h"},
                {"name":"Weekly","usage_percent":81.0,"reset_in":"4d"}
            ]}]}"#,
        );
        let openai = accounts
            .iter()
            .find(|account| account.id == "openai")
            .unwrap();
        assert_eq!(openai.limits.len(), 2);
        assert_eq!(openai.limits[0].name, "5 hour");
        assert_eq!(openai.limits[1].usage_percent, 81.0);
        assert_eq!(openai.limits[1].reset_in.as_deref(), Some("4d"));
        assert_eq!(openai.active_limits(), Some(openai.limits.as_slice()));
    }

    #[test]
    fn status_limits_select_active_login_and_refuse_ambiguous_reports() {
        let mut accounts =
            parse(r#"{"providers":[{"id":"claude","status":"available"}]}"#).unwrap();
        merge_usage(
            &mut accounts,
            r#"{"providers":[
            {"provider_name":"Anthropic - personal ✦","limits":[{"name":"5 hour","usage_percent":24.0}]},
            {"provider_name":"Anthropic - work","limits":[{"name":"5 hour","usage_percent":91.0}]}
        ]}"#,
        );
        assert_eq!(accounts[0].active_limits().unwrap()[0].usage_percent, 24.);
        accounts[0].usage_reports[0].provider_name = "Anthropic - personal".into();
        assert!(accounts[0].active_limits().is_none());
        accounts[0].usage_reports[1].provider_name.push_str(" ✦");
        assert_eq!(accounts[0].active_limits().unwrap()[0].usage_percent, 91.);
    }

    #[test]
    fn usage_merge_handles_duplicate_reports_empty_limits_and_bad_entries() {
        let mut accounts = parse(SAMPLE).unwrap();
        merge_usage(
            &mut accounts,
            r#"{"providers":[
                {"provider_name":"OpenAI (ChatGPT) first","limits":[
                    {"name":"5 hour","usage_percent":120.0,"reset_in":null},
                    {"name":"missing percent"}
                ]},
                {"provider_name":"OpenAI (ChatGPT) second","limits":[]},
                {"provider_name":"OpenAI (ChatGPT) third","limits":[
                    {"name":"Weekly","usage_percent":10.0,"reset_in":"6d"}
                ]},
                {"provider_name":"Unknown provider","limits":[
                    {"name":"Ignored","usage_percent":50.0}
                ]}
            ]}"#,
        );
        let openai = accounts
            .iter()
            .find(|account| account.id == "openai")
            .unwrap();
        assert_eq!(
            openai
                .limits
                .iter()
                .map(|limit| limit.name.as_str())
                .collect::<Vec<_>>(),
            ["5 hour", "Weekly"],
            "valid limits from duplicate credential reports accumulate"
        );
        assert_eq!(openai.limits[0].usage_percent, 120.0);
        assert_eq!(openai.limits[0].reset_in, None);
        assert!(
            accounts
                .iter()
                .filter(|account| account.id != "openai")
                .all(|account| account.limits.is_empty()),
            "unknown providers must not leak into another account"
        );
    }

    /// The refresh path: a poll between snapshots sees the latest one, stale
    /// intermediate snapshots are skipped, and an idle feed yields nothing, so
    /// the UI only repaints when the login state actually changed.
    #[test]
    fn the_feed_drains_to_the_newest_snapshot() {
        let (tx, rx) = channel();
        let feed = Feed {
            updates: Arc::new(Mutex::new(rx)),
        };
        assert_eq!(feed.latest(), None, "an idle feed reports no change");

        let snapshot = |id: &str| {
            vec![Account {
                id: id.into(),
                display_name: id.into(),
                status: "available".into(),
                auth_kind: "OAuth".into(),
                method: "OAuth".into(),
                usage_reports: Vec::new(),
                limits: Vec::new(),
            }]
        };
        tx.send(snapshot("stale")).unwrap();
        tx.send(snapshot("fresh")).unwrap();

        let latest = feed.latest().expect("two snapshots are pending");
        assert_eq!(latest[0].id, "fresh", "the poll skips stale snapshots");
        assert_eq!(feed.latest(), None, "and the queue is now drained");
    }

    /// Acceptance path: the real CLI's real report must parse and yield the
    /// credentials the runtime actually has. Ignored by default because it
    /// needs `jcode` on PATH and the user's credentials; run explicitly with
    /// `cargo test -- --ignored live_cli`.
    #[test]
    #[ignore = "requires the jcode CLI and user credentials"]
    fn live_cli_report_parses_and_lists_configured_accounts() {
        let accounts = fetch().expect("jcode auth status --json should run and parse");
        assert!(
            !accounts.is_empty(),
            "this machine has configured credentials, so the list must not be empty"
        );
        for account in &accounts {
            assert!(matches!(account.status.as_str(), "available" | "expired"));
            assert!(!account.display_name.is_empty());
            assert!(!account.auth_kind.is_empty());
        }
        let mut seen_expired = false;
        for account in &accounts {
            if account.available() {
                assert!(!seen_expired, "available accounts must sort before expired");
            } else {
                seen_expired = true;
            }
        }
    }

    #[test]
    fn every_shippable_provider_has_a_logo_and_the_rest_fall_back() {
        for id in [
            "claude",
            "jcode",
            "anthropic-api",
            "openai",
            "openai-api",
            "gemini",
            "google",
            "copilot",
            "openrouter",
            "bedrock",
            "azure",
            "cursor",
            "antigravity",
            "grok-build",
            "xai",
            "mistral",
            "deepseek",
            "moonshotai",
            "groq",
            "perplexity",
            "huggingface",
            "togetherai",
            "cerebras",
            "nvidia-nim",
            "lmstudio",
            "ollama",
        ] {
            let bytes = logo(id).unwrap_or_else(|| panic!("{id} should have a logo"));
            assert!(
                bytes.starts_with(b"<svg"),
                "{id}'s logo should be an svg document"
            );
        }
        assert!(logo("unknown-provider").is_none());
        assert_eq!(lettermark("jcode router"), "J");
        assert_eq!(lettermark(""), "?");
    }
}
