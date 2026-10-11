use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use minijinja::value::{Serde, Value};
use minijinja::{AutoEscape, Environment};
use serde::Serialize;

use crate::comment::output_block;
use crate::state::State;

pub const DEFAULT_TEMPLATE: &str = include_str!("default_template.md.j2");

/// GitHub's comment body limit. It's undocumented and reports disagree on
/// characters vs bytes, so budget in bytes.
pub const COMMENT_LIMIT: usize = 65_536;
/// GitHub's limit for one step's job summary.
pub const SUMMARY_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Comment,
    Summary,
}

/// What a Template can see about the GitHub context.
#[derive(Default, Serialize)]
pub struct GithubView {
    pub workflow: String,
    pub job: String,
    pub actor: String,
    pub repository: String,
    pub sha: String,
    pub short_sha: String,
    pub pr_number: Option<u64>,
    pub pr_url: Option<String>,
    pub server_url: String,
    pub updated_at: String,
}

#[derive(Default)]
pub struct RunInfo {
    pub url: Option<String>,
    pub attempt: String,
}

#[derive(Serialize)]
struct StepView<'a> {
    name: &'a str,
    display_name: &'a str,
    status: &'a str,
    failing: bool,
    builtin: bool,
    has_output: bool,
}

#[derive(Serialize)]
struct RunView<'a> {
    id: &'a str,
    attempt: &'a str,
    url: Option<&'a str>,
    failing: bool,
    finalized: bool,
}

pub struct Renderer<'a> {
    pub template: &'a str,
    pub github: &'a GithubView,
    pub run: &'a RunInfo,
}

impl Renderer<'_> {
    /// Renders the Template, truncating Outputs so the result plus `reserve` bytes
    /// fits in `limit`.
    pub fn render(
        &self,
        state: &State,
        target: Target,
        limit: usize,
        reserve: usize,
    ) -> Result<String> {
        let full: Vec<Option<String>> = state.steps.iter().map(|s| s.output.clone()).collect();
        let body = self.render_with(state, &full, target)?;
        let mut excess = (body.len() + reserve).saturating_sub(limit);
        if excess == 0 {
            return Ok(body);
        }

        let lengths: Vec<usize> = full
            .iter()
            .map(|o| o.as_ref().map_or(0, String::len))
            .collect();
        let failing: Vec<bool> = state.steps.iter().map(|s| s.status.is_failing()).collect();
        let total: usize = lengths.iter().sum();
        let mut last = body;
        // A Template may render an Output twice or wrap it: shrink and re-check
        // instead of trusting one estimate.
        for _ in 0..5 {
            let budget = total.saturating_sub(excess + 256 * state.steps.len());
            let caps = allocate(&lengths, &failing, budget);
            let outputs: Vec<Option<String>> = full
                .iter()
                .zip(&caps)
                .map(|(o, &cap)| o.as_ref().map(|o| truncate_tail(o, cap)))
                .collect();
            last = self.render_with(state, &outputs, target)?;
            let over = (last.len() + reserve).saturating_sub(limit);
            if over == 0 {
                return Ok(last);
            }
            excess += over * 2;
        }
        Ok(last)
    }

    fn render_with(
        &self,
        state: &State,
        outputs: &[Option<String>],
        target: Target,
    ) -> Result<String> {
        let by_name: HashMap<String, String> = state
            .steps
            .iter()
            .zip(outputs)
            .filter_map(|(s, o)| o.clone().map(|o| (s.name.clone(), o)))
            .collect();
        let by_name = Arc::new(by_name);

        let mut env = Environment::new();
        env.set_auto_escape_callback(|_| AutoEscape::None);
        env.set_syntax(
            minijinja::syntax::SyntaxConfig::builder()
                .trim_blocks(true)
                .lstrip_blocks(true)
                .build()
                .expect("default syntax is valid"),
        );
        env.add_filter("output", move |step: Value| -> Value {
            let name = step.get_attr("name").ok();
            let output = name
                .as_ref()
                .and_then(Value::as_str)
                .and_then(|n| by_name.get(n).map(|o| (n, o)));
            match output {
                Some((name, text)) => Value::from_safe_string(output_block(name, text)),
                None => Value::from(""),
            }
        });
        let template = env
            .template_from_str(self.template)
            .context("failed to parse the Template")?;

        let steps: Vec<StepView> = state
            .steps
            .iter()
            .zip(outputs)
            .map(|(s, o)| StepView {
                name: &s.name,
                display_name: &s.display_name,
                status: s.status.as_str(),
                failing: s.status.is_failing(),
                builtin: s.status.is_builtin(),
                has_output: o.is_some(),
            })
            .collect();
        let run = RunView {
            id: &state.run_id,
            attempt: &self.run.attempt,
            url: self.run.url.as_deref(),
            failing: state.is_failing(),
            finalized: state.finalized,
        };
        let ctx = minijinja::context! {
            steps => Value::from(Serde(&steps)),
            run => Value::from(Serde(&run)),
            github => Value::from(Serde(self.github)),
            target => Value::from(Serde(&target)),
        };
        template
            .render(ctx)
            .context("failed to render the Template")
    }
}

/// Splits `budget` bytes between Outputs: Failing Steps first, equal shares
/// within a group, unused share passed on.
fn allocate(lengths: &[usize], failing: &[bool], budget: usize) -> Vec<usize> {
    let mut caps = vec![0; lengths.len()];
    let mut remaining = budget;
    for group in [true, false] {
        let mut members: Vec<usize> = (0..lengths.len())
            .filter(|&i| failing[i] == group)
            .collect();
        members.sort_by_key(|&i| lengths[i]);
        let mut left = members.len();
        for i in members {
            let share = remaining / left;
            caps[i] = lengths[i].min(share);
            remaining -= caps[i];
            left -= 1;
        }
    }
    caps
}

/// Keeps the tail of `text` within `cap` bytes: errors usually come last.
fn truncate_tail(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    // Size the note for the worst case; the real count is never longer.
    let note_len = format!("… ({} bytes truncated)\n", text.len()).len();
    let keep = cap.saturating_sub(note_len);
    let mut start = text.len() - keep;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("… ({start} bytes truncated)\n{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::specs;
    use crate::status::Status;

    fn github() -> GithubView {
        GithubView {
            workflow: "Terraform".into(),
            job: "plan".into(),
            actor: "octocat".into(),
            short_sha: "abc123".into(),
            updated_at: "2026-10-10T12:00:00Z".into(),
            ..GithubView::default()
        }
    }

    fn render(template: &str, state: &State, target: Target) -> String {
        let github = github();
        let run = RunInfo {
            url: Some("https://github.com/o/r/actions/runs/1".into()),
            attempt: "1".into(),
        };
        Renderer {
            template,
            github: &github,
            run: &run,
        }
        .render(state, target, COMMENT_LIMIT, 0)
        .unwrap()
    }

    fn record(state: &mut State, name: &str, status: Status, output: Option<&str>) {
        state.record(name, status, output.map(String::from));
    }

    #[test]
    fn default_template_renders_table_and_outputs() {
        let mut state = State::new("1");
        state.set_steps(&specs(&["init", "plan", "apply"]));
        record(&mut state, "init", Status::Success, Some("Initialized"));
        record(&mut state, "plan", Status::Failure, Some("Error: boom"));
        let body = render(DEFAULT_TEMPLATE, &state, Target::Comment);

        assert!(body.contains("### Terraform · plan\n\n<sub>"), "{body}");
        assert!(body.contains("| Init | ✅ Success |"), "{body}");
        assert!(body.contains("| Plan | ❌ Failed |"), "{body}");
        assert!(body.contains("| Apply | ⬜ Pending |"), "{body}");
        // Failing Output is expanded, the rest collapsed.
        assert!(body.contains("#### Plan output"), "{body}");
        assert!(
            body.contains("<details><summary>Init output</summary>"),
            "{body}"
        );
        assert!(body.contains("[View workflow run](https://github.com/o/r/actions/runs/1)"));
        // No banner or celebration before Finalize.
        assert!(!body.contains("[!WARNING]"));
        assert!(!body.contains("🎉"));
    }

    #[test]
    fn table_rows_are_contiguous() {
        let mut state = State::new("1");
        state.set_steps(&specs(&["a", "b"]));
        let body = render(DEFAULT_TEMPLATE, &state, Target::Summary);
        assert!(
            body.contains("|------|--------|\n| A | ⬜ Pending |\n| B | ⬜ Pending |\n"),
            "{body}"
        );
    }

    #[test]
    fn output_is_not_html_escaped() {
        let mut state = State::new("1");
        record(
            &mut state,
            "plan",
            Status::Success,
            Some("<resource> & \"x\""),
        );
        let body = render(DEFAULT_TEMPLATE, &state, Target::Comment);
        assert!(body.contains("<resource> & \"x\""), "{body}");
    }

    #[test]
    fn custom_template_sees_context() {
        let mut state = State::new("7");
        record(
            &mut state,
            "plan",
            Status::Custom("changes".into()),
            Some("diff"),
        );
        let template = "{{ github.actor }} {{ run.id }} {{ target }}\n{% for s in steps %}{{ s.name }}={{ s.status }}\n{{ s | output }}{% endfor %}";
        let body = render(template, &state, Target::Comment);
        assert!(
            body.starts_with("octocat 7 comment\nplan=changes\n"),
            "{body}"
        );
        assert!(body.contains("```\ndiff\n```"), "{body}");
    }

    #[test]
    fn large_outputs_are_truncated_to_fit_keeping_the_tail() {
        let mut state = State::new("1");
        let big = format!("{}THE ERROR", "x".repeat(100_000));
        record(&mut state, "plan", Status::Failure, Some(&big));
        record(
            &mut state,
            "init",
            Status::Success,
            Some(&"y".repeat(100_000)),
        );
        let github = github();
        let run = RunInfo::default();
        let body = Renderer {
            template: DEFAULT_TEMPLATE,
            github: &github,
            run: &run,
        }
        .render(&state, Target::Comment, COMMENT_LIMIT, 1000)
        .unwrap();
        assert!(body.len() + 1000 <= COMMENT_LIMIT, "{}", body.len());
        assert!(body.contains("THE ERROR"));
        assert!(body.contains("bytes truncated"));
    }

    #[test]
    fn allocate_prioritises_failing_steps() {
        // Failing step needs 80 and gets it; the other two split the remaining 20.
        assert_eq!(
            allocate(&[80, 50, 50], &[true, false, false], 100),
            [80, 10, 10]
        );
        // Small outputs keep everything and give their spare share to bigger ones.
        assert_eq!(allocate(&[5, 100], &[false, false], 60), [5, 55]);
    }

    #[test]
    fn truncate_tail_respects_char_boundaries() {
        let text = "é".repeat(100);
        let out = truncate_tail(&text, 50);
        assert!(out.len() <= 50, "{}", out.len());
        assert!(out.ends_with('é'));
        assert_eq!(truncate_tail("short", 50), "short");
    }
}
