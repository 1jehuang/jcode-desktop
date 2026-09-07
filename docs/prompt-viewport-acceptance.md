# Viewport-aware numbered prompts: acceptance evidence

Verified 2026-09-06 with:

```sh
cargo test -p jcode-desktop-ui panel::prompt::tests -- --nocapture
```

Result: **6 passed, 0 failed**, including the actual Panel renderer, virtualized
transcript, painted caption bounds, and native GPUI scroll-event handlers.

The new `native_scroll_paints_historical_numbered_prompt_cards` test builds 13
turns, each with a distinct prompt and 40 response paragraphs. It does not set
ListState offsets directly or call the prompt-selection helper to simulate
scrolling. It dispatches pixel scroll events over the transcript hitbox and
asserts the caption elements that actually painted:

| Requirement | Executed observation |
| --- | --- |
| Number cards using `you, 13` | At the live end, the exact `you, 13` caption paints inside the pinned card bounds. |
| Show the prompt for the historical view | Upward scroll events replace the painted pinned `you, 13` with `you, 12`. The newer caption is absent. |
| Do not jump to a newly arrived prompt while reading history | Appending prompt 14 leaves `you, 12` painted and does not paint `you, 14`. |
| Number original transcript cards too | Scrolling upward and downward paints original `you, 1` and `you, 14` captions. |
| Avoid duplicate pinned cards when the prompt is visible | At both tested transcript boundaries, the numbered original card paints and the pinned card is absent. |
| Preserve existing behavior | Existing tests pass for partial prompt visibility, new visible prompts, resize, pinned-todo ordering, and three historical turns. |

The caption debug selector is derived from the same SharedString passed to the
rendered label, rather than from independently calculated test metadata.

This provides executed UI evidence beyond the earlier internal prompt-index
assertions and static screenshot. A failed boundary-test attempt used one
unrealistically large scroll delta. The final test uses repeated normal-sized
pixel events and waits for the original caption, not its pinned copy.

Scope: offline GPUI acceptance with the real rendering and event-handling paths.
It does not claim user confirmation or universal full-suite health. The earlier
full-suite run had failures outside these focused prompt tests.
