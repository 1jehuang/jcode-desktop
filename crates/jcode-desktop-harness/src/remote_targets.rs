//! Local-only SSH target discovery. Never runs ssh, a shell, or Match exec.
//!
//! Discovery is deliberately best effort: bounded regular-file reads, include
//! depth, directory entries, and target counts. Call it off the UI thread since
//! even local filesystem metadata may be slow on network-mounted home folders.

use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Read,
    net::Ipv6Addr,
    path::{Component, Path, PathBuf},
};

const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 64;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_HOSTS: usize = 512;

/// Validate a single SSH destination, not an SSH command or URL.
///
/// Surrounding spaces are trimmed. Control characters, non-ASCII characters,
/// options, whitespace within the target, and shell metacharacters are rejected.
/// Supports aliases, DNS names, IPv4, IPv6 (including brackets and scope IDs),
/// and an optional `user@` prefix. Returned spelling and case are preserved.
pub fn validate_host(value: &str) -> Result<String, String> {
    if value.chars().any(char::is_control) {
        return Err("SSH host must not contain control characters".into());
    }
    let value = value.trim();
    if value.is_empty() {
        return Err("Enter an SSH host or alias".into());
    }
    if value.len() > 1024 || !value.is_ascii() || value.bytes().any(|c| c.is_ascii_whitespace()) {
        return Err("SSH host must be a single ASCII destination".into());
    }
    let host = if let Some((user, host)) = value.split_once('@') {
        if !safe_name(user) || host.contains('@') {
            return Err("Invalid SSH username".into());
        }
        host
    } else {
        value
    };
    if host.contains(':') || host.starts_with('[') {
        let address = if host.starts_with('[') {
            host.strip_prefix('[')
                .and_then(|h| h.strip_suffix(']'))
                .ok_or_else(|| "Invalid bracketed IPv6 address".to_string())?
        } else {
            host
        };
        let ip = if let Some((ip, scope)) = address.split_once('%') {
            if !safe_name(scope) {
                return Err("Invalid IPv6 scope ID".into());
            }
            ip
        } else {
            address
        };
        if ip.parse::<Ipv6Addr>().is_err() {
            return Err("Invalid IPv6 address".into());
        }
    } else if !safe_name(host) {
        return Err(
            "Use an SSH alias, hostname, or IP address, without options or shell characters".into(),
        );
    }
    Ok(value.into())
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

/// Discover literal Host aliases from ~/.ssh/config, preserving discovery order.
/// Missing/unreadable files and unsupported Include patterns are ignored.
/// Relative Include paths resolve against ~/.ssh, as in OpenSSH. `~/`, `*`,
/// and `?` are supported, but not named-user expansion or bracket glob classes.
pub fn discover_hosts() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return Vec::new();
    };
    discover_hosts_at(&PathBuf::from(home))
}

fn discover_hosts_at(home: &Path) -> Vec<String> {
    let mut discovery = Discovery {
        home,
        ssh_dir: home.join(".ssh"),
        visited: HashSet::new(),
        files_left: MAX_FILES,
        hosts: Vec::new(),
        bytes_left: MAX_BYTES,
        entries_left: MAX_ENTRIES,
    };
    discovery.read(&home.join(".ssh/config"), 0);
    discovery.hosts
}

struct Discovery<'a> {
    home: &'a Path,
    ssh_dir: PathBuf,
    visited: HashSet<PathBuf>,
    files_left: usize,
    hosts: Vec<String>,
    bytes_left: usize,
    entries_left: usize,
}

impl Discovery<'_> {
    fn read(&mut self, path: &Path, depth: usize) {
        if depth > MAX_DEPTH
            || self.files_left == 0
            || self.bytes_left == 0
            || self.hosts.len() >= MAX_HOSTS
        {
            return;
        }
        self.files_left -= 1;
        let Ok(path) = fs::canonicalize(path) else {
            return;
        };
        if !self.visited.insert(path.clone()) {
            return;
        }
        // Reject devices, directories, and FIFOs before opening. O_NONBLOCK also
        // protects against a FIFO swapped in between metadata and open on Unix.
        if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            return;
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
        }
        let Ok(file) = options.open(&path) else {
            return;
        };
        if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
            return;
        }
        let mut bytes = Vec::new();
        if file
            .take(self.bytes_left as u64)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return;
        }
        self.bytes_left = self.bytes_left.saturating_sub(bytes.len());
        // A budget cutoff in the middle of a line must not invent a partial alias.
        if self.bytes_left == 0 && bytes.last() != Some(&b'\n') {
            bytes.truncate(
                bytes
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map_or(0, |i| i + 1),
            );
        }
        let Ok(text) = String::from_utf8(bytes) else {
            return;
        };
        for line in text.lines() {
            let Some(words) = config_words(line) else {
                continue;
            };
            let Some(key) = words.first() else { continue };
            if key.eq_ignore_ascii_case("host") {
                for word in &words[1..] {
                    if self.hosts.len() >= MAX_HOSTS {
                        return;
                    }
                    if let Ok(host) = validate_host(word) {
                        if !self.hosts.contains(&host) {
                            self.hosts.push(host);
                        }
                    }
                }
            } else if key.eq_ignore_ascii_case("include") {
                for pattern in &words[1..] {
                    if depth >= MAX_DEPTH || self.files_left == 0 || self.bytes_left == 0 {
                        break;
                    }
                    for path in self.expand(pattern) {
                        self.read(&path, depth + 1);
                    }
                }
            }
        }
    }

    fn expand(&mut self, pattern: &str) -> Vec<PathBuf> {
        let path = if let Some(relative) = pattern.strip_prefix("~/") {
            self.home.join(relative)
        } else if pattern.starts_with('~') || pattern.contains(['[', ']']) {
            return Vec::new();
        } else {
            self.ssh_dir.join(pattern)
        };
        let mut paths = vec![PathBuf::new()];
        // Bound both wildcard expansion and pathological path component counts.
        if path.components().count() > 64 {
            return Vec::new();
        }
        for component in path.components() {
            let Component::Normal(name) = component else {
                for path in &mut paths {
                    path.push(component.as_os_str());
                }
                continue;
            };
            let name = name.to_string_lossy();
            if !name.contains(['*', '?']) {
                for path in &mut paths {
                    path.push(name.as_ref());
                }
                continue;
            }
            let mut matches = Vec::new();
            for base in &paths {
                let Ok(entries) = fs::read_dir(base) else {
                    continue;
                };
                for entry in entries {
                    if self.entries_left == 0 {
                        break;
                    }
                    self.entries_left -= 1;
                    let Ok(entry) = entry else { continue };
                    if glob_matches(
                        name.as_bytes(),
                        entry.file_name().to_string_lossy().as_bytes(),
                    ) {
                        matches.push(entry.path());
                    }
                }
            }
            matches.sort();
            paths = matches;
        }
        paths
    }
}

// Wildcard matcher with backtracking only to the latest star.
fn glob_matches(pattern: &[u8], value: &[u8]) -> bool {
    let (mut p, mut v, mut star, mut retry) = (0, 0, None, 0);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            retry = v;
        } else if let Some(index) = star {
            retry += 1;
            v = retry;
            p = index + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

// SSH config quoting, comments, and optional key=value syntax, without expansion.
fn config_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quote, mut escaped) = (None, false);
    for c in line.chars() {
        if escaped {
            word.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '#' {
            break;
        } else if c.is_whitespace() || (c == '=' && words.len() <= 1) {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_destinations_and_preserves_spelling() {
        for host in [
            "desktop",
            "MY_alias-2",
            "example.org",
            "192.168.1.4",
            "user@host",
            "::1",
            "2001:db8::1",
            "user@[::1]",
            "[fe80::1%eth0]",
            "user@fe80::1%en0",
        ] {
            assert_eq!(validate_host(&format!("  {host}  ")).unwrap(), host);
        }
    }

    #[test]
    fn rejects_options_urls_and_shell_syntax() {
        for host in [
            "",
            "  ",
            "-Fconfig",
            "user@-host",
            "-user@host",
            "a b",
            "host\n",
            "\thost",
            "h\0ost",
            "a@b@c",
            "@host",
            "user@",
            "ssh://host",
            "host:22",
            "[host]",
            "[::1]:22",
            "::g",
            "fe80::1%",
            "host;id",
            "host&",
            "host|id",
            "$(id)",
            "`id`",
            "host/dir",
            "host\\dir",
            "host*",
            "!host",
            "éxample",
            "host'",
            "host\"",
        ] {
            assert!(validate_host(host).is_err(), "accepted {host:?}");
        }
    }

    #[test]
    fn discovers_aliases_and_bounded_recursive_includes_without_commands() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        fs::create_dir_all(ssh.join("conf.d")).unwrap();
        fs::write(ssh.join("config"), "# aliases\nHost desktop *.wild !excluded desktop other\nHostName not-an-alias\nInclude conf.d/*.conf\nInclude ~/extra.conf\nMatch exec \"touch should-not-exist\"\nHost=last\n").unwrap();
        fs::write(
            ssh.join("conf.d/a.conf"),
            "hOsT \"quoted\" 'second' # comment\nInclude config\n",
        )
        .unwrap();
        fs::write(
            ssh.join("conf.d/b.conf"),
            "Host other third\nHost * ?bad !negated\n",
        )
        .unwrap();
        fs::write(home.path().join("extra.conf"), "Host user@remote [::1]\n").unwrap();
        assert_eq!(
            discover_hosts_at(home.path()),
            [
                "desktop",
                "other",
                "quoted",
                "second",
                "third",
                "user@remote",
                "[::1]",
                "last"
            ]
        );
        assert!(!home.path().join("should-not-exist").exists());
    }

    #[test]
    fn ignores_missing_invalid_and_nonregular_files() {
        let home = tempfile::tempdir().unwrap();
        assert!(discover_hosts_at(home.path()).is_empty());
        fs::create_dir_all(home.path().join(".ssh/config")).unwrap();
        assert!(discover_hosts_at(home.path()).is_empty());
        fs::remove_dir(home.path().join(".ssh/config")).unwrap();
        fs::write(
            home.path().join(".ssh/config"),
            b"Host valid\nHost \"unterminated\nInclude missing\n",
        )
        .unwrap();
        assert_eq!(discover_hosts_at(home.path()), ["valid"]);
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_skipped_without_blocking() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".ssh")).unwrap();
        let path = home.path().join(".ssh/config");
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(discover_hosts_at(home.path()).is_empty());
    }

    #[test]
    fn discovery_caps_hosts_and_include_depth() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(ssh.join("config"), "Include level0\n").unwrap();
        for index in 0..20 {
            fs::write(
                ssh.join(format!("level{index}")),
                format!("Host host{index}\nInclude level{}\n", index + 1),
            )
            .unwrap();
        }
        assert_eq!(discover_hosts_at(home.path()).len(), MAX_DEPTH);
        let hosts = (0..MAX_HOSTS + 20)
            .map(|i| format!("Host host{i}\n"))
            .collect::<String>();
        fs::write(ssh.join("config"), hosts).unwrap();
        assert_eq!(discover_hosts_at(home.path()).len(), MAX_HOSTS);
    }

    #[test]
    fn glob_and_tokenization_are_literal_and_predictable() {
        assert!(glob_matches(b"a*?.conf", b"abc.conf"));
        assert!(!glob_matches(b"a?.conf", b"abc.conf"));
        assert_eq!(
            config_words("Host = one two # ignored").unwrap(),
            ["Host", "one", "two"]
        );
        assert_eq!(
            config_words("Include \"conf dir/*.conf\"").unwrap(),
            ["Include", "conf dir/*.conf"]
        );
    }

    #[test]
    fn discovery_caps_file_attempts_and_never_emits_a_truncated_alias() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let mut config = String::new();
        for index in 0..MAX_FILES + 10 {
            config.push_str(&format!("Include host{index}\n"));
            fs::write(
                ssh.join(format!("host{index}")),
                format!("Host alias{index}\n"),
            )
            .unwrap();
        }
        fs::write(ssh.join("config"), config).unwrap();
        assert_eq!(discover_hosts_at(home.path()).len(), MAX_FILES - 1);

        let mut config = "Host complete\n".to_string();
        config.push_str(&"#".repeat(MAX_BYTES - config.len() - 9));
        config.push_str("\nHost partial\n");
        fs::write(ssh.join("config"), config).unwrap();
        assert_eq!(discover_hosts_at(home.path()), ["complete"]);
    }
}
