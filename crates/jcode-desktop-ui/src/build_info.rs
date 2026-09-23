use std::time::{SystemTime, UNIX_EPOCH};

use crate::theme::Theme;
use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px};

pub(crate) const VERSION: &str = env!("JCODE_DESKTOP_DISPLAY_VERSION");
const BUILT_AT: &str = env!("JCODE_DESKTOP_BUILT_AT");
const BUILD_ID: &str = env!("JCODE_DESKTOP_BUILD_ID");

pub(crate) fn version() -> String {
    format!("v{VERSION}")
}

pub(crate) fn revision() -> String {
    let hash = env!("JCODE_DESKTOP_GIT_HASH");
    if env!("JCODE_DESKTOP_GIT_DIRTY") == "true" {
        format!("{hash}, dirty")
    } else {
        hash.to_owned()
    }
}

pub(crate) fn age() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    BUILT_AT
        .parse::<u64>()
        .map(|built| format!("Built {}", format_age(now.saturating_sub(built))))
        .unwrap_or_else(|_| "Build time unavailable".into())
}

pub fn label() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    footer_label(VERSION, development(), BUILT_AT.parse().ok(), now)
}

pub(crate) fn development() -> bool {
    env!("JCODE_DESKTOP_DEVELOPMENT") == "true"
}

/// Format presentation only. Release tags, update ordering and build identities
/// retain their original machine-readable values.
fn version_label(raw: &str) -> String {
    let Ok(version) = semver::Version::parse(raw.strip_prefix('v').unwrap_or(raw)) else {
        return raw.to_owned();
    };
    let base = format!("{}.{}.{}", version.major, version.minor, version.patch);
    if version.pre.is_empty() {
        return base;
    }
    let pre = version.pre.as_str();
    if pre == "dev" {
        return format!("{base}-dev");
    }
    let (channel, suffix) = pre.split_once('.').unwrap_or((pre, ""));
    let channel = match channel {
        "alpha" => "Alpha",
        "beta" => "Beta",
        "rc" => "RC",
        "dev" => "Dev",
        "nightly" => "Nightly",
        _ => return format!("{base} · {pre}"),
    };
    if suffix.is_empty() {
        format!("{base} · {channel}")
    } else {
        format!("{base} · {channel} {suffix}")
    }
}

fn footer_label(version: &str, development: bool, built_at: Option<u64>, now: u64) -> String {
    let mut label = format!("Desktop {}", version_label(version));
    if development {
        // Keep local builds distinct from the installed release, even when
        // Cargo's base package version has not changed between rebuilds.
        let already_dev = semver::Version::parse(version.strip_prefix('v').unwrap_or(version))
            .is_ok_and(|version| version.pre.as_str().split('.').next() == Some("dev"));
        if !already_dev {
            label.push_str(" · Dev");
        }
        if let Some(built_at) = built_at {
            label.push_str(&format!(
                " · built {}",
                format_age(now.saturating_sub(built_at))
            ));
        }
    }
    label
}

pub(crate) struct BuildTooltip;

impl Render for BuildTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global();
        div()
            .debug_selector(|| "panel-build-tooltip".into())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .max_w(px(420.))
            .rounded_md()
            .bg(theme.HEADER_BG)
            .border_1()
            .border_color(theme.PANEL_BORDER)
            .text_size(px(11.))
            .text_color(theme.TEXT_DIM)
            .child(format!(
                "Jcode Desktop · {}",
                if development() {
                    "Development build"
                } else {
                    "Release build"
                }
            ))
            .child(format!("Version: {VERSION}"))
            .child(format!("Commit: {}", revision()))
            .child(format!("Build ID: {BUILD_ID}"))
    }
}

fn format_age(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{}m ago", seconds / MINUTE),
        3600..86400 => format!("{}h ago", seconds / HOUR),
        _ => format!("{}d ago", seconds / DAY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn footer_exposes_exact_build_details_on_hover(cx: &mut gpui::TestAppContext) {
        let (bridge, _commands) = crate::harness::spawn_recording();
        let (_panel, vcx) = cx.add_window_view(|_, cx| {
            crate::panel::Panel::new("version-label-test".into(), None, None, bridge, cx)
        });
        vcx.run_until_parked();
        let footer = vcx
            .debug_bounds("panel-build")
            .expect("version label renders");
        vcx.update(|window, cx| window.simulate_mouse_move(footer.center(), cx));
        vcx.run_until_parked();
        vcx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("panel-build-tooltip").is_some());
    }

    #[test]
    fn release_channels_are_readable_without_changing_the_version_number() {
        for (raw, expected) in [
            ("0.1.0", "0.1.0"),
            ("v1.2.3", "1.2.3"),
            ("0.1.432-dev", "0.1.432-dev"),
            ("0.3.0-dev.12", "0.3.0 · Dev 12"),
            ("0.1.0-beta.15", "0.1.0 · Beta 15"),
            ("1.0.0-rc.2", "1.0.0 · RC 2"),
            ("1.0.0-alpha", "1.0.0 · Alpha"),
            ("1.0.0-nightly.20260917", "1.0.0 · Nightly 20260917"),
            ("1.0.0-preview.2", "1.0.0 · preview.2"),
            ("1.2.3+1789617600000", "1.2.3"),
            ("custom build", "custom build"),
        ] {
            assert_eq!(version_label(raw), expected);
        }
    }

    #[test]
    fn footer_distinguishes_local_builds_without_exposing_epoch_ids() {
        assert_eq!(
            footer_label("0.1.0", true, Some(100), 100),
            "Desktop 0.1.0 · Dev · built just now"
        );
        assert_eq!(
            footer_label("0.1.0", true, Some(100), 220),
            "Desktop 0.1.0 · Dev · built 2m ago"
        );
        assert_eq!(
            footer_label("0.1.0-beta.15", false, Some(100), 220),
            "Desktop 0.1.0 · Beta 15"
        );
        assert_eq!(footer_label("0.1.0", false, None, 220), "Desktop 0.1.0");
        assert_eq!(
            footer_label("0.1.0", true, None, 220),
            "Desktop 0.1.0 · Dev"
        );
        assert_eq!(
            footer_label("0.1.0-dev", true, Some(300), 220),
            "Desktop 0.1.0-dev · built just now"
        );
        assert_eq!(
            footer_label("0.3.0-dev.12", true, Some(100), 220),
            "Desktop 0.3.0 · Dev 12 · built 2m ago"
        );
    }

    #[test]
    fn age_is_concise_and_human_readable() {
        assert_eq!(format_age(0), "just now");
        assert_eq!(format_age(59), "just now");
        assert_eq!(format_age(60), "1m ago");
        assert_eq!(format_age(3_599), "59m ago");
        assert_eq!(format_age(3_600), "1h ago");
        assert_eq!(format_age(86_399), "23h ago");
        assert_eq!(format_age(86_400), "1d ago");
    }

    #[test]
    fn surfaces_share_the_numbered_build_version() {
        let version = semver::Version::parse(VERSION).expect("valid display version");
        assert_eq!(
            version.pre.as_str().split('.').next() == Some("dev"),
            development()
        );
        assert!(crate::build_version().starts_with(&format!("v{VERSION} (")));
        assert!(label().contains(&version_label(VERSION)));
    }
}
