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

pub(super) fn from_routes(routes: &[ModelRouteInfo]) -> HashMap<String, ModelDetails> {
    let mut details = HashMap::<String, ModelDetails>::new();
    for route in routes.iter().filter(|route| route.available) {
        let spec = route_spec(route);
        let candidate = ModelDetails {
            model: route.model.clone(),
            recommended: jcode_provider_core::model_route_metadata_is_recommended(
                jcode_provider_core::explicit_model_provider_prefix(&route.model)
                    .map_or(route.model.as_str(), |(_, _, bare)| bare),
                &route.provider,
                &route.api_method,
                route.available,
            ),
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

pub(super) fn rank(models: &mut [String], details: &HashMap<String, ModelDetails>) {
    models.sort_by(|a, b| {
        compare_model_usage(
            details.get(a).and_then(|detail| detail.usage.as_ref()),
            details.get(b).and_then(|detail| detail.usage.as_ref()),
        )
        .then_with(|| {
            details
                .get(b)
                .is_some_and(|d| d.recommended)
                .cmp(&details.get(a).is_some_and(|d| d.recommended))
        })
        .then_with(|| a.cmp(b))
    });
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
    pub(super) fn label(&self, now: u64) -> String {
        let route = match (self.provider.is_empty(), self.api_method.is_empty()) {
            (false, false) => format!(
                "{} · {}",
                self.provider,
                self.api_method.replace('_', " ").replace('-', " ")
            ),
            (false, true) => self.provider.clone(),
            (true, false) => self.api_method.clone(),
            _ => String::new(),
        };
        let usage = usage_label(self.usage.as_ref(), now);
        if route.is_empty() {
            usage
        } else {
            format!("{usage} · {route}")
        }
    }
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
        let details = from_routes(&[
            route("a-unknown", "openai-oauth", 0),
            route("gpt-5.5", "openai-oauth", 0),
            unavailable,
        ]);
        let mut models: Vec<_> = details.keys().cloned().collect();
        rank(&mut models, &details);
        assert_eq!(models, ["openai-oauth:gpt-5.5", "openai-oauth:a-unknown"]);
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
    let Some(detail) = details.get(model) else {
        return "Other models".into();
    };
    let method =
        jcode_provider_core::ModelRouteApiMethod::parse(&detail.api_method).display_label();
    let method = match method.as_str() {
        "oauth" => "OAuth".to_string(),
        "api key" => "API key".to_string(),
        _ => method.replace(['_', '-'], " "),
    };
    if detail.provider.is_empty() {
        method
    } else {
        format!("{} · {method}", detail.provider)
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
                    || details
                        .get(model)
                        .is_some_and(|detail| detail.model.to_ascii_lowercase().contains(&query))
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
