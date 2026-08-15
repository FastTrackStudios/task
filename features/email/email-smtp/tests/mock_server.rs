//! Tiny scripted SMTP submission server for integration tests.
//! Same shape as the IMAP mock — binds 127.0.0.1:0, scripts the
//! greeting + EHLO + AUTH + MAIL/RCPT/DATA dance, logs every
//! client line so tests can assert wire shape after sending.
//!
//! Not a generic SMTP server — only the verbs `mail-send`
//! actually issues are handled.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

#[derive(Debug, Default, Clone)]
pub struct Transcript {
    pub lines: Vec<String>,
    /// Bytes of the `DATA` payload, sans the terminating `\r\n.\r\n`.
    pub data_payload: Vec<u8>,
}

impl Transcript {
    #[must_use]
    pub fn contains(&self, needle: &str) -> bool {
        self.lines.iter().any(|l| l.contains(needle))
    }
    #[must_use]
    pub fn payload_str(&self) -> String {
        String::from_utf8_lossy(&self.data_payload).into_owned()
    }
}

pub struct MockServer {
    pub addr: SocketAddr,
    pub transcript: Arc<Mutex<Transcript>>,
    _task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    pub async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let transcript = Arc::new(Mutex::new(Transcript::default()));
        let handle = transcript.clone();
        let task = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                handle_connection(stream, handle).await;
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

    let _ = wr.write_all(b"220 mock.local ESMTP ready\r\n").await;

    let mut in_data = false;
    let mut line = String::new();
    loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            return;
        }

        if in_data {
            // Data phase: every line until `.\r\n` is payload.
            if line == ".\r\n" || line == ".\n" {
                in_data = false;
                let _ = wr.write_all(b"250 OK queued\r\n").await;
                continue;
            }
            let bytes = line.as_bytes();
            // RFC 5321 §4.5.2: a leading `.` is doubled by the
            // client; un-double it before recording.
            let stripped = if bytes.starts_with(b"..") {
                &bytes[1..]
            } else {
                bytes
            };
            log.lock().await.data_payload.extend_from_slice(stripped);
            continue;
        }

        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        log.lock().await.lines.push(trimmed.clone());
        let upper = trimmed.to_uppercase();

        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            let _ = wr
                .write_all(
                    b"250-mock.local hello\r\n\
                      250-PIPELINING\r\n\
                      250-8BITMIME\r\n\
                      250-SMTPUTF8\r\n\
                      250 AUTH PLAIN LOGIN\r\n",
                )
                .await;
        } else if upper.starts_with("AUTH PLAIN") {
            // mail-send sends `AUTH PLAIN <base64>` inline.
            let _ = wr
                .write_all(b"235 2.7.0 Authentication successful\r\n")
                .await;
        } else if upper.starts_with("AUTH LOGIN") {
            // Two-step AUTH LOGIN: prompt for username then
            // password, both base64.
            let _ = wr.write_all(b"334 VXNlcm5hbWU6\r\n").await;
            line.clear();
            let _ = reader.read_line(&mut line).await;
            let _ = wr.write_all(b"334 UGFzc3dvcmQ6\r\n").await;
            line.clear();
            let _ = reader.read_line(&mut line).await;
            let _ = wr.write_all(b"235 2.7.0 OK\r\n").await;
        } else if upper.starts_with("MAIL FROM") {
            let _ = wr.write_all(b"250 2.1.0 sender ok\r\n").await;
        } else if upper.starts_with("RCPT TO") {
            let _ = wr.write_all(b"250 2.1.5 recipient ok\r\n").await;
        } else if upper.starts_with("DATA") {
            let _ = wr
                .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .await;
            in_data = true;
        } else if upper.starts_with("QUIT") {
            let _ = wr.write_all(b"221 2.0.0 bye\r\n").await;
            return;
        } else if upper.starts_with("RSET") || upper.starts_with("NOOP") {
            let _ = wr.write_all(b"250 2.0.0 ok\r\n").await;
        } else {
            let _ = wr.write_all(b"500 5.5.2 syntax error\r\n").await;
        }
    }
}
