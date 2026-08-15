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
