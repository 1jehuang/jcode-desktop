//! Model picker metadata comes from the daemon, including shared TUI preferences.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use jcode_sdk::{ModelRouteInfo, ModelUsage, compare_model_usage};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelDetails {
    pub provider: String,
    pub api_method: String,
    pub usage: Option<ModelUsage>,
}

pub(super) fn from_routes(routes: &[ModelRouteInfo]) -> HashMap<String, ModelDetails> {
    let mut details = HashMap::<String, ModelDetails>::new();
    for route in routes.iter().filter(|route| route.available) {
        let candidate = ModelDetails {
            provider: route.provider.clone(),
            api_method: route.api_method.clone(),
            usage: route.usage.clone(),
        };
        match details.get(&route.model) {
            Some(existing)
                if !compare_model_usage(candidate.usage.as_ref(), existing.usage.as_ref())
                    .is_lt() => {}
            _ => {
                details.insert(route.model.clone(), candidate);
            }
        }
    }
    details
}

pub(super) fn rank(models: &mut [String], details: &HashMap<String, ModelDetails>) {
    models.sort_by(|a, b| {
        compare_model_usage(
            details.get(a).and_then(|detail| detail.usage.as_ref()),
            details.get(b).and_then(|detail| detail.usage.as_ref()),
        )
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
