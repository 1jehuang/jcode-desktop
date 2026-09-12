//! Stable, named fixtures for isolated self-development previews.
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreviewState {
    Empty,
    Streaming,
    LoginError,
    ModelAccessError,
    RateLimit,
    Disconnected,
    LoginDialogError,
}

impl PreviewState {
    pub const ALL: &'static [Self] = &[
        Self::Empty,
        Self::Streaming,
        Self::LoginError,
        Self::ModelAccessError,
        Self::RateLimit,
        Self::Disconnected,
        Self::LoginDialogError,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Streaming => "streaming",
            Self::LoginError => "login-error",
            Self::ModelAccessError => "model-access-error",
            Self::RateLimit => "rate-limit",
            Self::Disconnected => "disconnected",
            Self::LoginDialogError => "login-dialog-error",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Empty => "Empty session",
            Self::Streaming => "Streaming response",
            Self::LoginError => "Authentication error",
            Self::ModelAccessError => "Model access error",
            Self::RateLimit => "Rate limit",
            Self::Disconnected => "Disconnected",
            Self::LoginDialogError => "Login dialog error",
        }
    }
}

impl FromStr for PreviewState {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.id() == value)
            .ok_or_else(|| format!("Unknown preview state: {value}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_preview_names_round_trip() {
        for &state in PreviewState::ALL {
            assert_eq!(state.id().parse::<PreviewState>().unwrap(), state);
            assert_eq!(
                serde_json::to_string(&state).unwrap(),
                format!("\"{}\"", state.id())
            );
            assert_eq!(
                serde_json::from_str::<PreviewState>(&format!("\"{}\"", state.id())).unwrap(),
                state
            );
        }
        assert!("unknown".parse::<PreviewState>().is_err());
    }
}
