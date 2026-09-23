//! Persisted user configuration for Jcode Desktop.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::OnceLock, time::Duration};

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    pub appearance: AppearanceConfig,
    pub workspace: WorkspaceConfig,
    pub terminal: TerminalConfig,
    pub sounds: SoundsConfig,
    pub voice: VoiceConfig,
}

/// Background microphone access and physical input devices require explicit opt-in.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub global_hold: bool,
    /// Explicit keyboard paths only. An empty list must never discover all devices.
    pub global_devices: Vec<PathBuf>,
}

/// Desktop feedback is opt-in and independent of the terminal client's bell.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SoundsConfig {
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    /// Overall workspace presentation.
    pub layout_mode: LayoutMode,
    /// Built-in color theme. Missing or unknown names fall back to Parchment.
    pub theme: String,
    /// UI font family. The platform-specific built-in remains the default.
    pub ui_font: Option<String>,
    /// Assistant prose font. Defaults to the UI font, never changes code or input.
    pub ai_font: Option<String>,
    /// Monospace font used by code and terminal panels.
    pub mono_font: Option<String>,
    /// Global text scale. Values outside 0.75..=2.0 fall back to 1.0.
    pub text_scale: f32,
    /// Disable non-essential workspace motion.
    pub reduce_motion: bool,
    /// Semantic color overrides. Keys are documented in README.md.
    pub colors: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    Normal,
    #[default]
    FolderTabs,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    /// The optional Jcode account welcome screen was completed or skipped.
    pub account_sign_in_handled: bool,
    /// Selected once from session history, then fixed for Super+Enter / Super+;.
    pub pinned_working_dir: Option<String>,
    /// Default SSH target. Missing or empty means local sessions.
    pub default_remote_host: Option<String>,
    /// Previously used SSH targets, in most-recent-first order.
    pub remote_hosts: Vec<String>,
    pub sidebar: bool,
    pub showcase_keys: bool,
    pub coaching_hints: bool,
    pub session_refresh_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub scrollback_lines: usize,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            appearance: AppearanceConfig::default(),
            workspace: WorkspaceConfig::default(),
            terminal: TerminalConfig::default(),
            sounds: SoundsConfig::default(),
            voice: VoiceConfig::default(),
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            layout_mode: LayoutMode::default(),
            theme: crate::theme::ThemePreset::default().id().into(),
            ui_font: None,
            ai_font: None,
            mono_font: None,
            text_scale: 1.0,
            reduce_motion: false,
            colors: BTreeMap::new(),
        }
    }
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            account_sign_in_handled: false,
            pinned_working_dir: None,
            default_remote_host: None,
            remote_hosts: Vec::new(),
            sidebar: true,
            showcase_keys: true,
            coaching_hints: true,
            // Listing sessions can touch a very large on-disk index. Live
            // events already update the current workspace immediately, so a
            // slower reconciliation cadence keeps external sessions fresh
            // without continuously competing with UI work.
            session_refresh_seconds: 60,
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
        }
    }
}

impl DesktopConfig {
    fn normalize(mut self) -> Self {
        if !self.appearance.text_scale.is_finite()
            || !(0.75..=2.0).contains(&self.appearance.text_scale)
        {
            self.appearance.text_scale = 1.0;
        }
        self.workspace.session_refresh_seconds =
            self.workspace.session_refresh_seconds.clamp(5, 300);
        self.workspace.default_remote_host = self
            .workspace
            .default_remote_host
            .as_deref()
            .and_then(|host| crate::remote_targets::validate_host(host).ok());
        self.workspace.remote_hosts = normalized_remote_hosts(&self.workspace.remote_hosts);
        self.terminal.scrollback_lines = self.terminal.scrollback_lines.clamp(100, 1_000_000);
        self
    }

    pub fn session_refresh_interval(&self) -> Duration {
        Duration::from_secs(self.workspace.session_refresh_seconds)
    }
}

static CONFIG: OnceLock<DesktopConfig> = OnceLock::new();

pub fn get() -> &'static DesktopConfig {
    CONFIG.get_or_init(load)
}

/// Read the latest choice, including changes made by another window.
pub fn account_sign_in_handled() -> bool {
    load().workspace.account_sign_in_handled
}

#[cfg(not(test))]
pub fn persist_account_sign_in_handled() -> std::io::Result<()> {
    persist_value_at(
        &path(),
        std::env::var_os("JCODE_DESKTOP_CONFIG").is_some(),
        "workspace",
        "account_sign_in_handled",
        "true",
    )
}

#[cfg(test)]
pub fn persist_account_sign_in_handled() -> std::io::Result<()> {
    Ok(())
}

pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("JCODE_DESKTOP_CONFIG") {
        return PathBuf::from(path);
    }
    if let Some(home) = std::env::var_os("JCODE_HOME") {
        return PathBuf::from(home).join("config.toml");
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".jcode/config.toml")
}

/// Persist one appearance value without reserializing the rest of the shared
/// Jcode config (and thereby losing its comments or settings unknown to us).
#[cfg(not(test))]
pub fn persist_theme(theme: &str) -> std::io::Result<()> {
    let path = path();
    let standalone = std::env::var_os("JCODE_DESKTOP_CONFIG").is_some();
    persist_theme_at(&path, standalone, theme)
}

#[cfg(test)]
pub fn persist_theme(_theme: &str) -> std::io::Result<()> {
    // UI tests exercise the production selection path without ever touching
    // the developer's real ~/.jcode/config.toml. Disk behavior is covered by
    // persist_theme_at against an isolated temporary directory below.
    Ok(())
}

#[cfg(not(test))]
pub fn persist_layout_mode(layout_mode: LayoutMode) -> std::io::Result<()> {
    let path = path();
    let standalone = std::env::var_os("JCODE_DESKTOP_CONFIG").is_some();
    persist_layout_mode_at(&path, standalone, layout_mode)
}

#[cfg(test)]
pub fn persist_layout_mode(_layout_mode: LayoutMode) -> std::io::Result<()> {
    Ok(())
}

fn persist_theme_at(path: &std::path::Path, standalone: bool, theme: &str) -> std::io::Result<()> {
    persist_appearance_value_at(path, standalone, "theme", &format!("{theme:?}"))
}

fn persist_layout_mode_at(
    path: &std::path::Path,
    standalone: bool,
    layout_mode: LayoutMode,
) -> std::io::Result<()> {
    let value = toml::Value::try_from(layout_mode)
        .expect("layout mode serializes")
        .to_string();
    persist_appearance_value_at(path, standalone, "layout_mode", &value)
}

fn persist_appearance_value_at(
    path: &std::path::Path,
    standalone: bool,
    key: &str,
    value: &str,
) -> std::io::Result<()> {
    persist_value_at(path, standalone, "appearance", key, value)
}

#[cfg(not(test))]
pub fn persist_pinned_working_dir(directory: &str) -> std::io::Result<()> {
    persist_pinned_working_dir_at(
        &path(),
        std::env::var_os("JCODE_DESKTOP_CONFIG").is_some(),
        directory,
    )
}

#[cfg(test)]
pub fn persist_pinned_working_dir(_directory: &str) -> std::io::Result<()> {
    Ok(())
}

fn persist_pinned_working_dir_at(
    path: &std::path::Path,
    standalone: bool,
    directory: &str,
) -> std::io::Result<()> {
    persist_value_at(
        path,
        standalone,
        "workspace",
        "pinned_working_dir",
        &toml::Value::String(directory.into()).to_string(),
    )
}

/// Save the default SSH target, or an empty string to explicitly select local.
#[cfg(not(test))]
pub fn persist_default_remote_host(host: Option<&str>) -> std::io::Result<()> {
    persist_default_remote_host_at(
        &path(),
        std::env::var_os("JCODE_DESKTOP_CONFIG").is_some(),
        host,
    )
}

#[cfg(test)]
pub fn persist_default_remote_host(host: Option<&str>) -> std::io::Result<()> {
    // Selection tests must never write the user's real config.
    remote_host_value(host).map(|_| ())
}

/// Save normalized, deduplicated recent targets without rewriting shared settings.
#[cfg(not(test))]
pub fn persist_remote_hosts(hosts: &[String]) -> std::io::Result<()> {
    persist_remote_hosts_at(
        &path(),
        std::env::var_os("JCODE_DESKTOP_CONFIG").is_some(),
        hosts,
    )
}

#[cfg(test)]
pub fn persist_remote_hosts(_hosts: &[String]) -> std::io::Result<()> {
    Ok(())
}

fn remote_host_value(host: Option<&str>) -> std::io::Result<String> {
    match host.filter(|host| !host.trim().is_empty()) {
        Some(host) => crate::remote_targets::validate_host(host)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error)),
        None => Ok(String::new()),
    }
}

fn normalized_remote_hosts(hosts: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for host in hosts {
        if let Ok(host) = crate::remote_targets::validate_host(host) {
            if !result.contains(&host) {
                result.push(host);
            }
        }
    }
    result
}

fn persist_default_remote_host_at(
    path: &std::path::Path,
    standalone: bool,
    host: Option<&str>,
) -> std::io::Result<()> {
    persist_value_at(
        path,
        standalone,
        "workspace",
        "default_remote_host",
        &toml::Value::String(remote_host_value(host)?).to_string(),
    )
}

fn persist_remote_hosts_at(
    path: &std::path::Path,
    standalone: bool,
    hosts: &[String],
) -> std::io::Result<()> {
    let value = toml::Value::Array(
        normalized_remote_hosts(hosts)
            .into_iter()
            .map(toml::Value::String)
            .collect(),
    );
    persist_value_at(
        path,
        standalone,
        "workspace",
        "remote_hosts",
        &value.to_string(),
    )
}

fn persist_value_at(
    path: &std::path::Path,
    standalone: bool,
    section_name: &str,
    key: &str,
    value: &str,
) -> std::io::Result<()> {
    let section = if standalone {
        format!("[{section_name}]")
    } else {
        format!("[desktop.{section_name}]")
    };
    let mut text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let section_index = lines.iter().position(|line| {
        line.split('#')
            .next()
            .is_some_and(|value| value.trim() == section)
    });
    if let Some(start) = section_index {
        let end = lines[start + 1..]
            .iter()
            .position(|line| line.trim_start().starts_with('['))
            .map_or(lines.len(), |offset| start + 1 + offset);
        if let Some(index) = (start + 1..end).find(|&index| {
            lines[index]
                .split_once('=')
                .is_some_and(|(candidate, _)| candidate.trim() == key)
        }) {
            // Replace only the parsed value span. This also handles hand-edited
            // multiline arrays without losing the key's trailing comment or
            // rewriting neighboring settings.
            let mut tail = lines[index..end].join("\n");
            let values: BTreeMap<String, toml::Spanned<toml::Value>> = toml::from_str(&tail)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let span = values[key].span();
            tail.replace_range(span, value);
            lines.splice(index..end, tail.lines().map(str::to_owned));
        } else {
            lines.insert(start + 1, format!("{key} = {value}"));
        }
    } else {
        if !text.is_empty() && !text.ends_with('\n') {
            lines.push(String::new());
        }
        lines.push(section.into());
        lines.push(format!("{key} = {value}"));
    }
    text = lines.join("\n");
    text.push('\n');
    parse(&text, standalone)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, text)?;
    fs::rename(temporary, path)
}

#[cfg(not(test))]
pub fn persist_sounds_enabled(enabled: bool) -> std::io::Result<()> {
    persist_sounds_enabled_at(
        &path(),
        std::env::var_os("JCODE_DESKTOP_CONFIG").is_some(),
        enabled,
    )
}

#[cfg(test)]
pub fn persist_sounds_enabled(_enabled: bool) -> std::io::Result<()> {
    Ok(())
}

fn persist_sounds_enabled_at(
    path: &std::path::Path,
    standalone: bool,
    enabled: bool,
) -> std::io::Result<()> {
    persist_value_at(
        path,
        standalone,
        "sounds",
        "enabled",
        if enabled { "true" } else { "false" },
    )
}

fn load() -> DesktopConfig {
    let path = path();
    match fs::read_to_string(&path) {
        Ok(text) => match parse(&text, std::env::var_os("JCODE_DESKTOP_CONFIG").is_some()) {
            Ok(config) => config.normalize(),
            Err(error) => {
                eprintln!(
                    "ignoring invalid desktop config {}: {error}",
                    path.display()
                );
                DesktopConfig::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DesktopConfig::default(),
        Err(error) => {
            eprintln!("failed to read desktop config {}: {error}", path.display());
            DesktopConfig::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct JcodeConfig {
    desktop: DesktopConfig,
}

fn parse(text: &str, standalone: bool) -> Result<DesktopConfig, toml::de::Error> {
    if standalone {
        toml::from_str(text)
    } else {
        toml::from_str::<JcodeConfig>(text).map(|config| config.desktop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_voice_defaults_off_without_implicit_devices() {
        let defaults = DesktopConfig::default();
        assert!(!defaults.voice.global_hold);
        assert!(defaults.voice.global_devices.is_empty());
        for standalone in [false, true] {
            let prefix = if standalone { "voice" } else { "desktop.voice" };
            for text in [String::new(), format!("[{prefix}]\n")] {
                let voice = parse(&text, standalone).unwrap().voice;
                assert!(!voice.global_hold);
                assert!(voice.global_devices.is_empty());
            }
        }
    }

    #[test]
    fn global_voice_parses_explicit_opt_in_and_device_paths() {
        let device = "/dev/input/by-path/platform-i8042-serio-0-event-kbd";
        for standalone in [false, true] {
            let prefix = if standalone { "voice" } else { "desktop.voice" };
            let text = format!("[{prefix}]\nglobal_hold = true\nglobal_devices = ['{device}']\n");
            let voice = parse(&text, standalone).unwrap().voice;
            assert!(voice.global_hold);
            assert_eq!(voice.global_devices, vec![PathBuf::from(device)]);
            let voice = parse(&format!("[{prefix}]\nglobal_hold = true\n"), standalone).unwrap().voice;
            assert!(voice.global_hold);
            assert!(voice.global_devices.is_empty());
            let voice = parse(&format!("[{prefix}]\nglobal_devices = ['{device}']\n"), standalone).unwrap().voice;
            assert!(!voice.global_hold);
            assert_eq!(voice.global_devices, vec![PathBuf::from(device)]);
            for invalid in ["global_hold = 'true'", "global_devices = 'all'", "global_devices = [42]"] {
                assert!(parse(&format!("[{prefix}]\n{invalid}\n"), standalone).is_err());
            }
        }
    }

    #[test]
    fn account_sign_in_choice_survives_restart_without_changing_other_settings() {
        assert!(!DesktopConfig::default().workspace.account_sign_in_handled);
        for standalone in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let original = "# keep shared configuration\nmodel = 'keep-me'\n";
            fs::write(&path, original).unwrap();
            for _ in 0..2 {
                persist_value_at(
                    &path,
                    standalone,
                    "workspace",
                    "account_sign_in_handled",
                    "true",
                )
                .unwrap();
                let text = fs::read_to_string(&path).unwrap();
                assert!(text.starts_with(original));
                assert!(
                    parse(&text, standalone)
                        .unwrap()
                        .workspace
                        .account_sign_in_handled
                );
                assert_eq!(text.matches("account_sign_in_handled =").count(), 1);
            }
        }
    }

    #[test]
    fn sounds_default_off_and_persist_without_rewriting_other_preferences() {
        assert!(!DesktopConfig::default().sounds.enabled);
        for standalone in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let original = "# keep shared configuration\nmodel = 'keep-me'\n";
            fs::write(&path, original).unwrap();
            assert!(!parse(original, standalone).unwrap().sounds.enabled);
            for enabled in [true, false, true] {
                persist_sounds_enabled_at(&path, standalone, enabled).unwrap();
                let text = fs::read_to_string(&path).unwrap();
                assert!(text.starts_with(original));
                assert_eq!(parse(&text, standalone).unwrap().sounds.enabled, enabled);
                assert_eq!(text.matches("enabled =").count(), 1);
            }
        }
    }

    #[test]
    fn remote_preferences_default_local_and_normalize_safe_unique_targets() {
        let defaults = DesktopConfig::default();
        assert!(defaults.workspace.default_remote_host.is_none());
        assert!(defaults.workspace.remote_hosts.is_empty());
        let config = parse(
            "[workspace]\ndefault_remote_host = '  user@desktop  '\nremote_hosts = [' desktop ', 'desktop', '', '-Fbad', 'user@[::1]', 'other']\n",
            true,
        ).unwrap().normalize();
        assert_eq!(
            config.workspace.default_remote_host.as_deref(),
            Some("user@desktop")
        );
        assert_eq!(
            config.workspace.remote_hosts,
            ["desktop", "user@[::1]", "other"]
        );
        for host in ["", "   ", "-oProxyCommand=bad", "a b"] {
            let text = format!("[workspace]\ndefault_remote_host = {host:?}\n");
            assert!(
                parse(&text, true)
                    .unwrap()
                    .normalize()
                    .workspace
                    .default_remote_host
                    .is_none()
            );
        }
    }

    #[test]
    fn remote_preferences_persist_shared_and_standalone_and_switch_back_local() {
        for standalone in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let section = if standalone {
                "workspace"
            } else {
                "desktop.workspace"
            };
            fs::write(&path, format!("# preserve this\nmodel = 'shared-model'\n[{section}] # workspace preferences\nsidebar = false\n# host preference\n")).unwrap();
            persist_default_remote_host_at(&path, standalone, Some(" user@desktop ")).unwrap();
            persist_remote_hosts_at(
                &path,
                standalone,
                &[
                    " desktop ".into(),
                    "desktop".into(),
                    "user@[::1]".into(),
                    "-bad".into(),
                ],
            )
            .unwrap();
            let text = fs::read_to_string(&path).unwrap();
            for preserved in [
                "# preserve this",
                "model = 'shared-model'",
                "# workspace preferences",
                "sidebar = false",
                "# host preference",
            ] {
                assert!(text.contains(preserved));
            }
            let config = parse(&text, standalone).unwrap().normalize();
            assert_eq!(
                config.workspace.default_remote_host.as_deref(),
                Some("user@desktop")
            );
            assert_eq!(config.workspace.remote_hosts, ["desktop", "user@[::1]"]);
            for local in [None, Some(""), Some("   ")] {
                persist_default_remote_host_at(&path, standalone, local).unwrap();
                let text = fs::read_to_string(&path).unwrap();
                assert!(text.contains("default_remote_host = \"\""));
                assert_eq!(text.matches("default_remote_host =").count(), 1);
                let config = parse(&text, standalone).unwrap().normalize();
                assert!(config.workspace.default_remote_host.is_none());
                assert_eq!(config.workspace.remote_hosts, ["desktop", "user@[::1]"]);
            }
            persist_remote_hosts_at(&path, standalone, &[]).unwrap();
            assert!(
                parse(&fs::read_to_string(path).unwrap(), standalone)
                    .unwrap()
                    .workspace
                    .remote_hosts
                    .is_empty()
            );
        }
    }

    #[test]
    fn invalid_remote_preferences_do_not_overwrite_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "# original\nmodel = 'keep'\n";
        fs::write(&path, original).unwrap();
        for host in ["-Fbad", "user@-host", "host\n"] {
            assert_eq!(
                persist_default_remote_host_at(&path, false, Some(host))
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::InvalidInput
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
        }
        fs::write(&path, "[invalid").unwrap();
        assert!(persist_default_remote_host_at(&path, false, Some("desktop")).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "[invalid");
    }

    #[test]
    fn remote_preferences_preserve_inline_comments_and_replace_multiline_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[desktop.workspace]\ndefault_remote_host = 'old' # chosen host\nremote_hosts = [\n  'old',\n  'other',\n] # saved targets\nsidebar = false # untouched\n").unwrap();
        persist_default_remote_host_at(&path, false, None).unwrap();
        persist_remote_hosts_at(&path, false, &["new".into()]).unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("default_remote_host = \"\" # chosen host"));
        assert!(text.contains("# saved targets"));
        assert!(text.contains("sidebar = false # untouched"));
        let config = parse(&text, false).unwrap().normalize();
        assert!(config.workspace.default_remote_host.is_none());
        assert_eq!(config.workspace.remote_hosts, ["new"]);
    }

    #[test]
    fn pinned_directory_round_trips_in_shared_and_standalone_config() {
        for standalone in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            fs::write(&path, "# keep this comment\n").unwrap();
            let directory = "/home/person/project with 'quotes' and \"double quotes\"";
            persist_pinned_working_dir_at(&path, standalone, directory).unwrap();
            persist_theme_at(&path, standalone, "neutral-light").unwrap();
            let text = fs::read_to_string(&path).unwrap();
            assert!(text.contains("# keep this comment"));
            let config = parse(&text, standalone).unwrap();
            assert_eq!(
                config.workspace.pinned_working_dir.as_deref(),
                Some(directory)
            );
            assert_eq!(config.appearance.theme, "neutral-light");
        }
    }

    #[test]
    fn ai_font_parses_in_shared_and_standalone_config_without_ui_override() {
        for (text, standalone) in [
            ("[desktop.appearance]\nai_font = \"Urbanist\"", false),
            ("[appearance]\nai_font = \"Urbanist\"", true),
        ] {
            let config = parse(text, standalone).unwrap();
            assert_eq!(config.appearance.ai_font.as_deref(), Some("Urbanist"));
            assert!(config.appearance.ui_font.is_none());
            assert!(config.appearance.mono_font.is_none());
        }
        assert!(DesktopConfig::default().appearance.ai_font.is_none());
    }

    #[test]
    fn theme_defaults_to_parchment_and_preserves_explicit_preferences() {
        assert_eq!(DesktopConfig::default().appearance.theme, "parchment");
        for standalone in [false, true] {
            let section = if standalone {
                "appearance"
            } else {
                "desktop.appearance"
            };
            for text in [String::new(), format!("[{section}]\ntext_scale = 1.25\n")] {
                assert_eq!(
                    parse(&text, standalone).unwrap().appearance.theme,
                    "parchment"
                );
            }
            for preset in crate::theme::ThemePreset::ALL {
                let text = format!("[{section}]\ntheme = {:?}\n", preset.id());
                assert_eq!(
                    parse(&text, standalone).unwrap().appearance.theme,
                    preset.id()
                );
            }
        }
    }

    #[test]
    fn partial_config_keeps_safe_defaults_and_normalizes_ranges() {
        let config: DesktopConfig = toml::from_str(
            r##"
            [appearance]
            theme = "neutral-dark"
            text_scale = 9.0
            [appearance.colors]
            accent = "#ff00aa"
            [workspace]
            sidebar = false
            session_refresh_seconds = 0
            [terminal]
            scrollback_lines = 12
        "##,
        )
        .unwrap();
        let config = config.normalize();
        assert_eq!(config.appearance.text_scale, 1.0);
        assert_eq!(config.appearance.layout_mode, LayoutMode::FolderTabs);
        assert_eq!(config.appearance.theme, "neutral-dark");
        assert_eq!(config.appearance.colors["accent"], "#ff00aa");
        assert!(!config.workspace.sidebar);
        assert_eq!(config.workspace.session_refresh_seconds, 5);
        assert_eq!(config.terminal.scrollback_lines, 100);
    }

    #[test]
    fn desktop_section_coexists_with_shared_jcode_settings() {
        let config = parse(
            r#"
                model = "openai:gpt-5"
                [desktop.workspace]
                sidebar = false
            "#,
            false,
        )
        .unwrap();
        assert!(!config.workspace.sidebar);
        assert_eq!(config.appearance.text_scale, 1.0);
    }

    #[test]
    fn persisting_theme_preserves_shared_settings_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "# keep me\nmodel = \"openai:gpt-5\"\n[desktop.appearance] # ui\ntext_scale = 1.25\n",
        )
        .unwrap();
        persist_theme_at(&path, false, "neutral-light").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"));
        assert!(text.contains("model = \"openai:gpt-5\""));
        assert!(text.contains("text_scale = 1.25"));
        assert_eq!(
            parse(&text, false).unwrap().appearance.theme,
            "neutral-light"
        );
        persist_theme_at(&path, false, "warm-studio").unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert_eq!(text.matches("theme =").count(), 1);
        assert_eq!(parse(&text, false).unwrap().appearance.theme, "warm-studio");
    }

    #[test]
    fn layout_mode_defaults_to_folder_tabs_and_persists_without_losing_config() {
        assert_eq!(
            DesktopConfig::default().appearance.layout_mode,
            LayoutMode::FolderTabs
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# keep me\nmodel = \"openai:gpt-5\"\n").unwrap();

        persist_layout_mode_at(&path, false, LayoutMode::Normal).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep me"));
        assert!(text.contains("model = \"openai:gpt-5\""));
        assert_eq!(
            parse(&text, false).unwrap().appearance.layout_mode,
            LayoutMode::Normal
        );

        persist_layout_mode_at(&path, false, LayoutMode::FolderTabs).unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert_eq!(text.matches("layout_mode =").count(), 1);
        assert_eq!(
            parse(&text, false).unwrap().appearance.layout_mode,
            LayoutMode::FolderTabs
        );
    }
}
