use std::io::{Read, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub struct Executed {
    pub exit_code: i32,
    /// stdout and stderr merged, in the order they were printed.
    pub output: String,
}

/// Runs `command` under `bash -o pipefail`, streaming its output to `echo` while capturing it.
pub fn execute(command: &str, echo: &mut impl Write) -> Result<Executed> {
    // `exec 2>&1` merges stderr into the one pipe so ordering is preserved.
    let mut child = Command::new("bash")
        .args(["-o", "pipefail", "-c", &format!("exec 2>&1\n{command}")])
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to start bash")?;

    let mut pipe = child.stdout.take().expect("stdout is piped");
    let mut captured = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = pipe.read(&mut buf).context("reading command output")?;
        if n == 0 {
            break;
        }
        echo.write_all(&buf[..n])?;
        echo.flush()?;
        captured.extend_from_slice(&buf[..n]);
    }
    let status = child.wait().context("waiting for command")?;
    Ok(Executed {
        exit_code: exit_code(status),
        output: String::from_utf8_lossy(&captured).into_owned(),
    })
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_merged_output_in_order_and_echoes_it() {
        let mut echo = Vec::new();
        let result = execute("echo one; echo two >&2; echo three", &mut echo).unwrap();
        assert_eq!(result.output, "one\ntwo\nthree\n");
        assert_eq!(String::from_utf8(echo).unwrap(), result.output);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn pipefail_surfaces_upstream_failure() {
        let result = execute("false | cat", &mut Vec::new()).unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn signals_map_to_128_plus_signal() {
        let result = execute("kill -9 $$", &mut Vec::new()).unwrap();
        assert_eq!(result.exit_code, 137);
    }
}
