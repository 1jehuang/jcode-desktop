//! Bridge-run-scoped SSH master reuse with one short-lived unattached cloud client.
use jcode_sdk::{JcodeClient, SharedSshTransport, SshConnectOptions, WeakSharedSshTransport};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::{fs::MetadataExt, process::CommandExt};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SPARE_TTL: Duration = Duration::from_secs(60);
const MAX_TARGETS: usize = 32;
// Stay below sshd's usual MaxSessions=10. Reservations are cumulative, not
// returned, so concurrent selection cannot oversubscribe a master.
const CHANNELS_PER_MASTER: usize = 8;
const CONFIG_TIMEOUT: Duration = Duration::from_secs(2);
const CONFIG_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Default)]
pub(super) struct RemoteTransports {
    pool: Arc<Mutex<Pool<WeakSharedSshTransport>>>,
    // Provisioning is serialized independently of ordinary SSH selection.
    cloud: Arc<Mutex<CloudTransport<WeakSharedSshTransport, JcodeClient>>>,
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
        self.connect_with_progress(host, &mut |_| {})
    }

    /// Connect, reporting managed-cloud provisioning phases as they happen.
    pub(super) fn connect_with_progress(
        &self,
        host: &str,
        progress: &mut dyn FnMut(&str),
    ) -> jcode_sdk::Result<JcodeClient> {
        #[cfg(test)]
        if let Some(connector) = &self.connector {
            return connector(host);
        }
        if host == crate::managed_cloud::HOST {
            return self.connect_cloud(progress);
        }
        let host = crate::remote_targets::validate_host(host).map_err(|message| {
            jcode_sdk::Error::new(jcode_sdk::ErrorKind::InvalidOption, message)
        })?;
        let options = SshConnectOptions {
            // Temporarily disabled until inline tool responses have broader testing.
            supports_user_response: false,
            client_name: format!("jcode-desktop-remote/{}", crate::client_version()),
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

    #[cfg(test)]
    pub(super) fn cloud_spare_ready(&self) -> bool {
        self.cloud
            .lock()
            .unwrap()
            .spare
            .as_ref()
            .is_some_and(|(client, deadline)| Instant::now() < *deadline && !client.is_closed())
    }

    fn connect_cloud(&self, progress: &mut dyn FnMut(&str)) -> jcode_sdk::Result<JcodeClient> {
        let error = |message| jcode_sdk::Error::new(jcode_sdk::ErrorKind::ConnectFailed, message);
        // This lock serializes cloud initialization only, never the global pool.
        let mut cloud = self.cloud.lock().unwrap();
        let account = crate::managed_cloud::account();
        let fingerprint = crate::managed_cloud_parity::Snapshot::collect().fingerprint();
        cloud.select_account(account.as_ref().ok(), fingerprint);
        let account = account.map_err(error)?;
        let result = if let Some(client) = cloud
            .take_spare(Instant::now())
            .filter(|client| !client.is_closed())
        {
            progress("Reusing ready Jcode Cloud connection…");
            Ok(client)
        } else if let Some(owner) = cloud.reserve(WeakSharedSshTransport::upgrade) {
            progress("Reusing your Jcode Cloud connection…");
            owner.connect()
        } else {
            crate::managed_cloud::connect_with_sync_status(
                progress,
                format!("jcode-desktop-remote/{}", crate::client_version()),
                |current, key| {
                    // connect reads credentials again. Never authorize a new
                    // account under the identity selected above.
                    if !same_cloud_account(&account, current) {
                        return Err(
                            "Jcode account changed while connecting; reconnect explicitly".into(),
                        );
                    }
                    crate::managed_cloud::request_connect(current, key)
                },
                |options, synchronized| {
                    let owner = SharedSshTransport::new(options)?;
                    // Startup must finish inside this closure, while connect
                    // still owns the ephemeral private key and pinned hosts.
                    let client = owner.connect()?;
                    if synchronized {
                        cloud.publish(owner.downgrade());
                    }
                    Ok(client)
                },
            )
            .map_err(error)
        };
        // Credentials and synchronized defaults can change during provisioning
        // or an API handshake. Do not publish a client with stale parity.
        let unchanged = crate::managed_cloud::account()
            .is_ok_and(|current| same_cloud_account(&account, &current))
            && fingerprint == crate::managed_cloud_parity::Snapshot::collect().fingerprint();
        if result.is_err() || !unchanged {
            cloud.invalidate();
        }
        if !unchanged {
            return Err(error(
                "Jcode account or cloud configuration changed while connecting; reconnect explicitly".into(),
            ));
        }
        // Reserve the channel now, never again when the ready client is consumed.
        let prepare = result
            .as_ref()
            .ok()
            .and_then(|_| cloud.begin_prepare(WeakSharedSshTransport::upgrade, Instant::now()));
        drop(cloud);
        if let Some((generation, owner)) = prepare {
            let state = Arc::clone(&self.cloud);
            std::thread::spawn(move || {
                // No cloud lock during the API handshake. Foreground clicks can
                // select another channel without waiting for this preparation.
                let result = owner.connect();
                let account = crate::managed_cloud::account();
                let fingerprint = crate::managed_cloud_parity::Snapshot::collect().fingerprint();
                let mut cloud = state.lock().unwrap();
                cloud.select_account(account.as_ref().ok(), fingerprint);
                let published = cloud.finish_prepare(generation, result.ok(), Instant::now());
                drop(cloud);
                if published {
                    let weak = Arc::downgrade(&state);
                    drop(state);
                    // The timer owns no client or strong cache reference.
                    std::thread::spawn(move || {
                        std::thread::sleep(SPARE_TTL);
                        if let Some(state) = weak.upgrade() {
                            state
                                .lock()
                                .unwrap()
                                .expire_spare(generation, Instant::now());
                        }
                    });
                }
            });
        }
        // Do not retry failed API handshakes, which may be ambiguous.
        result
    }
}

fn same_cloud_account(
    a: &crate::managed_cloud::Account,
    b: &crate::managed_cloud::Account,
) -> bool {
    a.api_base == b.api_base && a.api_key == b.api_key
}

// Exact account matching avoids hash collisions. Never derive Debug for this
// state, which contains credentials, and never include credentials in errors.
struct CloudTransport<W, S = JcodeClient> {
    account: Option<crate::managed_cloud::Account>,
    fingerprint: u64,
    owner: Option<W>,
    reservations: usize,
    generation: u64,
    preparing: bool,
    spare: Option<(S, Instant)>,
}

impl<W, S> Default for CloudTransport<W, S> {
    fn default() -> Self {
        Self {
            account: None,
            fingerprint: 0,
            owner: None,
            reservations: 0,
            generation: 0,
            preparing: false,
            spare: None,
        }
    }
}

impl<W, S> CloudTransport<W, S> {
    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.preparing = false;
        self.spare = None;
        self.owner = None;
        self.reservations = 0;
    }

    fn select_account(
        &mut self,
        account: Option<&crate::managed_cloud::Account>,
        fingerprint: u64,
    ) {
        let unchanged = self
            .account
            .as_ref()
            .zip(account)
            .is_some_and(|(old, new)| same_cloud_account(old, new))
            && self.fingerprint == fingerprint;
        if !unchanged {
            self.invalidate();
            self.fingerprint = fingerprint;
            self.account = account.map(|a| crate::managed_cloud::Account {
                api_key: a.api_key.clone(),
                api_base: a.api_base.clone(),
            });
        }
    }

    fn reserve<T>(&mut self, upgrade: impl FnOnce(&W) -> Option<T>) -> Option<T> {
        if self.reservations < CHANNELS_PER_MASTER
            && let Some(owner) = self.owner.as_ref().and_then(upgrade)
        {
            self.reservations += 1;
            return Some(owner);
        }
        self.owner = None;
        None
    }

    fn take_spare(&mut self, now: Instant) -> Option<S> {
        self.spare
            .take()
            .and_then(|(client, deadline)| (now < deadline).then_some(client))
    }

    fn begin_prepare<T>(
        &mut self,
        upgrade: impl FnOnce(&W) -> Option<T>,
        now: Instant,
    ) -> Option<(u64, T)> {
        if self
            .spare
            .as_ref()
            .is_some_and(|(_, deadline)| now >= *deadline)
        {
            self.spare = None;
        }
        if self.preparing || self.spare.is_some() {
            return None;
        }
        let owner = self.reserve(upgrade)?;
        self.generation = self.generation.wrapping_add(1);
        self.preparing = true;
        Some((self.generation, owner))
    }

    fn finish_prepare(&mut self, generation: u64, client: Option<S>, now: Instant) -> bool {
        if generation != self.generation || !self.preparing {
            return false;
        }
        self.preparing = false;
        self.spare = client.map(|client| (client, now + SPARE_TTL));
        self.spare.is_some()
    }

    fn expire_spare(&mut self, generation: u64, now: Instant) {
        if generation == self.generation
            && self
                .spare
                .as_ref()
                .is_some_and(|(_, deadline)| now >= *deadline)
        {
            self.spare = None;
        }
    }

    fn publish(&mut self, owner: W) {
        self.invalidate();
        self.owner = Some(owner);
        self.reservations = 1;
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
    let child = crate::platform::no_console_window(&mut Command::new("ssh"))
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
        jcode_desktop_model::platform_dirs::home_dir().map(Into::into),
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
            Some('d') => output
                .push_str(&jcode_desktop_model::platform_dirs::home_string().unwrap_or_default()),
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
