//! A streaming tar writer.
//!
//! `files.write.surface` says a selection downloads as a single archive
//! stream, and `files.scale.large-media` says nothing is ever held whole.
//! Those two together rule out building an archive and then serving it —
//! a selection can be a whole root, and materialising one server-side
//! would cost the disk twice and the wait once.
//!
//! Tar is the format because it is genuinely streamable: a header, then
//! the bytes, then the next header, with nothing at the end that has to
//! know what came before. Zip's central directory is written last and
//! carries every entry's offset, so a zip cannot be produced without
//! either buffering or seeking backwards.
//!
//! Written here rather than taken as a dependency because the subset that
//! matters is a 512-byte header and a checksum, and this is the whole of
//! it. USTAR, which every extractor has read for forty years.

use std::io;

use tokio::io::{AsyncWrite, AsyncWriteExt};

const BLOCK: usize = 512;

/// The longest path USTAR can express without extensions: 100 bytes in
/// `name`, or a 155-byte `prefix` and a 100-byte `name` split on a `/`.
const NAME: usize = 100;
const PREFIX: usize = 155;

/// Why a path could not be archived.
#[derive(Debug)]
pub(crate) enum TarError {
    /// USTAR cannot express it, and silently truncating a name would
    /// produce an archive that extracts to the wrong place.
    PathTooLong(String),
    Io(io::Error),
}

impl std::fmt::Display for TarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PathTooLong(p) => write!(f, "{p}: too long for a tar header"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for TarError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Split a path into USTAR's `prefix` and `name` fields.
///
/// The split has to fall on a `/`, so a single component longer than 100
/// bytes is unrepresentable however short the rest is.
fn split_name(path: &str) -> Option<(&str, &str)> {
    if path.len() <= NAME {
        return Some(("", path));
    }
    // Prefer the latest split that leaves a representable name.
    path.char_indices()
        .filter(|(_, c)| *c == '/')
        .map(|(i, _)| i)
        .filter(|i| *i <= PREFIX && path.len() - i - 1 <= NAME)
        .next_back()
        .map(|i| (&path[..i], &path[i + 1..]))
}

fn octal(buf: &mut [u8], value: u64) {
    // USTAR numerics are octal, NUL-terminated, right-aligned in a field
    // one byte wider than the digits.
    let digits = buf.len() - 1;
    let text = format!("{value:0digits$o}");
    buf[..digits].copy_from_slice(text.as_bytes());
    buf[digits] = 0;
}

/// One 512-byte header.
fn header(path: &str, size: u64, mtime: u64, is_dir: bool) -> Result<[u8; BLOCK], TarError> {
    let (prefix, name) = split_name(path).ok_or_else(|| TarError::PathTooLong(path.to_string()))?;

    let mut h = [0u8; BLOCK];
    h[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut h[100..108], if is_dir { 0o755 } else { 0o644 });
    octal(&mut h[108..116], 0); // uid — nobody, on purpose: an archive
    octal(&mut h[116..124], 0); // gid   should not carry our accounts
    octal(&mut h[124..136], if is_dir { 0 } else { size });
    octal(&mut h[136..148], mtime);
    h[156] = if is_dir { b'5' } else { b'0' };
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());

    // The checksum is computed with its own field read as spaces, then
    // written into it — the one self-referential part of the format.
    h[148..156].fill(b' ');
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    let text = format!("{sum:06o}");
    h[148..154].copy_from_slice(text.as_bytes());
    h[154] = 0;
    h[155] = b' ';
    Ok(h)
}

/// Writes entries to `dest` as they are handed over.
pub(crate) struct Tar<W> {
    dest: W,
}

impl<W: AsyncWrite + Unpin> Tar<W> {
    pub(crate) fn new(dest: W) -> Self {
        Self { dest }
    }

    pub(crate) async fn directory(&mut self, path: &str, mtime: u64) -> Result<(), TarError> {
        // Trailing slash is how tar marks a directory in the name as well
        // as the typeflag; some extractors read one and some the other.
        let path = if path.ends_with('/') {
            path.to_string()
        } else {
            format!("{path}/")
        };
        let h = header(&path, 0, mtime, true)?;
        self.dest.write_all(&h).await?;
        Ok(())
    }

    /// Open an entry, then stream exactly `size` bytes into the returned
    /// writer before opening another.
    ///
    /// `size` is declared up front because tar's header precedes its
    /// body — which is what makes the format streamable, and what means
    /// a file changing length mid-archive would corrupt it. The caller
    /// stats and reads under the same root lock for that reason.
    pub(crate) async fn file(
        &mut self,
        path: &str,
        size: u64,
        mtime: u64,
    ) -> Result<Entry<'_, W>, TarError> {
        let h = header(path, size, mtime, false)?;
        self.dest.write_all(&h).await?;
        Ok(Entry {
            dest: &mut self.dest,
            remaining: size,
            declared: size,
        })
    }

    /// Two zero blocks, which is how a reader knows it reached the end
    /// rather than a truncated stream.
    pub(crate) async fn finish(mut self) -> Result<(), TarError> {
        self.dest.write_all(&[0u8; BLOCK * 2]).await?;
        self.dest.flush().await?;
        Ok(())
    }
}

/// One open entry's body.
pub(crate) struct Entry<'a, W> {
    dest: &'a mut W,
    /// Of the declared size, how much body is still owed.
    remaining: u64,
    declared: u64,
}

impl<W: AsyncWrite + Unpin> Entry<'_, W> {
    pub(crate) async fn write(&mut self, bytes: &[u8]) -> Result<(), TarError> {
        let take = bytes.len().min(self.remaining as usize);
        self.dest.write_all(&bytes[..take]).await?;
        self.remaining -= take as u64;
        Ok(())
    }

    /// Finish the entry: make up any shortfall, then pad to a block.
    ///
    /// Two different paddings, and both are needed. A short file — one
    /// that shrank between the stat and the read — is filled with zeroes
    /// to the size the header already declared, because the header
    /// cannot be rewritten once it is on the wire. Then the body is
    /// padded to the next 512-byte boundary, because that is where the
    /// reader will look for the next header.
    ///
    /// Getting either wrong misaligns the stream, and a misaligned tar
    /// is unreadable from that point on rather than merely wrong.
    pub(crate) async fn close(mut self) -> Result<(), TarError> {
        while self.remaining > 0 {
            let run = (self.remaining as usize).min(BLOCK);
            self.dest.write_all(&vec![0u8; run]).await?;
            self.remaining -= run as u64;
        }
        let pad = padding(self.declared);
        if pad > 0 {
            self.dest.write_all(&vec![0u8; pad]).await?;
        }
        Ok(())
    }
}

/// Bytes of padding an entry of `size` needs.
#[must_use]
pub(crate) fn padding(size: u64) -> usize {
    let rem = (size % BLOCK as u64) as usize;
    if rem == 0 { 0 } else { BLOCK - rem }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_path_needs_no_prefix() {
        assert_eq!(split_name("stems/kick.wav"), Some(("", "stems/kick.wav")));
    }

    #[test]
    fn a_long_path_splits_on_a_slash() {
        let deep = format!("{}/kick.wav", "d".repeat(120));
        let (prefix, name) = split_name(&deep).expect("representable");
        assert_eq!(prefix.len(), 120);
        assert_eq!(name, "kick.wav");
    }

    #[test]
    fn one_unsplittable_component_is_refused_rather_than_truncated() {
        // Truncating would produce an archive that extracts to a
        // different path than the one asked for, silently.
        let single = "x".repeat(120);
        assert_eq!(split_name(&single), None);
        assert!(header(&single, 0, 0, false).is_err());
    }

    #[test]
    fn the_checksum_is_what_an_extractor_recomputes() {
        let h = header("mix.wav", 8, 0, false).unwrap();
        let mut check = h;
        check[148..156].fill(b' ');
        let expect: u32 = check.iter().map(|b| u32::from(*b)).sum();
        let text = std::str::from_utf8(&h[148..154]).unwrap();
        assert_eq!(u32::from_str_radix(text, 8).unwrap(), expect);
    }

    #[test]
    fn a_header_is_ustar() {
        let h = header("mix.wav", 8, 0, false).unwrap();
        assert_eq!(&h[257..263], b"ustar\0");
        assert_eq!(h[156], b'0');
        assert_eq!(header("d", 0, 0, true).unwrap()[156], b'5');
    }

    #[tokio::test]
    async fn an_archive_is_block_aligned_and_terminated() {
        let mut out = Vec::new();
        let mut tar = Tar::new(&mut out);
        let mut e = tar.file("mix.wav", 8, 0).await.unwrap();
        e.write(b"take one").await.unwrap();
        e.close().await.unwrap();
        tar.finish().await.unwrap();

        assert_eq!(out.len() % BLOCK, 0, "tar is block-aligned");
        assert_eq!(&out[512..520], b"take one");
        assert!(
            out[out.len() - BLOCK * 2..].iter().all(|b| *b == 0),
            "two zero blocks mark the end"
        );
    }

    #[test]
    fn padding_rounds_to_a_block() {
        assert_eq!(padding(0), 0);
        assert_eq!(padding(1), 511);
        assert_eq!(padding(512), 0);
        assert_eq!(padding(513), 511);
    }
}
