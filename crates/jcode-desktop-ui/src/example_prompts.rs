//! Personalized example prompts for the empty chat composer.
//!
//! Unfinished todos left in closed sessions are the best hint of what the
//! user wants to do next, so they lead the composer's typewriter examples.
//! The generic catalog in `input_motion` fills in when there are none.
//! Prompts are scoped to the composer's project, since a suggestion from
//! another repository would start work in the wrong place.

use std::path::Path;
use std::sync::Arc;

use gpui::{App, Global, SharedString};
use jcode_sdk::SessionInfo;

use crate::harness::UnfinishedSession;

/// Longest personalized prompt, in characters, before it is shortened.
const MAX_CHARS: usize = 60;
/// Todos taken from one session, so one long plan does not crowd out others.
const PER_SESSION: usize = 2;
/// Total personalized prompts kept.
const MAX_PROMPTS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExamplePrompt {
    pub working_dir: Option<String>,
    pub text: SharedString,
}

/// Process-wide personalized prompts, newest session first.
#[derive(Default)]
pub(crate) struct ExamplePrompts(pub Arc<Vec<ExamplePrompt>>);

impl Global for ExamplePrompts {}

/// Personalized prompts for a composer working in `working_dir`. With no
/// directory, every project's prompts apply.
pub(crate) fn for_dir(cx: &App, working_dir: Option<&str>) -> Vec<SharedString> {
    let Some(prompts) = cx.try_global::<ExamplePrompts>() else {
        return Vec::new();
    };
    prompts
        .0
        .iter()
        .filter(
            |prompt| match (working_dir, prompt.working_dir.as_deref()) {
                (None, _) => true,
                (Some(dir), Some(source)) => same_project(dir, source),
                (Some(_), None) => false,
            },
        )
        .map(|prompt| prompt.text.clone())
        .collect()
}

/// Replace the global prompts, notifying windows only when they changed.
pub(crate) fn set(prompts: Vec<ExamplePrompt>, cx: &mut App) {
    if cx
        .try_global::<ExamplePrompts>()
        .is_some_and(|current| *current.0 == prompts)
    {
        return;
    }
    cx.set_global(ExamplePrompts(Arc::new(prompts)));
    cx.refresh_windows();
}

/// Reads durable todo snapshots, so call it off the UI thread.
pub(crate) fn load(sessions: &[SessionInfo]) -> Vec<ExamplePrompt> {
    match crate::harness::jcode_home() {
        Some(home) => load_from(&home, sessions),
        None => Vec::new(),
    }
}

pub(crate) fn load_from(home: &Path, sessions: &[SessionInfo]) -> Vec<ExamplePrompt> {
    let mut sessions = sessions.to_vec();
    sessions.sort_by_key(|session| {
        std::cmp::Reverse(
            session
                .last_active_at_ms
                .max(session.updated_at_ms)
                .unwrap_or_default(),
        )
    });
    from_unfinished(crate::harness::unfinished_sessions_in(home, &sessions))
}

pub(crate) fn from_unfinished(sessions: Vec<UnfinishedSession>) -> Vec<ExamplePrompt> {
    let mut prompts: Vec<ExamplePrompt> = Vec::new();
    for session in sessions {
        let mut todos: Vec<_> = session
            .todos
            .iter()
            .filter(|todo| {
                !matches!(
                    todo.status.to_ascii_lowercase().as_str(),
                    "completed" | "cancelled" | "canceled"
                )
            })
            .collect();
        // Work that was in progress when the session closed comes first.
        todos.sort_by_key(|todo| !todo.status.eq_ignore_ascii_case("in_progress"));
        let texts = todos
            .into_iter()
            .filter_map(|todo| prompt_text(&todo.content))
            .filter(|text| {
                !prompts
                    .iter()
                    .any(|prompt| prompt.text.eq_ignore_ascii_case(text))
            })
            .take(PER_SESSION)
            .collect::<Vec<_>>();
        for text in texts {
            prompts.push(ExamplePrompt {
                working_dir: session.working_dir.clone(),
                text: text.into(),
            });
        }
        if prompts.len() >= MAX_PROMPTS {
            prompts.truncate(MAX_PROMPTS);
            break;
        }
    }
    prompts
}

/// One short line suitable for a placeholder, or `None` for unusable text.
fn prompt_text(content: &str) -> Option<String> {
    let line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let line = line
        .trim_start_matches(['-', '*', '•'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let line = line.trim_end_matches(['.', ':', ',', ';']).trim();
    let line = strip_label(line, &["next:", "todo:", "then:"]);
    if line.chars().count() < 4 || !actionable(line) {
        return None;
    }
    if line.chars().count() <= MAX_CHARS {
        return Some(line.to_owned());
    }
    let mut short = String::new();
    for word in line.split(' ') {
        if short.chars().count() + word.chars().count() + 1 > MAX_CHARS - 1 {
            break;
        }
        if !short.is_empty() {
            short.push(' ');
        }
        short.push_str(word);
    }
    if short.is_empty() {
        short = line.chars().take(MAX_CHARS - 1).collect();
    }
    let short = short.trim_end_matches([',', ';', ':', '.', ' ']);
    Some(format!("{short}…"))
}

/// A session in a subdirectory of the composer's project, or the reverse,
/// belongs to the same project.
fn same_project(a: &str, b: &str) -> bool {
    let (a, b) = (Path::new(a), Path::new(b));
    a.starts_with(b) || b.starts_with(a)
}

fn strip_label<'a>(line: &'a str, labels: &[&str]) -> &'a str {
    for label in labels {
        if line.len() >= label.len()
            && line.is_char_boundary(label.len())
            && line[..label.len()].eq_ignore_ascii_case(label)
        {
            return line[label.len()..].trim_start();
        }
    }
    line
}

/// Whether a todo is work the user could start in a new session. Blocked
/// items wait on someone else, and wrap-up steps (test, commit, reload) only
/// make sense inside the session that did the work.
fn actionable(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("blocked") || lower.contains("blocked on") || lower.contains("waiting on")
    {
        return false;
    }
    const WRAP_UP: &[&str] = &[
        "test",
        "tests",
        "run",
        "commit",
        "push",
        "reload",
        "rebuild",
        "build",
        "verify",
        "screenshot",
        "screenshots",
        "show",
        "report",
        "review",
        "validate",
        "check",
    ];
    // "Test, screenshot, commit and reload": the leading verbs are all wrap-up.
    let verbs: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| {
            !word.is_empty()
                && !matches!(
                    *word,
                    "and" | "then" | "the" | "both" | "all" | "ci" | "user"
                )
        })
        .take(3)
        .collect();
    !(verbs.len() >= 2 && verbs.iter().all(|word| WRAP_UP.contains(word)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::UnfinishedTodo;

    fn todo(content: &str, status: &str) -> UnfinishedTodo {
        UnfinishedTodo {
            content: content.into(),
            status: status.into(),
            group: None,
        }
    }

    fn session(dir: &str, todos: Vec<UnfinishedTodo>) -> UnfinishedSession {
        UnfinishedSession {
            session_id: dir.into(),
            title: dir.into(),
            working_dir: Some(dir.into()),
            todos,
        }
    }

    #[test]
    fn blocked_and_wrap_up_todos_are_not_suggested() {
        for skipped in [
            "BLOCKED on user: stale GPUI_REVISION",
            "Run CI, review screenshots, show user",
            "Test, screenshot, commit both repos, reload desktop",
            "Build, test, commit",
        ] {
            assert_eq!(prompt_text(skipped), None, "{skipped}");
        }
        for kept in [
            "Move limit bars into method pill hover card",
            "Test-controllable animation clock to replace thread::sleep",
            "Review the login flow for races",
            "Fix the parser",
        ] {
            assert!(prompt_text(kept).is_some(), "{kept}");
        }
        assert_eq!(
            prompt_text("Next: remaining slow tests").as_deref(),
            Some("remaining slow tests")
        );
    }

    #[test]
    fn in_progress_todos_lead_and_each_session_is_capped() {
        let prompts = from_unfinished(vec![
            session(
                "/repo/a",
                vec![
                    todo("Write docs", "pending"),
                    todo("Fix the parser", "in_progress"),
                    todo("Ship it", "pending"),
                    todo("Old work", "completed"),
                ],
            ),
            session(
                "/repo/b",
                vec![
                    todo("Drop it", "cancelled"),
                    todo("fix the PARSER", "pending"),
                ],
            ),
        ]);
        let texts: Vec<_> = prompts.iter().map(|p| p.text.as_ref()).collect();
        assert_eq!(texts, ["Fix the parser", "Write docs"]);
    }

    #[test]
    fn prompt_text_is_one_short_clean_line() {
        assert_eq!(
            prompt_text("  - Add   retries.\nmore detail").as_deref(),
            Some("Add retries")
        );
        assert_eq!(prompt_text("ok"), None);
        let long = prompt_text(
            "Load pending todos from closed sessions off the UI thread and feed them to composers",
        )
        .unwrap();
        assert!(
            long.ends_with('…') && long.chars().count() <= MAX_CHARS,
            "{long}"
        );
        assert!(long.starts_with("Load pending todos from closed sessions"));
    }

    #[gpui::test]
    fn prompts_are_scoped_to_the_composers_project(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(for_dir(cx, None).is_empty());
            set(
                from_unfinished(vec![
                    session("/repo/a", vec![todo("Fix a", "pending")]),
                    session("/repo/b/sub", vec![todo("Fix b", "pending")]),
                ]),
                cx,
            );
            assert_eq!(for_dir(cx, Some("/repo/a")), ["Fix a"]);
            assert_eq!(for_dir(cx, Some("/repo/b")), ["Fix b"]);
            assert!(for_dir(cx, Some("/elsewhere")).is_empty());
            assert_eq!(for_dir(cx, None).len(), 2);
        });
    }
}
