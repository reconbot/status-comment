use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;

use anyhow::{Context, Result};

pub struct Executed {
    pub exit_code: i32,
    /// stdout and stderr merged, in the order they arrived.
    pub output: String,
}

/// Runs `command` under `bash -o pipefail`, echoing its stdout to `out` and its
/// stderr to `err` as they arrive, while capturing both into one Output.
pub fn execute(command: &str, out: impl Write + Send, err: impl Write + Send) -> Result<Executed> {
    let mut child = Command::new("bash")
        .args(["-o", "pipefail", "-c", command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start bash")?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    let captured = Mutex::new(Vec::new());
    thread::scope(|scope| {
        let stdout = scope.spawn(|| relay(stdout, out, &captured));
        let stderr = scope.spawn(|| relay(stderr, err, &captured));
        stdout.join().expect("stdout relay panicked")?;
        stderr.join().expect("stderr relay panicked")
    })
    .context("reading command output")?;

    let status = child.wait().context("waiting for command")?;
    let captured = captured.into_inner().expect("relays have finished");
    Ok(Executed {
        exit_code: exit_code(status),
        output: String::from_utf8_lossy(&captured).into_owned(),
    })
}

/// Copies one of the command's streams to `echo` chunk by chunk, appending each
/// chunk to the shared capture as it arrives.
fn relay(
    mut pipe: impl Read,
    mut echo: impl Write,
    captured: &Mutex<Vec<u8>>,
) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    loop {
        let n = pipe.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        let chunk = &buf[..n];
        captured
            .lock()
            .expect("no relay panics holding it")
            .extend_from_slice(chunk);
        echo.write_all(chunk)?;
        echo.flush()?;
    }
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
    fn echoes_each_stream_to_its_own_and_captures_both_in_order() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        // The pauses let each line arrive before the next is printed.
        let result = execute(
            "echo one; sleep 0.1; echo two >&2; sleep 0.1; echo three",
            &mut out,
            &mut err,
        )
        .unwrap();
        assert_eq!(result.output, "one\ntwo\nthree\n");
        assert_eq!(String::from_utf8(out).unwrap(), "one\nthree\n");
        assert_eq!(String::from_utf8(err).unwrap(), "two\n");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn pipefail_surfaces_upstream_failure() {
        let result = execute("false | cat", Vec::new(), Vec::new()).unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn signals_map_to_128_plus_signal() {
        let result = execute("kill -9 $$", Vec::new(), Vec::new()).unwrap();
        assert_eq!(result.exit_code, 137);
    }
}
