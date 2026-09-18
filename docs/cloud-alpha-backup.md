# Personal cloud alpha backup and recovery drill

This is an **explicit, user-managed file export**, not a VM image backup, managed backup service, snapshot, or SLA. There is no scheduling, replication, retention policy, automatic deletion, or live-VM restore. Keep your own verified copies on storage you control. Losing the VM before making a copy can mean losing your work.

## Scope and prerequisites

Run `scripts/cloud_alpha/backup.py` on your **local Linux machine** with Python 3.11+ and OpenSSH. The remote Linux account needs Python 3.11+. Configure an SSH host alias, existing authentication, and a previously verified host key yourself. The script uses batch authentication and strict host-key checking. It never provisions infrastructure, invokes AWS, resets credentials, or accepts arbitrary remote paths or SSH flags. Use your SSH config for ports, identity files, and jump hosts. IPv6 literals are not accepted directly, so use an alias.

Only these two remote roots are exported:

- `~/workspaces`
- `~/.jcode/sessions`

Both must already exist as real directories. On an untouched VM with no sessions directory, export fails closed rather than silently claiming to back up session history. Initialize the expected directories deliberately before retrying. **The entire `~/.jcode` directory is never backed up.** Other settings, databases, provider authentication, VM configuration, software installations, and credentials must be independently re-created. This is a **filtered export, NOT a full repository backup**. A complete checkout or complete history is not guaranteed: auth-named source modules and other matching legitimate files are omitted. `.git` is excluded, including local-only commits and Git metadata. Push important commits separately.

Before exporting, save your files and preferably leave a clean working tree. Stop active writers and let sessions finish. Reads are not an atomic snapshot: per-file changes cause failure when detected, but a multi-file consistent point in time is not guaranteed. Do not run this against an actively modified or adversarial source tree.

## Make a local copy

From the repository root, on your local machine:

```sh
mkdir -m 700 "$HOME/alpha-backups"
python3 scripts/cloud_alpha/backup.py backup \
  --host my-alpha \
  --backup-dir "$HOME/alpha-backups"
```

Use an already-private existing directory if the directory already exists. The script refuses group/world permissions or symlinks in its directory path. It creates a unique `backup-TIMESTAMP-UUID/` bundle containing:

- `archive.tar`: uncompressed USTAR, mode 0600
- `manifest.json`: SHA-256, archive bytes, payload bytes, member count, format version, mode 0600

Bundle directories are mode 0700. No existing file is overwritten. The default maximum is **1 GiB (1,073,741,824 bytes)** for the archive and recovered payload, with a maximum of 100,000 entries. Tar headers/padding count toward the archive limit. Remote accounting reserves padding conservatively, so an export near the limit may be refused. Use an explicit `--max-bytes NUMBER` on both `backup` and `verify` if you deliberately need a different limit. The total SSH operation defaults to 300 seconds, configurable with `--timeout SECONDS`. Limits apply per attempt, not to the accumulated directory size.

The source script is sent through SSH stdin to Python without installing anything on the VM. Tar streams to the local private bundle. Size overflow, a missing root, read failure, detected changing file, SSH timeout/nonzero exit, or archive validation failure means failure. A syntactically complete tar stream is **not** success when SSH exits unsuccessfully. A success manifest is written only after SSH success and validation.

Failed bundles and partial recovery directories are retained, never automatically deleted. Do not count a bundle without a manifest as a backup. A retained partial tar may contain private workspace/session material. Keep it private and decide yourself whether to remove it. The script does not print remote stderr, to avoid inadvertently echoing sensitive source paths/content.

## Privacy boundary

The same restrictive path-component policy is applied at every depth during export and verification. It excludes `.env*`, `env` components and variants, environment/credential/secret names, auth/OAuth/token/password/keyring names, `id_rsa`/`id_ed25519` and related key names, PEM/key/P12/PFX/keystore files, and common SSH/cloud/provider credential directories. `.config`, `.claude`, `.codex`, `.gemini`, `.copilot`, `.aws`, `.azure`, `.ssh`, `.gnupg`, `.kube`, `.docker`, `.netrc`, `.npmrc`, and `.pypirc` are excluded. Git metadata, dependency directories, virtual environments, and Python caches are also excluded. Exclusions are intentionally conservative and can omit legitimate code or documentation with matching names. There is no bypass flag.

A streaming content check additionally refuses recognizable private-key headers, common provider/access-token forms, bearer tokens, and common secret assignments. This is **defense in depth, not a complete secret detector**. Arbitrarily named or encoded secrets, opaque archives, and secrets pasted into session conversations cannot reliably be identified. Review both source roots first, keep credentials outside them, and do not paste credentials into sessions. Session history and source code remain sensitive even without credentials. No encryption at rest is provided. Use a trusted local disk with appropriate encryption and keep any secondary copies private. A same-user process or root can modify your private files, so do not concurrently modify bundles during verification or recovery.

## Verify and rehearse local recovery

Use the exact bundle path printed after a successful backup:

```sh
python3 scripts/cloud_alpha/backup.py verify \
  "$HOME/alpha-backups/backup-TIMESTAMP-UUID"

mkdir -m 700 "$HOME/alpha-recovery-drills"
python3 scripts/cloud_alpha/backup.py verify \
  "$HOME/alpha-backups/backup-TIMESTAMP-UUID" \
  --restore-to "$HOME/alpha-recovery-drills/drill-001"
```

The recovery parent must already be private and owned by you. `drill-001` **must not exist**, even as an empty directory. The resulting layout is `drill-001/workspaces/...` and `drill-001/.jcode/sessions/...`. Verification is offline and does not connect to SSH, AWS, or the live VM. It validates the full archive and manifest before creating the recovery destination. It then copies only ordinary files/directories with new 0600/0700 permissions, ignoring archived owners, executable bits, and other privileges. It never invokes `tar.extractall` or writes into live source roots. Recovery does not preserve executable bits, extended attributes, ACLs, ownership, or timestamps. Review and reinstate executable permissions manually if needed.

Archive validation rejects traversal, absolute paths, backslashes, control characters, paths outside the two roots, credential-excluded members, symlinks, hardlinks, devices, FIFOs, sparse/extended tar records, duplicate members, file/directory conflicts, missing roots, truncation, missing end markers, and trailing nonzero data. Export also refuses non-excluded symlinks, hard-linked files, and special files instead of following or silently dropping them. Long paths that cannot be represented in plain USTAR fail closed.

Compare a few important recovered source files and session histories with what you expected. Repeat the drill after copying a bundle to secondary storage. SHA-256 detects accidental corruption, **not malicious replacement of both archive and manifest**. Retain trusted copies accordingly. Any later transfer of reviewed files to a new VM is a separate, deliberate operation. This tool never overwrites a live VM.

## Offline tests

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s scripts/cloud_alpha -p test_backup.py -v
```

Tests import the real script, construct synthetic malicious archives, invoke the real exporter on isolated local fixture homes, and substitute the SSH boundary with local child processes. They cover recovery contents and permissions, no-overwrite behavior, checksums, privacy exclusions, source links/special files, traversal and archive types, cap enforcement, SSH nonzero exits, timeout, malformed streams, and retained incomplete bundles. They do not contact AWS or live SSH. A real configured SSH export remains an explicit operator acceptance check.
