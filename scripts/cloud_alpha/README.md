# Jcode Cloud personal alpha

This is a **single-user test deployment**, not a launched subscription backend.
The $20/month recommendation and proposed customer limits are in
[cloud-economics.md](../../docs/cloud-economics.md).

## Test from this workstation

After setup, use:

```sh
jcode-cloud-alpha status
jcode-cloud-alpha wake
# Desktop: Machines → jcode-cloud-alpha → Connect
jcode-cloud-alpha stop
```

Use the existing native **Machines** panel. Reopen it if it was already open
before SSH configuration was installed. Local panels are unaffected. The alpha
uses the existing SSH transport, so its label is SSH, not the future managed
Cloud label. For a terminal, `jcode-cloud-alpha ssh` wakes and connects.
Repositories belong under `~/workspaces` on the VM. No local checkout, model
credential, AWS credential, or SSH private key is copied to the VM.

Closing a panel does not delete the disk or promise that an agent has stopped.
`stop` is explicit and ends running processes. Stop/start preserves files and
saved conversation history, **not process memory**. Use `wake` before reconnecting
an asleep machine. This alpha does not implement seamless managed wake yet.

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
