use std::collections::BTreeMap;
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};

use crate::status::Status;

/// The rules that turn a command's exit code into a Status for its Step.
#[derive(Debug, Clone)]
pub struct ExitCodeMap(BTreeMap<i32, Status>);

impl Default for ExitCodeMap {
    fn default() -> Self {
        ExitCodeMap(BTreeMap::from([(0, Status::Success)]))
    }
}

impl ExitCodeMap {
    /// The mapped Status, or `None` when the exit code isn't handled.
    pub fn get(&self, code: i32) -> Option<&Status> {
        self.0.get(&code)
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
            let status: Status = status.trim().parse()?;
            if map.insert(code, status).is_some() {
                return Err(anyhow!("exit code {code} is mapped more than once"));
            }
        }
        if map.is_empty() {
            return Err(anyhow!("exit code map is empty"));
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
        assert_eq!(map.get(0), Some(&Status::Success));
        assert_eq!(map.get(1), Some(&Status::Failure));
        assert_eq!(map.get(2), Some(&Status::Custom("changes".into())));
        assert_eq!(map.get(137), None);
    }

    #[test]
    fn rejects_malformed_entries() {
        assert!("0success".parse::<ExitCodeMap>().is_err());
        assert!("x=success".parse::<ExitCodeMap>().is_err());
        assert!("0=".parse::<ExitCodeMap>().is_err());
        assert!("".parse::<ExitCodeMap>().is_err());
        assert!("0=success,0=failure".parse::<ExitCodeMap>().is_err());
    }

    #[test]
    fn missing_comma_is_rejected() {
        // Must not silently parse as one Custom Status named `failure=2=changes`.
        assert!(
            "0=success,1=failure=2=changes"
                .parse::<ExitCodeMap>()
                .is_err()
        );
    }
}
