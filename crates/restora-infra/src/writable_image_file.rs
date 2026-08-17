//! A distinct type from `ImageFileSource`, on purpose. `ImageFileSource`
//! (Phase 1) opens its file handle read-only at the OS level and
//! implements only `ByteSource` — there is no code path, anywhere, that
//! turns one into something writable. `WritableImageFileSource` is a
//! separate type with a separate constructor, used only by
//! `restora-application`'s wipe job. Its name alone should raise a flag
//! in review if it ever shows up somewhere unexpected.
//!
//! It implements BOTH `ByteSource` (needed so the Phase 3 carver can run
//! its self-verification pass directly against this same source after a
//! wipe — see `restora-application::wipe_job`) and `WritableByteSource`.
//! Implementing `ByteSource` here does not weaken the read-only guarantee
//! anywhere else in the codebase: a function that only receives a
//! `&dyn ByteSource` trait object still has no `write` method available
//! through that reference, no matter what the concrete type behind it
//! could otherwise do — the capability comes from which trait a caller is
//! handed, not from what the type is capable of in principle.

use crate::byte_source::{ByteSource, ByteSourceError, Result};
use crate::writable_byte_source::WritableByteSource;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

pub struct WritableImageFileSource {
    file: Mutex<File>,
    size: u64,
    label: String,
}

impl WritableImageFileSource {
    /// The only constructor — deliberately verbose to name what it does,
    /// unlike `ImageFileSource::open`'s plain name. Requesting write
    /// access should never look like an accident at the call site.
    pub fn open_read_write(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file: Mutex::new(file),
            size,
            label: path.display().to_string(),
        })
    }
}

impl ByteSource for WritableImageFileSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(ByteSourceError::OutOfBounds { offset, len: buf.len(), size: self.size })?;
        if end > self.size {
            return Err(ByteSourceError::OutOfBounds { offset, len: buf.len(), size: self.size });
        }
        let mut file = self.file.lock().expect("image file mutex poisoned");
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buf)?;
        Ok(())
    }

    fn label(&self) -> &str {
        &self.label
    }
}

impl WritableByteSource for WritableImageFileSource {
    fn write_at(&self, offset: u64, data: &[u8]) -> Result<()> {
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(ByteSourceError::OutOfBounds { offset, len: data.len(), size: self.size })?;
        if end > self.size {
            return Err(ByteSourceError::OutOfBounds { offset, len: data.len(), size: self.size });
        }
        let mut file = self.file.lock().expect("image file mutex poisoned");
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_back_round_trips() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, &[0u8; 32]).unwrap();

        let source = WritableImageFileSource::open_read_write(tmp.path()).unwrap();
        source.write_at(10, b"hello").unwrap();

        let readback = source.read_vec(10, 5).unwrap();
        assert_eq!(&readback, b"hello");

        // Untouched regions remain zero, proving write_at didn't clobber
        // anything outside the requested range.
        let before = source.read_vec(0, 10).unwrap();
        assert!(before.iter().all(|&b| b == 0));
    }

    #[test]
    fn rejects_write_past_end_of_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, &[0u8; 8]).unwrap();

        let source = WritableImageFileSource::open_read_write(tmp.path()).unwrap();
        let err = source.write_at(4, b"toolong!").unwrap_err(); // 4+8=12 > size 8
        assert!(matches!(err, ByteSourceError::OutOfBounds { .. }));
    }
}
