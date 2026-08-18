use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: &str = env!("JCODE_DESKTOP_VERSION");
const BUILT_AT: &str = env!("JCODE_DESKTOP_BUILT_AT");

pub fn label() -> String {
    let built_at = BUILT_AT.parse::<u64>().unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!(
        "v{VERSION} · built {}",
        format_age(now.saturating_sub(built_at))
    )
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
}
