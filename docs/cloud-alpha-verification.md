# Personal cloud alpha verification

Date: 2026-09-18 UTC. Result: **single-user alpha is running and usable through
Desktop's existing native SSH machine support**. This is not a launched
subscription control plane or public customer service.

## Observed acceptance evidence

| Requirement | Check and observed result |
| --- | --- |
| One predictable test VM | CloudFormation created one `m7i.large` in `us-east-1`, with 30 GiB encrypted gp3. Existing west-region development VM was untouched. |
| Private connection | Dedicated SSH key, host public key fetched through authenticated SSM, strict host-key verification, no inbound security-group rules. Native harness connected successfully over the SSM SSH tunnel. |
| Multiple panels/sessions | Two separate harness connections created distinct sessions on the same host through the same `jcode api --stdio` interface used by Desktop. No second VM was created. |
| Real agent/tool execution | Nova Micro completed a real turn using `bash` and `read`, writing and reading `~/workspaces/cloud-alpha-smoke.txt` with `JCODE_CLOUD_ALPHA_OK`. This was not mocked. |
| Independent cost-safety stop | Temporarily reduced the deployed Lambda boot lease, invoked the actual deployed function, observed `stop_requested=true` with `boot_lease`, and verified the EC2 instance reached `stopped`. Restored the normal two-hour lease in `finally`. |
| Normal operator lifecycle | `jcode-cloud-alpha stop` and `wake` succeeded. Wake checked the actual SSH/bootstrap path, not only potentially stale SSM health. |
| Persistent files | After full stop/start, SSH independently read the original test file and checked its exact contents. |
| Persistent saved conversations | Two sessions with completed/saved conversation content successfully reattached after full stop/start. The model/tool conversation history included the test marker. |
| Idle-check conservatism | Live `idle.py --check` reported `idle=false`, `would_poweroff=false`, and a non-authoritative session listing while a daemon/SSH/user processes existed. It did not stop live work. |
| Runtime safety installed | Both root-owned systemd timers were active. The deployed idle script SHA-256 matched the committed source. The independent guard returned a valid current usage ledger. |
| Reproducible infrastructure | Final generated template matched the applied CloudFormation update. The persistent host was not replaced. Stack policy prohibits host replacement/deletion. |
| Unit economics | Official AWS Pricing API rates and 11 arithmetic tests support the documented $20 / 50-hour recommendation without promotional credits. Costs for shared services, payment processing, support and reserves are explicitly assumptions. |

Offline checks: **82 alpha tests + 11 economics tests passed**, Python compilation
and `bash -n` passed. These cover resource isolation, retained encrypted disk,
fail-closed wake, stale SSM readiness, transactional usage accounting, clock/month
boundaries, cumulative-second rounding, failures/overlap, idle-policy fail-safe
behavior, and economics arithmetic. They do not substitute for the live checks
above or prove production reliability.

## Iterations and unresolved alpha limits

- Initial reconnect testing tried to restore a completely untouched empty
  session. It had never been persisted. The final test uses saved conversations.
  **An empty server-side session is not promised to survive a reboot.**
- One additional context-seeding (`no_reply`) model run returned a Bedrock
  validation error about a missing `toolResult`. The field exists in the pinned
  release, so this is not claimed to be an unsupported-field diagnosis or a
  fixed runtime bug. Ordinary completed-turn/tool tests passed. Broader Nova
  conversation/parallel-tool robustness remains outside this acceptance result.
- Nova Micro is a low-cost test model, not a representative frontier coding
  quality benchmark. No customer model budget or provider billing is implemented.
- The two-hour maximum boot lease deliberately stops even active work. This is
  a personal-alpha spend backstop, not graceful customer session suspension.
- Automatic genuine-idle stopping cannot be proved from current unattached
  session listings. The conservative checker preserves a live daemon, so the
  independent maximum-runtime lease remains necessary.
- No production subscription entitlement, managed automatic wake UI, bandwidth
  cap, concurrency limiter, backup SLA, or customer billing was launched.
- Workstation control still uses the existing root-authenticated browser-login
  profile. Host and guard have scoped roles and receive no workstation secrets.
  Non-root operator login remains a prerequisite for shared/customer operation.
- No Desktop UI/runtime source was changed. The SSH alias is discovered by the
  existing Machines panel. Validation exercised the real remote harness rather
  than claiming a new rendered Cloud panel or account activation flow exists.

## Re-run

```sh
python3 -m unittest discover -s scripts/cloud_alpha -p 'test_*.py' -v
python3 scripts/cloud_economics.py --test
jcode-cloud-alpha wake
python3 scripts/cloud_alpha/smoke.py --model-call --output "$JCODE_SCRATCH_DIR/cloud-alpha-smoke.json"
# Only stop once all real user work is safely finished.
jcode-cloud-alpha stop
# Wait for stopped, then wake.
jcode-cloud-alpha wake
python3 scripts/cloud_alpha/smoke.py --verify-history --output "$JCODE_SCRATCH_DIR/cloud-alpha-smoke.json"
```

`--model-call` explicitly incurs a small Bedrock inference charge. Do not run
stop tests while another user or agent has real work on this personal host.
See the [runbook](../scripts/cloud_alpha/README.md) and
[economics](cloud-economics.md) for operation and launch boundaries.
