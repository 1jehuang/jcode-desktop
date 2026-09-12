# Slash and model picker

Type `/` in the composer to browse all registered commands. Arrow keys move the
high-contrast selected row, which has a trailing checkmark. The right-hand
scrollbar shows the position in overflowing lists. Mouse wheel and touchpad
scrolling work independently of the keyboard selection. Filtering narrows the
list without dropping commands behind an arbitrary result limit.

`/model` uses the same menu with two-line rows:

- Model identity and provider logo.
- **Tracked turns** and **Last used**, followed by provider/authentication route.
- **Current** identifies the session's active model, independently of the row
  highlighted for selection.

Usage is supplied by the shared Jcode runtime, not inferred from token totals or
locally counted picker clicks. One successful agent turn counts once. Historical
TUI picker choices are displayed separately as **prior selections**, with their
selection time, until tracked-turn data exists. They are not relabeled as turns.
A model without history says **No recorded usage yet**. Older daemons that omit
usage metadata show **Usage history unavailable**, rather than claiming the
model has never been used.

The shared desktop/TUI priority order is tracked-turn count, last-use recency,
prior-selection count, then last-selection recency. Names break remaining ties.
Filtering preserves that order. A runtime usage update retains the highlighted
model even if the model's position changes, so Enter does not unexpectedly choose
a different model.

Tracking persists across restarts, and its start time is carried in the SDK's
`ModelUsage` metadata. Older sessions do not retain reliable per-turn serving-model
history, so counts are not presented as lifetime totals. A daemon built before
model-usage tracking must be safely reloaded before it can supply these fields.

## Verification

```sh
cargo test -p jcode-desktop-ui --lib input:: -- --test-threads=1
python3 scripts/screenshot.py target/slash-review.png --transcript empty --slash-interact
python3 scripts/screenshot.py target/model-review.png --transcript empty --model-interact
```

These captures run the real app with offline fixture data on a private Xvfb
display, without opening windows on the user's desktop. Use fresh output paths.
