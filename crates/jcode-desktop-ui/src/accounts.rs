//! Connected accounts: which OAuth logins and API keys the runtime can use,
//! fetched from `jcode auth status --json` and shown with provider logos.
//!
//! Keep the full runtime catalog so users can see both connected accounts and
//! supported providers they have not configured yet.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

static AUTH_REFRESH: (Mutex<u64>, std::sync::Condvar) = (Mutex::new(0), std::sync::Condvar::new());

/// Wake account feeds after a native login without waiting for the slow poll.
pub fn request_refresh() {
    if let Ok(mut generation) = AUTH_REFRESH.0.lock() {
        *generation = generation.wrapping_add(1);
        AUTH_REFRESH.1.notify_all();
    }
}

/// One supported provider, as reported by the CLI's canonical auth report.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    /// Stable provider id (`claude`, `openai-api`, ...): keys the logo lookup.
    pub id: String,
    pub display_name: String,
    /// `available`, `expired`, or `not_configured`.
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
    /// A banked usage reset this login can redeem, from `jcode usage --json`.
    pub banked_reset: Option<BankedReset>,
}

/// Read-only reset availability. Redeeming always needs a fresh, confirmed
/// preparation against the provider, never these cached facts alone.
#[derive(Debug, Clone, PartialEq)]
pub struct BankedReset {
    pub provider: ResetProvider,
    /// Login the reset is pinned to. `None` is the default login.
    pub account_label: Option<String>,
    pub available_count: u64,
    /// The limit is actually enforced, so a reset helps right now.
    pub limit_reached: bool,
    /// RFC3339 time the next reset becomes available when none is left.
    pub next_available_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetProvider {
    OpenAi,
    Claude,
}

impl ResetProvider {
    /// Harness API provider id for usage invalidation.
    pub fn api_id(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Claude => "claude",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Claude => "Claude",
        }
    }
}

impl BankedReset {
    fn parse(value: &serde_json::Value) -> Option<Self> {
        let provider = match value.get("provider")?.as_str()? {
            "openai" => ResetProvider::OpenAi,
            "claude" => ResetProvider::Claude,
            _ => return None,
        };
        let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        Some(Self {
            provider,
            account_label: text("account_label"),
            available_count: value.get("available_count")?.as_u64()?,
            limit_reached: value
                .get("limit_reached")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            next_available_at: text("next_available_at"),
        })
    }

    /// Worth offering: redeemable now. OpenAI resets are banked, so they are
    /// only suggested once the limit actually binds, never spent early.
    pub fn offerable(&self) -> bool {
        self.available_count > 0 && (self.limit_reached || self.provider == ResetProvider::Claude)
    }

    /// Compact pill caption.
    pub fn pill_label(&self) -> String {
        match (self.provider, self.available_count) {
            (ResetProvider::OpenAi, count) if count > 1 => format!("Reset limits · {count}"),
            (ResetProvider::OpenAi, _) => "Reset limits".to_string(),
            (ResetProvider::Claude, _) => "Reset session".to_string(),
        }
    }
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

    pub fn status_label(&self) -> &'static str {
        match self.status.as_str() {
            "available" if matches!(self.auth_kind.as_str(), "OAuth" | "device code") => {
                "Signed in"
            }
            "available" => "Connected",
            "expired" => "Expired · sign in again",
            "not_configured" if matches!(self.auth_kind.as_str(), "OAuth" | "device code") => {
                "Not signed in"
            }
            "not_configured" => "Not configured",
            _ => "Status unknown",
        }
    }

    /// Resets this account can offer, one per login, in report order.
    pub fn offerable_resets(&self) -> impl Iterator<Item = &BankedReset> {
        self.usage_reports
            .iter()
            .filter_map(|report| report.banked_reset.as_ref())
            .filter(|reset| reset.offerable())
    }

    pub fn shows_oauth_history(&self) -> bool {
        self.id == "openai" && self.status != "not_configured"
    }

    /// Jcode plan upgrade suggested by `jcode usage` when a daily included
    /// allowance is running out: (label, url). Only jcode.sh links are offered.
    pub fn upgrade_offer(&self) -> Option<(String, String)> {
        if self.id != "jcode" {
            return None;
        }
        self.usage_reports.iter().find_map(|report| {
            let (_, value) = report.extra_info.iter().find(|(key, _)| key == "Upgrade")?;
            let (label, url) = value.rsplit_once(": ")?;
            let url = url.trim();
            (url.starts_with("https://jcode.sh/") || url.starts_with("https://www.jcode.sh/"))
                .then(|| (label.trim().to_owned(), url.to_owned()))
        })
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
        let priority = if active == Some(account.id.as_str()) {
            0
        } else if let Some(index) = recent.iter().position(|id| id == &account.id) {
            index + 1
        } else if account.available() {
            4
        } else {
            5
        };
        (!account.available(), priority)
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
///
/// Every window subscribes to one process-wide poller. A shared single-panel
/// host used to run a thread and a `jcode auth status` subprocess per window,
/// which scaled memory and CPU with the number of open panels for identical
/// data. The poller survives hot reload only as long as this UI generation.
pub fn spawn() -> Feed {
    let (tx, rx) = channel();
    if crate::harness::screenshot_mode() {
        let accounts = [
            ("openai", "OpenAI", "available", "OAuth"),
            ("claude", "Claude", "available", "OAuth"),
            ("jcode", "Jcode", "available", "API key"),
            ("gemini", "Gemini", "not_configured", "OAuth"),
            ("openrouter", "OpenRouter", "not_configured", "API key"),
            ("copilot", "Copilot", "expired", "device code"),
        ]
        .into_iter()
        .map(|(id, name, status, auth_kind)| Account {
            id: id.into(),
            display_name: name.into(),
            status: status.into(),
            auth_kind: auth_kind.into(),
            method: "Offline fixture".into(),
            usage_reports: if id == "claude" {
                vec![UsageReport {
                    provider_name: "Anthropic (Claude)".into(),
                    account_label: None,
                    limits: Vec::new(),
                    extra_info: Vec::new(),
                    banked_reset: Some(BankedReset {
                        provider: ResetProvider::Claude,
                        account_label: None,
                        available_count: 1,
                        limit_reached: true,
                        next_available_at: None,
                    }),
                }]
            } else if id == "openai" {
                vec![UsageReport {
                    provider_name: "OpenAI (ChatGPT)".into(),
                    account_label: Some("personal".into()),
                    limits: vec![UsageLimit { name: "5 hour".into(), usage_percent: 25., reset_in: Some("2h".into()) }],
                    banked_reset: None,
                    extra_info: vec![
                        ("Today".into(), "120000 input / 8000 output tokens (90000 cached input), $0.4200 API-equivalent estimate, not a bill; recorded only; since local midnight".into()),
                        ("Lifetime".into(), "2400000 input / 160000 output tokens (1800000 cached input), $8.4000 known + unknown cost (2 unpriced responses) API-equivalent estimate, not a bill; recorded only; partial token counts; since 2026-09-01 10:00 -07:00".into()),
                    ],
                }]
            } else {
                Vec::new()
            },
            limits: if status == "available" { vec![UsageLimit {
                name: "5 hour".into(),
                usage_percent: 25.0,
                reset_in: Some("2h".into()),
            }] } else { Vec::new() },
        })
        .collect();
        let _ = tx.send(accounts);
        return Feed {
            updates: Arc::new(Mutex::new(rx)),
        };
    }
    shared_poller().subscribe(tx);
    Feed {
        updates: Arc::new(Mutex::new(rx)),
    }
}

/// Process-wide account poller with fan-out to every window's feed.
struct Poller {
    subscribers: Mutex<Vec<Sender<Vec<Account>>>>,
    latest: Mutex<Option<Vec<Account>>>,
}

impl Poller {
    fn subscribe(&self, tx: Sender<Vec<Account>>) {
        // A new window gets the current snapshot immediately instead of
        // waiting up to a minute for the next poll.
        if let Some(latest) = self.latest.lock().unwrap().clone() {
            let _ = tx.send(latest);
        }
        self.subscribers.lock().unwrap().push(tx);
    }

    /// Deliver to live subscribers and drop closed windows' senders.
    fn publish(&self, accounts: Vec<Account>) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|subscriber| subscriber.send(accounts.clone()).is_ok());
        *self.latest.lock().unwrap() = Some(accounts);
    }
}

fn shared_poller() -> &'static Poller {
    static POLLER: OnceLock<Poller> = OnceLock::new();
    POLLER.get_or_init(|| {
        std::thread::Builder::new()
            .name("jcode-accounts".into())
            .spawn(|| {
                loop {
                    let generation = *AUTH_REFRESH.0.lock().unwrap();
                    if let Some(accounts) = fetch() {
                        shared_poller().publish(accounts);
                    }
                    let _ = AUTH_REFRESH.1.wait_timeout_while(
                        AUTH_REFRESH.0.lock().unwrap(),
                        Duration::from_secs(60),
                        |current| *current == generation,
                    );
                }
            })
            .expect("spawn accounts thread");
        Poller {
            subscribers: Mutex::new(Vec::new()),
            latest: Mutex::new(None),
        }
    })
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

/// Parse the complete CLI catalog, with connected providers first.
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
            (!account.id.is_empty()).then_some(account)
        })
        .collect();
    accounts.sort_by_key(|account| !account.available());
    Some(accounts)
}

#[cfg(test)]
pub(crate) fn merge_usage_for_tests(accounts: &mut [Account], json: &str) {
    merge_usage(accounts, json)
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
            banked_reset: provider.get("banked_reset").and_then(BankedReset::parse),
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
    fn parse_keeps_all_providers_and_puts_available_first() {
        let accounts = parse(SAMPLE).expect("sample parses");
        assert_eq!(
            accounts
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            vec!["anthropic-api", "openai", "claude", "openrouter"],
            "unconfigured and expired providers remain visible after connected ones"
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
    fn signed_out_active_and_recent_accounts_do_not_split_connection_groups() {
        let accounts = parse(SAMPLE).unwrap();
        let rows = ordered(&accounts, Some("openrouter"), &["claude".into()]);
        assert_eq!(
            rows.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["anthropic-api", "openai", "openrouter", "claude"]
        );
    }

    #[test]
    fn labels_distinguish_login_key_expiry_and_unknown_status() {
        let mut account = parse(SAMPLE).unwrap().remove(0);
        for (status, kind, label) in [
            ("available", "OAuth", "Signed in"),
            ("available", "device code", "Signed in"),
            ("available", "API key", "Connected"),
            ("available", "local endpoint", "Connected"),
            ("expired", "OAuth", "Expired · sign in again"),
            ("not_configured", "OAuth", "Not signed in"),
            ("not_configured", "device code", "Not signed in"),
            ("not_configured", "API key", "Not configured"),
            ("not_configured", "local endpoint", "Not configured"),
            ("new-status", "OAuth", "Status unknown"),
        ] {
            account.status = status.into();
            account.auth_kind = kind.into();
            assert_eq!(account.status_label(), label);
        }
    }

    #[test]
    fn completely_unconfigured_catalog_is_not_an_empty_account_list() {
        let accounts = parse(
            r#"{"providers":[
            {"id":"openai","status":"not_configured","auth_kind":"OAuth"},
            {"id":"ollama","status":"not_configured","auth_kind":"local endpoint"},
            {"id":"future-provider","status":"unknown"}, {}
        ]}"#,
        )
        .unwrap();
        assert_eq!(accounts.len(), 3);
        assert!(accounts.iter().all(|a| !a.available()));
        assert!(!accounts[0].shows_oauth_history());
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
    fn banked_resets_merge_per_login_and_only_redeemable_ones_are_offered() {
        let mut accounts = parse(SAMPLE).unwrap();
        merge_usage(
            &mut accounts,
            r#"{"providers":[
                {"provider_name":"OpenAI - work","limits":[],"extra_info":[["Account label","work"]],
                 "banked_reset":{"provider":"openai","account_label":"work","available_count":2,"limit_reached":true}},
                {"provider_name":"OpenAI - personal","limits":[],
                 "banked_reset":{"provider":"openai","account_label":"personal","available_count":1,"limit_reached":false}},
                {"provider_name":"OpenAI - broken","limits":[],"banked_reset":{"provider":"mystery","available_count":1}},
                {"provider_name":"Anthropic (Claude)","limits":[],
                 "banked_reset":{"provider":"claude","account_label":null,"available_count":0,"limit_reached":true,
                                 "next_available_at":"2099-01-08T00:00:00Z"}}
            ]}"#,
        );
        let openai = accounts
            .iter()
            .find(|account| account.id == "openai")
            .unwrap();
        let offered: Vec<_> = openai.offerable_resets().collect();
        assert_eq!(
            offered.len(),
            1,
            "unbound limits and unknown providers are not offered"
        );
        assert_eq!(offered[0].account_label.as_deref(), Some("work"));
        assert_eq!(offered[0].provider, ResetProvider::OpenAi);
        assert!(openai.usage_reports[2].banked_reset.is_none());
        let claude = accounts
            .iter()
            .find(|account| account.id == "claude")
            .unwrap();
        let spent = claude.usage_reports[0].banked_reset.as_ref().unwrap();
        assert!(!spent.offerable());
        assert_eq!(
            spent.next_available_at.as_deref(),
            Some("2099-01-08T00:00:00Z")
        );
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
    fn jcode_subscription_usage_attaches_daily_limits_and_upgrade_hint() {
        let mut accounts = parse(
            r#"{"any_available": true, "providers": [
                {"id": "jcode", "display_name": "Jcode", "status": "available",
                 "method": "API key", "auth_kind": "API key"}
            ]}"#,
        )
        .unwrap();
        merge_usage(
            &mut accounts,
            r#"{"providers":[{"provider_name":"Jcode subscription","limits":[
                {"name":"Memory recall (daily)","usage_percent":100.0,"reset_in":"20h 21m"},
                {"name":"Browser automation (daily)","usage_percent":0.05,"reset_in":"20h 21m"}
              ],"extra_info":[["Plan","Plus"],["Upgrade","Pro raises daily limits: https://jcode.sh/pricing"]]}]}"#,
        );
        let jcode = &accounts[0];
        assert_eq!(jcode.limits.len(), 2);
        assert_eq!(jcode.limits[0].usage_percent, 100.0);
        let report = &jcode.usage_reports[0];
        assert!(
            report
                .extra_info
                .iter()
                .any(|(key, value)| key == "Upgrade" && value.contains("https://jcode.sh/pricing"))
        );
    }

    #[test]
    fn upgrade_offer_only_for_jcode_with_jcode_sh_links() {
        let mut accounts = parse(
            r#"{"providers":[{"id":"jcode","display_name":"Jcode","status":"available","auth_kind":"API key"},
                             {"id":"openrouter","display_name":"OpenRouter","status":"available","auth_kind":"API key"}]}"#,
        )
        .unwrap();
        let jcode = accounts.iter().position(|a| a.id == "jcode").unwrap();
        let other = accounts.iter().position(|a| a.id == "openrouter").unwrap();
        for index in [jcode, other] {
            accounts[index].usage_reports.push(UsageReport {
                provider_name: "Jcode subscription".into(),
                account_label: None,
                banked_reset: None,
                limits: Vec::new(),
                extra_info: vec![(
                    "Upgrade".into(),
                    "Pro raises daily limits: https://jcode.sh/pricing".into(),
                )],
            });
        }
        assert_eq!(
            accounts[jcode].upgrade_offer(),
            Some((
                "Pro raises daily limits".into(),
                "https://jcode.sh/pricing".into()
            ))
        );
        assert_eq!(accounts[other].upgrade_offer(), None);
        accounts[jcode].usage_reports[0].extra_info[0].1 = "Pro: https://evil.example/pay".into();
        assert_eq!(accounts[jcode].upgrade_offer(), None);
        accounts[jcode].usage_reports[0].extra_info[0].1 = "no link here".into();
        assert_eq!(accounts[jcode].upgrade_offer(), None);
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
    fn live_cli_report_parses_and_lists_all_accounts() {
        let accounts = fetch().expect("jcode auth status --json should run and parse");
        assert!(
            !accounts.is_empty(),
            "the runtime provider catalog must not be empty even without credentials"
        );
        for account in &accounts {
            assert!(matches!(
                account.status.as_str(),
                "available" | "expired" | "not_configured"
            ));
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
