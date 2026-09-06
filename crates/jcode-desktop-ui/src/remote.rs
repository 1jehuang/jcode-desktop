//! Persistent session addresses for SSH-backed native panels.
//!
//! Only the address crosses the UI/snapshot boundary. SDK calls always receive
//! the original remote ID, never its desktop namespace.

const PREFIX: &str = "ssh://";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionAddress {
    pub host: Option<String>,
    pub session_id: String,
}

impl SessionAddress {
    pub fn parse(id: &str) -> Result<Self, String> {
        let Some(remote) = id.strip_prefix(PREFIX) else {
            return Ok(Self {
                host: None,
                session_id: id.to_owned(),
            });
        };
        let (host, session_id) = remote
            .split_once('/')
            .ok_or("Invalid remote session address")?;
        let host = decode(host)?;
        let validated = crate::remote_targets::validate_host(&host)?;
        if host != validated {
            return Err("Invalid remote session host".into());
        }
        let session_id = decode(session_id)?;
        if session_id.is_empty() || session_id.chars().any(char::is_control) {
            return Err("Invalid remote session ID".into());
        }
        Ok(Self {
            host: Some(host),
            session_id,
        })
    }

    pub fn ui_id(&self, real_id: &str) -> String {
        match &self.host {
            Some(host) => namespace(host, real_id),
            None => real_id.to_owned(),
        }
    }

    pub fn session_info(&self, mut session: jcode_sdk::SessionInfo) -> jcode_sdk::SessionInfo {
        session.session_id = self.ui_id(&session.session_id);
        session
    }
}

pub(super) fn is_remote(id: &str) -> bool {
    id.starts_with(PREFIX)
}

pub(super) fn namespace(host: &str, session_id: &str) -> String {
    format!("{PREFIX}{}/{}", encode(host), encode(session_id))
}

fn encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            output.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn decode(value: &str) -> Result<String, String> {
    let mut bytes = value.bytes();
    let mut output = Vec::new();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next().and_then(|b| char::from(b).to_digit(16));
            let low = bytes.next().and_then(|b| char::from(b).to_digit(16));
            match (high, low) {
                (Some(high), Some(low)) => output.push((high * 16 + low) as u8),
                _ => return Err("Invalid remote session percent encoding".into()),
            }
        } else if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            output.push(byte);
        } else {
            return Err("Remote session address must be percent encoded".into());
        }
    }
    String::from_utf8(output).map_err(|_| "Remote session address is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ids_are_unchanged() {
        let address = SessionAddress::parse("session_example_123").unwrap();
        assert_eq!(address.host, None);
        assert_eq!(address.ui_id(&address.session_id), "session_example_123");
    }

    #[test]
    fn remote_ids_round_trip_without_collisions() {
        for host in ["desktop", "user@host", "user@[::1]", "fe80::1%eth0"] {
            for id in ["session_123", "id/with?reserved%characters", "unicode-猫"] {
                let encoded = namespace(host, id);
                let address = SessionAddress::parse(&encoded).unwrap();
                assert_eq!(address.host.as_deref(), Some(host));
                assert_eq!(address.session_id, id);
                assert_eq!(address.ui_id(id), encoded);
                assert_ne!(encoded, namespace("other-host", id));
            }
        }
        assert_eq!(
            namespace("user@host", "session_1"),
            "ssh://user%40host/session_1"
        );
    }

    #[test]
    fn malformed_remote_ids_never_fall_back_to_local() {
        for id in [
            "ssh://",
            "ssh://host",
            "ssh:///id",
            "ssh://host/",
            "ssh://host/id/extra",
            "ssh://host/%",
            "ssh://host/%FF",
            "ssh://host/%00",
            "ssh://-oProxyCommand/id",
            "ssh://bad%20host/id",
            "ssh://user@host/id",
        ] {
            assert!(SessionAddress::parse(id).is_err(), "accepted {id}");
            assert!(is_remote(id));
        }
    }
}
