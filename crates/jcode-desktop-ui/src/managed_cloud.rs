//! Managed Jcode Cloud transport.
//!
//! The Cloud destination is Jcode-operated infrastructure authorized by the
//! user's Jcode account. No AWS credential, SSH config, agent, or personal key
//! is involved. For each connection Desktop generates a fresh ed25519 key that
//! the control plane authorizes on the account's own host for 60 seconds, then
//! connects with that key only, pinned to host keys the control plane captured
//! at the host's first boot.
use jcode_sdk::{JcodeClient, SshConnectOptions};
use serde::Deserialize;
use std::path::Path;
use std::time::{Duration, Instant};

/// Reserved destination name. Never resolved through SSH config or DNS.
pub const HOST: &str = "jcode-cloud";
const DEFAULT_API_BASE: &str = "https://api.jcode.sh/v1";
/// First boot installs Jcode. Allow generous time before giving up.
const PROVISION_DEADLINE: Duration = Duration::from_secs(8 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(4);

#[derive(Debug, Deserialize)]
pub struct ConnectReply {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub host_keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorReply {
    error: ErrorBody,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    code: String,
    message: String,
}

/// Account credentials come from the same hosted-account file Jcode uses.
pub struct Account {
    pub api_key: String,
    pub api_base: String,
}

pub fn account() -> Result<Account, String> {
    let api_key = jcode_base::subscription_catalog::configured_api_key()
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            "Sign in to your Jcode account to use Jcode Cloud. No local session was opened."
                .to_string()
        })?;
    let api_base = jcode_base::subscription_catalog::configured_api_base()
        .filter(|base| base.starts_with("https://"))
        .unwrap_or_else(|| DEFAULT_API_BASE.into());
    Ok(Account {
        api_key: api_key.trim().to_owned(),
        api_base: api_base.trim_end_matches('/').to_owned(),
    })
}

/// A key pair valid for one connection. Files live in a private temporary
/// directory that is removed when this value drops.
pub struct EphemeralKey {
    dir: tempfile::TempDir,
    pub public_key: String,
}

impl EphemeralKey {
    pub fn generate() -> Result<Self, String> {
        let dir = tempfile::Builder::new()
            .prefix("jcode-cloud-")
            .tempdir()
            .map_err(|e| format!("Could not create a private key directory: {e}"))?;
        let key = dir.path().join("id");
        let output = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "jcode-desktop", "-f"])
            .arg(&key)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("ssh-keygen is required for Jcode Cloud: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "Could not generate a connection key: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let public_key = std::fs::read_to_string(key.with_extension("pub"))
            .map_err(|e| format!("Could not read the connection key: {e}"))?
            .trim()
            .to_owned();
        Ok(Self { dir, public_key })
    }

    pub fn private_key(&self) -> std::path::PathBuf {
        self.dir.path().join("id")
    }

    fn known_hosts(&self) -> std::path::PathBuf {
        self.dir.path().join("known_hosts")
    }
}

/// Validate the reply before any value reaches an SSH command line.
pub fn validated_target(
    reply: &ConnectReply,
) -> Result<(String, u16, String, Vec<String>), String> {
    let address = reply
        .address
        .as_deref()
        .filter(|a| a.parse::<std::net::IpAddr>().is_ok())
        .ok_or("Jcode Cloud returned an invalid host address")?;
    let user = reply
        .user
        .as_deref()
        .filter(|u| {
            !u.is_empty()
                && u.len() <= 32
                && u.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
        .ok_or("Jcode Cloud returned an invalid user")?;
    let keys: Vec<String> = reply
        .host_keys
        .iter()
        .filter(|key| {
            let mut parts = key.split(' ');
            matches!(
                (parts.next(), parts.next(), parts.next()),
                (Some("ssh-ed25519" | "ecdsa-sha2-nistp256"), Some(blob), None)
                    if blob.len() <= 1024 && blob.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
            )
        })
        .cloned()
        .collect();
    if keys.is_empty() {
        return Err("Jcode Cloud did not provide verified host keys".into());
    }
    Ok((
        address.to_owned(),
        reply.port.unwrap_or(22),
        user.to_owned(),
        keys,
    ))
}

pub fn known_hosts_contents(address: &str, port: u16, keys: &[String]) -> String {
    let host = if port == 22 {
        address.to_owned()
    } else {
        format!("[{address}]:{port}")
    };
    keys.iter().map(|key| format!("{host} {key}\n")).collect()
}

/// One control-plane request. Idempotent: each call advances the host toward
/// readiness and, when ready, authorizes `public_key` for one login.
pub fn request_connect(account: &Account, public_key: &str) -> Result<ConnectReply, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(format!("jcode-desktop/{}", crate::build_info::VERSION))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .post(format!("{}/cloud/host/connect", account.api_base))
        .bearer_auth(&account.api_key)
        .json(&serde_json::json!({ "public_key": public_key }))
        .send()
        .map_err(|e| format!("Could not reach Jcode Cloud: {e}"))?;
    let status = response.status();
    let body = response.text().map_err(|e| e.to_string())?;
    if status.is_success() {
        return serde_json::from_str(&body)
            .map_err(|e| format!("Unexpected Jcode Cloud reply: {e}"));
    }
    Err(match serde_json::from_str::<ErrorReply>(&body) {
        Ok(reply) => describe_error(status.as_u16(), &reply.error.code, &reply.error.message),
        Err(_) => format!("Jcode Cloud request failed (HTTP {status})"),
    })
}

fn describe_error(status: u16, code: &str, message: &str) -> String {
    match (status, code) {
        (401, _) => "Your Jcode sign-in has expired. Sign in again to use Jcode Cloud.".into(),
        (402, "cloud_not_entitled") => {
            "Jcode Cloud requires an active paid Jcode subscription.".into()
        }
        (402, "cloud_allowance_exhausted") => message.to_owned(),
        _ => message.to_owned(),
    }
}

/// Wake or provision the account's host and connect a Jcode API client.
pub fn connect(
    progress: &mut dyn FnMut(&str),
    client_name: String,
    mut request: impl FnMut(&Account, &str) -> Result<ConnectReply, String>,
    open: impl FnOnce(SshConnectOptions) -> jcode_sdk::Result<JcodeClient>,
) -> Result<JcodeClient, String> {
    let account = account()?;
    let key = EphemeralKey::generate()?;
    let deadline = Instant::now() + PROVISION_DEADLINE;
    let mut last = String::new();
    let reply = loop {
        let reply = request(&account, &key.public_key)?;
        if reply.ready {
            break reply;
        }
        let message = reply
            .message
            .clone()
            .unwrap_or_else(|| format!("Jcode Cloud machine is {}…", reply.state));
        if message != last {
            progress(&message);
            last = message;
        }
        if Instant::now() >= deadline {
            return Err("Jcode Cloud did not become ready in time. Retry to keep waiting.".into());
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    let (address, port, user, host_keys) = validated_target(&reply)?;
    write_private(
        &key.known_hosts(),
        &known_hosts_contents(&address, port, &host_keys),
    )?;
    progress("Connecting to your Jcode Cloud machine…");
    open(SshConnectOptions {
        port: Some(port),
        user: Some(user),
        identity_file: Some(key.private_key()),
        known_hosts_file: Some(key.known_hosts()),
        isolated_config: true,
        client_name,
        connect_timeout: Duration::from_secs(45),
        request_timeout: Some(Duration::from_secs(30)),
        ..SshConnectOptions::new(address)
    })
    .map_err(|e| e.to_string())
    // `key` drops here. The authorized key expires server-side within 60s and
    // the established connection does not need the file again.
}

fn write_private(path: &Path, contents: &str) -> Result<(), String> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    file.write_all(contents.as_bytes())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(json: serde_json::Value) -> ConnectReply {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn target_validation_rejects_anything_that_could_reach_ssh_options() {
        let good = reply(serde_json::json!({
            "state": "running", "ready": true, "address": "203.0.113.7", "port": 22,
            "user": "ec2-user", "host_keys": ["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHostKey"]
        }));
        let (address, port, user, keys) = validated_target(&good).unwrap();
        assert_eq!(
            (address.as_str(), port, user.as_str()),
            ("203.0.113.7", 22, "ec2-user")
        );
        assert_eq!(keys.len(), 1);
        for bad in [
            serde_json::json!({"address": "-oProxyCommand=x", "user": "ec2-user", "host_keys": ["ssh-ed25519 AAAA"]}),
            serde_json::json!({"address": "evil.example", "user": "ec2-user", "host_keys": ["ssh-ed25519 AAAA"]}),
            serde_json::json!({"address": "203.0.113.7", "user": "-l root", "host_keys": ["ssh-ed25519 AAAA"]}),
            serde_json::json!({"address": "203.0.113.7", "user": "ec2-user", "host_keys": []}),
            serde_json::json!({"address": "203.0.113.7", "user": "ec2-user", "host_keys": ["ssh-ed25519 AAAA\n* ssh-ed25519 BBBB"]}),
            serde_json::json!({"address": "203.0.113.7", "user": "ec2-user", "host_keys": ["ssh-rsa AAAA"]}),
        ] {
            assert!(
                validated_target(&reply(bad.clone())).is_err(),
                "accepted {bad}"
            );
        }
    }

    #[test]
    fn known_hosts_pins_only_the_published_address() {
        let keys = vec![
            "ssh-ed25519 AAAAC3".to_string(),
            "ecdsa-sha2-nistp256 AAAAE2".to_string(),
        ];
        assert_eq!(
            known_hosts_contents("203.0.113.7", 22, &keys),
            "203.0.113.7 ssh-ed25519 AAAAC3\n203.0.113.7 ecdsa-sha2-nistp256 AAAAE2\n"
        );
        assert!(
            known_hosts_contents("203.0.113.7", 2222, &keys).starts_with("[203.0.113.7]:2222 ")
        );
    }

    #[test]
    fn ephemeral_keys_are_fresh_private_and_removed_on_drop() {
        let first = EphemeralKey::generate().unwrap();
        let second = EphemeralKey::generate().unwrap();
        assert!(first.public_key.starts_with("ssh-ed25519 "));
        assert_ne!(first.public_key, second.public_key);
        let path = first.private_key();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        drop(first);
        assert!(!path.exists());
    }

    #[test]
    fn errors_explain_sign_in_and_subscription() {
        assert!(describe_error(401, "unauthorized", "x").contains("Sign in"));
        assert!(describe_error(402, "cloud_not_entitled", "x").contains("subscription"));
        assert_eq!(describe_error(503, "cloud_unavailable", "Down"), "Down");
    }
}
