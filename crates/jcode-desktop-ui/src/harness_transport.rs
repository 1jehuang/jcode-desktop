//! Bridge-run-scoped SSH master reuse. Only transport owners are cached, never
//! attached API clients. Weak entries release idle masters with the last client.
use jcode_sdk::{JcodeClient, SharedSshTransport, SshConnectOptions, WeakSharedSshTransport};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::{fs::MetadataExt, process::CommandExt};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_TARGETS: usize = 32;
// Stay below sshd's usual MaxSessions=10. Reservations are cumulative, not
// returned, so concurrent selection cannot oversubscribe a master.
const CHANNELS_PER_MASTER: usize = 8;
const CONFIG_TIMEOUT: Duration = Duration::from_secs(2);
const CONFIG_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Default)]
pub(super) struct RemoteTransports {
    pool: Arc<Mutex<Pool<WeakSharedSshTransport>>>,
    #[cfg(test)]
    connector: Option<Arc<dyn Fn(&str) -> jcode_sdk::Result<JcodeClient> + Send + Sync>>,
}

impl RemoteTransports {
    #[cfg(test)]
    pub(super) fn testing(
        connector: impl Fn(&str) -> jcode_sdk::Result<JcodeClient> + Send + Sync + 'static,
    ) -> Self {
        Self {
            connector: Some(Arc::new(connector)),
            ..Self::default()
        }
    }

    pub(super) fn connect(&self, host: &str) -> jcode_sdk::Result<JcodeClient> {
        #[cfg(test)]
        if let Some(connector) = &self.connector {
            return connector(host);
        }
        let host = crate::remote_targets::validate_host(host).map_err(|message| {
            jcode_sdk::Error::new(jcode_sdk::ErrorKind::InvalidOption, message)
        })?;
        let options = SshConnectOptions {
            client_name: format!("jcode-desktop-remote/{}", crate::build_info::VERSION),
            connect_timeout: Duration::from_secs(20),
            request_timeout: Some(Duration::from_secs(30)),
            ..SshConnectOptions::new(&host)
        };
        // Resolve config for each new API channel. This detects Include changes,
        // cloud alias retargeting, identity changes and SSH agent replacement.
        // Never reuse an unverifiable target. A failed probe opens no connection.
        let identity = config_identity(&host).map_err(|error| {
            jcode_sdk::Error::new(
                jcode_sdk::ErrorKind::ConnectFailed,
                format!("Could not resolve SSH configuration for {host}: {error}"),
            )
        })?;
        let Some(identity) = identity else {
            // Preserve arbitrary SSH configurations whose file expansions we
            // cannot fingerprint, without weakening authentication policy.
            return JcodeClient::connect_ssh(options);
        };
        let key = Target { host, identity };
        let (owner, generation) = self.pool.lock().unwrap().acquire(
            key.clone(),
            WeakSharedSshTransport::upgrade,
            || {
                let owner = SharedSshTransport::new(options)?;
                let weak = owner.downgrade();
                Ok::<_, jcode_sdk::Error>((owner, weak))
            },
        )?;
        // No network operation under the pool lock. The SDK serializes just
        // this target's master startup and gives each call a fresh API stream.
        let result = owner.connect().and_then(|client| {
            // Config is mutable between the probe and lazy master startup.
            // Do not publish a client/create if the target changed meanwhile.
            match config_identity(&key.host) {
                Ok(Some(identity)) if identity == key.identity => Ok(client),
                _ => Err(jcode_sdk::Error::new(
                    jcode_sdk::ErrorKind::ConnectFailed,
                    "SSH configuration changed while connecting; reconnect explicitly",
                )),
            }
        });
        if result.is_err() {
            // A failed/full master is replaced only on a *future* connection.
            // Never retry a create, including ambiguous API handshake failures.
            self.pool.lock().unwrap().remove(&key, generation);
        }
        result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Target {
    host: String,
    identity: u64,
}

struct Entry<W> {
    key: Target,
    generation: u64,
    owner: W,
    reservations: usize,
}

struct Pool<W> {
    entries: VecDeque<Entry<W>>,
    generation: u64,
}

impl<W> Default for Pool<W> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            generation: 0,
        }
    }
}

impl<W> Pool<W> {
    fn acquire<T, E>(
        &mut self,
        key: Target,
        upgrade: impl Fn(&W) -> Option<T>,
        make: impl FnOnce() -> Result<(T, W), E>,
    ) -> Result<(T, u64), E> {
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            let mut entry = self.entries.remove(index).unwrap();
            if entry.reservations < CHANNELS_PER_MASTER
                && let Some(owner) = upgrade(&entry.owner)
            {
                entry.reservations += 1;
                let generation = entry.generation;
                self.entries.push_back(entry);
                return Ok((owner, generation));
            }
        }
        // Retargeting replaces the cache entry, not the old owner. Existing
        // clients keep their immutable old target until their own disconnect.
        self.entries
            .retain(|entry| entry.key.host != key.host && upgrade(&entry.owner).is_some());
        let (owner, weak) = make()?;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        if self.entries.len() == MAX_TARGETS {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            key,
            generation,
            owner: weak,
            reservations: 1,
        });
        Ok((owner, generation))
    }

    fn remove(&mut self, key: &Target, generation: u64) {
        // A late failure from an old connection must not evict its replacement.
        self.entries
            .retain(|entry| entry.key != *key || entry.generation != generation);
    }
}

/// OpenSSH itself handles wildcard Includes, Match, user/system configuration
/// and aliases. No authentication or network session is opened by `-G`.
/// Match exec is user-configured executable code, just as for the real connect.
fn config_identity(host: &str) -> std::io::Result<Option<u64>> {
    let mut output = tempfile::tempfile()?;
    let child = Command::new("ssh")
        .args([
            "-G",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "PermitLocalCommand=no",
            "--",
            host,
        ])
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let mut guard = ConfigProbe(Some(child));
    let deadline = Instant::now() + CONFIG_TIMEOUT;
    let status = loop {
        match guard.0.as_mut().unwrap().try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline && output.metadata()?.len() <= CONFIG_LIMIT => {
                std::thread::sleep(Duration::from_millis(2));
            }
            result => {
                return Err(result.err().unwrap_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "SSH configuration probe exceeded its bound",
                    )
                }));
            }
        }
    };
    guard.0.take();
    if !status.success() || output.metadata()?.len() > CONFIG_LIMIT {
        return Err(std::io::Error::other("SSH configuration probe failed"));
    }
    output.seek(SeekFrom::Start(0))?;
    let mut config = String::new();
    output.take(CONFIG_LIMIT).read_to_string(&mut config)?;
    Ok(fingerprint(host, &config))
}

// RAII covers try_wait, metadata, timeout and all future early-return paths.
struct ConfigProbe(Option<std::process::Child>);
impl Drop for ConfigProbe {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            // The group belongs to our child, including a hung Match exec.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
        }
    }
}

fn fingerprint(host: &str, config: &str) -> Option<u64> {
    fingerprint_with_environment(
        host,
        config,
        std::env::var_os("HOME"),
        std::env::var_os("SSH_AUTH_SOCK"),
    )
}

fn fingerprint_with_environment(
    host: &str,
    config: &str,
    home: Option<std::ffi::OsString>,
    agent: Option<std::ffi::OsString>,
) -> Option<u64> {
    let mut reusable = true;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    config.hash(&mut hash);
    home.hash(&mut hash);
    agent.hash(&mut hash);
    if let Some(agent) = agent {
        hash_file(std::path::Path::new(&agent), &mut hash);
    }
    // The managed proxy reads this JSON at runtime, so ssh -G alone cannot
    // observe instance/account retargeting behind an unchanged ProxyCommand.
    if host.rsplit('@').next() == Some("jcode-cloud-alpha") {
        if let Some(home) = &home {
            let home = std::path::Path::new(home);
            hash_file(&home.join(".config/jcode/cloud-alpha.json"), &mut hash);
            hash_file(&home.join(".local/bin/jcode-cloud-alpha"), &mut hash);
        }
    }
    for line in config.lines() {
        let Some((name, value)) = line.split_once(' ') else {
            continue;
        };
        if matches!(
            name,
            "identityfile" | "certificatefile" | "userknownhostsfile" | "globalknownhostsfile"
        ) {
            let paths: Vec<&str> = if matches!(name, "identityfile" | "certificatefile") {
                vec![value]
            } else {
                // OpenSSH prints known-hosts lists without quoting. Include
                // every contiguous span so filenames containing spaces are
                // fingerprinted too, without guessing their boundaries.
                let boundaries: Vec<usize> = std::iter::once(0)
                    .chain(value.match_indices(' ').map(|(index, _)| index + 1))
                    .take(64)
                    .collect();
                let mut paths = Vec::new();
                for &start in &boundaries {
                    for &end in &boundaries {
                        if end > start {
                            paths.push(value[start..end - 1].trim());
                        }
                    }
                    paths.push(value[start..].trim());
                }
                paths
            };
            for path in paths {
                let path = expand_tokens(path, host, config, &mut reusable);
                let path = path.as_str();
                let path = if let Some(relative) = path.strip_prefix("~/") {
                    home.as_ref()
                        .map(|home| std::path::PathBuf::from(home).join(relative))
                } else {
                    Some(path.into())
                };
                if let Some(path) = path {
                    hash_file(&path, &mut hash);
                }
            }
        }
    }
    reusable.then(|| hash.finish())
}

fn expand_tokens(value: &str, host: &str, config: &str, reusable: &mut bool) -> String {
    let field = |key: &str| {
        config
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap_or("")
    };
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('%') => output.push('%'),
            Some('h') => output.push_str(field("hostname ")),
            Some('n') => output.push_str(host.split_once('@').map_or(host, |(_, host)| host)),
            Some('r') => output.push_str(field("user ")),
            Some('p') => output.push_str(field("port ")),
            Some('d') => output.push_str(&std::env::var("HOME").unwrap_or_default()),
            // Exotic token expansions must not accidentally reuse a master
            // whose authentication files we could not identify safely.
            _ => *reusable = false,
        }
    }
    if output.contains("${") || output.starts_with('~') && !output.starts_with("~/") {
        *reusable = false;
    }
    output
}

fn hash_file(path: &std::path::Path, hash: &mut impl Hasher) {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            true.hash(hash);
            (
                metadata.dev(),
                metadata.ino(),
                metadata.len(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            )
                .hash(hash);
        }
        Err(error) => {
            false.hash(hash);
            error.kind().hash(hash);
        }
    }
}

#[cfg(test)]
#[path = "harness_transport_tests.rs"]
mod tests;
