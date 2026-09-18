# Jcode Cloud: $20/month economics and proposed allowances

Status: planning recommendation, 2026-09-18 UTC. **Not a deployed entitlement,
price change, or provisioning authorization.** Existing subscriptions and model
subscriptions remain unchanged. This document owns economics only. See
[cloud-panels.md](cloud-panels.md) for the native-panel product contract.

## Recommendation

Start the personal alpha with **one `m7i.large` in `us-east-1`, 50 running
VM-hours per month, 30 GiB total gp3 storage, and BYOK inference**. The eventual
$20/month offer can use the same allowance, subject to measured shared costs and
quota enforcement. Multiple native panels share this VM and its allowance.

This is a **persistent workspace, not an always-running machine**. At full
allowance and continuous full CPU, modeled monthly cost is **$14.52**, leaving
**$5.48 / 27.4% contribution after the explicit reserves below**. Direct variable
AWS cost is $9.64. These figures assume no promotional credits, free tier,
Savings Plans, spot discount, or idle CPU-credit subsidy. The margin is modest,
not a claim of business profitability.

### Exact proposed base allowance

| Item | Recommendation |
| --- | --- |
| Price | $20/month gross subscription receipts, before separately collected sales tax |
| Hosts | One Linux x86-64 `m7i.large`, 2 vCPU, 8 GiB RAM, no GPU |
| Compute | 50 billable VM-hours per billing month, no rollover, no automatic paid overage |
| Panels/concurrency | Two concurrently active agent turns, sharing one host. Queue additional turns and limit heavy build/test jobs to one at a time |
| Disk | 30 GiB gp3 **total including OS/root, repositories, caches and swap**, not 30 GiB plus an unbudgeted root disk |
| gp3 performance | Included baseline only: 3,000 IOPS and 125 MiB/s. No extra provisioned IOPS/throughput |
| Snapshots | Up to 30 GB-month aggregate retained billable snapshot blocks. One current weekly checkpoint, not unlimited daily history |
| Internet egress | 5 GB/month, including remote transport and downloads from the VM to the client |
| Model inference | **$0 included paid-provider budget and zero included paid-model tokens. BYOK**, with separate provider charges |
| Idle sleep | Stop after 30 minutes without agent/tool/background-job activity. Connected idle panels do not keep it awake |
| Runtime safety | Four-hour renewable lease, bounded by the remaining monthly allowance. Warn before graceful stop, then enforce stop independently of Desktop |
| Limit behavior | No automatic upgrade, extra disk, second VM or paid overage. Warn at 80% and 95%, refuse new work and stop safely at exhaustion |
| Persistence | Keep the disk while subscription/alpha authorization is active, including after compute exhaustion. Processes do not survive stop/start |
| Cancellation proposal | Seven-day export grace, then explicit published deletion policy. Reserve covers this tail. Do not silently alter current customer retention |

Count AWS-billable instance runtime, including boot, reconnect, idle timeout and
background jobs, not just agent-token time. Account for EC2's minimum 60-second
charge at each start. A second panel must not allocate a second host. Preserve
files and interrupted-work state before stopping. Do not market 50 hours as
50 hours of uninterrupted foreground work plus free idle time.

The snapshot allowance is an economic bound, not an existing enforcement
feature. Incremental snapshots can retain old blocks: multiple snapshots of a
30 GiB disk can cost more than 30 GB-month. Rotation can transiently retain two
full checkpoints. Bound that overlap to one day (about $0.05 extra on a 30-day
month), charge it to the reserve, and fail closed on backup growth. Additional
restore-test volumes and logs also consume the reserve. Do not promise a backup
SLA until restore and retention behavior are tested.

## Official AWS price evidence

Fetched using **AWS CLI `pricing get-products`, profile `jcode-personal`, endpoint
region `us-east-1`**, on 2026-09-18 UTC. Product location is **US East (N. Virginia)**,
Linux, shared tenancy, no preinstalled commercial software, on-demand USD rates.
The current profile is root-authenticated. Only catalog/instance-offering reads
were performed for this analysis. Use a scoped non-root identity for routine
cloud operations. No infrastructure or existing west-region VM was changed.

EC2/storage/network catalog terms below have effective date 2026-09-01.
Internet data-transfer terms have effective date 2026-06-01. These are retrieved
catalog dates, not invented forward price guarantees. The refresh command below
preserves product SKUs, price dimensions, filters and effective dates.

| Instance | vCPU / RAM GiB | Arch | Base $/hour | At sustained full CPU $/hour | 50h full-load total cost¹ | Contribution¹ |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| t3.medium | 2 / 4 | x86-64 | 0.04160 | 0.12160 | $15.560 | 22.20% |
| t3.large | 2 / 8 | x86-64 | 0.08320 | 0.15320 | $17.140 | 14.30% |
| t3a.medium | 2 / 4 | x86-64 | 0.03760 | 0.11760 | $15.360 | 23.20% |
| t3a.large | 2 / 8 | x86-64 | 0.07520 | 0.14520 | $16.740 | 16.30% |
| m7a.medium | 1 / 4 | x86-64 | 0.05796 | 0.05796 | $12.378 | 38.11% |
| **m7i.large** | **2 / 8** | **x86-64** | **0.10080** | **0.10080** | **$14.520** | **27.40%** |
| c7i.large | 2 / 4 | x86-64 | 0.08925 | 0.08925 | $13.9425 | 30.29% |
| m7g.medium | 1 / 4 | ARM64 | 0.04080 | 0.04080 | $11.520 | 42.40% |
| m7g.large | 2 / 8 | ARM64 | 0.08160 | 0.08160 | $13.560 | 32.20% |
| c7g.medium | 1 / 2 | ARM64 | 0.03630 | 0.03630 | $11.295 | 43.52% |
| c7g.large | 2 / 4 | ARM64 | 0.07250 | 0.07250 | $13.105 | 34.48% |

¹ Includes all other full quotas, payment assumption, shared allocation and
reserves in the primary model. Decimal rounding can affect the last displayed
digit. Burstable rows use Unlimited with no starting CPU credits. Nonburstable
rows have no CPU-credit fee. A vCPU is not an equivalent physical core across
all families, so this table does not claim equal throughput.

`m7a.medium` **does exist** and was returned by both pricing and regional
`describe-instance-type-offerings`. It is not 2 vCPU / 8 GiB. The read also
confirmed m7i.large, c7i.large, m7g.medium, m7g.large and c7g.medium offerings in
this region, but not capacity or account launch authorization. `m7a.medium`
is an economical light-workload alternative, not the primary developer default.
The 4 GiB c7i.large saves only $0.5775 per 50h versus m7i.large and halves RAM.
Use the 8 GiB machine first to reduce memory pressure from compilers and agents.

### CPU-credit trap

Official [AWS credit concepts and baseline table](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/burstable-credits-baseline-concepts.html):

- t3/t3a.medium earns 24 credits/hour, equivalent to **0.4 sustained vCPU**
  across its two vCPUs, or 20% each.
- t3/t3a.large earns 36 credits/hour, equivalent to **0.6 sustained vCPU**, or
  30% each. These are not two unrestricted CPU cores at the headline price.
- Linux T3 and T3a surplus credits cost **$0.05/vCPU-hour**. In Unlimited mode,
  long-run full-load cost is `base + 2 × (1 - baseline) × $0.05`.
- Thus t3a.large is $0.0752 + $0.07 = **$0.1452/hour**, $7.26 for 50h, versus
  m7i.large's predictable $5.04. It costs **$2.22 more** at full load.
- At/under baseline, or in Standard mode without paid bursting, t3a.large is
  $3.76 compute / $13.24 all-in at 50h (33.8% contribution). But Standard
  throttles to baseline when credits run out. That saving buys weaker service.
- T3/T3a default to Unlimited. Explicitly select Standard if testing a
  credit-charge-free burstable option. Never infer the mode from instance type.
- Credits accrue while running, not while stopped. Earned credits survive stop
  for seven days, not forever. Do not assume every morning starts with a free
  full credit bucket. Unlimited surplus debt can be charged on stop/termination
  or mode change, so stopping is not a way to erase its cost.

The full-load table is a conservative steady-state comparison with zero initial
credit balance, not a prediction of AWS's rolling-credit line item at each
minute. **Recommendation: nonburstable m7i.large avoids both variable CPU fees
and credit starvation.** Promotional AWS credits and EC2 CPU credits are two
unrelated concepts, and neither is required by this recommendation.

### Storage and network catalog rates

| Charge | Official unit rate | Example SKU |
| --- | --- | --- |
| gp3 provisioned storage | $0.08/GB-month | JG3KUJMBRGHV3N8G |
| Standard EBS snapshots | $0.05/GB-month retained blocks | 7U7TWP44UP36AT3R |
| Extra gp3 IOPS | $0.005/IOPS-month | 7Q58NR58VQEASA4W |
| Extra gp3 throughput | $0.04/MiB/s-month (catalog $40.96/GiB/s-month) | SQUFRQX4K92S4SBB |
| Linux T3 / T3a surplus CPU | $0.05/vCPU-hour | DHXM9J4FT38HQAQ6 / ZA9GXD8U7NKKZHC8 |
| Public IPv4, in use or idle | $0.005/address-hour | 4GQUNXTFWVSGPUZK / T6YDQKTMVWKNJFJ8 |
| NAT Gateway | $0.045/hour + $0.045/GB processed | M2YSHUBETB3JX4M4 / 59S5R83GFPUAGVR5 |
| Internet egress, first paid 10 TB tier | $0.09/GB | HQEH3ZWJVT46JHRG |

AWS's [gp3 documentation](https://docs.aws.amazon.com/ebs/latest/userguide/general-purpose.html)
confirms the included 3,000 IOPS and 125 MiB/s performance.

Storage units follow AWS's billed capacity convention. A provisioned 30 GiB
EBS volume is modeled as 30 billing GB-month, not 30 multiplied by a decimal
conversion factor. No provisioned-storage saving is assumed for sparse files.
Stopped instances stop compute charges, **not disk or snapshot charges**.
A fully stopped workspace still costs **$3.90/month** for the full storage and
snapshot allowance. More generally, billing is prorated by resource duration.

### Network topology assumptions, not hidden free infrastructure

The alpha uses an **ephemeral public IPv4 for outbound traffic, no inbound
security-group rules, and SSH through SSM**. A stopped VM releases its ephemeral
address. The model charges $0.25 for 50 running address-hours. It does not
include a retained Elastic IP or NAT Gateway. Transport, SSM permissions and
outbound connectivity still need end-to-end validation. Public address does
not mean public SSH access.

An allocated Elastic IP retained all month costs **$3.65 at 730 hours**, including
idle hours. That would add $3.40 over the modeled $0.25 and reduce contribution
to 10.4%. A 31-day month is 744 hours, not 730.

If production requires private hosts behind a shared NAT Gateway, budget the
NAT **and its public IPv4**: `(0.045 + 0.005) × 730 = $36.50/month` before
traffic. Per-user incremental cost relative to this model is:

```
$36.50 / paying_users + $0.045 × total_NAT_processed_GB - $0.25
```

At 10 processed GB/user, this adds $36.70 with one user, $3.85 at ten users, or
$0.565 at 100 users. Those yield $51.22, $18.37 and $15.085 total user costs,
respectively. NAT processes downloads as well as uploads. A 5 GB internet
**egress** cap alone does not bound NAT fees. These examples assume same-AZ
routing, one NAT and no HA requirement. Multiple AZs, cross-AZ transfer,
interface endpoints, load balancers and relays need separate budgets. They
are not invisibly included in the direct AWS subtotal.

## Full-allowance $20 unit economics

Every included compute hour, storage unit, snapshot unit and egress byte is used.
Compute runs at 100% for all 50 hours. Disk and snapshots persist all month.

| Monthly component | Calculation | USD |
| --- | --- | ---: |
| m7i.large compute | 50 × $0.1008 | 5.04 |
| Ephemeral IPv4 | 50 × $0.005 | 0.25 |
| gp3 disk, including root | 30 × $0.08 | 2.40 |
| Retained snapshot blocks | 30 × $0.05 | 1.50 |
| Internet egress | 5 × $0.09 | 0.45 |
| **Direct variable AWS subtotal** | No free tiers/credits | **9.64** |
| Shared services allocation | **Assumption**, see below | 1.00 |
| Payment processing | **Assumed 2.9% + $0.30**, one $20 payment | 0.88 |
| Support allowance | **Assumed**, about two minutes/month at $60/hour loaded | 2.00 |
| Risk/cost reserve | **Assumed**, billing lag, stop overshoot, failed launches, backup overlap, retention tails/refunds | 1.00 |
| Included model inference | BYOK only | 0.00 |
| **Total modeled costs and reserves** | | **14.52** |
| **Contribution after reserves** | $20 - $14.52 | **5.48 (27.4%)** |

Payment fees are a planning assumption, **not a fetched or contracted processor
quote**. Cross-border cards, FX, billing-service fees and chargebacks may add
cost. $20 is not assumed tax-inclusive. Taxes collected for remittance are not
revenue. Tax-inclusive pricing or nonrecoverable AWS taxes need recomputation.
Engineering salaries, general corporate overhead and acquisition costs are not
covered by this contribution calculation. Two minutes of support is not a
support SLA and may be unrealistic for an early alpha. This model allocates the
entire proposed $20 revenue to the cloud offer. If cloud becomes a benefit of an
existing $20 subscription that already includes costly services or model usage,
subtract those costs too. Do not count the same revenue twice or remove existing
benefits to make these numbers work. The BYOK recommendation is for this
proposed cloud offer, not a change to existing subscription entitlements.

Seven extra days of full disk/snapshot retention cost about $0.91 on a 30-day
month, nearly consuming the $1 reserve by themselves. A cancellation plus
backup overlap, failed launches or chargebacks can exceed that reserve. Measure
these tails and increase the reserve or reduce allowance before broad release.

The $1 shared allocation is a **budget constraint, not a discovered AWS bill**.
For example, a $100/month shared control plane/logging/relay budget requires
100 paying users to reach $1/user. With ten paying users its allocation is $10,
raising all-in user cost to $23.52, a loss. With one user it is $100, producing
$113.52 cost including commercial reserves. Do not claim the per-user model
proves an independently operated one-user service costs under $20.

### Stress and sensitivity, not just average utilization

| m7i.large runtime | Other quotas fully consumed | Contribution on $20 |
| --- | ---: | ---: |
| 20h illustrative lighter use | $11.346 | 43.27% |
| **50h recommended maximum** | **$14.520** | **27.40%** |
| 100h | $19.810 | 0.95% |
| 730h always on, typical average month | $86.464 | Loss of $66.464 |
| 744h always on, 31-day month | $87.9452 | Loss of $67.9452 |

The fixed monthly modeled amount excluding compute/ephemeral IP is $9.23.
At a minimum **25% contribution target**, the math permits only
`($20 × 0.75 - $9.23) / ($0.1008 + $0.005) = 54.5369h`.
Choose 50h rather than rounding up. The $1 risk reserve is already included,
and there is only another **$0.48** of headroom before falling below 25%.
A $1 increase in unmodeled shared/support costs lowers margin by five percentage
points. A stricter 30% target requires at most 45.085h, so choose **45h** if that
is the business requirement. Do not advertise 100 hours or always-on at $20.

## Model usage: BYOK is not an included token budget

**Launch recommendation: BYOK with no included paid tokens.** The subscriber
supplies separately authorized provider credentials and pays that provider.
Do not copy local credentials automatically, repurpose an existing consumer
subscription, or imply any provider's consumer plan permits hosted/API use.
Provider terms and authentication methods must be checked independently.
Existing Jcode/customer subscriptions are unchanged by this proposal.

A hosted Bedrock model being available, including Nova Micro, does not make
inference free. No Bedrock price is assumed or verified in this calculation.
Separately metered hosted inference would require its own cost ledger, hard
spend authorization and explicit customer consent. It is not silently included
in $20 or deducted from the customer's existing provider subscription.

If later offering included inference, advertise an explicit **provider-dollar
budget or model-specific input/output allowances**, not one universal token
number. For prices `p_in` and `p_out` per million tokens, budget consumption is
`input_tokens × p_in / 1e6 + output_tokens × p_out / 1e6`, plus cache/tool/request
charges where applicable. Apply the cap to actual provider cost, reserving the
maximum possible completion before dispatch and reconciling after the request.
All agent panels, retries and background calls share it. Provider-specific
pricing must be fetched before promising quantities.

Example **budget**, not model pricing: adding $2 included inference to the
unchanged 50h plan yields **$16.52 cost, $3.48 contribution, 17.4% margin**.
To preserve 25% at $20, reduce compute to **35h** (total $14.933, margin 25.335%),
or raise the proposed plan price to at least about **$22.11** under the same
fee assumption. Neither alternative is an approved change. BYOK allows the
50h offer without depending on cheap models or a favorable input/output mix.

## ARM compatibility

Graviton is a real option, not a drop-in promise. The Jcode repository's release
workflow contains `aarch64-unknown-linux-gnu` CLI artifacts, and Desktop has an
ARM64 release target. The user's native Desktop need not run on the server's
CPU architecture because panels communicate through a protocol.

However, repository toolchains, native dependencies, downloaded executables,
container images and browser/test binaries must support Linux ARM64. An x86-only
project may fail or incur expensive emulation. Release workflow presence does
not prove the entire remote workflow has been tested on ARM. Validate harness,
PTY/tools, reconnect and representative builds on ARM before offering it.
`m7g.large` saves only **$0.96 per 50h** versus m7i.large, so use x86-64 for the
personal alpha and consider ARM as an explicit compatible-workload choice.
The 2 GiB c7g.medium is not a credible default for heavier development workloads.

## Personal alpha decision and scope

### Implementation status versus proposed commercial allowances

Coordinator-reported deployment verification, 2026-09-18 06:32 UTC. The
coordinator verified native SSH, two sessions, a real Nova Micro agent run using
bash/read tools, and a persisted test file. The independent Lambda lease stop
was actually exercised, then the normal two-hour lease was restored. These are
reported integration results, not independent verification by this economics
worker. They do not establish customer billing or monthly/network quota enforcement:

- The personal alpha uses one m7i.large with 30 GiB gp3 in us-east-1.
  The **50h monthly allowance is the planning target**, not evidence of an
  implemented customer entitlement or authoritative monthly billing ledger.
- Operator access is local login through SSM. The instance has a scoped role
  permitting a first-party Nova Micro smoke test, with no imported provider
  credentials. Test inference is **additional metered AWS usage**, not included
  subscription tokens and not covered by the calculator's zero model budget.
  Attribute the actual test charge separately. Its price/spend ceiling has not
  been verified here, so do not claim an exact inference dollar bound.
- The coordinator reports a **two-hour independent stop mechanism plus local
  lease guard** for the alpha. This differs deliberately from the proposed
  four-hour renewable commercial lease. The independent stop path is reported
  tested above. Local guard behavior still needs its own verification.
- Idle detection is conservative: a live daemon can prevent an idle stop because
  runtime listing is not authoritative evidence of inactivity. Do not assume
  the proposed 30-minute idle policy is currently effective. Active/idle runtime
  still consumes the economic allowance, and the independent lease is essential.
- Customer authentication/subscription billing and a **hard network allowance
  are not enforced yet**. The modeled 5 GB egress maximum is therefore a planning
  assumption, not a current cap. Snapshot retention and other proposed quotas
  also require separate verification before being described as enforced.

Consequently, **$9.64 direct AWS / $14.52 all-in are conditional full-allowance
models, not maximum possible alpha bills**. The $20 alpha envelope is a budget,
not an exact dollar cap. Actual alpha cost can exceed it, especially through
unenforced traffic, model calls, persistent resources or repeated runtime
leases. Commercial launch remains BYOK or separately authorized/billed hosted
inference. Nothing here changes existing customer subscriptions.

### Budget decision

Use the primary 50h/30GiB/5GB/BYOK limits as the target for one personal account,
not a public paid launch. Plan a **$20/month incremental AWS envelope**: $9.64
maximum modeled direct resources leaves $10.36 for real shared overhead,
monitoring and transient operations. This is a planning budget, not an AWS
Budgets hard shutdown guarantee. Enforced leases/resource quotas are needed,
and asynchronous billing alerts cannot guarantee an exact dollar stop.

Reuse already-funded shared services where appropriate, but report their
allocated cost separately. If a new $100/month control plane is required,
accept an explicit larger alpha budget or redesign before launching it.
No NAT Gateway, persistent EIP, paid included inference or additional host fits
silently into this alpha envelope. AWS promotional credits may reduce cash
paid during testing but must be tracked separately from undiscounted usage.
They are temporary subsidy, not recurring subscription margin.

Validate actual idle stop, allowance exhaustion, snapshot growth, reconnect,
billed start minimums and support demand before opening the offer to others.
This analysis did not provision anything, alter subscriptions, move the
existing west-region VM, or implement those controls.

## Reproduce and verify

Python 3.10+ standard library only. No dependencies or credentials needed for
local calculations/tests. The optional refresh uses only AWS Pricing reads:

```bash
python3 scripts/cloud_economics.py
python3 scripts/cloud_economics.py --test
python3 scripts/cloud_economics.py --refresh-prices \
  --output "$JCODE_SCRATCH_DIR/cloud-economics-official-prices.json"
```

Refresh explicitly uses profile `jcode-personal` and region `us-east-1` and saves
raw catalog terms. It **does not update pinned assumptions, subscriptions, or
resources**. Inspect SKU/product attributes before choosing dimensions: CPU
queries also return Windows prices, and data-transfer queries include unrelated
accelerated-transfer rates. The selected ordinary internet tier is $0.09/GB,
not the unrelated $0.02 AWS transfer dimension. No account-level free egress is
allocated to every subscriber.

Validated on 2026-09-18: 11 offline unit tests pass, the report reproduces the
primary/full-load/sensitivity totals, and all seven live catalog refresh queries
succeeded. Checks cover stopped storage, burstable full load/baseline/Standard,
nonburstable utilization independence, margin ceilings, model budget, NAT
sharing, absence of free-credit assumptions and invalid inputs. All 11 pinned
compute rates were also compared exactly against the fetched live catalog.
This validates the arithmetic and price retrieval, **not real deployed billing or quota
controls**. No app rebuild is required for these documentation/calculator-only
changes.
