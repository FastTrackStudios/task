//! Minimal cross-cutting helpers for the vertical-slice CLI.
//!
//! Holds [`RemoteVoxConfig`] — endpoint URL resolution for the
//! `task doctor` probe. The previous `LiveSession` /
//! `ServerRegistry` machinery (synced local `CrdtDoc` over
//! `WorkspaceSync`, per-server token registry) was ripped along
//! with the Loro entity layer. The endpoint-resolution logic
//! stays so future commands that hit a remote vox surface (e.g.
//! `AuthService::sign_in`) don't have to re-derive URL shaping.
//!
//! Also holds the small cross-command helpers (`confirm`,
//! `resolve_body`, `short_uuid`, `git`) that more than one
//! command module needs.

#[derive(Debug, Clone)]
pub(crate) struct RemoteVoxConfig {
    pub(crate) display_url: String,
}

impl RemoteVoxConfig {
    pub(crate) fn from_args(
        server: String,
        session_token: Option<String>,
        organization_id: Option<String>,
    ) -> eyre::Result<Self> {
        let base = normalize_vox_url(&server);
        let mut display_url = base;
        if let Some(_token) = session_token.as_deref().filter(|s| !s.is_empty()) {
            append_query_param(&mut display_url, "token", "<redacted>");
        }
        if let Some(org) = organization_id.as_deref().filter(|s| !s.is_empty()) {
            append_query_param(&mut display_url, "organization_id", org);
        }
        Ok(Self { display_url })
    }
}

fn normalize_vox_url(server: &str) -> String {
    let trimmed = server.trim().trim_end_matches('/');
    if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        trimmed.to_string()
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{}/vox", rest.trim_end_matches("/vox"))
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{}/vox", rest.trim_end_matches("/vox"))
    } else {
        format!("ws://{}/vox", trimmed.trim_end_matches("/vox"))
    }
}

fn append_query_param(url: &mut String, key: &str, value: &str) {
    let separator = if url.contains('?') { '&' } else { '?' };
    url.push(separator);
    url.push_str(key);
    url.push('=');
    url.push_str(&percent_encode_query_value(value));
}

fn percent_encode_query_value(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub(crate) fn resolve_body(arg: Option<String>) -> eyre::Result<String> {
    use std::io::Read;
    match arg {
        None => Ok(String::new()),
        Some(s) if s == "-" => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            Ok(buf)
        }
        Some(s) => Ok(s),
    }
}

pub(crate) fn confirm(prompt: &str) -> eyre::Result<bool> {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    write!(out, "{prompt} [y/N] ")?;
    out.flush()?;
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Is stdin a terminal? Prompting only makes sense when a person is
/// there to answer — piped or CI invocations must fail with the
/// "pass --email" error instead of blocking forever on a read.
pub(crate) fn stdin_is_tty() -> bool {
    // SAFETY: `isatty` reads kernel state for a fd and has no
    // preconditions; fd 0 is always valid to ask about.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

/// Prompt for a line of visible input (an email, a name).
pub(crate) fn prompt_line(label: &str) -> eyre::Result<String> {
    use std::io::{BufRead, Write};
    let mut out = std::io::stdout();
    write!(out, "{label}: ")?;
    out.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

/// Prompt for a secret with terminal echo disabled, so the password
/// never appears on screen — and, unlike `--password`, never reaches
/// `ps` output or shell history.
///
/// Echo is restored on every path including the error one: leaving a
/// terminal with echo off is the kind of breakage a user has to type
/// `stty sane` blind to escape.
pub(crate) fn prompt_secret(label: &str) -> eyre::Result<String> {
    use std::io::{BufRead, Write};
    let mut out = std::io::stdout();
    write!(out, "{label}: ")?;
    out.flush()?;

    let guard = EchoGuard::disable()?;
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    drop(guard);
    // The newline the user typed was swallowed with the echo.
    writeln!(out)?;
    read?;
    Ok(line.trim_end_matches(['\n', '\r']).to_owned())
}

/// Terminal echo turned off for as long as this lives. `Drop` restores
/// the original termios, so a `?` between here and the end of the read
/// can't strand the terminal.
struct EchoGuard(Option<libc::termios>);

impl EchoGuard {
    fn disable() -> eyre::Result<Self> {
        // Not a terminal (piped stdin) — nothing to mask, and
        // tcgetattr would fail. Read it plainly.
        if !stdin_is_tty() {
            return Ok(Self(None));
        }
        // SAFETY: `termios` is a plain C struct with no invalid bit
        // patterns for our purposes; tcgetattr fully initializes it
        // when it returns 0, and we only read it on that path.
        let mut term: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: fd 0 is a terminal (checked above) and `term` is a
        // valid, properly-aligned out-pointer.
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut term) } != 0 {
            return Err(eyre::eyre!("read terminal settings: {}", last_os_error()));
        }
        let original = term;
        term.c_lflag &= !libc::ECHO;
        // SAFETY: same fd, and `term` is an initialized termios.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term) } != 0 {
            return Err(eyre::eyre!("disable terminal echo: {}", last_os_error()));
        }
        Ok(Self(Some(original)))
    }
}

impl Drop for EchoGuard {
    fn drop(&mut self) {
        if let Some(original) = self.0 {
            // SAFETY: restoring the exact termios we captured.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &original);
            }
        }
    }
}

fn last_os_error() -> String {
    std::io::Error::last_os_error().to_string()
}

pub(crate) fn short_uuid(u: &uuid::Uuid) -> String {
    let s = u.to_string();
    s.chars().take(8).collect()
}

/// Run `git <args>` in the cwd, returning trimmed stdout.
pub(crate) fn git(args: &[&str]) -> eyre::Result<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|e| eyre::eyre!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(eyre::eyre!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
