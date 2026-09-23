//! Pure account discovery policy. Counts are recorded runtime turns, not logins.
use super::connection::{ConnectionStatus, ConnectionStatuses, status_for};
use jcode_sdk::{LoginMethod, LoginProvider};
use std::collections::HashMap;

pub(super) type MethodUsage = HashMap<String, u64>;

fn usage_key(provider: &LoginProvider) -> &str {
    match provider.id {
        "claude" => "claude-oauth",
        "anthropic-api" => "anthropic-api-key",
        "openai" => "openai-oauth",
        "openai-api" => "openai-api-key",
        "jcode" => "jcode-subscription",
        "gemini" => "code-assist-oauth",
        "gemini-api" => "openai-compatible:gemini-api",
        other => other,
    }
}

fn health_group(status: ConnectionStatus) -> u8 {
    match status {
        ConnectionStatus::Connected => 0,
        ConnectionStatus::Expired | ConnectionStatus::Failed => 1,
        // Saved but unverified credentials must not masquerade as working.
        ConnectionStatus::Unverified => 2,
        ConnectionStatus::Checking | ConnectionStatus::Unknown => 3,
        ConnectionStatus::NotConnected => 4,
    }
}

pub(super) fn method_label(method: LoginMethod) -> &'static str {
    match method {
        LoginMethod::ApiKey => "API key",
        LoginMethod::OAuth => "OAuth",
        LoginMethod::DeviceCode => "Device sign-in",
    }
}

pub(super) fn filtered_providers(
    providers: &[LoginProvider],
    query: &str,
    statuses: Option<&ConnectionStatuses>,
    loading: bool,
    usage: Option<&MethodUsage>,
) -> Vec<LoginProvider> {
    let query = query.trim().to_lowercase();
    let mut result: Vec<_> = providers
        .iter()
        .copied()
        .filter(|provider| {
            [
                provider.display_name,
                provider.id,
                provider.detail,
                method_label(provider.method),
                status_for(statuses, provider.id, loading).label(),
            ]
            .iter()
            .any(|field| field.to_lowercase().contains(&query))
        })
        .collect();
    result.sort_by_cached_key(|provider| {
        let count = usage
            .and_then(|counts| counts.get(usage_key(provider)))
            .copied()
            .unwrap_or(0);
        (
            health_group(status_for(statuses, provider.id, loading)),
            std::cmp::Reverse(count),
            provider.display_name.to_lowercase(),
            provider.id,
        )
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn provider(id: &'static str, name: &'static str) -> LoginProvider {
        LoginProvider {
            id,
            display_name: name,
            detail: "Subscription connection",
            method: LoginMethod::OAuth,
        }
    }
    #[test]
    fn health_precedes_usage_and_usage_descends_with_stable_ties() {
        let providers = vec![
            provider("absent", "Absent"),
            provider("expired", "Expired"),
            provider("openai", "OpenAI"),
            provider("claude", "Claude"),
            provider("failed", "Failed"),
            provider("saved", "Saved"),
        ];
        let statuses = ConnectionStatuses::from([
            ("openai".into(), ConnectionStatus::Connected),
            ("claude".into(), ConnectionStatus::Connected),
            ("expired".into(), ConnectionStatus::Expired),
            ("failed".into(), ConnectionStatus::Failed),
            ("saved".into(), ConnectionStatus::Unverified),
        ]);
        let usage = MethodUsage::from([
            ("openai-oauth".into(), 40),
            ("claude-oauth".into(), 8),
            ("expired".into(), 100),
            ("absent".into(), 1000),
        ]);
        let ids: Vec<_> = filtered_providers(&providers, "", Some(&statuses), false, Some(&usage))
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(
            ids,
            ["openai", "claude", "expired", "failed", "saved", "absent"]
        );
        let unknown = filtered_providers(&providers, "", None, false, None);
        assert_eq!(unknown[0].id, "absent");
    }
    #[test]
    fn usage_keeps_oauth_and_api_key_accounts_distinct() {
        for (id, expected) in [
            ("claude", "claude-oauth"),
            ("anthropic-api", "anthropic-api-key"),
            ("openai", "openai-oauth"),
            ("openai-api", "openai-api-key"),
            ("gemini", "code-assist-oauth"),
            ("google", "google"),
            ("gemini-api", "openai-compatible:gemini-api"),
            ("jcode", "jcode-subscription"),
            ("copilot", "copilot"),
        ] {
            assert_eq!(usage_key(&provider(id, id)), expected);
        }
    }

    #[test]
    fn search_matches_name_id_detail_method_and_status_case_insensitively() {
        let providers = [
            provider("openai", "ChatGPT"),
            LoginProvider {
                id: "openai-api",
                display_name: "OpenAI API key",
                detail: "Metered developer access",
                method: LoginMethod::ApiKey,
            },
        ];
        for query in ["  CHATGPT  ", "subscription", "oauth"] {
            assert_eq!(
                filtered_providers(&providers, query, None, false, None),
                vec![providers[0]]
            );
        }
        for query in ["openai-api", "METERED", "API KEY"] {
            assert_eq!(
                filtered_providers(&providers, query, None, false, None),
                vec![providers[1]]
            );
        }
        assert_eq!(
            filtered_providers(&providers, "openai", None, false, None).len(),
            2
        );
        assert!(filtered_providers(&providers, "no-such-provider", None, false, None).is_empty());
        assert_eq!(
            filtered_providers(&providers, "", None, false, None).len(),
            2
        );
    }
}
