//! Typo-tolerant, ranked search for the `/model` picker.
//!
//! Model ids mix separators and casing freely (`claude-opus-4-8`,
//! `Claude Opus 4.8`, `gpt-5.5`, `GPT5.5`), and people type them from memory.
//! Each query word is matched against a set of search fields in tiers, from
//! most to least literal, and the best tier wins:
//!
//! 1. literal word prefix or substring (`opus`, `4.8`)
//! 2. separator-insensitive substring (`gpt55`, `opus48`, `claudeopus`)
//! 3. ordered subsequence and small typos via the shared `jcode-fuzzy` matcher
//!    (`sonet`, `opsu`, `gtp`, `clade`)
//!
//! Every query word must match somewhere. Words may match different fields in
//! any order, so `anthropic opus`, `opus api`, and `4.8 opus` all work.

use std::collections::HashMap;

use super::model_menu::ModelDetails;

/// Prepared search text for one model route.
pub(super) struct SearchEntry {
    /// Lowercased fields: pretty name, raw id, route spec, provider, method.
    fields: Vec<String>,
    /// The same fields with every non-alphanumeric character removed.
    compact: Vec<String>,
    /// Whitespace-joined fields for the shared token matcher.
    joined: String,
}

impl SearchEntry {
    pub(super) fn new(spec: &str, details: Option<&ModelDetails>, group_label: &str) -> Self {
        let mut fields = Vec::with_capacity(6);
        let raw = details.map_or(spec, |detail| detail.model.as_str());
        fields.push(jcode_provider_core::model_names::pretty_picker_model_name(raw).to_lowercase());
        fields.push(raw.to_lowercase());
        if raw != spec {
            fields.push(spec.to_lowercase());
        }
        if let Some(detail) = details {
            if !detail.provider.is_empty() {
                fields.push(detail.provider.to_lowercase());
            }
            if !detail.api_method.is_empty() {
                fields.push(detail.api_method.to_lowercase());
            }
        }
        fields.push(group_label.to_lowercase());
        fields.retain(|field| !field.is_empty());
        fields.dedup();
        let compact = fields.iter().map(|field| compact(field)).collect();
        // The shared token matcher splits on whitespace. Present every field as
        // separator-split tokens as well as whole ids so `opus` can match the
        // `opus` token of `claude-opus-4-8` with a word-boundary bonus.
        let joined = fields
            .iter()
            .flat_map(|field| {
                std::iter::once(field.replace(char::is_whitespace, "-")).chain(
                    field
                        .split(|c: char| !c.is_alphanumeric() && c != '.')
                        .map(str::to_string),
                )
            })
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            fields,
            compact,
            joined,
        }
    }
}

fn compact(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// A query prepared once per keystroke and scored against every entry.
pub(super) struct ModelQuery {
    words: Vec<QueryWord>,
}

struct QueryWord {
    text: String,
    compact: String,
    fuzzy: jcode_fuzzy::PreparedTokenQuery,
}

/// Literal tiers always outrank fuzzy matches, whatever the fuzzy score.
const PREFIX_TIER: i32 = 30_000;
const SUBSTRING_TIER: i32 = 20_000;
const COMPACT_TIER: i32 = 10_000;

impl ModelQuery {
    pub(super) fn new(query: &str) -> Self {
        let words = query
            .split_whitespace()
            .map(|word| {
                let text = word.to_lowercase();
                QueryWord {
                    compact: compact(&text),
                    fuzzy: jcode_fuzzy::PreparedTokenQuery::new(&text),
                    text,
                }
            })
            .filter(|word| !word.text.is_empty())
            .collect();
        Self { words }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Higher is better. `None` means at least one query word did not match.
    pub(super) fn score(&self, entry: &SearchEntry) -> Option<i32> {
        self.word_scores(entry)
            .map(|scores| scores.iter().fold(0i32, |sum, s| sum.saturating_add(*s)))
    }

    /// Per-word scores, or `None` when any word fails to match.
    #[cfg_attr(not(test), allow(dead_code))]
    fn word_scores(&self, entry: &SearchEntry) -> Option<Vec<i32>> {
        self.words.iter().map(|word| word.score(entry)).collect()
    }
}

impl QueryWord {
    fn score(&self, entry: &SearchEntry) -> Option<i32> {
        let mut best: Option<i32> = None;
        let keep = |best: &mut Option<i32>, score: i32| {
            *best = Some(best.map_or(score, |b| b.max(score)));
        };
        for (index, field) in entry.fields.iter().enumerate() {
            // Earlier fields (the name people read) beat provider/method hits.
            let field_bonus = 50 - (index as i32 * 8).min(48);
            if field == &self.text {
                keep(&mut best, PREFIX_TIER + 2_000 + field_bonus);
                continue;
            }
            if let Some(at) = field.find(&self.text) {
                let at_boundary = at == 0
                    || field[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| !c.is_alphanumeric());
                // A whole-token hit (`gpt-5` in `gpt-5-mini`, not `gpt-5.5`)
                // beats a bare prefix. Otherwise equal hits tie, so the
                // caller's usage ranking decides.
                let end = at + self.text.len();
                let token_end = field[end..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric() && c != '.');
                let whole = if at_boundary && token_end { 1_000 } else { 0 };
                let base = if at_boundary {
                    PREFIX_TIER
                } else {
                    SUBSTRING_TIER
                };
                keep(&mut best, base + whole + field_bonus);
            }
        }
        if best.is_some_and(|score| score >= SUBSTRING_TIER) {
            return best;
        }
        if !self.compact.is_empty() {
            for (index, compact) in entry.compact.iter().enumerate() {
                if let Some(at) = compact.find(&self.compact) {
                    let field_bonus = 50 - (index as i32 * 8).min(48);
                    let whole = if compact.len() == self.compact.len() {
                        1_000
                    } else {
                        0
                    };
                    let start = if at == 0 { 500 } else { 0 };
                    keep(&mut best, COMPACT_TIER + whole + field_bonus + start);
                }
            }
        }
        if best.is_some() {
            return best;
        }
        // Version numbers are never typos: `atlas-0007` must not match
        // `atlas-0001`, nor `opus 4.8` match `opus 4.6`.
        let digits_present = self
            .text
            .split(|c: char| !c.is_ascii_digit())
            .filter(|run| !run.is_empty())
            .all(|run| entry.compact.iter().any(|field| field.contains(run)));
        if !digits_present {
            return None;
        }
        // Subsequence and typo tolerance from the shared matcher. Scores are
        // small (well under COMPACT_TIER), so literal hits always sort first.
        self.fuzzy.score(&entry.joined).map(|score| score.max(1))
    }
}

/// Filter and rank `models` for `query`. Returns `(spec, score)` sorted best
/// first, keeping the input order (usage rank) for ties.
///
/// Typo correction is a per-word fallback: when any model matches a query word
/// literally, other models may not satisfy that word through a typo, so `opus`
/// does not also list `plus`. A word that nothing matches literally (`sonet`)
/// may match through typo correction.
pub(super) fn rank_matches<'a>(
    models: impl IntoIterator<Item = &'a String>,
    details: &HashMap<String, ModelDetails>,
    group_label: impl Fn(&str) -> String,
    query: &ModelQuery,
) -> Vec<(&'a String, i32)> {
    // Score each word independently for every route first. Whether a word has
    // a literal match must be judged across the whole catalog, not only across
    // routes that happen to match every other word too.
    let per_word: Vec<(&String, Vec<Option<i32>>)> = models
        .into_iter()
        .map(|model| {
            let entry = SearchEntry::new(model, details.get(model), &group_label(model));
            let scores = query.words.iter().map(|word| word.score(&entry)).collect();
            (model, scores)
        })
        .collect();
    let literal_words: Vec<bool> = (0..query.words.len())
        .map(|i| {
            per_word
                .iter()
                .any(|(_, scores)| scores[i].is_some_and(|score| score >= COMPACT_TIER))
        })
        .collect();
    let scored = per_word.into_iter().filter_map(|(model, scores)| {
        scores
            .into_iter()
            .collect::<Option<Vec<i32>>>()
            .map(|scores| (model, scores))
    });
    let mut kept: Vec<_> = scored
        .filter(|(_, scores)| {
            scores
                .iter()
                .zip(&literal_words)
                .all(|(score, literal)| !literal || *score >= COMPACT_TIER)
        })
        .map(|(model, scores)| {
            let total = scores.iter().fold(0i32, |sum, s| sum.saturating_add(*s));
            (model, total)
        })
        .collect();
    // Stable sort: equal scores keep the caller's usage ranking.
    kept.sort_by(|a, b| b.1.cmp(&a.1));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(model: &str, provider: &str, method: &str) -> ModelDetails {
        ModelDetails {
            model: model.into(),
            recommended: false,
            provider: provider.into(),
            api_method: method.into(),
            usage: None,
        }
    }

    fn catalog() -> (Vec<String>, HashMap<String, ModelDetails>) {
        let rows = [
            (
                "claude-oauth:claude-opus-4-8",
                "claude-opus-4-8",
                "Anthropic",
                "claude-oauth",
            ),
            (
                "claude-oauth:claude-sonnet-4-6",
                "claude-sonnet-4-6",
                "Anthropic",
                "claude-oauth",
            ),
            (
                "claude-oauth:claude-haiku-4-5",
                "claude-haiku-4-5",
                "Anthropic",
                "claude-oauth",
            ),
            ("openai-oauth:gpt-5.5", "gpt-5.5", "OpenAI", "openai-oauth"),
            (
                "openai-oauth:gpt-5.1-codex-max",
                "gpt-5.1-codex-max",
                "OpenAI",
                "openai-oauth",
            ),
            (
                "openai-api:gpt-5-mini",
                "gpt-5-mini",
                "OpenAI",
                "openai-api-key",
            ),
            (
                "gemini:gemini-3-pro-preview",
                "gemini-3-pro-preview",
                "Gemini",
                "code-assist",
            ),
            (
                "openrouter:deepseek/deepseek-v4-pro",
                "deepseek/deepseek-v4-pro",
                "OpenRouter",
                "api-key",
            ),
            (
                "openrouter:moonshotai/kimi-k2.5",
                "moonshotai/kimi-k2.5",
                "OpenRouter",
                "api-key",
            ),
            (
                "openrouter:qwen/qwen3-coder-plus",
                "qwen/qwen3-coder-plus",
                "OpenRouter",
                "api-key",
            ),
        ];
        let models = rows.iter().map(|row| row.0.to_string()).collect();
        let details = rows
            .iter()
            .map(|(spec, model, provider, method)| {
                (spec.to_string(), detail(model, provider, method))
            })
            .collect();
        (models, details)
    }

    fn search(query: &str) -> Vec<String> {
        let (models, details) = catalog();
        let query = ModelQuery::new(query);
        rank_matches(&models, &details, |_| String::new(), &query)
            .into_iter()
            .map(|(model, _)| model.clone())
            .collect()
    }

    fn top(query: &str) -> String {
        search(query).into_iter().next().unwrap_or_default()
    }

    #[test]
    fn typos_transpositions_and_missing_letters_find_the_model() {
        for (query, expected) in [
            ("sonet", "claude-oauth:claude-sonnet-4-6"),
            ("sonnnet", "claude-oauth:claude-sonnet-4-6"),
            ("opsu", "claude-oauth:claude-opus-4-8"),
            ("haiklu", "claude-oauth:claude-haiku-4-5"),
            ("gtp-5.5", "openai-oauth:gpt-5.5"),
            ("codxe", "openai-oauth:gpt-5.1-codex-max"),
            ("gemni", "gemini:gemini-3-pro-preview"),
            ("deepsek", "openrouter:deepseek/deepseek-v4-pro"),
            ("kimmi", "openrouter:moonshotai/kimi-k2.5"),
            ("qwn coder", "openrouter:qwen/qwen3-coder-plus"),
        ] {
            assert_eq!(top(query), expected, "query {query:?}");
        }
    }

    #[test]
    fn separators_case_and_pretty_names_are_interchangeable() {
        for (query, expected) in [
            ("gpt55", "openai-oauth:gpt-5.5"),
            ("GPT-5.5", "openai-oauth:gpt-5.5"),
            ("opus48", "claude-oauth:claude-opus-4-8"),
            ("opus 4.8", "claude-oauth:claude-opus-4-8"),
            ("Claude Opus 4.8", "claude-oauth:claude-opus-4-8"),
            ("claudeopus", "claude-oauth:claude-opus-4-8"),
            ("k2.5", "openrouter:moonshotai/kimi-k2.5"),
            ("codex max", "openai-oauth:gpt-5.1-codex-max"),
        ] {
            assert_eq!(top(query), expected, "query {query:?}");
        }
    }

    #[test]
    fn words_match_any_field_in_any_order_and_all_must_match() {
        assert_eq!(top("anthropic opus"), "claude-oauth:claude-opus-4-8");
        assert_eq!(top("opus anthropic"), "claude-oauth:claude-opus-4-8");
        assert_eq!(top("mini api"), "openai-api:gpt-5-mini");
        assert!(search("opus openai").is_empty());
        assert!(search("zzzz-no-model-937").is_empty());
    }

    #[test]
    fn literal_matches_outrank_fuzzy_ones() {
        // `gpt-5` literally prefixes several ids; the exact family wins, and a
        // typo-only candidate never jumps ahead of a literal match.
        let results = search("gpt-5");
        assert!(results.iter().take(3).all(|model| model.contains("gpt-5")));
        let results = search("pro");
        assert!(results[0].ends_with("pro-preview") || results[0].ends_with("v4-pro"));
        // Broad provider queries keep every route from that provider.
        assert_eq!(search("openrouter").len(), 3);
        assert_eq!(search("claude").len(), 3);
    }

    #[test]
    fn short_noise_does_not_match_everything() {
        assert!(search("xq").is_empty());
        assert!(search("zz").is_empty());
    }
}
