//! The single abstraction that everything above the infra layer reads through.
//!
//! This is the most important type in the whole project. Every filesystem
//! parser and every carver operates on a `&dyn ByteSource`, never on a raw
//! file handle or device path directly. That means:
//!
//!  - You can develop and test 100% of the parsing/carving logic against
//!    small `.img` fixture files, with zero risk to a real disk.
//!  - The *type system* enforces read-only access during recovery: a
//!    `ByteSource` has no `write` method at all. Only the infra layer's
//!    wipe-specific types (built later, in Phase 6) expose writing, and only
//!    behind an explicit, separately-gated construction path.

use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ByteSourceError {
    #[error("read out of bounds: offset {offset} + len {len} exceeds source size {size}")]
    OutOfBounds { offset: u64, len: usize, size: u64 },

    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, ByteSourceError>;

/// A read-only, randomly-addressable source of bytes.
///
/// Implementations: `ImageFileSource` (a `.img`/`.dd` file — what you'll use
/// for almost all development), and later `RawDiskSource` (a live physical
/// device, gated behind privilege elevation).
pub trait ByteSource: Send + Sync {
    /// Total size of the source in bytes.
    fn size(&self) -> u64;

    /// Read exactly `buf.len()` bytes starting at `offset` into `buf`.
    /// Errors if the read would run past the end of the source.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// Convenience: read `len` bytes into a freshly allocated Vec.
    fn read_vec(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.read_at(offset, &mut buf)?;
        Ok(buf)
    }

    /// A human-readable label for logs/audit trail (e.g. a file path or
    /// device name). Never contains sensitive data beyond that.
    fn label(&self) -> &str;
}
