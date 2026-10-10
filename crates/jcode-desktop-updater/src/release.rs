//! Read-only public release metadata and platform release assets.
use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use std::{io::Read, time::Duration};

pub const PUBLIC_RELEASES: &str =
    "https://github.com/1jehuang/jcode-desktop-releases/releases/download";

pub fn parse_version(text: &str) -> Result<Version> {
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

/// Whether `latest` should replace `running`. Build metadata never counts.
pub fn is_newer(latest: &Version, running: &Version) -> bool {
    latest.cmp_precedence(running).is_gt()
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

pub(crate) fn client(user_agent: &str, timeout: Duration) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?)
}

/// Fetch metadata only. No assets, file writes, installer, or platform callbacks.
pub fn latest() -> Result<Version> {
    let client = client("Jcode-Desktop-Release-Check", Duration::from_secs(20))?;
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

/// The published package for one operating system and architecture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target {
    pub platform: &'static str,
    pub architecture: &'static str,
    pub checksum_manifest: &'static str,
    pub zip: bool,
}

impl Target {
    pub fn current() -> Result<Self> {
        Self::for_platform(std::env::consts::OS, std::env::consts::ARCH)
    }

    pub fn for_platform(os: &str, architecture: &str) -> Result<Self> {
        let (platform, architecture, checksum_manifest, zip) = match (os, architecture) {
            ("linux", "x86_64") => ("linux", "x86_64", "SHA256SUMS-linux", false),
            ("linux", "aarch64") => ("linux", "aarch64", "SHA256SUMS-linux-aarch64", false),
            ("freebsd", "x86_64") => ("freebsd", "x86_64", "SHA256SUMS-freebsd-x86_64", false),
            ("windows", "x86_64") => ("windows", "x86_64", "SHA256SUMS-windows", true),
            ("windows", "aarch64") => ("windows", "aarch64", "SHA256SUMS-windows-aarch64", true),
            _ => bail!(
                "Automatic desktop updates do not support {os} (unsupported architecture: {architecture})"
            ),
        };
        Ok(Self {
            platform,
            architecture,
            checksum_manifest,
            zip,
        })
    }

    /// The archive's single top-level directory and the asset file name.
    pub fn archive_names(&self, version: &Version) -> (String, String) {
        let root = format!("Jcode-{version}-{}-{}", self.platform, self.architecture);
        let asset = format!("{root}.{}", if self.zip { "zip" } else { "tar.gz" });
        (root, asset)
    }

    pub fn executable(&self) -> &'static str {
        if self.zip {
            "jcode-desktop.exe"
        } else {
            "jcode-desktop"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn metadata_fetch_is_read_only_and_reports_network_errors() {
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

    #[test]
    fn semantic_versions_order_prereleases_numerically() {
        assert!(
            parse_version("0.1.0-beta.10").unwrap()
                > parse_version("desktop-v0.1.0-beta.9").unwrap()
        );
        assert!(parse_version("v0.1.0").unwrap() > parse_version("0.1.0-beta.99").unwrap());
        for version in ["../0.1.0", "0.1.0/evil", "0.1.0+ambiguous", "latest"] {
            assert!(parse_version(version).is_err());
        }
    }

    #[test]
    fn published_asset_names_match_every_supported_package() {
        let version = Version::parse("0.5.1").unwrap();
        for (os, arch, asset, sums) in [
            ("linux", "x86_64", "Jcode-0.5.1-linux-x86_64.tar.gz", "SHA256SUMS-linux"),
            ("linux", "aarch64", "Jcode-0.5.1-linux-aarch64.tar.gz", "SHA256SUMS-linux-aarch64"),
            ("freebsd", "x86_64", "Jcode-0.5.1-freebsd-x86_64.tar.gz", "SHA256SUMS-freebsd-x86_64"),
            ("windows", "x86_64", "Jcode-0.5.1-windows-x86_64.zip", "SHA256SUMS-windows"),
            ("windows", "aarch64", "Jcode-0.5.1-windows-aarch64.zip", "SHA256SUMS-windows-aarch64"),
        ] {
            let target = Target::for_platform(os, arch).unwrap();
            let (root, name) = target.archive_names(&version);
            assert_eq!(name, asset);
            assert!(asset.starts_with(&root));
            assert_eq!(target.checksum_manifest, sums);
        }
    }

    #[test]
    fn unsupported_platforms_are_rejected() {
        for (os, arch) in [("linux", "x86"), ("linux", "arm64"), ("macos", "aarch64"), ("linux", "../x86_64")] {
            let error = Target::for_platform(os, arch).unwrap_err();
            assert!(error.to_string().contains("unsupported architecture"));
        }
    }
}
