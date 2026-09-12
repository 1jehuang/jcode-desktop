# Context and credential usage meters

The session footer shows context-window occupancy and the rolling limits reported
for the current provider and authentication method. Hover a meter for token counts
or the limit's reset time. Colors turn amber at 70% consumed and red at 90%.

- Context occupancy uses Jcode's shared prompt-token accounting. OpenAI cached
  input is not counted twice. Anthropic cache reads and writes count toward the
  prompt, and generated output is not added to the last request's occupancy.
- Capacity comes from Jcode's shared model catalog and is identified as an
  estimate in the tooltip. Unknown capacity or missing usage is not shown as 0%.
- Limits use the existing background account feed. OAuth and API-key credentials
  remain separate. Multi-login reports select the CLI's active-login marker, or
  the sole report. Ambiguous reports are unavailable, never added together.
- Local account limits are not presented as remote-host limits.
- Before runtime identity arrives, no extra footer group is inserted, preserving
  the welcome composer geometry. The footer stays one fixed-height row. Meter
  names truncate on narrow windows while percentages and bars remain readable.

## Verification

The real offline app was built and inspected at 1440×1000 and 640×480 using
`scripts/screenshot.py --transcript tokens`. The context fixture is 37% and the
account feed reports 25% consumed. Missing account data remains explicitly
unavailable until the feed arrives. Artifacts: `target/context-fixed-row.png`
and `target/context-fixed-row-narrow.png`.

Tests cover provider-specific cache accounting, non-cumulative request updates,
OAuth/API-key selection, active multi-login quota isolation, ambiguous reports,
and meters actually painting and changing after a credential switch. Final
focused checks passed, including fixed-height status layout, narrow-window meter
geometry, the existing identity-footer geometry test, and all 12 account tests.
The broader suite also exposed unrelated in-progress startup/scroll tests.

The SDK/API bridge now preserves the optional cache-creation token counter.
Older bridges remain compatible but omit that counter. Full Anthropic cache-write
accounting therefore takes effect after the normal runtime/bridge update. The
existing shared bridge was not interrupted to activate this additive change.
