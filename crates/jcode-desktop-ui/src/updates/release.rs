//! Read-only public release metadata shared by the status check and installer.
use anyhow::{Context, Result, ensure};
use semver::Version;
use std::{io::Read, time::Duration};

pub(super) const PUBLIC_RELEASES: &str =
    "https://github.com/1jehuang/jcode-desktop-releases/releases/download";

pub(super) fn parse_version(text: &str) -> Result<Version> {
    let text = text
        .strip_prefix("desktop-v")
        .or_else(|| text.strip_prefix('v'))
        .unwrap_or(text);
    let version = Version::parse(text).context("Invalid desktop release version")?;
    ensure!(
        version.build.is_empty(),
        "Desktop release versions with build metadata are not supported"
    );
    Ok(version)
}

fn parse_metadata(bytes: &[u8]) -> Result<Version> {
    let metadata: serde_json::Value =
        serde_json::from_slice(bytes).context("Invalid public latest desktop release metadata")?;
    let tag = metadata["tag_name"]
        .as_str()
        .context("Latest release metadata has no tag_name")?;
    let version = parse_version(
        tag.strip_prefix("desktop-v")
            .context("Unexpected desktop release tag")?,
    )?;
    ensure!(
        tag == format!("desktop-v{version}"),
        "Noncanonical desktop release tag"
    );
    Ok(version)
}

/// Fetch metadata only. No assets, file writes, installer, or platform callbacks.
pub(super) fn latest() -> Result<Version> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Jcode-Desktop-Release-Check")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    fetch(
        &client,
        &format!("{PUBLIC_RELEASES}/desktop-latest/latest.json"),
    )
}

fn fetch(client: &reqwest::blocking::Client, url: &str) -> Result<Version> {
    let response = client
        .get(url)
        .send()
        .context("Checking latest desktop release")?
        .error_for_status()
        .context("Latest desktop release request failed")?;
    const LIMIT: u64 = 1024 * 1024;
    ensure!(
        response.content_length().is_none_or(|size| size <= LIMIT),
        "Release metadata exceeds safety size limit"
    );
    let mut bytes = Vec::new();
    response
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .context("Reading latest desktop release")?;
    ensure!(
        bytes.len() as u64 <= LIMIT,
        "Release metadata exceeds safety size limit"
    );
    parse_metadata(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_fetch_is_read_only_and_reports_network_errors() {
        use std::io::Write;
        for (status, body, expected) in [
            ("200 OK", r#"{"tag_name":"desktop-v0.1.0-beta.10"}"#, true),
            ("503 Unavailable", "offline", false),
            ("200 OK", "malformed", false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/latest.json", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 4096];
                let count = stream.read(&mut request).unwrap();
                assert!(
                    std::str::from_utf8(&request[..count])
                        .unwrap()
                        .starts_with("GET /latest.json HTTP/1.1")
                );
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            let client = reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            assert_eq!(fetch(&client, &url).is_ok(), expected);
            server.join().unwrap();
        }
    }

    #[test]
    fn public_metadata_requires_canonical_desktop_tag() {
        assert_eq!(
            parse_metadata(br#"{"tag_name":"desktop-v0.1.0-beta.10"}"#).unwrap(),
            Version::parse("0.1.0-beta.10").unwrap()
        );
        for bytes in [
            br#"{}"#.as_slice(),
            br#"{"tag_name":"v0.1.0"}"#,
            br#"{"tag_name":"desktop-vv0.1.0"}"#,
            br#"{"tag_name":"desktop-v0.1.0+foo"}"#,
            br#"{"tag_name":"desktop-v../evil"}"#,
            b"not json",
        ] {
            assert!(parse_metadata(bytes).is_err());
        }
    }
}
