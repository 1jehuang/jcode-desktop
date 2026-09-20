# Jcode Cloud personal alpha

This is a **single-user test deployment**, not a launched subscription backend.
The $20/month recommendation and proposed customer limits are in
[cloud-economics.md](../../docs/cloud-economics.md).

### Hosting and billing boundary

The intended hosted product authenticates customers with their **Jcode account**,
maps each user to a shared-per-user VM in **Jcode's AWS hosting account**, and keeps
AWS credentials and VM lifecycle management on the service side. AWS bills the
hosting account owner, who owns connection latency and operating costs. Customers
should not need an AWS account, AWS CLI, or local SSH setup.

**That managed access path is not implemented by this personal alpha.** Today the
workstation's configured AWS CLI profile and SSH identity access the operator's
AWS account. AWS charges go to that account. Jcode sign-in does not yet provision
or authorize a customer's VM, and no customer cloud subscription charge or
entitlement is enforced here. Multiple panels already share the configured VM,
but this does not establish multi-user isolation or a customer hosting service.

## Test from this workstation

After setup, use:

```sh
jcode-cloud-alpha status
jcode-cloud-alpha wake
# Desktop: Machines → jcode-cloud-alpha → Connect
jcode-cloud-alpha stop
```

Use the existing native **Machines** panel. Reopen it if it was already open
before SSH configuration was installed. **Connect** immediately opens an editable
panel, then checks readiness using the fixed local
`~/.local/bin/jcode-cloud-alpha wake` helper before creating the remote session.
Choose **Set default** for `jcode-cloud-alpha`
to use this path for new session panels. All cloud panels share the one configured
VM, with a separate Jcode session per panel, not a new VM per panel.

While connecting, the panel shows **Jcode Cloud VM**, a cloud watermark, and live
sign-in, guard, VM, SSM, and SSH progress. Its checklist shows **AWS access**,
**Runtime guard and allowance**, **Shared VM running**, **Private SSH connection**,
and **Jcode session**, with completed, active, waiting, and failed states based on
observed progress, never a timer. Enter queues a prompt until connection
and history initialization finish. The same panel/editor is promoted when ready.
Failures stay beside the preserved draft, with **Retry connection** and an explicit
**Use this computer** option. Retry and reload retain the draft's original remote
destination even if the default machine changes. There is no implicit local
fallback. Explicit local-folder actions and existing panels are unaffected.
The connected alpha still uses the existing SSH transport and SSH identity label,
not a managed multi-user provisioning service. For a terminal,
`jcode-cloud-alpha ssh` wakes and connects.

After a successful readiness check, another panel within 30 seconds can reuse
that result instead of repeating AWS guard queries and the bootstrap probe. The
checklist calls these steps **Recently verified**, not newly checked. Reuse does
not extend the cache deadline or the VM's lease. Connection failures, unavailable
or unsafe status, changed helper/configuration files, and expiry invalidate reuse.
Requests waiting on the same successful wake share this short-lived result.
Each panel still creates its own remote API session. A running VM is not itself
proof of a working connection, and this optimization does not guarantee instant
attachment or bypass the independent shutdown guards.

Panel appearance and session readiness are different measurements. A pending
editor should appear immediately, while a usable session requires an independent
API attachment and loaded history. The warm-session target is hundreds of
milliseconds when readiness is already verified and a live transport can be
reused. It is not a cold-boot guarantee or a promise to skip expired safety checks.
On Unix, Desktop reuses a private authenticated SSH master while panels use it,
but opens a separate remote API connection for each panel. The pool is scoped to
the current Desktop bridge, not a persistent global SSH configuration. Changed
SSH/cloud identity, transport failure, or a conservative eight-connection shard
limit causes a future connection to use a new master. Other platforms retain the
isolated SSH path. Closing the last owning panel releases its master, allowing
idle shutdown. No user-configured SSH master is adopted or terminated.

This removes repeated SSH/SSM authentication from eligible warm connections. It
does not eliminate network round trips, expired readiness checks, or cold-boot
latency, and it does not automatically retry ambiguous session creation.
A [fresh end-to-end recheck](../../docs/cloud-session-latency.md) measured 389 ms
for an immediate repeat but 5.587 seconds after a 35-second pause, with the same
VM and SSH master still connected. An already-running VM alone is therefore not
a guarantee of subsecond new sessions.

Repositories belong under `~/workspaces` on the VM. No local checkout, AWS
credential, or SSH private key is copied to the VM.

### Personal model synchronization

For this explicitly authorized **personal alpha only**, successful `wake`, `ssh`,
and Desktop cloud-session creation synchronize allowlisted local model credentials
and settings before spawning the session. This is not a default credential-export
policy for arbitrary SSH machines or a multi-user hosted service.

- Jcode-owned OpenAI, Claude, and Gemini model OAuth stores are projected to their
  provider-specific fields. Gmail's `google_oauth.json`, Slides, Jcode account
  login tokens, AWS credentials, GitHub/Hugging Face account tokens, SSH keys,
  external-tool stores, unrelated `.env` entries, hooks, and integration secrets
  are excluded. No complete local configuration file or environment is copied.
- Built-in model API keys use the explicit `API_KEYS` allowlist in `model_sync.py`.
  Provider/model defaults, reasoning, transport, service-tier, model-picker, and
  swarm-model/effort settings are projected separately. Recognized local model
  environment overrides take precedence over saved settings. Session-only model
  selections are not persistent defaults and are not inferred from transcripts.
- Named/custom provider profiles fail closed rather than exporting arbitrary
  headers, environment names, or files. Broad account-backed providers and
  external-tool discovery are intentionally unsupported. This is not universal
  parity for every possible local provider.
- Secrets travel only in encrypted, pinned-host SSH **stdin**, never argv, logs,
  progress output, or a local plaintext snapshot. Local deduplication stores only
  a target-bound digest and timestamp. A 30-second cache avoids an additional
  AWS/SSH handshake for unchanged warm panels. Changed credentials/settings,
  SSH configuration, host pin, target, or sync implementation invalidate it.
- Remote writes are bounded and atomic, mode `0600`, with symlink and special-file
  rejection. Unrelated remote configuration is preserved semantically, although
  TOML comments/formatting are not retained. Local credential removal does not
  revoke or delete remote credentials.
- **OAuth is never overwritten automatically.** Initial publication uses an
  atomic no-replace operation because native OAuth refresh writers do not share
  the synchronizer's lock. An equal existing store can be adopted. Unchanged local
  input preserves remotely rotated tokens. A differing existing store or changed
  local OAuth account fails closed and requires explicit account reconciliation.
  API-key/config updates are atomic, but manual concurrent edits are not a
  multi-file transaction. No process or VM is restarted by model synchronization.
- A running native daemon is refreshed with secret-free `notify_auth_changed`
  messages on a dedicated no-prompt connection. Completion waits for the native
  catalog-completion notification, not just its enqueue acknowledgement. Existing
  sessions retain their selected models. Fresh daemons read the native stores.

The alpha's older Jcode v0.84 runtime does not implement all current settings,
notably Gemini forced-OAuth/project configuration and Anthropic one-hour caching.
Copying settings cannot add unsupported runtime behavior. Do not assume Gemini
billing/auth-route parity on that version when both OAuth and an API key exist.
Provider-side OAuth refresh-token rotation can also invalidate another device's
copy independently of synchronization.

Use `jcode-cloud-alpha sync-models` to explicitly recheck the snapshot without
waking or restarting the VM. A sync error blocks new cloud-session creation and
does not fall back to a local session. The receiver carries the local Python
standard-library TOML parser in memory for the alpha's Python 3.9, requiring no
remote package installation. Local helper execution requires Python 3.11+.

Closing a panel does not delete the disk or promise that an agent has stopped.
`stop` is explicit and ends running processes. Stop/start preserves files and
saved conversation history, **not process memory**. New cloud panels wake
automatically, but reconnecting an existing panel to an asleep machine may
still require `wake`. This is personal-alpha integration, not managed
subscription provisioning or credential-free customer access.

Machines shows the last checked monthly allowance and an estimated two-hour
cutoff. The sidebar's Machines control warns with ten minutes or less left on
the lease or the monthly allowance, even when the management panel is closed.
These are advisory displays, not a renewed lease or guaranteed usable runtime.
AWS status is refreshed about once a minute. Expired AWS login still requires
`aws login --profile jcode-personal --region us-east-1` on the workstation.

For an explicit, bounded local working-file/session export and an offline
recovery drill, see [backup and recovery](../../docs/cloud-alpha-backup.md).
This filtered export is not a complete repository or VM backup.
See [access setup](../../docs/cloud-alpha-access.md) for the remaining non-root
operator and coding-model prerequisites. These have not been provisioned by
the Desktop lifecycle integration.

## Actual alpha resources and limits

- One x86-64 Linux `m7i.large`, 2 vCPU / 8 GiB RAM, in `us-east-1`.
- One encrypted 30 GiB gp3 root/workspace volume. OS and caches count against it.
- No security-group ingress, including SSH. SSH is end-to-end encrypted over an
  authenticated SSM tunnel, with a dedicated local key and a pinned host key.
- An ephemeral public IPv4 provides outbound access. It is not retained while
  stopped. No NAT gateway or Elastic IP is provisioned.
- The instance role permits SSM management and **only Amazon Nova Micro** model
  inference in this region. It cannot provision machines, manage IAM, or invoke
  arbitrary more expensive models. Model listing is allowed for UI discovery,
  but the list does not imply invocation permission.
- Nova Micro is an inexpensive smoke-test model, not a promise of frontier-model
  coding quality or included paid inference in a customer subscription.
- An independent EventBridge/Lambda guard checks every minute. It requests stop
  after **two continuous hours** or **50 conservative machine-hours per UTC
  calendar month**. This alpha is not synchronized to a customer's billing cycle.
- A root-owned local systemd timer also powers off two hours after boot. It is a
  second safety layer, not a replacement for the independent guard.
- **The two-hour lease stops even active work.** This is a deliberately bounded
  personal alpha, not the graceful/renewable lease proposed for a customer plan.
- Host-idle checking uses a 30-minute grace, but conservatively refuses shutdown
  while any user work, SSH session, live Jcode daemon, or uncertain task state
  remains. Current unattached session listings are not authoritative. The
  two-hour lease remains the cost backstop for these cases.
- Usage ledger writes are transactional and version-conditional. Overlap/errors
  fail closed by requesting a stop. New accounts may have only ten Lambda
  concurrent executions, so this template does not reserve concurrency.
- Usage accounting accumulates conservative elapsed seconds, including uncertain
  gaps and stop transitions. It does not round every 60.1-second interval up to
  two minutes. Displayed minutes round the cumulative total up. This is an
  alpha safety allowance, not a customer billing ledger.

**This is not a hard dollar cap.** EventBridge/Lambda/API outages and EC2 stop
latency can exceed runtime thresholds. Storage continues billing while stopped.
Model inference, internet transfer, snapshots, and logs are separate charges.
No customer bandwidth cap, model-spend quota, backup schedule, two-agent limit,
or subscription entitlement enforcement is implemented by this alpha. Do not
open it to customers until those controls exist.

## Operator identity

The local `jcode-personal` browser-login profile currently authenticates as AWS
root. AWS refuses GetFederationToken from these temporary login credentials, so
no pretend scoped session is installed. **Root credentials never leave this
workstation**. CloudFormation bootstraps scoped instance and guard roles, and
SSM authenticates the workstation using its existing login.

A separate non-root operator login is still required before routine shared or
customer operation. There are no new long-lived IAM access keys. The local
helper deliberately supports a configurable profile so it can move to that
operator identity without changing the host or copying credentials.

The helper checks the recorded account and host ownership tag. Wake fails closed
when the independent guard is disabled, its heartbeat is stale, or the current
month's ledger is absent/exhausted. Local `~/.config/jcode/cloud-alpha.json`
contains only resource identifiers and a profile name, not credentials.

## Reproduce or review

The infrastructure source is `template.py`, incorporating `bootstrap.sh`,
`guard.py`, and `idle.py`. Generate a CloudFormation template with a dedicated
OpenSSH ed25519 **public** key:

```sh
python3 -m unittest discover -s scripts/cloud_alpha -p 'test_*.py' -v
python3 scripts/cloud_economics.py --test
python3 scripts/cloud_alpha/template.py --public-key ~/.ssh/jcode_cloud_alpha.pub > "$JCODE_SCRATCH_DIR/cloud-alpha-stack.json"
aws cloudformation validate-template --template-body "file://$JCODE_SCRATCH_DIR/cloud-alpha-stack.json" --profile jcode-personal --region us-east-1
```

Inspect the target account, VPC, subnet, and AL2023 x86-64 AMI before creating a
change set. Supply `VpcId`, `SubnetId`, `ImageId`, and `CAPABILITY_IAM`, then review
all changes before executing. Do not create a second stack to repair a failed
host. Inspect CloudFormation and cloud-init first. Resource names/IDs from old
cloud notes may refer to a different AWS account.

The runtime is pinned to published Jcode v0.84.0, SHA-256 verified during
bootstrap. If that runtime lacks a required SDK capability, update the pinned
release through the same reviewed template, not an unverified download.
The workstation's Session Manager plugin comes from AWS's official HTTPS Linux
package and is installed under `~/.local/bin`, without sudo. Local SSH config
uses `StrictHostKeyChecking yes` and a separate known-hosts file. Obtain the host
public key through authenticated SSM Run Command, not unauthenticated keyscan.

Never commit local resource configuration, SSH private keys, AWS caches, or
provider credentials. Existing desktop source and staged work are unrelated and
must not be included in the alpha commit.

## Operations and recovery

- Bootstrap log: `/var/log/jcode-alpha-bootstrap.log` on the host, readable via
  SSM Run Command. Success marker: `/opt/jcode-alpha/ready`.
- Read-only idle inspection: `sudo python3 /opt/jcode-alpha/idle.py --check`.
- Guard log: `/aws/lambda/jcode-cloud-alpha-guard`, seven-day retention.
- If wake refuses because the guard is stale, fix/invoke the guard and inspect
  its result. Do not bypass it by manually starting a host.
- To test the independent stop path, temporarily reduce the guard's boot lease
  through an explicit temporary operator test, observe an actual stop, then restore
  the configured two-hour limit before allowing normal wake.
- Monthly counters are retained. Do not delete or reset usage to bypass limits.
- The stack has termination protection, the host has API termination protection,
  the stack policy denies host replacement/deletion, and both instance and ledger
  have CloudFormation retention policies. The disk
  has `DeleteOnTermination=false`. **Deleting the stack will not clean up these
  retained resources or their costs.** Stop first, export/snapshot deliberately,
  and get explicit approval for any permanent data deletion.
- A stopped 30 GiB disk costs approximately $2.40/month before snapshots or other
  charges. An unused retained workspace is not free.

## Before customer launch

Implement account-owned entitlements, idempotent activation/wake, revocable
managed connection grants, automatic Desktop wake/progress, accurate metering,
concurrency and transfer limits, model billing, tested backups/export/deletion,
non-root operator access, telemetry/alerts, and a published retention policy.
The proposed $20 offer must not silently replace existing subscription benefits.
