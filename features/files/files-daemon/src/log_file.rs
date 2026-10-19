//! The agent's own log file, with a ceiling.
//!
//! An installed agent's log used to be launchd's `StandardOutPath`: a file
//! launchd appends to forever and nothing ever trims. On 2026-10-19 a
//! retry loop and an iroh error storm took `~/Library/Logs/task-sync.log`
//! to 8 GB in an afternoon and filled the boot disk — the machine stopped
//! being usable because a background sync agent was unhappy about a
//! folder.
//!
//! So the agent writes its own log and bounds it: past [`CappedLog::max_bytes`]
//! the file is moved aside to `<name>.1` (replacing the previous one) and
//! started again. At most two files of the cap each, whatever the agent
//! is doing — a loop costs some history, never the disk.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Ten megabytes: a day or two of an ordinary agent's `info` lines, and
/// small enough that two of them are nothing on any disk.
pub const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// A log file that rolls over to `<path>.1` instead of growing past a
/// size.
///
/// A plain [`Write`]; wrap it in a `Mutex` to hand it to a tracing
/// subscriber. Each `write` from the formatter is one whole event, so
/// rolling between writes never splits a line.
#[derive(Debug)]
pub struct CappedLog {
    path: PathBuf,
    max_bytes: u64,
    file: std::fs::File,
    written: u64,
}

impl CappedLog {
    /// Open `path` for appending, keeping what is already there — a
    /// restart continues the same file rather than wiping the lines that
    /// explain why it restarted.
    pub fn open(path: impl Into<PathBuf>, max_bytes: u64) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = Self::append(&path)?;
        let written = file.metadata()?.len();
        let mut log = Self {
            path,
            max_bytes,
            file,
            written,
        };
        // A file left over-size by something else (an older agent that
        // wrote through launchd) is rolled now, not after another cap's
        // worth on top of it.
        if log.written >= log.max_bytes {
            log.roll()?;
        }
        Ok(log)
    }

    /// The previous file, once one has been rolled aside.
    #[must_use]
    pub fn rolled_path(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(".1");
        PathBuf::from(name)
    }

    /// The size a file may reach before it is rolled.
    #[must_use]
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn append(path: &Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    }

    fn roll(&mut self) -> std::io::Result<()> {
        self.file.flush()?;
        std::fs::rename(&self.path, Self::rolled_path(&self.path))?;
        self.file = Self::append(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for CappedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written > 0 && self.written + buf.len() as u64 > self.max_bytes {
            // A failed roll is not worth losing the line over — keep
            // writing where we are and try again on the next one.
            let _ = self.roll();
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(path: &Path) -> u64 {
        std::fs::metadata(path).map_or(0, |m| m.len())
    }

    /// The whole point: however much is written, the two files together
    /// never hold much more than twice the cap.
    #[test]
    fn a_runaway_writer_cannot_grow_the_log_past_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task-sync.log");
        let mut log = CappedLog::open(&path, 1024).unwrap();
        let line =
            b"ERROR iroh::socket::remote_map::remote_state: could not close last open path\n";
        for _ in 0..2_000 {
            log.write_all(line).unwrap();
        }
        log.flush().unwrap();
        assert!(size(&path) <= 1024, "current file is {} bytes", size(&path));
        let rolled = CappedLog::rolled_path(&path);
        assert!(
            size(&rolled) <= 1024,
            "rolled file is {} bytes",
            size(&rolled)
        );
        assert!(size(&rolled) > 0, "nothing was kept from before the roll");
    }

    /// Lines are never split across the roll — each write is one event.
    #[test]
    fn every_line_survives_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task-sync.log");
        let mut log = CappedLog::open(&path, 100).unwrap();
        for i in 0..50 {
            log.write_all(format!("line {i:03} of the agent's log\n").as_bytes())
                .unwrap();
        }
        log.flush().unwrap();
        for file in [&path, &CappedLog::rolled_path(&path)] {
            let text = std::fs::read_to_string(file).unwrap();
            assert!(text.ends_with('\n'), "{text:?}");
            for l in text.lines() {
                assert!(
                    l.starts_with("line ") && l.ends_with("log"),
                    "split line {l:?}"
                );
            }
        }
    }

    /// A log an older agent left at gigabytes is rolled on open rather
    /// than appended to.
    #[test]
    fn an_oversized_leftover_is_rolled_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task-sync.log");
        std::fs::write(&path, vec![b'x'; 4096]).unwrap();
        let mut log = CappedLog::open(&path, 1024).unwrap();
        log.write_all(b"fresh\n").unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh\n");
        assert_eq!(size(&CappedLog::rolled_path(&path)), 4096);
    }
}
