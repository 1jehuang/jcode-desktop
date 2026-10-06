//! Model picker metadata comes from the daemon, including shared TUI preferences.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use jcode_sdk::{ModelRouteInfo, ModelUsage, compare_model_usage};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelDetails {
    pub model: String,
    pub recommended: bool,
    pub provider: String,
    pub api_method: String,
    pub usage: Option<ModelUsage>,
}

/// Route identity: everything that decides its spec and recommendation.
/// Usage is deliberately absent, since usage ticks are what rebroadcast.
type RouteKey = (String, String, String, String);

thread_local! {
    static ROUTE_SPECS: std::cell::RefCell<HashMap<RouteKey, (String, bool)>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Spec and recommendation, memoized. Every model usage tick resends the
/// full catalog, and re-deriving routing policy for each route was a steady
/// share of UI-thread time in live profiles. Bounded so catalog churn cannot
/// grow it without limit.
fn route_identity(route: &ModelRouteInfo) -> (String, bool) {
    const MAX_ENTRIES: usize = 8192;
    let key = (
        route.model.clone(),
        route.provider.clone(),
        route.api_method.clone(),
        route.detail.clone(),
    );
    ROUTE_SPECS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(hit) = cache.get(&key) {
            return hit.clone();
        }
        let value = (
            route_spec(route),
            jcode_provider_core::model_route_metadata_is_recommended(
                jcode_provider_core::explicit_model_provider_prefix(&route.model)
                    .map_or(route.model.as_str(), |(_, _, bare)| bare),
                &route.provider,
                &route.api_method,
                true,
            ),
        );
        if cache.len() >= MAX_ENTRIES {
            cache.clear();
        }
        cache.insert(key, value.clone());
        value
    })
}

pub(super) fn from_routes(routes: &[ModelRouteInfo]) -> HashMap<String, ModelDetails> {
    let mut details = HashMap::<String, ModelDetails>::with_capacity(routes.len());
    for route in routes.iter().filter(|route| route.available) {
        let (spec, recommended) = route_identity(route);
        let candidate = ModelDetails {
            model: route.model.clone(),
            recommended,
            provider: route.provider.clone(),
            api_method: route.api_method.clone(),
            usage: route.usage.clone(),
        };
        match details.get(&spec) {
            Some(existing)
                if !compare_model_usage(candidate.usage.as_ref(), existing.usage.as_ref())
                    .is_lt() => {}
            _ => {
                details.insert(spec, candidate);
            }
        }
    }
    details
}

/// Use the same routing policy as the daemon and TUI, never infer auth from a label.
fn route_spec(route: &ModelRouteInfo) -> String {
    use jcode_provider_core::{ModelRoute, RouteSelection, explicit_model_provider_prefix};
    let bare = explicit_model_provider_prefix(&route.model)
        .map_or(route.model.as_str(), |(_, _, bare)| bare);
    let mut selection = RouteSelection::from_model_route(&ModelRoute {
        model: bare.to_string(),
        provider: route.provider.clone(),
        api_method: route.api_method.clone(),
        available: route.available,
        detail: route.detail.clone(),
        cheapness: None,
        usage: route.usage.clone(),
    });
    let spec = selection.routed_model_spec();
    selection.model.clear();
    let prefix = selection.routed_model_spec();
    if !prefix.is_empty() && bare.starts_with(&prefix) {
        bare.to_string()
    } else {
        spec
    }
}

pub(super) fn current_spec<'a>(
    current: Option<&str>,
    models: &'a [String],
    details: &HashMap<String, ModelDetails>,
) -> Option<&'a str> {
    let current = current?;
    models
        .iter()
        .find(|model| model.as_str() == current)
        .or_else(|| {
            models
                .iter()
                .find(|model| details.get(*model).is_some_and(|d| d.model == current))
        })
        .map(String::as_str)
}

/// Order routes by usage, then recommendation, then spec.
///
/// Every usage tick rebroadcasts the whole catalog to every panel, so this
/// runs often on the UI thread. Looking each route up once, instead of four
/// SipHash lookups per comparison, took it from the top live-profile hotspot
/// to noise.
pub(super) fn rank(models: &mut Vec<String>, details: &HashMap<String, ModelDetails>) {
    let mut keyed: Vec<_> = std::mem::take(models)
        .into_iter()
        .map(|model| {
            let detail = details.get(&model);
            let usage = detail.and_then(|detail| detail.usage.as_ref());
            let recommended = detail.is_some_and(|detail| detail.recommended);
            (usage, recommended, model)
        })
        .collect();
    keyed.sort_by(|(a_usage, a_rec, a), (b_usage, b_rec, b)| {
        compare_model_usage(*a_usage, *b_usage)
            .then_with(|| b_rec.cmp(a_rec))
            .then_with(|| a.cmp(b))
    });
    models.extend(keyed.into_iter().map(|(_, _, model)| model));
}

pub(super) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn relative_time(timestamp: u64, now: u64) -> String {
    let age = now.saturating_sub(timestamp);
    match age {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", age / 60),
        3600..86400 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86400),
    }
}

pub(super) fn usage_label(usage: Option<&ModelUsage>, now: u64) -> String {
    let Some(usage) = usage else {
        return "Usage history unavailable".into();
    };
    if usage.count > 0 {
        let count = usage.count;
        let unit = if count == 1 {
            "tracked turn"
        } else {
            "tracked turns"
        };
        let last = usage
            .last_used_unix_secs
            .map(|timestamp| format!("Last used {}", relative_time(timestamp, now)))
            .unwrap_or_else(|| "Last use unknown".into());
        return format!("{count} {unit} · {last}");
    }
    if usage.selection_count > 0 {
        let count = usage.selection_count;
        let unit = if count == 1 {
            "prior selection"
        } else {
            "prior selections"
        };
        let last = usage
            .last_selected_unix_secs
            .map(|timestamp| format!("Selected {}", relative_time(timestamp, now)))
            .unwrap_or_else(|| "Selection time unknown".into());
        return format!("{count} {unit} · {last} · No tracked turns");
    }
    "No recorded usage yet".into()
}

impl ModelDetails {
    /// Secondary line under the pretty title. The title already names the
    /// model and the group header names provider and auth, so only usage is
    /// new information. Search still matches the exact id.
    pub(super) fn label(&self, now: u64) -> String {
        usage_label(self.usage.as_ref(), now)
    }
}

/// How a route authenticates, phrased and iconed for the group header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthKind {
    /// Signed in with a provider account (ChatGPT, Claude, GitHub, Google).
    Account,
    ApiKey,
    Subscription,
    CloudCredentials,
    Other,
}

impl AuthKind {
    pub(super) fn of(api_method: &str) -> Self {
        use jcode_provider_core::ModelRouteApiMethod as M;
        match M::parse(api_method) {
            M::ClaudeOAuth
            | M::OpenAIOAuth
            | M::CodeAssistOAuth
            | M::Copilot
            | M::Cursor
            | M::AntigravityHttps
            | M::GrokBuild => Self::Account,
            M::AnthropicApiKey | M::OpenAIApiKey | M::OpenRouter | M::OpenAiCompatible { .. } => {
                Self::ApiKey
            }
            M::JcodeSubscription => Self::Subscription,
            M::Bedrock => Self::CloudCredentials,
            M::Other(method) => {
                let method = method.to_ascii_lowercase();
                if method.contains("oauth") {
                    Self::Account
                } else if method.contains("key") || method.ends_with("-api") {
                    Self::ApiKey
                } else {
                    Self::Other
                }
            }
            M::RemoteCatalog | M::Current => Self::Other,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Account => "Signed in",
            Self::ApiKey => "API key",
            Self::Subscription => "Subscription",
            Self::CloudCredentials => "Cloud credentials",
            Self::Other => "Connected",
        }
    }

    pub(super) fn icon(self) -> &'static [u8] {
        match self {
            Self::Account => include_bytes!("../../../assets/icons/account.svg"),
            Self::ApiKey | Self::CloudCredentials => {
                include_bytes!("../../../assets/icons/key.svg")
            }
            Self::Subscription => include_bytes!("../../../assets/icons/subscription.svg"),
            Self::Other => include_bytes!("../../../assets/icons/plug.svg"),
        }
    }
}

/// Group header parts: provider brand, its logo id, and the auth method.
pub(super) struct HeaderParts {
    pub provider: String,
    pub logo: &'static str,
    pub auth: AuthKind,
}

pub(super) fn header_parts(model: &str, details: &HashMap<String, ModelDetails>) -> HeaderParts {
    let Some(detail) = details.get(model) else {
        return HeaderParts {
            provider: "Other models".into(),
            logo: "",
            auth: AuthKind::Other,
        };
    };
    let provider = if detail.provider.is_empty() {
        crate::panel::pretty_provider_name(&detail.api_method)
    } else {
        crate::panel::pretty_provider_name(&detail.provider)
    };
    HeaderParts {
        logo: provider_logo(&detail.provider, &detail.api_method),
        provider,
        auth: AuthKind::of(&detail.api_method),
    }
}

/// Logo id (see `accounts::logo`) for the service a route goes through.
fn provider_logo(provider: &str, api_method: &str) -> &'static str {
    let haystack = format!("{provider} {api_method}").to_ascii_lowercase();
    for (needle, logo) in [
        ("openrouter", "openrouter"),
        ("copilot", "copilot"),
        ("cursor", "cursor"),
        ("bedrock", "bedrock"),
        ("azure", "azure"),
        ("antigravity", "antigravity"),
        ("jcode", "jcode"),
        ("anthropic", "anthropic-api"),
        ("claude", "anthropic-api"),
        ("openai", "openai"),
        ("chatgpt", "openai"),
        ("gemini", "gemini"),
        ("code-assist", "gemini"),
        ("google", "gemini"),
        ("grok", "xai"),
        ("xai", "xai"),
        ("mistral", "mistral"),
        ("deepseek", "deepseek"),
        ("groq", "groq"),
        ("ollama", "ollama"),
        ("lmstudio", "lmstudio"),
    ] {
        if haystack.contains(needle) {
            return logo;
        }
    }
    ""
}

/// Logo id for the model's own family, so an OpenAI model reads as OpenAI
/// whether it is served directly or through OpenRouter. Unknown families
/// fall back to the serving provider's logo.
pub(super) fn model_logo(model: &str, details: &HashMap<String, ModelDetails>) -> &'static str {
    let detail = details.get(model);
    let raw = detail.map_or(model, |detail| detail.model.as_str());
    let raw = jcode_provider_core::explicit_model_provider_prefix(raw)
        .map_or(raw, |(_, _, bare)| bare)
        .to_ascii_lowercase();
    let (vendor, name) = raw.rsplit_once('/').unwrap_or(("", raw.as_str()));
    let vendor = vendor.rsplit('/').next().unwrap_or(vendor);
    let by_vendor = match vendor {
        "anthropic" => "anthropic-api",
        "openai" => "openai",
        "google" => "gemini",
        "x-ai" | "xai" => "xai",
        "mistralai" | "mistral" => "mistral",
        "deepseek" | "deepseek-ai" => "deepseek",
        "moonshotai" => "kimi",
        "qwen" => "alibaba-coding-plan",
        "z-ai" | "zai" | "zhipuai" => "zai",
        "minimax" => "minimax",
        _ => "",
    };
    if !by_vendor.is_empty() {
        return by_vendor;
    }
    // Bedrock-style `anthropic.claude-...` ids name the family after a dot.
    let name = name.split_once('.').map_or(name, |(head, rest)| {
        if head.chars().all(|c| c.is_ascii_alphabetic()) {
            rest
        } else {
            name
        }
    });
    for (prefix, logo) in [
        ("claude", "anthropic-api"),
        ("gpt", "openai"),
        ("codex", "openai"),
        ("o1", "openai"),
        ("o3", "openai"),
        ("o4", "openai"),
        ("gemini", "gemini"),
        ("gemma", "gemini"),
        ("grok", "xai"),
        ("mistral", "mistral"),
        ("codestral", "mistral"),
        ("devstral", "mistral"),
        ("deepseek", "deepseek"),
        ("kimi", "kimi"),
        ("qwen", "alibaba-coding-plan"),
        ("glm", "zai"),
        ("minimax", "minimax"),
    ] {
        if name.starts_with(prefix) {
            return logo;
        }
    }
    detail.map_or("", |detail| {
        provider_logo(&detail.provider, &detail.api_method)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(count: u64, last: Option<u64>, selections: u64) -> ModelUsage {
        ModelUsage {
            count,
            last_used_unix_secs: last,
            tracking_started_unix_secs: Some(10),
            selection_count: selections,
            last_selected_unix_secs: Some(20),
        }
    }

    fn route(model: &str, method: &str, count: u64) -> ModelRouteInfo {
        ModelRouteInfo {
            model: model.into(),
            provider: "OpenAI".into(),
            api_method: method.into(),
            available: true,
            detail: String::new(),
            usage: Some(usage(count, Some(count), 0)),
        }
    }

    #[test]
    fn groups_keep_auth_routes_distinct_and_canonicalize_aliases() {
        let routes = [
            route("atlas", "openai-oauth", 2),
            route("atlas", "openai-api-key", 1),
            route("openai-oauth:atlas", "openai-oauth", 0),
        ];
        let details = from_routes(&routes);
        assert_eq!(details.len(), 2);
        assert!(details.contains_key("openai-oauth:atlas"));
        assert!(details.contains_key("openai-api:atlas"));
        assert_eq!(
            route_spec(&route("openai:atlas", "openai-api-key", 0)),
            "openai-api:atlas"
        );
        let rows = grouped_rows(
            &details.keys().cloned().collect::<Vec<_>>(),
            &details,
            None,
            &Default::default(),
            "",
        );
        assert_eq!(rows.iter().filter(|r| r.header.is_some()).count(), 2);
        assert_eq!(rows[0].value, "/model openai-oauth:atlas");
        assert_eq!(rows[1].value, "/model openai-api:atlas");
        assert_eq!(
            route_spec(&route("bedrock:anthropic.claude-v1:0", "bedrock", 0)),
            "bedrock:anthropic.claude-v1:0"
        );
    }

    #[test]
    fn groups_show_three_distinct_ranked_models_and_search_hidden_choices() {
        let routes: Vec<_> = (1..=6)
            .map(|n| route(&format!("atlas-{n}"), "openai-oauth", n))
            .collect();
        let details = from_routes(&routes);
        let mut models: Vec<_> = details.keys().cloned().collect();
        models.push("openai-oauth:atlas-6".into());
        let closed = grouped_rows(&models, &details, Some("atlas-1"), &Default::default(), "");
        assert_eq!(
            closed.iter().map(|r| r.value.as_str()).collect::<Vec<_>>(),
            [
                "/model openai-oauth:atlas-1",
                "/model openai-oauth:atlas-6",
                "/model openai-oauth:atlas-5",
                "Show 3 more models"
            ]
        );
        assert!(closed[0].header.is_some());
        let key = closed[3].toggle.clone().unwrap();
        let expanded = [key].into_iter().collect();
        let open = grouped_rows(&models, &details, Some("atlas-1"), &expanded, "");
        assert_eq!(open.len(), 7);
        assert_eq!(open[6].value, "Show fewer models");
        let search = grouped_rows(&models, &details, None, &Default::default(), "ATLAS-2");
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].value, "/model openai-oauth:atlas-2");
        assert!(search[0].toggle.is_none());
        assert_eq!(
            grouped_rows(&models, &details, None, &Default::default(), "oauth").len(),
            6
        );
        assert!(grouped_rows(&models, &details, None, &expanded, "missing").is_empty());
    }

    #[test]
    fn recommendation_breaks_usage_ties_and_unavailable_routes_are_excluded() {
        let mut unavailable = route("unavailable", "openai-oauth", 100);
        unavailable.available = false;
        let recommended = jcode_provider_core::DEFAULT_OPENAI_MODEL;
        let details = from_routes(&[
            route("a-unknown", "openai-oauth", 0),
            route(recommended, "openai-oauth", 0),
            unavailable,
        ]);
        let mut models: Vec<_> = details.keys().cloned().collect();
        rank(&mut models, &details);
        assert_eq!(
            models,
            [
                format!("openai-oauth:{recommended}"),
                "openai-oauth:a-unknown".to_string()
            ]
        );
    }

    #[test]
    fn metadata_does_not_claim_unknown_or_untracked_models_were_never_used() {
        assert_eq!(usage_label(None, 100), "Usage history unavailable");
        assert_eq!(
            usage_label(Some(&usage(0, None, 0)), 100),
            "No recorded usage yet"
        );
        assert_eq!(
            usage_label(Some(&usage(0, None, 4)), 100),
            "4 prior selections · Selected 1m ago · No tracked turns"
        );
        assert_eq!(
            usage_label(Some(&usage(1, Some(90), 4)), 100),
            "1 tracked turn · Last used just now"
        );
        assert_eq!(
            usage_label(Some(&usage(12, Some(100), 4)), 7300),
            "12 tracked turns · Last used 2h ago"
        );
        assert_eq!(relative_time(100, 864100), "10d ago");
        assert_eq!(relative_time(200, 100), "just now");
    }

    #[test]
    fn row_logo_names_the_model_maker_and_headers_name_route_and_auth() {
        let mut routed = route("openai/gpt-6-astra", "openrouter", 0);
        routed.provider = "OpenRouter".into();
        let details = from_routes(&[
            route("gpt-6-astra", "openai-api-key", 3),
            route("gpt-6-astra", "openai-oauth", 1),
            routed,
        ]);
        assert_eq!(details.len(), 3);
        for (spec, detail) in &details {
            assert_eq!(model_logo(spec, &details), "openai", "{spec}");
            let parts = header_parts(spec, &details);
            let expected = match detail.api_method.as_str() {
                "openrouter" => ("OpenRouter", "openrouter", AuthKind::ApiKey),
                "openai-oauth" => ("OpenAI", "openai", AuthKind::Account),
                _ => ("OpenAI", "openai", AuthKind::ApiKey),
            };
            assert_eq!((parts.provider.as_str(), parts.logo, parts.auth), expected);
        }
        assert_eq!(AuthKind::of("jcode-subscription"), AuthKind::Subscription);
        assert_eq!(AuthKind::of("bedrock"), AuthKind::CloudCredentials);
        assert_eq!(AuthKind::of("claude-oauth"), AuthKind::Account);
        let mut bedrock = route("anthropic.claude-v1:0", "bedrock", 0);
        bedrock.provider = "Bedrock".into();
        let details = from_routes(&[bedrock]);
        let spec = details.keys().next().unwrap();
        assert_eq!(model_logo(spec, &details), "anthropic-api");
        // Unknown families fall back to the serving provider.
        let mut private = route("house-model", "openrouter", 0);
        private.provider = "OpenRouter".into();
        let details = from_routes(&[private]);
        let spec = details.keys().next().unwrap();
        assert_eq!(model_logo(spec, &details), "openrouter");
    }

    #[test]
    fn detail_line_is_usage_only() {
        let detail =
            &from_routes(&[route("gpt-6-astra", "openai-api-key", 0)])["openai-api:gpt-6-astra"];
        assert_eq!(detail.label(100), "No recorded usage yet");
    }

    #[test]
    fn rank_uses_usage_then_stable_names_and_retains_every_model() {
        let mut models = vec![
            "unknown-z".into(),
            "frequent".into(),
            "unknown-a".into(),
            "recent".into(),
            "legacy".into(),
        ];
        let details = [
            ("frequent", usage(12, Some(100), 0)),
            ("recent", usage(12, Some(200), 0)),
            ("legacy", usage(0, None, 9)),
        ]
        .into_iter()
        .map(|(model, usage)| {
            (
                model.into(),
                ModelDetails {
                    model: model.into(),
                    recommended: false,
                    provider: String::new(),
                    api_method: String::new(),
                    usage: Some(usage),
                },
            )
        })
        .collect();
        rank(&mut models, &details);
        assert_eq!(
            models,
            ["recent", "frequent", "legacy", "unknown-a", "unknown-z"]
        );
    }
}

/// A route header belongs to its first selectable row, not the keyboard index space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GroupRow {
    pub value: String,
    pub header: Option<String>,
    pub toggle: Option<String>,
}

pub(super) fn group_key(model: &str, details: &HashMap<String, ModelDetails>) -> String {
    details
        .get(model)
        .map(|d| format!("{} · {}", d.provider, d.api_method))
        .unwrap_or_else(|| "Other models".into())
}

fn group_label(model: &str, details: &HashMap<String, ModelDetails>) -> String {
    let parts = header_parts(model, details);
    if details.get(model).is_none() {
        return parts.provider;
    }
    format!("{} · {}", parts.provider, parts.auth.label())
}

/// Friendly picker title for a route spec (`claude-oauth:claude-opus-4-8`
/// reads `Claude Opus 4.8`). The exact id stays visible in the detail line.
pub(super) fn pretty_title(model: &str, details: &HashMap<String, ModelDetails>) -> String {
    let raw = details
        .get(model)
        .map_or(model, |detail| detail.model.as_str());
    let pretty = jcode_provider_core::model_names::pretty_picker_model_name(raw);
    if pretty.is_empty() {
        raw.to_string()
    } else {
        pretty
    }
}

pub(super) fn grouped_rows(
    models: &[String],
    details: &HashMap<String, ModelDetails>,
    current: Option<&str>,
    expanded: &std::collections::HashSet<String>,
    query: &str,
) -> Vec<GroupRow> {
    let mut ranked = models.to_vec();
    rank(&mut ranked, details);
    let current = current_spec(current, &ranked, details).map(str::to_string);
    ranked.sort_by_key(|model| current.as_deref() != Some(model.as_str()));
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut group_indices = HashMap::new();
    let mut seen = std::collections::HashSet::with_capacity(ranked.len());
    for model in ranked {
        // Preserve ranked first occurrence and group order without repeatedly
        // scanning every previous group or every member of a large group.
        if !seen.insert(model.clone()) {
            continue;
        }
        let key = group_key(&model, details);
        let index = *group_indices.entry(key.clone()).or_insert_with(|| {
            groups.push((key, Vec::new()));
            groups.len() - 1
        });
        groups[index].1.push(model);
    }
    let query = super::model_search::ModelQuery::new(query);
    let searching = !query.is_empty();
    if searching {
        // Rank every route, then keep provider grouping but order groups by
        // their best hit so Enter always selects the strongest match.
        let ranked = super::model_search::rank_matches(
            groups.iter().flat_map(|(_, members)| members.iter()),
            details,
            |model| {
                format!(
                    "{} {}",
                    group_label(model, details),
                    group_key(model, details)
                )
            },
            &query,
        );
        let scores: HashMap<&String, (usize, i32)> = ranked
            .iter()
            .enumerate()
            .map(|(order, (model, score))| (*model, (order, *score)))
            .collect();
        let mut matched: Vec<(usize, Vec<String>)> = groups
            .iter()
            .filter_map(|(_, members)| {
                let mut hits: Vec<_> = members
                    .iter()
                    .filter_map(|model| scores.get(model).map(|(order, _)| (*order, model.clone())))
                    .collect();
                hits.sort_by_key(|(order, _)| *order);
                let best = hits.first()?.0;
                Some((best, hits.into_iter().map(|(_, model)| model).collect()))
            })
            .collect();
        matched.sort_by_key(|(best, _)| *best);
        let mut rows = Vec::new();
        for (_, members) in matched {
            for (index, model) in members.into_iter().enumerate() {
                rows.push(GroupRow {
                    header: (index == 0).then(|| group_label(&model, details)),
                    value: format!("/model {model}"),
                    toggle: None,
                });
            }
        }
        return rows;
    }
    let mut rows = Vec::new();
    for (key, matching) in groups {
        let count = matching.len();
        let open = expanded.contains(&key);
        for (index, model) in matching
            .into_iter()
            .take(if open { usize::MAX } else { 3 })
            .enumerate()
        {
            rows.push(GroupRow {
                header: (index == 0).then(|| group_label(&model, details)),
                value: format!("/model {model}"),
                toggle: None,
            });
        }
        if count > 3 {
            rows.push(GroupRow {
                value: if open {
                    "Show fewer models".into()
                } else {
                    format!("Show {} more models", count - 3)
                },
                header: None,
                toggle: Some(key),
            });
        }
    }
    rows
}

#[cfg(test)]
mod grouping_performance_tests {
    use super::*;
    use std::collections::HashSet;

    fn fixture(count: usize, group_count: usize) -> (Vec<String>, HashMap<String, ModelDetails>) {
        let models: Vec<_> = (0..count)
            .map(|i| format!("provider:model-{i:04}"))
            .collect();
        let details = models
            .iter()
            .enumerate()
            .map(|(i, model)| {
                (
                    model.clone(),
                    ModelDetails {
                        model: format!("model-{i:04}"),
                        recommended: i % 7 == 0,
                        provider: format!("Provider {}", i % group_count),
                        api_method: "openai-oauth".into(),
                        usage: None,
                    },
                )
            })
            .collect();
        (models, details)
    }

    #[test]
    fn indexed_grouping_matches_legacy_order_filtering_and_deduplication() {
        for group_count in [1, 8, 200] {
            let (mut models, details) = fixture(200, group_count);
            // Deliberately repeated and missing-metadata entries exercise stable
            // deduplication and the fallback group, not just unique route maps.
            models.extend(models.clone());
            models.extend(["unknown-model".into(), "unknown-model".into()]);
            let expanded: HashSet<_> = models.iter().map(|m| group_key(m, &details)).collect();
            for expansion in [&HashSet::new(), &expanded] {
                // Search is ranked by `model_search` and intentionally differs
                // from the legacy substring filter; browsing must not.
                for query in [""] {
                    for current in [None, Some("provider:model-0199"), Some("model-0005")] {
                        assert_eq!(
                            grouped_rows(&models, &details, current, expansion, query),
                            legacy_grouped_rows(&models, &details, current, expansion, query),
                            "groups={group_count}, query={query}, current={current:?}",
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "opt-in CPU microbenchmark, run with --ignored --nocapture"]
    fn grouped_rows_profile() {
        use std::hint::black_box;
        use std::time::Instant;
        type Grouping = fn(
            &[String],
            &HashMap<String, ModelDetails>,
            Option<&str>,
            &HashSet<String>,
            &str,
        ) -> Vec<GroupRow>;
        for count in [40, 200, 1000] {
            for group_count in [1, 8] {
                let (models, details) = fixture(count, group_count);
                let closed = HashSet::new();
                let expanded: HashSet<_> = models.iter().map(|m| group_key(m, &details)).collect();
                for (scenario, expansion, query) in [
                    ("collapsed", &closed, ""),
                    ("expanded", &expanded, ""),
                    ("broad-search", &closed, "model"),
                    ("narrow-search", &closed, "model-001"),
                    ("no-match", &closed, "missing"),
                ] {
                    let current = Some(models[count - 1].as_str());
                    if query.is_empty() {
                        assert_eq!(
                            grouped_rows(&models, &details, current, expansion, query),
                            legacy_grouped_rows(&models, &details, current, expansion, query)
                        );
                    }
                    for (implementation, group) in [
                        ("legacy", legacy_grouped_rows as Grouping),
                        ("indexed", grouped_rows as Grouping),
                    ] {
                        for _ in 0..20 {
                            black_box(group(&models, &details, current, expansion, query));
                        }
                        let mut micros = Vec::with_capacity(200);
                        for _ in 0..200 {
                            let started = Instant::now();
                            black_box(group(
                                black_box(&models),
                                &details,
                                current,
                                expansion,
                                query,
                            ));
                            micros.push(started.elapsed().as_secs_f64() * 1_000_000.);
                        }
                        micros.sort_by(f64::total_cmp);
                        eprintln!(
                            "grouped_rows {implementation} n={count} groups={group_count} {scenario}: p50={:.1}us p95={:.1}us max={:.1}us",
                            micros[100], micros[189], micros[199]
                        );
                    }
                }
            }
        }
    }

    // Frozen pre-indexing implementation: a same-binary reference for both
    // correctness and performance, not a second production code path.
    fn legacy_grouped_rows(
        models: &[String],
        details: &HashMap<String, ModelDetails>,
        current: Option<&str>,
        expanded: &std::collections::HashSet<String>,
        query: &str,
    ) -> Vec<GroupRow> {
        let mut ranked = models.to_vec();
        rank(&mut ranked, details);
        let current = current_spec(current, &ranked, details).map(str::to_string);
        ranked.sort_by_key(|model| current.as_deref() != Some(model.as_str()));
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for model in ranked {
            let key = group_key(&model, details);
            let index = groups
                .iter()
                .position(|(group, _)| group == &key)
                .unwrap_or_else(|| {
                    groups.push((key.clone(), Vec::new()));
                    groups.len() - 1
                });
            let members = &mut groups[index].1;
            if !members.contains(&model) {
                members.push(model);
            }
        }
        let query = query.trim().to_ascii_lowercase();
        let mut rows = Vec::new();
        for (key, members) in groups {
            let searching = !query.is_empty();
            let matching: Vec<_> = members
                .into_iter()
                .filter(|model| {
                    !searching
                        || model.to_ascii_lowercase().contains(&query)
                        || details.get(model).is_some_and(|detail| {
                            detail.model.to_ascii_lowercase().contains(&query)
                        })
                        || group_label(model, details)
                            .to_ascii_lowercase()
                            .contains(&query)
                        || key.to_ascii_lowercase().contains(&query)
                })
                .collect();
            let count = matching.len();
            let open = expanded.contains(&key);
            for (index, model) in matching
                .into_iter()
                .take(if searching || open { usize::MAX } else { 3 })
                .enumerate()
            {
                rows.push(GroupRow {
                    header: (index == 0).then(|| group_label(&model, details)),
                    value: format!("/model {model}"),
                    toggle: None,
                });
            }
            if !searching && count > 3 {
                rows.push(GroupRow {
                    value: if open {
                        "Show fewer models".into()
                    } else {
                        format!("Show {} more models", count - 3)
                    },
                    header: None,
                    toggle: Some(key),
                });
            }
        }
        rows
    }
}
