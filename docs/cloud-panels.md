# Jcode Cloud panels

Status: product contract with a personal alpha, 2026-09-18. A single-user AWS
alpha now reuses Desktop's native SSH panels. Managed subscription provisioning
is not implemented. See the [alpha runbook](../scripts/cloud_alpha/README.md)
and [$20 economics](cloud-economics.md) for actual versus proposed capabilities.
This document does not announce customer availability.

## Product

A paid Jcode subscriber can choose **New cloud panel** and work in a normal
native Jcode panel whose agent, tools, processes, and files run on a managed VM.
No AWS account, SSH configuration, or infrastructure knowledge is required.
Local, self-hosted SSH, and managed cloud panels can coexist.

The initial allocation is **one persistent cloud workspace/VM per subscriber**.
Panels are independent Jcode sessions on that VM, not separate billable VMs.
They share the VM's resource limits. A new panel does not create another host.
Separate project checkouts or worktrees prevent concurrent agents from modifying
the same checkout unintentionally. Strong isolation between projects belonging
to the same subscriber is not implied by a separate panel.

Cloud is an optional subscription capability, not a replacement for local use.
AWS promotional credits subsidize the service, but are not a reason to promise
unlimited compute. Included VM-hours, storage, concurrency, and any overage
pricing must be measured and approved before customer launch.

## Desktop experience

1. Add **Jcode Cloud** alongside **This computer** and self-hosted SSH machines.
   It can become the default destination for new agent panels. Existing panels
   retain their destination, and explicit local-folder actions stay local.
2. A signed-out user is offered Jcode sign-in. A user without cloud entitlement
   sees subscription/account management instead of a provisioning attempt.
3. A subscriber selects or clones a repository in the cloud workspace. Local
   folders are not silently uploaded. Repository authorization/import is explicit.
4. The new panel shows provisioning or waking progress, then becomes the normal
   chat panel. Its destination label is **Cloud**, not an SSH hostname.
5. Reloading Desktop or reconnecting attaches to the same server session. Drafts
   survive connection errors. Retrying a launch cannot create duplicate sessions
   or duplicate hosts.
6. Closing a panel closes its view. It does not delete the workspace or
   automatically cancel an agent. Expose **Stop agent**, **Sleep cloud**, and
   **Delete cloud workspace** as distinct actions with appropriate confirmation.
7. Show host state and remaining compute allowance. When a limit is reached,
   explain why the host cannot run and preserve both the workspace and drafts.

Do not offer a functional-looking cloud launch button until the backend and
transport can fulfill it. An unavailable service must never fall back to local
execution or treat a cloud identifier as an SSH host.

## Runtime and persistence

- One isolated VM per subscriber initially, with encrypted persistent storage.
- Sessions and files survive a host stop/start cycle. Agent processes do not:
  reconnecting to saved history must not be described as resuming a suspended
  process. Interrupted work is visibly marked and requires safe restart handling.
- Agents may continue when Desktop disconnects, within the subscriber's limits.
- Stop after 30 minutes of genuine inactivity. An agent turn, tool subprocess,
  or explicitly budgeted background job is activity. A connected idle panel is
  not activity. A detached process cannot extend the lease indefinitely.
- Enforce a separate maximum runtime lease and hard compute allowance even when
  activity continues. Graceful cancellation precedes forced host stop.
- Stopped storage still costs money. Include storage quotas and a published
  retention/export/deletion policy, including after subscription cancellation.
- No automatic import of local provider credentials, AWS credentials, SSH keys,
  environment files, or arbitrary home-directory contents.

## Control-plane authority

Use the existing Jcode account identity and subscription backend. A backend
migration is not a prerequisite for cloud panels. The Rust client's paid-plan
check is useful for display, but is not an authorization boundary.

The service must authenticate every operation, derive the owner from the token,
validate cloud entitlement server-side, enforce owner-scoped session access, and
reserve quota before provisioning. A forged desktop tier or account ID must not
be able to launch or attach to a host. Subscription changes must invalidate or
bound existing host leases, not merely hide the new-panel button.

Retain the planned `/v1/cloud` lifecycle contract from the shared Jcode design:
status, activate, wake, stop, device grants, explicit transfer, and deletion.
Create/wake and session creation need durable idempotency scoped to the owner.
Concurrent requests for the same subscriber converge on the same host.

The backend returns an opaque host identity and a short-lived, revocable
connection grant. The SDK handles the authenticated managed transport, while
Desktop owns panel rendering and interactions. Do not expose AWS credentials to
clients or use the root CLI profile as a runtime credential. Hosts have no
public inbound application or SSH ports. Operator access uses SSM.

The managed transport must carry the existing harness session protocol, enforce
account/host/session scope, and support reconnect without replaying accepted
prompts. Document token expiry, refresh, and cancellation behavior before client
integration. Long-lived bearer credentials must not appear in panel IDs, URLs,
logs, workspace files, or screenshots.

## Existing implementation seams

Desktop already provides native SSH-backed panels, mixed local/remote sessions,
persistent remote identity, and machine defaults:

- `crates/jcode-desktop-ui/src/workspace_remotes.rs`: machine selection and launch.
- `crates/jcode-desktop-ui/src/remote.rs`: persisted remote session addressing.
- `crates/jcode-desktop-ui/src/harness.rs`: per-panel SDK connections and events.
- `crates/jcode-desktop-ui/src/harness_remote_tests.rs` and
  `workspace_remote_tests.rs`: remote panel regression coverage.
- [Desktop remote-machine behavior](desktop-guide.md#remote-machines).

The adjacent Jcode repository owns shared SDK transports, harness protocol,
account authentication, and `crates/jcode-base/src/subscription_api.rs`.
Its `docs/JCODE_CLOUD_AWS.md` contains the earlier infrastructure direction.
Extend these shared interfaces rather than creating a Desktop-only runtime.

The shared design identifies `solosystems-backend` as the subscription service
owner. That repository was not found in the local checkouts inspected for this
plan. Its deployed entitlement and identity contracts still need inspection.
Do not infer deployed cloud APIs from a design document.

## Rollout and acceptance gates

### 1. Personal alpha

Verify the target development account before provisioning. The personal alpha
uses the existing root-authenticated browser login for workstation control, with
scoped instance and guard roles and no credential transfer. A separate non-root
operator login is outstanding and required before shared/customer operation.
Older cloud-host notes refer to another AWS account, so their host IDs and
guardrails are not proof that this account has a deployment.

Create infrastructure through reviewed IaC. Start with one allowlisted user,
one host, a small explicit spend cap, no automatic overage, and a tested stop
switch. Do not provision a customer fleet or change subscription pricing during
this milestone.

Acceptance:

- Create one host, open two distinct agent sessions, and verify both execute on
  that host rather than the laptop.
- Disconnect and reconnect without losing history or duplicating accepted work.
- Stop and wake the host, verifying persisted files and truthful interrupted-job
  state. Test idle shutdown and the independent runtime cap.
- Verify the host cannot access operator credentials, another user's data, or
  unrestricted control-plane operations.
- Verify actual cost attribution and storage charges. Document alpha limits.

### 2. Subscription service

Implement cloud entitlement and quota reservation in the account backend,
idempotent lifecycle jobs, scoped connection grants, usage reconciliation, and
revocation. Deploy with a closed allowlist before enabling self-service.

Acceptance:

- Signed-out, inactive, revoked, unknown-tier, and quota-exhausted requests fail
  closed. Another user's host/session is inaccessible.
- Concurrent activation and retry after timeout create at most one host.
- Failed provisioning releases reservations and surfaces an actionable error.
- Duplicate and out-of-order billing events cannot restore obsolete entitlement.
- The usage limit and global stop switch work independently of Desktop uptime.

### 3. Native cloud panels

Add a managed target to the shared SDK and persisted target representation, then
integrate it into Desktop's machine selector and new-panel flow. Keep SSH IDs
backward compatible. Do not encode tokens into target identifiers.

Acceptance:

- Real end-to-end sign-in, entitlement, wake/create, session creation, streamed
  response, tool execution, reconnect, and stop on the allowlisted service.
- Cloud and local panels coexist without cross-target commands or file access.
- Failed wake/retry preserves drafts without silently executing locally.
- Reload/restart restores the correct account, host, and session. Switching
  accounts cannot attach to the previous account's workspace.
- Visual checks use the isolated screenshot harness. Significant Desktop changes
  are verified and delivered with the app's Ctrl+R rebuild-and-reload action.

### 4. Customer beta

Publish measured allowances and retention policy. Complete billing reconciliation,
restore/export/deletion tests, tenancy threat review, support procedures, and
explicit overage consent before opening provisioning to all subscribers.
