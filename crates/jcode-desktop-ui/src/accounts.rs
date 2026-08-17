//! Connected accounts: which OAuth logins and API keys the runtime can use,
//! fetched from `jcode auth status --json` and shown with provider logos.
//!
//! The desktop shows only configured credentials (available or expired), not
//! the full catalog of possible providers: the question this surface answers
//! is "what am I logged into", not "what could I log into".

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageLimit {
    pub name: String,
    /// Percentage consumed, clamped by the UI when painted.
    pub usage_percent: f32,
    pub reset_in: Option<String>,
}

impl Account {
    pub fn available(&self) -> bool {
        self.status == "available"
    }
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
    std::thread::Builder::new()
        .name("jcode-accounts".into())
        .spawn(move || {
            loop {
                if let Some(accounts) = fetch() {
                    if tx.send(accounts).is_err() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_secs(60));
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
        let Some(limits) = provider.get("limits").and_then(|value| value.as_array()) else {
            continue;
        };
        account.limits.extend(limits.iter().filter_map(|limit| {
            Some(UsageLimit {
                name: limit.get("name")?.as_str()?.to_owned(),
                usage_percent: limit.get("usage_percent")?.as_f64()? as f32,
                reset_in: limit
                    .get("reset_in")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned),
            })
        }));
    }
}

fn usage_provider_matches(account: &Account, provider_name: &str) -> bool {
    let name = provider_name.to_ascii_lowercase();
    match account.id.as_str() {
        "claude" => name.starts_with("anthropic (claude)"),
        "anthropic-api" => name.starts_with("anthropic api"),
        "openai" => name.starts_with("openai (chatgpt)"),
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
