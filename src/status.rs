use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// The current state of a Step: either a Built-in Status or a Custom Status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
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

    pub fn is_builtin(&self) -> bool {
        !matches!(self, Status::Custom(_))
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

impl FromStr for Status {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        // `=` and `,` are separators in an Exit Code Map.
        if s.is_empty()
            || s.chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '=' || c == ',')
        {
            bail!("invalid status {s:?}: must be non-empty with no whitespace, `=` or `,`");
        }
        Ok(match s {
            "pending" => Status::Pending,
            "running" => Status::Running,
            "success" => Status::Success,
            "failure" => Status::Failure,
            "skipped" => Status::Skipped,
            // GitHub spells it `cancelled`; accept the American spelling as an alias.
            "cancelled" | "canceled" => Status::Cancelled,
            other => Status::Custom(other.to_string()),
        })
    }
}

impl TryFrom<String> for Status {
    type Error = anyhow::Error;

    fn try_from(s: String) -> Result<Self> {
        s.parse()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_builtins() {
        for s in [
            "pending",
            "running",
            "success",
            "failure",
            "skipped",
            "cancelled",
        ] {
            let status: Status = s.parse().unwrap();
            assert!(status.is_builtin(), "{s}");
            assert_eq!(status.as_str(), s);
        }
    }

    #[test]
    fn canceled_is_an_alias_of_cancelled() {
        assert_eq!("canceled".parse::<Status>().unwrap(), Status::Cancelled);
    }

    #[test]
    fn unknown_values_are_custom() {
        let status: Status = "changes".parse().unwrap();
        assert_eq!(status, Status::Custom("changes".into()));
        assert!(!status.is_builtin());
    }

    #[test]
    fn only_failure_and_cancelled_are_failing() {
        assert!(Status::Failure.is_failing());
        assert!(Status::Cancelled.is_failing());
        for s in [
            Status::Pending,
            Status::Running,
            Status::Success,
            Status::Skipped,
        ] {
            assert!(!s.is_failing(), "{s}");
        }
        assert!(!Status::Custom("changes".into()).is_failing());
    }

    #[test]
    fn rejects_empty_and_whitespace() {
        assert!("".parse::<Status>().is_err());
        assert!("two words".parse::<Status>().is_err());
        assert!("line\nbreak".parse::<Status>().is_err());
        assert!("a=b".parse::<Status>().is_err());
        assert!("a,b".parse::<Status>().is_err());
    }

    #[test]
    fn serializes_as_plain_string() {
        let json = serde_json::to_string(&Status::Custom("changes".into())).unwrap();
        assert_eq!(json, "\"changes\"");
        let back: Status = serde_json::from_str("\"canceled\"").unwrap();
        assert_eq!(back, Status::Cancelled);
    }
}
