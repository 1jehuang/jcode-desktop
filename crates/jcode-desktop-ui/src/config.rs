//! Persisted user configuration for Jcode Desktop.

use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::PathBuf, sync::OnceLock, time::Duration};

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    pub appearance: AppearanceConfig,
    pub workspace: WorkspaceConfig,
    pub terminal: TerminalConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    /// Built-in color theme. Unknown names fall back to warm-neutral.
    pub theme: String,
    /// UI font family. The platform-specific built-in remains the default.
    pub ui_font: Option<String>,
    /// Monospace font used by code and terminal panels.
    pub mono_font: Option<String>,
    /// Global text scale. Values outside 0.75..=2.0 fall back to 1.0.
    pub text_scale: f32,
    /// Disable non-essential workspace motion.
    pub reduce_motion: bool,
    /// Semantic color overrides. Keys are documented in README.md.
    pub colors: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
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
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: "warm-neutral".into(),
            ui_font: None,
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
pub fn persist_theme(theme: &str) -> std::io::Result<()> {
    let path = path();
    let standalone = std::env::var_os("JCODE_DESKTOP_CONFIG").is_some();
    persist_theme_at(&path, standalone, theme)
}

fn persist_theme_at(path: &std::path::Path, standalone: bool, theme: &str) -> std::io::Result<()> {
    let section = if standalone {
        "[appearance]"
    } else {
        "[desktop.appearance]"
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
                .is_some_and(|(key, _)| key.trim() == "theme")
        }) {
            lines[index] = format!("theme = {theme:?}");
        } else {
            lines.insert(start + 1, format!("theme = {theme:?}"));
        }
    } else {
        if !text.is_empty() && !text.ends_with('\n') {
            lines.push(String::new());
        }
        lines.push(section.into());
        lines.push(format!("theme = {theme:?}"));
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
}
