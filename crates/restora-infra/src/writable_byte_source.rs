//! A second, separate trait for write access — deliberately NOT a method
//! added to `ByteSource`.
//!
//! This is the write-blocking-by-construction principle from the
//! architecture, made concrete: `ByteSource` (used everywhere in
//! scanning, parsing, and carving — Phases 1 through 5) has no `write`
//! method at all, so nothing holding only a `&dyn ByteSource` can ever
//! write, regardless of what the underlying concrete type could
//! technically do. `WritableByteSource` is a second trait that only
//! `secure_delete`/wipe code ever asks for by name — reaching for it is a
//! conscious, visible choice in the code, not something that falls out of
//! a type you already happened to be holding for an unrelated reason.

use crate::byte_source::Result;

pub trait WritableByteSource {
    /// Writes `data` at `offset`, overwriting whatever was there.
    fn write_at(&self, offset: u64, data: &[u8]) -> Result<()>;
}
