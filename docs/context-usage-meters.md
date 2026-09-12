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
  the welcome composer geometry. Narrow windows wrap the meters.

## Verification

The real offline app was built and inspected at 1440×1000 and 800×600 using
`scripts/screenshot.py --transcript tokens`. Both show Context 37%, a 5-hour
quota at 25%, and an unobstructed composer. Artifacts: `target/ui-review.png`
and `target/context-status-narrow.png`.

Tests cover provider-specific cache accounting, non-cumulative request updates,
OAuth/API-key selection, active multi-login quota isolation, ambiguous reports,
and meters actually painting and changing after a credential switch.

The SDK/API bridge now preserves the optional cache-creation token counter.
Older bridges remain compatible but omit that counter. Full Anthropic cache-write
accounting therefore takes effect after the normal runtime/bridge update. The
existing shared bridge was not interrupted to activate this additive change.
