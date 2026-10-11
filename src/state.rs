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

/// Everything about a Run that the Status Comment remembers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct State {
    pub run_id: String,
    #[serde(default)]
    pub finalized: bool,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// What a single invocation records for one Step.
#[derive(Debug, Clone, Default)]
pub struct StepUpdate {
    pub status: Option<Status>,
    pub display_name: Option<String>,
    /// `None` keeps any existing Output.
    pub output: Option<String>,
}

impl State {
    pub fn new(run_id: &str) -> Self {
        State {
            run_id: run_id.to_string(),
            ..State::default()
        }
    }

    /// Carries the previous state forward within the same Run, or starts fresh for a new one.
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

    /// Declares Steps as `pending`, keeping the order of earlier declarations.
    pub fn declare(&mut self, names: &[String]) {
        for name in names {
            if self.step(name).is_none() {
                self.steps.push(Step::new(name));
            }
        }
    }

    pub fn record(&mut self, name: &str, update: StepUpdate) {
        self.finalized = false;
        let step = self.step_mut_or_append(name);
        if let Some(status) = update.status {
            step.status = status;
        }
        if let Some(display_name) = update.display_name {
            step.display_name = display_name;
        }
        if let Some(output) = update.output {
            step.output = Some(output);
        }
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
mod tests {
    use super::*;

    fn names(state: &State) -> Vec<&str> {
        state.steps.iter().map(|s| s.name.as_str()).collect()
    }

    fn update(status: Status) -> StepUpdate {
        StepUpdate {
            status: Some(status),
            ..StepUpdate::default()
        }
    }

    #[test]
    fn display_names_are_title_cased() {
        assert_eq!(display_name_for("rules-check"), "Rules Check");
        assert_eq!(display_name_for("plan"), "Plan");
        assert_eq!(display_name_for("bar step"), "Bar Step");
        assert_eq!(display_name_for("tf_apply"), "Tf Apply");
    }

    #[test]
    fn declared_steps_start_pending() {
        let mut state = State::new("1");
        state.declare(&["init".into(), "plan".into()]);
        assert_eq!(names(&state), ["init", "plan"]);
        assert!(state.steps.iter().all(|s| s.status == Status::Pending));
    }

    #[test]
    fn later_declarations_merge_keeping_first_order() {
        let mut state = State::new("1");
        state.declare(&["init".into(), "plan".into()]);
        state.declare(&["plan".into(), "apply".into(), "init".into()]);
        assert_eq!(names(&state), ["init", "plan", "apply"]);
    }

    #[test]
    fn undeclared_steps_are_appended() {
        let mut state = State::new("1");
        state.declare(&["init".into(), "plan".into()]);
        state.record("fmt", update(Status::Success));
        assert_eq!(names(&state), ["init", "plan", "fmt"]);
    }

    #[test]
    fn redeclaring_does_not_reset_status() {
        let mut state = State::new("1");
        state.record("plan", update(Status::Success));
        state.declare(&["plan".into()]);
        assert_eq!(state.step("plan").unwrap().status, Status::Success);
    }

    #[test]
    fn recording_without_output_keeps_existing_output() {
        let mut state = State::new("1");
        state.record(
            "plan",
            StepUpdate {
                status: Some(Status::Running),
                output: Some("log".into()),
                ..StepUpdate::default()
            },
        );
        state.record("plan", update(Status::Success));
        assert_eq!(state.step("plan").unwrap().output.as_deref(), Some("log"));
    }

    #[test]
    fn display_name_override() {
        let mut state = State::new("1");
        state.record(
            "rules-check",
            StepUpdate {
                display_name: Some("OPA Rules".into()),
                ..StepUpdate::default()
            },
        );
        assert_eq!(state.step("rules-check").unwrap().display_name, "OPA Rules");
    }

    #[test]
    fn finalize_skips_pending_and_cancels_running() {
        let mut state = State::new("1");
        state.declare(&["init".into(), "plan".into(), "apply".into()]);
        state.record("init", update(Status::Success));
        state.record("plan", update(Status::Running));
        state.finalize();
        assert_eq!(state.step("init").unwrap().status, Status::Success);
        assert_eq!(state.step("plan").unwrap().status, Status::Cancelled);
        assert_eq!(state.step("apply").unwrap().status, Status::Skipped);
        assert!(state.finalized);
        assert!(state.is_failing());
    }

    #[test]
    fn custom_statuses_are_not_failing() {
        let mut state = State::new("1");
        state.record("plan", update(Status::Custom("changes".into())));
        assert!(!state.is_failing());
    }

    #[test]
    fn recording_after_finalize_reopens_the_run() {
        let mut state = State::new("1");
        state.finalize();
        state.record("plan", update(Status::Running));
        assert!(!state.finalized);
    }

    #[test]
    fn new_run_starts_fresh_and_same_run_carries_on() {
        let mut previous = State::new("1");
        previous.record("plan", update(Status::Success));

        let same = State::for_run(Some(previous.clone()), "1");
        assert_eq!(names(&same), ["plan"]);

        let fresh = State::for_run(Some(previous), "2");
        assert!(fresh.steps.is_empty());
        assert_eq!(fresh.run_id, "2");
    }

    #[test]
    fn output_is_not_serialized() {
        let mut state = State::new("1");
        state.record(
            "plan",
            StepUpdate {
                output: Some("secret-ish log".into()),
                ..StepUpdate::default()
            },
        );
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("secret-ish log"));
    }
}
