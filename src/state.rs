use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::status::Status;

/// One row in a Status Comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub display_name: String,
    pub status: Status,
    /// Never serialized: Output lives only in the rendered comment (ADR 0001).
    #[serde(skip)]
    pub output: Option<String>,
}

impl Step {
    pub fn new(name: &str) -> Self {
        Step {
            name: name.to_string(),
            display_name: display_name_for(name),
            status: Status::Pending,
            output: None,
        }
    }
}

/// `rules-check` → `Rules Check`.
pub fn display_name_for(name: &str) -> String {
    name.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// A `steps set` argument: `name` or `name=Display Name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSpec {
    pub name: String,
    pub display_name: Option<String>,
}

impl FromStr for StepSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let (name, display_name) = match s.split_once('=') {
            Some((name, display)) if !display.trim().is_empty() => {
                (name, Some(display.trim().to_string()))
            }
            Some(_) => bail!("invalid step {s:?}: the display name after `=` is empty"),
            None => (s, None),
        };
        if name.trim().is_empty() {
            bail!("invalid step {s:?}: the name is empty");
        }
        Ok(StepSpec {
            name: name.trim().to_string(),
            display_name,
        })
    }
}

/// Everything about a Run that the Status Comment remembers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct State {
    pub run_id: String,
    #[serde(default)]
    pub finalized: bool,
    #[serde(default)]
    pub steps: Vec<Step>,
}

impl State {
    pub fn new(run_id: &str) -> Self {
        State {
            run_id: run_id.to_string(),
            ..State::default()
        }
    }

    /// The previous state if it's the same Run, otherwise a fresh one.
    pub fn for_run(previous: Option<State>, run_id: &str) -> State {
        match previous {
            Some(state) if state.run_id == run_id => state,
            _ => State::new(run_id),
        }
    }

    pub fn step(&self, name: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.name == name)
    }

    fn step_mut_or_append(&mut self, name: &str) -> &mut Step {
        let index = match self.steps.iter().position(|s| s.name == name) {
            Some(index) => index,
            None => {
                self.steps.push(Step::new(name));
                self.steps.len() - 1
            }
        };
        &mut self.steps[index]
    }

    /// Sets the Steps to exactly `specs`, in that order, with their display
    /// names, and reopens the Run. Listed Steps keep their Status and Output;
    /// new ones start `pending`.
    pub fn set_steps(&mut self, specs: &[StepSpec]) {
        let mut previous = std::mem::take(&mut self.steps);
        for spec in specs {
            if self.step(&spec.name).is_some() {
                continue;
            }
            let mut step = match previous.iter().position(|s| s.name == spec.name) {
                Some(i) => previous.remove(i),
                None => Step::new(&spec.name),
            };
            step.display_name = spec
                .display_name
                .clone()
                .unwrap_or_else(|| display_name_for(&spec.name));
            self.steps.push(step);
        }
        self.finalized = false;
    }

    /// Sets a Step's Status, and its Output when given; unknown Steps are appended.
    pub fn record(&mut self, name: &str, status: Status, output: Option<String>) {
        self.finalized = false;
        let step = self.step_mut_or_append(name);
        step.status = status;
        if let Some(output) = output {
            step.output = Some(output);
        }
    }

    /// Every Step back to `pending` with no Output; the Run reopens.
    pub fn reset(&mut self) {
        for step in &mut self.steps {
            step.status = Status::Pending;
            step.output = None;
        }
        self.finalized = false;
    }

    /// Steps that never ran become `skipped`; Steps still running become `cancelled`.
    pub fn finalize(&mut self) {
        for step in &mut self.steps {
            match step.status {
                Status::Pending => step.status = Status::Skipped,
                Status::Running => step.status = Status::Cancelled,
                _ => {}
            }
        }
        self.finalized = true;
    }

    /// A Run is Failing when any of its Steps are.
    pub fn is_failing(&self) -> bool {
        self.steps.iter().any(|s| s.status.is_failing())
    }
}

#[cfg(test)]
pub fn specs(args: &[&str]) -> Vec<StepSpec> {
    args.iter().map(|a| a.parse().unwrap()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(state: &State) -> Vec<&str> {
        state.steps.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn display_names_are_title_cased() {
        assert_eq!(display_name_for("rules-check"), "Rules Check");
        assert_eq!(display_name_for("plan"), "Plan");
        assert_eq!(display_name_for("bar step"), "Bar Step");
        assert_eq!(display_name_for("tf_apply"), "Tf Apply");
    }

    #[test]
    fn step_specs_parse_names_and_display_names() {
        assert_eq!(
            "rules-check=OPA Rules".parse::<StepSpec>().unwrap(),
            StepSpec {
                name: "rules-check".into(),
                display_name: Some("OPA Rules".into())
            }
        );
        assert_eq!("plan".parse::<StepSpec>().unwrap().display_name, None);
        assert!("plan=".parse::<StepSpec>().is_err());
        assert!("=Plan".parse::<StepSpec>().is_err());
    }

    #[test]
    fn set_steps_sets_exactly_the_listed_steps_in_order() {
        let mut state = State::new("1");
        state.set_steps(&specs(&["init", "plan", "apply"]));
        state.set_steps(&specs(&["plan", "validate", "init", "plan"]));
        assert_eq!(names(&state), ["plan", "validate", "init"]);
        assert_eq!(state.step("validate").unwrap().status, Status::Pending);
    }

    #[test]
    fn set_steps_defines_display_names() {
        let mut state = State::new("1");
        state.set_steps(&specs(&["rules-check=OPA Rules", "plan"]));
        assert_eq!(state.step("rules-check").unwrap().display_name, "OPA Rules");
        state.set_steps(&specs(&["rules-check"]));
        assert_eq!(
            state.step("rules-check").unwrap().display_name,
            "Rules Check"
        );
    }

    #[test]
    fn set_steps_keeps_status_and_output_of_listed_steps() {
        let mut state = State::new("1");
        state.record("plan", Status::Success, Some("log".into()));
        state.set_steps(&specs(&["plan"]));
        let plan = state.step("plan").unwrap();
        assert_eq!(plan.status, Status::Success);
        assert_eq!(plan.output.as_deref(), Some("log"));
    }

    #[test]
    fn set_steps_reopens_a_finalized_run() {
        let mut state = State::new("1");
        state.finalize();
        state.set_steps(&specs(&["plan"]));
        assert!(!state.finalized);
    }

    #[test]
    fn reset_keeps_steps_but_clears_their_data() {
        let mut state = State::new("1");
        state.set_steps(&specs(&["plan=Terraform Plan"]));
        state.record("plan", Status::Failure, Some("log".into()));
        state.finalize();
        state.reset();
        let plan = state.step("plan").unwrap();
        assert_eq!(plan.status, Status::Pending);
        assert_eq!(plan.output, None);
        assert_eq!(plan.display_name, "Terraform Plan");
        assert!(!state.finalized);
    }

    #[test]
    fn steps_recorded_after_set_steps_are_appended() {
        let mut state = State::new("1");
        state.set_steps(&specs(&["init", "plan"]));
        state.record("fmt", Status::Success, None);
        assert_eq!(names(&state), ["init", "plan", "fmt"]);
    }

    #[test]
    fn recording_without_output_keeps_existing_output() {
        let mut state = State::new("1");
        state.record("plan", Status::Running, Some("log".into()));
        state.record("plan", Status::Success, None);
        assert_eq!(state.step("plan").unwrap().output.as_deref(), Some("log"));
    }

    #[test]
    fn finalize_skips_pending_and_cancels_running() {
        let mut state = State::new("1");
        state.set_steps(&specs(&["init", "plan", "apply"]));
        state.record("init", Status::Success, None);
        state.record("plan", Status::Running, None);
        state.finalize();
        assert_eq!(state.step("init").unwrap().status, Status::Success);
        assert_eq!(state.step("plan").unwrap().status, Status::Cancelled);
        assert_eq!(state.step("apply").unwrap().status, Status::Skipped);
        assert!(state.finalized);
        assert!(state.is_failing());
    }

    #[test]
    fn recording_after_finalize_reopens_the_run() {
        let mut state = State::new("1");
        state.finalize();
        state.record("plan", Status::Running, None);
        assert!(!state.finalized);
    }

    #[test]
    fn new_run_starts_fresh_and_same_run_carries_on() {
        let mut previous = State::new("1");
        previous.record("plan", Status::Success, None);

        let same = State::for_run(Some(previous.clone()), "1");
        assert_eq!(names(&same), ["plan"]);

        let fresh = State::for_run(Some(previous), "2");
        assert!(fresh.steps.is_empty());
        assert_eq!(fresh.run_id, "2");
    }

    #[test]
    fn output_is_not_serialized() {
        let mut state = State::new("1");
        state.record("plan", Status::Success, Some("secret-ish log".into()));
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("secret-ish log"));
    }
}
