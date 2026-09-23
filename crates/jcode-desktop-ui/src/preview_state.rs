//! Stable, named fixtures for isolated self-development previews.
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreviewState {
    Empty,
    Streaming,
    Interrupted,
    Crashed,
    VoiceConnecting,
    VoiceListening,
    VoiceRouting,
    VoiceCodingAgent,
    VoiceQuickAction,
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
        Self::Interrupted,
        Self::Crashed,
        Self::VoiceConnecting,
        Self::VoiceListening,
        Self::VoiceRouting,
        Self::VoiceCodingAgent,
        Self::VoiceQuickAction,
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
            Self::Interrupted => "interrupted",
            Self::Crashed => "crashed",
            Self::VoiceConnecting => "voice-connecting",
            Self::VoiceListening => "voice-listening",
            Self::VoiceRouting => "voice-routing",
            Self::VoiceCodingAgent => "voice-coding-agent",
            Self::VoiceQuickAction => "voice-quick-action",
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
            Self::Interrupted => "Interrupted response",
            Self::Crashed => "Session crashed",
            Self::VoiceConnecting => "Connecting microphone",
            Self::VoiceListening => "Live voice transcription",
            Self::VoiceRouting => "Jev choosing a voice route",
            Self::VoiceCodingAgent => "Jev chose coding agent",
            Self::VoiceQuickAction => "Jev chose quick action",
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
