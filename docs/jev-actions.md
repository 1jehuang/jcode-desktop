# Jev voice actions in Jcode Desktop

This is the complete **Desktop voice-routing** action catalog, based on the
shared `jcode-base::voice_intent` classifier and Desktop's voice dispatch code.
Jev chooses an outcome. Desktop performs the action. Jev does not generate shell
commands, file paths, or arbitrary tool arguments through this interface.

## Available actions

| Action | Decision ID | What happens | Example utterance |
| --- | --- | --- | --- |
| Send to the coding agent | `coding_agent` | Sends or queues the transcribed utterance for reasoning, coding, questions, discussion, or ordinary dictation. Existing composer text and attachments are not submitted with it. | "Find why the tests fail and fix it." |
| Create a conversation | `new_session` | Creates a new empty Jcode conversation through Desktop's normal new-panel action. | "Start a new conversation." |
| Next conversation | `next_session` | Focuses the next eligible open conversation in the sidebar's top-to-bottom strip order. Skips utility and closing panels. Does not wrap at the end. | "Go to the next session." |
| Previous conversation | `previous_session` | Focuses the previous eligible open conversation in that same order. Skips utility and closing panels. Does not wrap at the beginning. | "Go to the previous session." |
| Open an existing conversation | `candidate_0` through `candidate_19`, for offered candidates only | Opens or resumes the selected existing conversation. The selected candidate maps locally to its exact session ID. | "Open the terminal rendering conversation." |
| Keep the transcript in the draft | `uncertain` | Does not submit or navigate. Appends the transcription to the composer and shows that Jev could not choose a supported interpretation. | An unsupported or unclear request, when `uncertain` scores highest. |

The example utterances illustrate intent, not hard-coded commands or guaranteed
model responses. There are **three quick actions**, one agent route, and one
open-session action per offered conversation. `uncertain` is a no-action outcome.

## How the winner is selected

- The highest-scoring concrete outcome wins. There is no minimum confidence,
  competing-score ceiling, or required margin between first and second place.
- Concrete outcomes are `coding_agent`, `uncertain`, the three quick actions,
  and each offered `candidate_N`.
- `navigation` and `quick_action` are also scored and visible in the decision
  report. They describe action families, not executable actions, and do not
  impose confidence gates on the winner.
- `uncertain` remains an explicit competing outcome, not a fallback triggered
  merely because a score is low.
- Ties use a stable order: `uncertain`, `coding_agent`, `new_session`,
  `next_session`, `previous_session`, then candidates in numeric caller order.
  Only a strictly higher score replaces the current winner. All-zero scores
  therefore select `uncertain`.
- Scores still must be valid finite probabilities from 0 to 1. All requested
  answers must be present and typed correctly before selection runs.

For example, `new_session = 0.45` wins when every other concrete outcome is
below 0.45, even if the family score `quick_action` is low. A higher
`uncertain` score instead retains the draft.

## Existing-conversation scope

Desktop offers at most the **20 most recent non-archived conversations**. Pending
session placeholders and duplicate session IDs are excluded. This is recency
order, not the sidebar's saved-first ordering. `candidate_0` is the newest offered
conversation. A request for the most recent conversation is different from
"previous session", which follows currently open panel order.

Jev receives candidate titles and working-directory metadata. Actual session IDs
stay local. It cannot invent a destination, search older history through this
route, or treat a working-directory string as a command or navigation path.

## Interpretation and execution safeguards

The classification instructions distinguish pure UI commands from work for the
agent. Mixed navigation and coding requests, how-to questions, quoted commands,
hypothetical commands, and negated commands are described as agent input.
Unsupported actions, multiple immediate commands, and unmatched or ambiguous
existing-conversation requests are described as uncertain. These instructions
shape scores, rather than overriding the highest-score selection afterward.

Removing confidence thresholds does not remove execution checks:

- Empty input produces `uncertain` without calling Jev.
- Invalid input, malformed responses, and authentication or transport failures
  do not cause an action. Desktop retains the transcription in the draft.
- Canceled or superseded voice requests cannot submit or navigate later.
- A closed recording owner or visible account sign-in flow prevents navigation.
- An open-session result must still belong to the offered session snapshot.
- An unavailable action, including no adjacent conversation, retains the draft.
- Existing composer drafts remain separate from agent submissions and navigation.

## Not separate Jev voice actions

- `Dictation` exists as a legacy enum variant, but the classifier no longer emits
  it. Ordinary dictation routes to the coding agent. Draft retention uses
  `uncertain` or error handling.
- Starting/stopping/canceling recording are Desktop controls, not Jev decisions.
- Opening a terminal, changing settings, deleting/archiving sessions, opening
  arbitrary files or URLs, and running commands are not direct voice actions in
  this catalog. Work routed to the coding agent is handled by that agent's own
  tools and authorization rules.
- Jev's browser automation and memory integrations are separate interfaces, not
  additional Desktop voice commands.

## Source of truth

- Shared action types, question IDs, validation, and selection:
  [`voice_intent.rs`](../../jcode/crates/jcode-base/src/voice_intent.rs).
- Desktop candidate collection, navigation, and quick-action execution:
  [`workspace_voice.rs`](../crates/jcode-desktop-ui/src/workspace_voice.rs).
- Agent submission, draft retention, and canceled-request protection:
  [`panel_voice.rs`](../crates/jcode-desktop-ui/src/panel_voice.rs).

Keep this catalog synchronized when adding a `QuickAction`, changing
`VoiceIntent`, or adding a Desktop voice dispatch branch.
