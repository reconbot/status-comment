use serde::{Deserialize, Serialize};

use crate::status::Status;

/// One row in a Status Comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub status: Status,
    /// Never serialized: Output lives only in the rendered comment (ADR 0001).
    #[serde(skip)]
    pub output: Option<String>,
}

impl Step {
    pub fn new(name: &str) -> Self {
        Step {
            name: name.to_string(),
            status: Status::Pending,
            output: None,
        }
    }
}

/// The status-comment version, recorded in every state it writes.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything about a Run that the Status Comment remembers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The status-comment version that wrote it; another version's state is never read.
    pub version: String,
    pub run_id: String,
    /// The Run's re-run attempt that last wrote this state.
    pub attempt: u32,
    pub finalized: bool,
    pub steps: Vec<Step>,
}

impl State {
    pub fn new(run_id: &str, attempt: u32) -> Self {
        State {
            version: VERSION.to_string(),
            run_id: run_id.to_string(),
            attempt,
            finalized: false,
            steps: Vec::new(),
        }
    }

    /// The previous state if it's the same Run, otherwise a fresh one.
    /// Either way it's now written by `attempt`.
    pub fn for_run(previous: Option<State>, run_id: &str, attempt: u32) -> State {
        match previous {
            Some(state) if state.run_id == run_id => State { attempt, ..state },
            _ => State::new(run_id, attempt),
        }
    }

    /// Whether this state was written by a later Run, or a later attempt of the
    /// same Run, than `run_id`/`attempt`. Run IDs only grow; non-numeric ones
    /// (Local Mode) are never newer.
    pub fn is_newer_than(&self, run_id: &str, attempt: u32) -> bool {
        match (self.run_id.parse::<u64>(), run_id.parse::<u64>()) {
            (Ok(theirs), Ok(ours)) => (theirs, self.attempt) > (ours, attempt),
            _ => false,
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

    /// Sets the Steps to exactly `names`, in that order.
    /// Listed Steps keep their Status and Output; new ones start `pending`.
    pub fn set_steps(&mut self, names: &[String]) {
        let mut previous = std::mem::take(&mut self.steps);
        for name in names {
            if self.step(name).is_some() {
                continue;
            }
            let step = match previous.iter().position(|s| &s.name == name) {
                Some(i) => previous.remove(i),
                None => Step::new(name),
            };
            self.steps.push(step);
        }
    }

    /// Sets a Step's Status, and its Output when given; unknown Steps are appended.
    pub fn record(&mut self, name: &str, status: Status, output: Option<String>) {
        let step = self.step_mut_or_append(name);
        step.status = status;
        if let Some(output) = output {
            step.output = Some(output);
        }
    }

    /// Every Step back to `pending` with no Output.
    pub fn reset(&mut self) {
        for step in &mut self.steps {
            step.status = Status::Pending;
            step.output = None;
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

    fn names(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    fn step_names(state: &State) -> Vec<&str> {
        state.steps.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn set_steps_sets_exactly_the_listed_steps_in_order() {
        let mut state = State::new("1", 1);
        state.set_steps(&names(&["init", "plan", "apply"]));
        state.set_steps(&names(&["plan", "validate", "init", "plan"]));
        assert_eq!(step_names(&state), ["plan", "validate", "init"]);
        assert_eq!(state.step("validate").unwrap().status, Status::Pending);
    }

    #[test]
    fn set_steps_keeps_status_and_output_of_listed_steps() {
        let mut state = State::new("1", 1);
        state.record("plan", Status::Success, Some("log".into()));
        state.set_steps(&names(&["plan"]));
        let plan = state.step("plan").unwrap();
        assert_eq!(plan.status, Status::Success);
        assert_eq!(plan.output.as_deref(), Some("log"));
    }

    #[test]
    fn reset_keeps_steps_but_clears_their_data() {
        let mut state = State::new("1", 1);
        state.set_steps(&names(&["plan"]));
        state.record("plan", Status::Failure, Some("log".into()));
        state.reset();
        let plan = state.step("plan").unwrap();
        assert_eq!(plan.status, Status::Pending);
        assert_eq!(plan.output, None);
    }

    #[test]
    fn steps_recorded_after_set_steps_are_appended() {
        let mut state = State::new("1", 1);
        state.set_steps(&names(&["init", "plan"]));
        state.record("fmt", Status::Success, None);
        assert_eq!(step_names(&state), ["init", "plan", "fmt"]);
    }

    #[test]
    fn recording_without_output_keeps_existing_output() {
        let mut state = State::new("1", 1);
        state.record("plan", Status::Running, Some("log".into()));
        state.record("plan", Status::Success, None);
        assert_eq!(state.step("plan").unwrap().output.as_deref(), Some("log"));
    }

    #[test]
    fn finalize_skips_pending_and_cancels_running() {
        let mut state = State::new("1", 1);
        state.set_steps(&names(&["init", "plan", "apply"]));
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
    fn new_run_starts_fresh_and_same_run_carries_on() {
        let mut previous = State::new("1", 1);
        previous.record("plan", Status::Success, None);

        let same = State::for_run(Some(previous.clone()), "1", 2);
        assert_eq!(step_names(&same), ["plan"]);
        assert_eq!(same.attempt, 2);

        let fresh = State::for_run(Some(previous), "2", 1);
        assert!(fresh.steps.is_empty());
        assert_eq!(fresh.run_id, "2");
    }

    #[test]
    fn newer_runs_and_attempts_are_newer() {
        let state = State::for_run(None, "200", 2);
        assert!(state.is_newer_than("199", 5));
        assert!(state.is_newer_than("200", 1));
        assert!(!state.is_newer_than("200", 2));
        assert!(!state.is_newer_than("200", 3));
        assert!(!state.is_newer_than("201", 1));
        assert!(!State::new("local", 2).is_newer_than("local", 1));
    }

    #[test]
    fn output_is_not_serialized() {
        let mut state = State::new("1", 1);
        state.record("plan", Status::Success, Some("secret-ish log".into()));
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("secret-ish log"));
    }
}
