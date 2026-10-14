//! Agent-driven slash commands. These are workflows, not runtime setting changes.

/// Expand only complete command tokens, preserving the user's optional target.
pub fn workflow_prompt(input: &str) -> Option<String> {
    let input = input.trim();
    let (name, rest) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
    let rest = rest.trim();
    let instructions = match name {
        "/plan" => {
            "Produce a grounded, concrete plan for the current task. This is plan-only: do not edit files, apply patches, or change git state. Read and search as needed. Present a fenced ```plan block with a title, goal, affected areas, ordered approach, validation, and open questions. Stop and wait for approval before implementation."
        }
        "/test" => {
            "Run a layered verification pass for the stated claim, or the current changes if no claim is given. Begin with a reproduction, then focused unit and integration tests, real end-user workflow checks, static analysis, and relevant edge, failure, concurrency, and regression cases. Track the work with todo. Do not confuse compile success with behavioral proof. Finish with observed evidence, confidence, and explicitly unverified gaps."
        }
        "/improve" => {
            "Run an improvement workflow for this repository. Inspect the repository and current changes first. Rank 3-7 high-impact, low-risk, verifiable improvements with todo. Implement the strongest items, preserve unrelated work, validate each result, and reassess. Stop when remaining ideas are not worth the churn. If the argument starts with 'plan', only inspect and plan, making no edits. For 'status', report the existing improvement todos and evidence without starting work. For 'resume', continue the existing improvement work rather than inventing a new batch. For 'stop', stop the improvement workflow without starting more work."
        }
        "/refactor" => {
            "Run a safe refactor workflow. Inspect current repository state, identify a bounded structural improvement, establish behavior-preserving regression checks, and track the work with todo. Make small validated steps, preserving unrelated work and avoiding speculative rewrites. Stop when additional churn is not justified. If the argument starts with 'plan', only inspect and plan, making no edits. For 'status', report existing refactor todos and evidence without starting work. For 'resume', continue the existing refactor. For 'stop', stop the refactor workflow without starting more work."
        }
        "/triage" => {
            "Triage this repository's new GitHub issues. Read the issue and repository context, identify duplicates, risks, and reproducible problems. Autonomously fix only safely scoped issues with evidence-backed regression tests. Preserve unrelated work. Ask before destructive actions or externally posting messages, closing issues, or changing labels. Report issues examined, changes made, validation, and deferred risks."
        }
        "/feedback" => {
            "Help the user send feedback about Jcode using maintainer_feedback. If a description is supplied, prepare a concise self-contained report with reproducible details. Obtain approval before sharing user-originated feedback, and exclude secrets or private data. If no description is supplied, ask what feedback the user wants to share. Do not claim delivery without the tool's success."
        }
        _ => return None,
    };
    Some(format!(
        "{instructions}\n\nUser-supplied target or arguments:\n{}",
        if rest.is_empty() {
            "Use the current session task and repository context."
        } else {
            rest
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_tokens_only() {
        for name in [
            "/plan",
            "/test",
            "/improve",
            "/refactor",
            "/triage",
            "/feedback",
        ] {
            assert!(workflow_prompt(name).is_some(), "{name}");
            assert!(
                workflow_prompt(&format!("{name} target 日本語"))
                    .unwrap()
                    .contains("target 日本語")
            );
            assert!(workflow_prompt(&format!("{name}extra")).is_none());
        }
        assert!(workflow_prompt("/skills").is_none());
        assert!(workflow_prompt("hello").is_none());
    }

    #[test]
    fn planning_is_read_only_and_has_plan_card_contract() {
        let prompt = workflow_prompt("/plan ship feature").unwrap();
        assert!(prompt.contains("do not edit files"));
        assert!(prompt.contains("```plan"));
        assert!(prompt.contains("wait for approval"));
    }
}
