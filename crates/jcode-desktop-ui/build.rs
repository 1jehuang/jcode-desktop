use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // The footer is also the hot-reload generation indicator. Re-run this
    // script for every UI source change so a rebuilt cdylib carries a fresh
    // timestamp instead of inheriting the first build's metadata.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=JCODE_DESKTOP_VERSION");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let version = env::var("JCODE_DESKTOP_VERSION").unwrap_or_else(|_| {
        env::var("CARGO_PKG_VERSION").expect("Cargo package version is missing")
    });
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch");
    let built_at = env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| now.as_secs().to_string());
    built_at
        .parse::<u64>()
        .expect("SOURCE_DATE_EPOCH must be a non-negative Unix timestamp");

    println!("cargo:rustc-env=JCODE_DESKTOP_VERSION={version}");
    println!("cargo:rustc-env=JCODE_DESKTOP_BUILT_AT={built_at}");
    // Include sub-second precision so two quick hot reloads still have visibly
    // different identities. Reproducible builds retain a stable identifier.
    let build_id = env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| now.as_millis().to_string());
    println!("cargo:rustc-env=JCODE_DESKTOP_BUILD_ID={build_id}");
}
