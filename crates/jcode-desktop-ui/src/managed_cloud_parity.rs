//! Make a Jcode Cloud machine use the same models and logins as this computer.
//!
//! A fresh cloud machine only knows the account's Jcode subscription, so every
//! session there used to start on the subscription's default model no matter
//! what the user picked locally. Before each connection this module reads the
//! local defaults (`[provider] default_model` and friends) plus the logins
//! those defaults need, and applies them on the machine over the same pinned,
//! single-use SSH key the API connection uses. Secrets travel on SSH stdin,
//! never in argv, logs, or error text.
//!
//! OAuth logins go through `jcode auth import`, which refuses to overwrite an
//! existing store. A cloud login that already exists (for example one the user
//! created there, or a refreshed copy from an earlier sync) is kept.
use base64::Engine as _;
use jcode_base::auth::transfer::{self, TransferProvider};
use std::time::Duration;

const SYNC_TIMEOUT: Duration = Duration::from_secs(60);

/// Managed hosts were bootstrapped with `jcode --provider jcode serve`, which
/// can only reach the Jcode subscription, so a synced Anthropic or OpenAI
/// login was unusable. Point the service at a launcher that serves every
/// configured provider once the machine has its own model login, and keeps
/// the subscription-only daemon otherwise. The daemon restarts only when that
/// choice changes, so reconnecting does not interrupt running sessions.
const SERVE_MODE_SCRIPT: &str = r#"has_login() {
  [ -s "$d/auth.json" ] || [ -s "$d/openai-auth.json" ] ||
    grep -qs '_API_KEY=.' "$c/anthropic.env" "$c/openai.env" "$c/openrouter.env" "$c/gemini.env"
}
unit="$HOME/.config/systemd/user/jcode-cloud.service"
if [ -f "$unit" ] && [ -x /opt/jcode-cloud/jcode ]; then
  launcher="$HOME/.local/bin/jcode-cloud-serve"
  mkdir -p "$HOME/.local/bin"
  cat > "$launcher.new" <<'EOF'
#!/bin/sh
# Serve every configured model provider once this machine has its own model
# login. Otherwise route through the account's Jcode subscription.
d="${JCODE_HOME:-$HOME/.jcode}"
c="${XDG_CONFIG_HOME:-$HOME/.config}/jcode"
if [ -s "$d/auth.json" ] || [ -s "$d/openai-auth.json" ] ||
  grep -qs '_API_KEY=.' "$c/anthropic.env" "$c/openai.env" "$c/openrouter.env" "$c/gemini.env"; then
  exec /opt/jcode-cloud/jcode serve
fi
exec /opt/jcode-cloud/jcode --provider jcode serve
EOF
  chmod 755 "$launcher.new" && mv "$launcher.new" "$launcher"
  mode=jcode
  if has_login; then mode=auto; fi
  restart=0
  if ! grep -qx "ExecStart=$launcher" "$unit"; then
    sed "s|^ExecStart=.*|ExecStart=$launcher|" "$unit" > "$unit.new" && mv "$unit.new" "$unit"
    restart=1
  fi
  [ "$(cat "$d/cloud-serve-mode" 2>/dev/null)" = "$mode" ] || restart=1
  if [ "$restart" = 1 ]; then
    XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"; export XDG_RUNTIME_DIR
    if systemctl --user daemon-reload && systemctl --user restart jcode-cloud.service; then
      printf '%s\n' "$mode" > "$d/cloud-serve-mode"
      i=0; while [ "$i" -lt 30 ] && ! [ -S "$XDG_RUNTIME_DIR/jcode.sock" ]; do sleep 1; i=$((i+1)); done
    fi
  fi
fi
"#;

/// API-key providers whose keys are copied when they are configured locally.
/// Only model credentials: no GitHub, AWS, Google, or integration secrets.
const API_KEYS: &[(&str, &str)] = &[
    ("ANTHROPIC_API_KEY", "anthropic.env"),
    ("OPENAI_API_KEY", "openai.env"),
    ("OPENROUTER_API_KEY", "openrouter.env"),
    ("GEMINI_API_KEY", "gemini.env"),
];

/// `[provider]` settings that decide which model and route a session uses.
const PROVIDER_FIELDS: &[&str] = &[
    "default_model",
    "default_provider",
    "anthropic_reasoning_effort",
    "openai_reasoning_effort",
    "anthropic_cache_ttl_1h",
    "openai_service_tier",
    "openai_transport",
    "preserve_reasoning_context",
];

/// Everything the cloud machine needs to mirror local model selection.
#[derive(Default)]
pub struct Snapshot {
    /// `[provider]` table entries, already rendered as TOML values.
    provider: Vec<(String, String)>,
    oauth: Vec<(TransferProvider, Vec<u8>)>,
    api_keys: Vec<(&'static str, &'static str, String)>,
    /// Daemon provider ids to notify after credentials change.
    notify: Vec<&'static str>,
    pub default_model: Option<String>,
    pub reasoning_effort: Option<String>,
}

impl Snapshot {
    /// Read local settings and credentials. Never refreshes or migrates them.
    pub fn collect() -> Self {
        let mut snapshot = Self::from_config_toml(&read_local_config());
        for provider in [TransferProvider::Claude, TransferProvider::OpenAi] {
            if matches!(transfer::available_local(provider), Ok(true))
                && let Ok(payload) = transfer::export_local(provider)
            {
                snapshot.oauth.push((provider, payload.as_bytes().to_vec()));
                snapshot.notify.push(match provider {
                    TransferProvider::Claude => "claude",
                    TransferProvider::OpenAi => "openai",
                });
            }
        }
        for &(key, file) in API_KEYS {
            if let Some(value) =
                jcode_base::provider_catalog::load_api_key_from_env_or_config(key, file)
                && valid_secret(&value)
            {
                snapshot.api_keys.push((key, file, value));
                snapshot.notify.push(match key {
                    "ANTHROPIC_API_KEY" => "claude-api",
                    "OPENAI_API_KEY" => "openai-api",
                    "OPENROUTER_API_KEY" => "openrouter",
                    _ => "gemini",
                });
            }
        }
        snapshot
    }

    /// Local model defaults and which logins exist, without reading any
    /// secret values. Used to steer each new cloud session.
    pub fn session_defaults() -> Self {
        let mut snapshot = Self::from_config_toml(&read_local_config());
        for provider in [TransferProvider::Claude, TransferProvider::OpenAi] {
            if matches!(transfer::available_local(provider), Ok(true)) {
                snapshot.notify.push(match provider {
                    TransferProvider::Claude => "claude",
                    TransferProvider::OpenAi => "openai",
                });
            }
        }
        snapshot
    }

    fn from_config_toml(raw: &str) -> Self {
        let mut snapshot = Self::default();
        let Ok(config) = raw.parse::<toml::Table>() else {
            return snapshot;
        };
        let Some(provider) = config.get("provider").and_then(|v| v.as_table()) else {
            return snapshot;
        };
        for &field in PROVIDER_FIELDS {
            let Some(value) = provider.get(field) else {
                continue;
            };
            let rendered = match value {
                toml::Value::String(text) if safe_setting(text) => {
                    toml::Value::String(text.clone()).to_string()
                }
                toml::Value::Boolean(flag) => flag.to_string(),
                _ => continue,
            };
            snapshot.provider.push((field.to_owned(), rendered));
        }
        let text = |field: &str| {
            provider
                .get(field)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| safe_setting(v))
                .map(str::to_owned)
        };
        snapshot.default_model = text("default_model");
        let model = snapshot.default_model.as_deref().unwrap_or_default();
        snapshot.reasoning_effort = if is_openai_model(model) {
            text("openai_reasoning_effort")
        } else {
            text("anthropic_reasoning_effort")
        };
        snapshot
    }

    /// Whether there is anything local worth applying.
    pub fn is_empty(&self) -> bool {
        self.provider.is_empty() && self.oauth.is_empty() && self.api_keys.is_empty()
    }

    /// POSIX shell program applied on the cloud machine. Secrets are embedded
    /// as base64 in this stdin-only script and never appear in argv.
    pub fn script(&self) -> String {
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let mut script = String::from(
            "set -u\numask 077\nd=\"${JCODE_HOME:-$HOME/.jcode}\"\nc=\"${XDG_CONFIG_HOME:-$HOME/.config}/jcode\"\nmkdir -p \"$d\" \"$c\"\n",
        );
        for (provider, payload) in &self.oauth {
            // Import refuses an existing store, so a cloud login is never replaced.
            script.push_str(&format!(
                "printf '%s' '{}' | base64 -d | \"$JCODE_BIN\" --no-update auth import --provider {} --stdin --json >/dev/null 2>&1 || true\n",
                b64(payload),
                provider.as_str(),
            ));
        }
        for (key, file, value) in &self.api_keys {
            script.push_str(&format!(
                "f=\"$c/{file}\"; touch \"$f\"; grep -v '^{key}=' \"$f\" > \"$f.tmp\" || true; printf '%s=%s\\n' '{key}' \"$(printf '%s' '{}' | base64 -d)\" >> \"$f.tmp\"; mv \"$f.tmp\" \"$f\"\n",
                b64(value.as_bytes()),
            ));
        }
        if !self.provider.is_empty() {
            let table = self
                .provider
                .iter()
                .map(|(key, value)| format!("{key} = {value}\n"))
                .collect::<String>();
            // Replace only the [provider] table, preserving every other section.
            script.push_str(&format!(
                "cfg=\"$d/config.toml\"; touch \"$cfg\"; printf '%s' '{}' | base64 -d > \"$cfg.provider\"\n\
                 awk 'BEGIN{{skip=0}} /^\\[/{{skip=($0==\"[provider]\")}} !skip' \"$cfg\" > \"$cfg.tmp\"\n\
                 {{ printf '[provider]\\n'; cat \"$cfg.provider\"; printf '\\n'; cat \"$cfg.tmp\"; }} > \"$cfg.new\" && mv \"$cfg.new\" \"$cfg\"\n\
                 rm -f \"$cfg.tmp\" \"$cfg.provider\"\n",
                b64(table.as_bytes()),
            ));
        }
        script.push_str(SERVE_MODE_SCRIPT);
        script.push_str("echo jcode-cloud-parity-ok\n");
        script
    }

    /// Daemon providers whose credentials changed, for `notify_auth_changed`.
    pub fn changed_providers(&self) -> &[&'static str] {
        &self.notify
    }
}

fn read_local_config() -> String {
    jcode_base::storage::jcode_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join("config.toml")).ok())
        .unwrap_or_default()
}

fn valid_secret(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}

fn safe_setting(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn is_openai_model(model: &str) -> bool {
    let bare = model.rsplit(':').next().unwrap_or(model);
    model.starts_with("openai") || bare.starts_with("gpt") || bare.starts_with('o')
}

/// Apply the snapshot using the connection's own SSH options. Failure is
/// reported but does not block the session: the user can still work on the
/// cloud machine's existing models.
pub fn apply(options: &jcode_sdk::SshConnectOptions, snapshot: &Snapshot) -> Result<(), String> {
    if snapshot.is_empty() {
        return Ok(());
    }
    let output = options
        .run_script(snapshot.script().as_bytes(), SYNC_TIMEOUT)
        .map_err(|_| "Could not copy your model settings to Jcode Cloud.".to_string())?;
    if String::from_utf8_lossy(&output.stdout).contains("jcode-cloud-parity-ok") {
        Ok(())
    } else {
        Err("Jcode Cloud did not accept your model settings.".into())
    }
}

/// After the API connection is up: reload changed credentials in the shared
/// daemon, then put the new session on the local default model and effort.
pub fn apply_to_session(
    client: &jcode_sdk::JcodeClient,
    session_id: &str,
    snapshot: &Snapshot,
) -> Result<(), String> {
    for provider in snapshot.changed_providers() {
        let _ = client.notify_auth_changed(provider);
    }
    if let Some(model) = &snapshot.default_model {
        client
            .set_model(session_id, model)
            .map_err(|error| format!("Could not select {model} on Jcode Cloud: {error}"))?;
    }
    if let Some(effort) = &snapshot.reasoning_effort {
        let _ = client.set_reasoning_effort(session_id, effort);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_defaults_are_read_from_local_config() {
        let snapshot = Snapshot::from_config_toml(
            r#"
[provider]
default_model = "claude-oauth:claude-opus-5-5"
default_provider = "claude"
anthropic_reasoning_effort = "medium"
openai_reasoning_effort = "high"
anthropic_cache_ttl_1h = true
unrelated = "ignored"

[agents]
swarm_model = "x"
"#,
        );
        assert_eq!(
            snapshot.default_model.as_deref(),
            Some("claude-oauth:claude-opus-5-5")
        );
        assert_eq!(snapshot.reasoning_effort.as_deref(), Some("medium"));
        let keys: Vec<_> = snapshot.provider.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "default_model",
                "default_provider",
                "anthropic_reasoning_effort",
                "openai_reasoning_effort",
                "anthropic_cache_ttl_1h"
            ]
        );
    }

    #[test]
    fn openai_default_uses_openai_effort() {
        let snapshot = Snapshot::from_config_toml(
            "[provider]\ndefault_model = \"gpt-5.5\"\nopenai_reasoning_effort = \"high\"\nanthropic_reasoning_effort = \"low\"\n",
        );
        assert_eq!(snapshot.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn missing_or_malformed_config_yields_no_settings() {
        assert!(Snapshot::from_config_toml("").is_empty());
        assert!(Snapshot::from_config_toml("not = [toml").is_empty());
        assert!(
            Snapshot::from_config_toml("[provider]\ndefault_model = \"a\\nb\"\n")
                .default_model
                .is_none()
        );
    }

    #[test]
    fn secrets_never_appear_in_plain_text_in_the_script() {
        let mut snapshot = Snapshot::from_config_toml(
            "[provider]\ndefault_model = \"anthropic-api:claude-opus-5-5\"\n",
        );
        snapshot.api_keys.push((
            "ANTHROPIC_API_KEY",
            "anthropic.env",
            "sk-ant-secret'value".into(),
        ));
        snapshot.oauth.push((
            TransferProvider::Claude,
            b"{\"refresh\":\"rt-secret\"}".to_vec(),
        ));
        let script = snapshot.script();
        assert!(!script.contains("sk-ant-secret"));
        assert!(!script.contains("rt-secret"));
        assert!(script.contains("auth import --provider claude --stdin"));
        assert!(script.contains("anthropic.env"));
        assert!(script.ends_with("echo jcode-cloud-parity-ok\n"));
    }

    /// Run the generated script against a scratch home to prove it rewrites
    /// only `[provider]` and keeps everything else in config.toml.
    #[cfg(unix)]
    #[test]
    fn script_replaces_only_the_provider_table() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".jcode");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            data.join("config.toml"),
            "[provider]\ndefault_provider = \"jcode\"\n\n[display]\ntheme = \"dark\"\n",
        )
        .unwrap();
        let mut snapshot = Snapshot::from_config_toml(
            "[provider]\ndefault_model = \"claude-oauth:claude-opus-5-5\"\ndefault_provider = \"claude\"\n",
        );
        snapshot
            .api_keys
            .push(("OPENAI_API_KEY", "openai.env", "sk-test'quote".into()));
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(snapshot.script())
            .env("HOME", home.path())
            .env_remove("JCODE_HOME")
            .env_remove("XDG_CONFIG_HOME")
            .env("JCODE_BIN", "/bin/false")
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("jcode-cloud-parity-ok"));
        let config = std::fs::read_to_string(data.join("config.toml")).unwrap();
        let parsed: toml::Table = config.parse().unwrap();
        assert_eq!(
            parsed["provider"]["default_model"].as_str(),
            Some("claude-oauth:claude-opus-5-5")
        );
        assert_eq!(
            parsed["provider"]["default_provider"].as_str(),
            Some("claude")
        );
        assert_eq!(parsed["display"]["theme"].as_str(), Some("dark"));
        let env = std::fs::read_to_string(home.path().join(".config/jcode/openai.env")).unwrap();
        assert_eq!(env, "OPENAI_API_KEY=sk-test'quote\n");
    }
}
