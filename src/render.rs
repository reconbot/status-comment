use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use minijinja::value::{Serde, Value};
use minijinja::{AutoEscape, Environment};
use serde::Serialize;

use crate::comment::output_block;
use crate::config::Config;
use crate::state::State;

pub const DEFAULT_TEMPLATE: &str = include_str!("default_template.md.j2");

/// GitHub's comment body limit, in characters.
pub const COMMENT_LIMIT: usize = 65_536;

/// Ends the line that replaces the middle of a truncated Output.
const OMITTED: &str = "omitted by status-comment …";
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
    display_name: String,
    status: &'a str,
    status_label: String,
    failing: bool,
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
    /// Display names and Status labels.
    pub config: &'a Config,
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
        let mut excess = (body.chars().count() + reserve).saturating_sub(limit);
        if excess == 0 {
            return Ok(body);
        }

        let lengths: Vec<usize> = full
            .iter()
            .map(|o| o.as_ref().map_or(0, |o| o.chars().count()))
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
                .map(|(o, &cap)| o.as_ref().map(|o| truncate_middle(o, cap)))
                .collect();
            last = self.render_with(state, &outputs, target)?;
            let over = (last.chars().count() + reserve).saturating_sub(limit);
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
        let run_url = self.run.url.clone();
        env.add_filter("output", move |step: Value| -> Value {
            let name = step.get_attr("name").ok();
            let output = name
                .as_ref()
                .and_then(Value::as_str)
                .and_then(|n| by_name.get(n).map(|o| (n, o)));
            let Some((name, text)) = output else {
                return Value::from("");
            };
            let mut block = output_block(name, text);
            // After the end marker, so it's never read back as Output.
            if text.contains(OMITTED) {
                block.push_str(&match &run_url {
                    Some(url) => {
                        format!("\n\n_Output truncated: the full output is in the [workflow run]({url})._")
                    }
                    None => "\n\n_Output truncated._".to_string(),
                });
            }
            Value::from_safe_string(block)
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
                display_name: self.config.step_name(&s.name),
                status: s.status.as_str(),
                status_label: self.config.status_label(s.status.as_str()),
                failing: s.status.is_failing(),
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

/// Fits `text` within `cap` characters by keeping whole lines from the start
/// (where the cause usually is) and the end (where the summary usually is), and
/// replacing the middle with one line saying how much was left out.
fn truncate_middle(text: &str, cap: usize) -> String {
    let len = text.chars().count();
    if len <= cap {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    // Size the note for the worst case; the real count is never longer.
    let budget = cap.saturating_sub(format!("… {len} characters {OMITTED}\n").chars().count());

    let mut head = 0;
    let mut head_used = 0;
    while let Some(line) = lines.get(head) {
        let n = line.chars().count();
        if head_used + n > budget / 2 {
            break;
        }
        head_used += n;
        head += 1;
    }
    let mut tail = lines.len();
    let mut tail_used = 0;
    while tail > head {
        let n = lines[tail - 1].chars().count();
        if head_used + tail_used + n > budget {
            break;
        }
        tail_used += n;
        tail -= 1;
    }

    if head == 0 && tail == lines.len() {
        // No line fits whole (e.g. one huge line): keep the last characters.
        let kept: String = text.chars().skip(len - budget).collect();
        return format!("… {} characters {OMITTED}\n{kept}", len - budget);
    }
    format!(
        "{}… {} lines {OMITTED}\n{}",
        lines[..head].concat(),
        tail - head,
        lines[tail..].concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::Status;

    fn config() -> Config {
        let mut config = Config::default();
        config.fill_defaults(None);
        config
    }

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

    fn record(state: &mut State, name: &str, status: Status, output: Option<&str>) {
        state.record(name, status, output.map(String::from));
    }

    #[test]
    fn large_outputs_keep_head_and_tail_and_link_to_the_run() {
        let mut state = State::new("1");
        let lines: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
        let big = format!("ROOT CAUSE\n{lines}THE ERROR\n");
        record(&mut state, "plan", Status::Failure, Some(&big));
        record(
            &mut state,
            "init",
            Status::Success,
            Some(&"y\n".repeat(50_000)),
        );
        let github = github();
        let run = RunInfo {
            url: Some("https://github.com/o/r/actions/runs/1".into()),
            attempt: "1".into(),
        };
        let body = Renderer {
            template: DEFAULT_TEMPLATE,
            github: &github,
            run: &run,
            config: &config(),
        }
        .render(&state, Target::Comment, COMMENT_LIMIT, 1000)
        .unwrap();
        assert!(body.chars().count() + 1000 <= COMMENT_LIMIT);
        assert!(body.contains("ROOT CAUSE"));
        assert!(body.contains("THE ERROR"));
        assert!(body.contains(&format!("lines {OMITTED}")));
        assert!(body.contains(
            "_Output truncated: the full output is in the [workflow run](https://github.com/o/r/actions/runs/1)._"
        ));
    }

    #[test]
    fn truncation_cuts_between_whole_lines() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let out = truncate_middle(&text, 200);
        assert!(out.chars().count() <= 200, "{}", out.chars().count());
        assert!(out.starts_with("line 0\n"));
        assert!(out.ends_with("line 99\n"));
        for line in out.lines().filter(|l| !l.contains(OMITTED)) {
            assert!(line.starts_with("line "), "partial line {line:?}");
        }
        assert_eq!(truncate_middle("short", 50), "short");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        // 100 two-byte characters fit a 100-character budget untouched.
        let text = "é".repeat(100);
        assert_eq!(truncate_middle(&text, 100), text);
        let out = truncate_middle(&"é".repeat(1000), 100);
        assert!(out.chars().count() <= 100, "{}", out.chars().count());
        assert!(out.ends_with('é'));
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
}
