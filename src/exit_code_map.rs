use std::collections::BTreeMap;
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};

use crate::status::Status;

/// The rules that turn a command's exit code into a Status for its Step.
#[derive(Debug, Clone)]
pub struct ExitCodeMap(BTreeMap<i32, Status>);

impl ExitCodeMap {
    /// The mapped Status; unmapped exit codes are `failure`.
    pub fn status_for(&self, code: i32) -> Status {
        self.0.get(&code).cloned().unwrap_or(Status::Failure)
    }
}

impl FromStr for ExitCodeMap {
    type Err = anyhow::Error;

    /// Parses `0=success,1=failure,2=changes`.
    fn from_str(s: &str) -> Result<Self> {
        let mut map = BTreeMap::new();
        for entry in s.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let (code, status) = entry.split_once('=').ok_or_else(|| {
                anyhow!("invalid exit code map entry {entry:?}: expected CODE=STATUS")
            })?;
            let code: i32 = code
                .trim()
                .parse()
                .with_context(|| format!("invalid exit code in {entry:?}"))?;
            map.insert(code, status.trim().into());
        }
        Ok(ExitCodeMap(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries() {
        let map: ExitCodeMap = "0=success, 1=failure,2=changes".parse().unwrap();
        assert_eq!(map.status_for(0), Status::Success);
        assert_eq!(map.status_for(2), Status::Custom("changes".into()));
        assert_eq!(map.status_for(137), Status::Failure);
    }

    #[test]
    fn rejects_malformed_entries() {
        assert!("0success".parse::<ExitCodeMap>().is_err());
        assert!("x=success".parse::<ExitCodeMap>().is_err());
    }
}
