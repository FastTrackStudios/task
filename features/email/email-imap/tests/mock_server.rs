//! Tiny scripted IMAP server for integration tests. Binds
//! 127.0.0.1:0, accepts one connection, and replies to a fixed
//! sequence of commands the test drives through `Backend`.
//!
//! Why hand-rolled instead of testcontainers + mailpit: this
//! runs in-process on every `cargo test`, no docker, no network,
//! ~10ms per test, fully deterministic. The testcontainers
//! suite is gated separately for cross-cutting integration
//! (see plan).
//!
//! The mock is **not a generic IMAP server.** It pattern-matches
//! the tag + verb of each command and replies with a canned
//! response that's good enough for our backend's specific
//! command shapes (LOGIN, LIST, SELECT, UID FETCH, UID STORE,
//! UID EXPUNGE, UID SEARCH, APPEND, LOGOUT, IDLE/DONE). Adding
//! a new test command shape means adding a new pattern here.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// Snapshot of what the client sent. Tests assert against this
/// after exercising the backend.
#[derive(Debug, Default, Clone)]
pub struct Transcript {
    pub lines: Vec<String>,
}

impl Transcript {
    #[must_use]
    pub fn contains(&self, needle: &str) -> bool {
        self.lines.iter().any(|l| l.contains(needle))
    }
    #[allow(dead_code)] // used by other tests in the suite once they wire it
    #[must_use]
    pub fn count(&self, needle: &str) -> usize {
        self.lines.iter().filter(|l| l.contains(needle)).count()
    }
}

pub struct MockServer {
    pub addr: SocketAddr,
    pub transcript: Arc<Mutex<Transcript>>,
    _task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    /// Spawn a server that serves `connections` accept cycles
    /// before exiting. Most tests want `connections = 1` (one
    /// backend call per test), but `locate_uid` opens fresh
    /// sessions per folder it scans — those tests pass a
    /// larger count.
    pub async fn spawn(connections: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let transcript = Arc::new(Mutex::new(Transcript::default()));
        let transcript_handle = transcript.clone();
        let task = tokio::spawn(async move {
            for _ in 0..connections {
                let (stream, _) = match listener.accept().await {
                    Ok(p) => p,
                    Err(_) => return,
                };
                let log = transcript_handle.clone();
                tokio::spawn(async move {
                    handle_connection(stream, log).await;
                });
            }
        });
        Self {
            addr,
            transcript,
            _task: task,
        }
    }
}

async fn handle_connection(stream: TcpStream, log: Arc<Mutex<Transcript>>) {
    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);
    let mut buf = String::new();

    // Greeting.
    let _ = wr
        .write_all(b"* OK [CAPABILITY IMAP4rev1 LITERAL+ UIDPLUS MOVE IDLE] mock\r\n")
        .await;

    loop {
        buf.clear();
        let n = match reader.read_line(&mut buf).await {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            return;
        }
        let line = buf.trim_end_matches(['\r', '\n']).to_string();
        log.lock().await.lines.push(line.clone());

        let parts: Vec<&str> = line.splitn(3, ' ').collect();
        let tag = parts.first().copied().unwrap_or("*");
        let verb = parts.get(1).copied().unwrap_or("").to_uppercase();
        let rest = parts.get(2).copied().unwrap_or("");

        match verb.as_str() {
            "LOGIN" => {
                let _ = wr
                    .write_all(format!("{tag} OK LOGIN completed\r\n").as_bytes())
                    .await;
            }
            "LIST" => {
                let _ = wr
                    .write_all(
                        b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
                          * LIST (\\HasNoChildren \\Sent) \"/\" \"Sent\"\r\n\
                          * LIST (\\HasNoChildren \\Drafts) \"/\" \"Drafts\"\r\n",
                    )
                    .await;
                let _ = wr
                    .write_all(format!("{tag} OK LIST completed\r\n").as_bytes())
                    .await;
            }
            "SELECT" => {
                let _ = wr
                    .write_all(
                        b"* 2 EXISTS\r\n\
                          * 0 RECENT\r\n\
                          * OK [UIDVALIDITY 1] UIDs valid\r\n\
                          * OK [UIDNEXT 3] next UID\r\n\
                          * FLAGS (\\Seen \\Answered \\Flagged \\Deleted \\Draft)\r\n",
                    )
                    .await;
                let _ = wr
                    .write_all(format!("{tag} OK [READ-WRITE] SELECT completed\r\n").as_bytes())
                    .await;
            }
            "UID" => {
                // `UID FETCH …`, `UID STORE …`, `UID EXPUNGE …`,
                // `UID SEARCH …`. rest holds the sub-verb + args.
                let sub = rest.split_whitespace().next().unwrap_or("").to_uppercase();
                match sub.as_str() {
                    "FETCH" => {
                        // Reply with two synthetic envelopes. We
                        // serve a 0-byte header literal so the
                        // parser doesn't actually need bytes —
                        // backend `envelope_from_bytes` synth's
                        // a message-id from the empty header.
                        let header = "Message-ID: <m1@mock>\r\n\
                                      From: a@example.com\r\n\
                                      To: you@example.com\r\n\
                                      Subject: First\r\n\
                                      Date: Mon, 14 Nov 2023 12:00:00 +0000\r\n\
                                      \r\n";
                        let header2 = "Message-ID: <m2@mock>\r\n\
                                       From: b@example.com\r\n\
                                       To: you@example.com\r\n\
                                       Subject: Second\r\n\
                                       Date: Mon, 14 Nov 2023 13:00:00 +0000\r\n\
                                       \r\n";
                        // The literal `{N}\r\n` is followed by
                        // exactly N bytes (the header, which
                        // already ends `\r\n\r\n`) and then the
                        // rest of the FETCH paren-list closes.
                        // No extra CRLF between header and `)`.
                        let _ = wr
                            .write_all(
                                format!(
                                    "* 1 FETCH (UID 1 FLAGS (\\Seen) RFC822.SIZE {} BODY[HEADER] {{{}}}\r\n{header})\r\n",
                                    header.len(),
                                    header.len()
                                )
                                .as_bytes(),
                            )
                            .await;
                        let _ = wr
                            .write_all(
                                format!(
                                    "* 2 FETCH (UID 2 FLAGS () RFC822.SIZE {} BODY[HEADER] {{{}}}\r\n{header2})\r\n",
                                    header2.len(),
                                    header2.len()
                                )
                                .as_bytes(),
                            )
                            .await;
                        let _ = wr
                            .write_all(format!("{tag} OK UID FETCH completed\r\n").as_bytes())
                            .await;
                    }
                    "STORE" => {
                        // Echo a FETCH with the new flags so
                        // async-imap's STORE-result stream
                        // produces an item.
                        let _ = wr
                            .write_all(b"* 1 FETCH (UID 1 FLAGS (\\Seen \\Answered))\r\n")
                            .await;
                        let _ = wr
                            .write_all(format!("{tag} OK UID STORE completed\r\n").as_bytes())
                            .await;
                    }
                    "EXPUNGE" => {
                        let _ = wr.write_all(b"* 1 EXPUNGE\r\n").await;
                        let _ = wr
                            .write_all(format!("{tag} OK UID EXPUNGE completed\r\n").as_bytes())
                            .await;
                    }
                    "SEARCH" => {
                        let _ = wr.write_all(b"* SEARCH 1\r\n").await;
                        let _ = wr
                            .write_all(format!("{tag} OK UID SEARCH completed\r\n").as_bytes())
                            .await;
                    }
                    _ => {
                        let _ = wr
                            .write_all(format!("{tag} BAD unknown UID sub {sub}\r\n").as_bytes())
                            .await;
                    }
                }
            }
            "APPEND" => {
                // `APPEND <mailbox> (flags) {literal-bytes}`.
                // We need to read the literal bytes so the
                // client side doesn't hang. Look for `{N}` at
                // end of `rest` and slurp that many bytes plus
                // the trailing CRLF.
                if let Some(brace) = rest.rfind('{') {
                    if let Some(close) = rest[brace..].find('}') {
                        let nbytes: usize = rest[brace + 1..brace + close]
                            .trim_end_matches('+')
                            .parse()
                            .unwrap_or(0);
                        let _ = wr.write_all(b"+ Ready for literal\r\n").await;
                        let mut payload = vec![0u8; nbytes];
                        use tokio::io::AsyncReadExt;
                        let _ = reader.read_exact(&mut payload).await;
                        // Read trailing CRLF.
                        buf.clear();
                        let _ = reader.read_line(&mut buf).await;
                    }
                }
                let _ = wr
                    .write_all(format!("{tag} OK APPEND completed\r\n").as_bytes())
                    .await;
            }
            "IDLE" => {
                let _ = wr.write_all(b"+ idling\r\n").await;
                // Wait for DONE on the same stream.
                buf.clear();
                let _ = reader.read_line(&mut buf).await;
                log.lock().await.lines.push(buf.trim().to_string());
                let _ = wr
                    .write_all(format!("{tag} OK IDLE done\r\n").as_bytes())
                    .await;
            }
            "LOGOUT" => {
                let _ = wr.write_all(b"* BYE mock signing off\r\n").await;
                let _ = wr
                    .write_all(format!("{tag} OK LOGOUT completed\r\n").as_bytes())
                    .await;
                return;
            }
            "CAPABILITY" => {
                let _ = wr
                    .write_all(b"* CAPABILITY IMAP4rev1 LITERAL+ UIDPLUS MOVE IDLE\r\n")
                    .await;
                let _ = wr
                    .write_all(format!("{tag} OK CAPABILITY completed\r\n").as_bytes())
                    .await;
            }
            _ => {
                let _ = wr
                    .write_all(format!("{tag} BAD unsupported in mock: {verb}\r\n").as_bytes())
                    .await;
            }
        }
    }
}
