//! A `Write` adapter that digests bytes as they go past.
//!
//! Every artefact digest in the manifest is taken over *the bytes that were written*, not over a
//! re-read of the file, so the digest cannot disagree with the artefact even if something else
//! touches the file afterwards.

use std::io::Write;

use sha2::{Digest, Sha256};

/// Render a raw SHA-256 the way the IR spells it: `sha256:<64 lowercase hex>`.
pub fn render(hash: [u8; 32]) -> String {
    let mut out = String::with_capacity(71);
    out.push_str("sha256:");
    for b in hash {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// `sha256:…` over a byte slice.
pub fn digest_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    render(h.finalize().into())
}

#[derive(Debug)]
pub struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    bytes: u64,
}

impl<W: Write> HashingWriter<W> {
    pub fn new(inner: W) -> HashingWriter<W> {
        HashingWriter {
            inner,
            hasher: Sha256::new(),
            bytes: 0,
        }
    }

    /// `(digest, byte count)`.
    pub fn finish(self) -> (String, u64) {
        (render(self.hasher.finalize().into()), self.bytes)
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
