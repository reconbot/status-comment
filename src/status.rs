use std::fmt;

use serde::{Deserialize, Serialize};

/// The current state of a Step: either a Built-in Status or a Custom Status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Status {
    Pending,
    Running,
    Success,
    Failure,
    Skipped,
    Cancelled,
    Custom(String),
}

impl Status {
    /// A Step is Failing when its Status is `failure` or `cancelled`.
    pub fn is_failing(&self) -> bool {
        matches!(self, Status::Failure | Status::Cancelled)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Success => "success",
            Status::Failure => "failure",
            Status::Skipped => "skipped",
            Status::Cancelled => "cancelled",
            Status::Custom(s) => s,
        }
    }
}

impl From<&str> for Status {
    fn from(s: &str) -> Self {
        match s {
            "pending" => Status::Pending,
            "running" => Status::Running,
            "success" => Status::Success,
            "failure" => Status::Failure,
            "skipped" => Status::Skipped,
            "cancelled" => Status::Cancelled,
            other => Status::Custom(other.to_string()),
        }
    }
}

impl From<String> for Status {
    fn from(s: String) -> Self {
        s.as_str().into()
    }
}

impl From<Status> for String {
    fn from(s: Status) -> String {
        s.as_str().to_string()
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
