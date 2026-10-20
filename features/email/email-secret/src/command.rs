//! Shell-out resolver. `tokio::process::Command` + timeout.
//! Trim a single trailing newline so `printf "%s\n" "$secret"`
//! shaped scripts (the common case) round-trip cleanly.

use crate::{SecretError, SecretValue};
use std::time::Duration;
use tokio::process::Command;

pub async fn run(argv: &[String], timeout: Duration) -> Result<SecretValue, SecretError> {
    let (bin, rest) = argv.split_first().ok_or(SecretError::EmptyArgv)?;
    let fut = Command::new(bin).args(rest).output();
    let output = match tokio::time::timeout(timeout, fut).await {
        Ok(r) => r?,
        Err(_) => return Err(SecretError::CommandTimedOut),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(SecretError::CommandFailed(format!(
            "{:?} {}",
            argv,
            stderr.trim()
        )));
    }
    let mut s = String::from_utf8(output.stdout).map_err(|_| SecretError::NonUtf8)?;
    // `printf "%s\n"` is the common output shape — strip one
    // trailing newline. Be permissive about CRLF.
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    Ok(SecretValue::new(s))
}

/// [`run`] without a runtime: `std::process`, polled against the same
/// timeout, killed when it runs over.
pub fn run_blocking(argv: &[String], timeout: Duration) -> Result<SecretValue, SecretError> {
    use std::io::Read;
    let (bin, rest) = argv.split_first().ok_or(SecretError::EmptyArgv)?;
    let mut child = std::process::Command::new(bin)
        .args(rest)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SecretError::CommandTimedOut);
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut stdout)?;
    }
    if let Some(mut err) = child.stderr.take() {
        err.read_to_end(&mut stderr)?;
    }
    if !status.success() {
        return Err(SecretError::CommandFailed(format!(
            "{:?} {}",
            argv,
            String::from_utf8_lossy(&stderr).trim()
        )));
    }
    let mut s = String::from_utf8(stdout).map_err(|_| SecretError::NonUtf8)?;
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    Ok(SecretValue::new(s))
}
