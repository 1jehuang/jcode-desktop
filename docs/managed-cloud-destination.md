# Managed Jcode Cloud destination

## Contract

The Desktop Cloud button means Jcode-operated infrastructure authorized by the
user's Jcode account and subscription. It must not mean a personal AWS profile,
local `aws login`, a configured SSH alias, or the personal `jcode-cloud-alpha`
helper. Cloud compute/build entitlement alone does not establish interactive VM
availability.

## Current availability

As of September 21, 2026, the managed interactive host control plane is not
implemented in the inspected backend. `solosystems-backend/workers/api/src/worker.js`
exposes compilation and compute usage, not interactive host provisioning,
connection, or lifecycle endpoints. `jcode/docs/JCODE_CLOUD_AWS.md` describes the
planned architecture and explicitly says its managed customer control plane is
not deployed.

Desktop therefore fails closed for the managed destination. It preserves the
pending draft and queued prompts, explains unavailability, and offers the Jcode
account page for account/subscription management. The account link is not a claim
that sign-in or payment will provision a host. No local session, personal AWS
helper, or SSH connection is a permissible fallback for managed Cloud.

The legacy personal alpha remains separate implementation code for explicitly
personal infrastructure. An existing live session must not be moved to a different
machine or destroyed just because the destination for new sessions changes.

## Work required to enable managed interactive sessions

- Add authenticated host lifecycle APIs to the Jcode-operated backend.
- Enforce active subscription entitlement and runtime/storage allowances server-side.
- Provision account-isolated hosts using server-side infrastructure permissions.
- Implement a scoped, revocable interactive session transport, with account/host
  authorization on every connection and reconnection.
- Supply Desktop a real capability and connection contract. Never invent a success
  response, reuse a compile job as a session, or trust a client-side plan flag.
- Verify entitlement denial, cross-account isolation, exhausted allowance,
  cancellation, reconnect, draft preservation, and absence of local AWS access.
- Deploy and exercise the complete managed workflow before advertising availability.
