//! Reads a `ByteSource` from a plain disk-image file on the local
//! filesystem (e.g. produced with `dd if=/dev/sdb of=test.img`).
//!
//! This is what you develop and test almost everything against before ever
//! touching a live device. It's deliberately simple: open once, seek+read
//! per call. No caching here — that's `SectorCache`'s job (Phase 1, next
//! file), layered on top of any `ByteSource`.

use crate::byte_source::{ByteSource, ByteSourceError, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

pub struct ImageFileSource {
    file: Mutex<File>,
    size: u64,
    label: String,
}

impl ImageFileSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file: Mutex::new(file),
            size,
            label: path.display().to_string(),
        })
    }
}

impl ByteSource for ImageFileSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(ByteSourceError::OutOfBounds {
                offset,
                len: buf.len(),
                size: self.size,
            })?;
        if end > self.size {
            return Err(ByteSourceError::OutOfBounds {
                offset,
                len: buf.len(),
                size: self.size,
            });
        }

        // Lock scope: one thread touches the file cursor at a time. Fine for
        // now; Phase 1's SectorCache will add read parallelism without each
        // caller needing its own file handle.
        let mut file = self.file.lock().expect("image file mutex poisoned");
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buf)?;
        Ok(())
    }

    fn label(&self) -> &str {
        &self.label
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_bytes_at_offset() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"0123456789ABCDEF").unwrap();
        let src = ImageFileSource::open(tmp.path()).unwrap();

        assert_eq!(src.size(), 16);
        let bytes = src.read_vec(4, 4).unwrap();
        assert_eq!(&bytes, b"4567");
    }

    #[test]
    fn rejects_out_of_bounds_read() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"short").unwrap();
        let src = ImageFileSource::open(tmp.path()).unwrap();

        let err = src.read_vec(0, 100).unwrap_err();
        assert!(matches!(err, ByteSourceError::OutOfBounds { .. }));
    }
}
