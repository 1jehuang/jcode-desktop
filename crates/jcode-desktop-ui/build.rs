use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-env-changed=JCODE_DESKTOP_VERSION");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let version = env::var("JCODE_DESKTOP_VERSION").unwrap_or_else(|_| {
        env::var("CARGO_PKG_VERSION").expect("Cargo package version is missing")
    });
    let built_at = env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_secs()
            .to_string()
    });
    built_at
        .parse::<u64>()
        .expect("SOURCE_DATE_EPOCH must be a non-negative Unix timestamp");

    println!("cargo:rustc-env=JCODE_DESKTOP_VERSION={version}");
    println!("cargo:rustc-env=JCODE_DESKTOP_BUILT_AT={built_at}");
}
