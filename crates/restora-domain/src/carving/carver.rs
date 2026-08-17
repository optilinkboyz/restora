//! `SignatureCarver`: scans raw bytes for known file signatures, entirely
//! independent of any filesystem metadata.
//!
//! **Why chunked scanning needs overlap.** We can't read a multi-gigabyte
//! disk into memory at once, so we scan in fixed-size chunks. But a file
//! signature (say, PNG's 8-byte header) could straddle exactly the
//! boundary between chunk N and chunk N+1 — 5 bytes of it at the end of
//! chunk N, 3 bytes at the start of chunk N+1 — and a naive per-chunk scan
//! would miss it entirely, seeing neither chunk contain the full pattern.
//!
//! The fix: each chunk after the first starts `overlap` bytes *before*
//! where the previous chunk ended, where `overlap` = the longest header
//! among all known signatures. This guarantees that any header which
//! crosses a chunk boundary is, by construction, still short enough to
//! fit entirely within the overlap region — so the *next* chunk always
//! contains it in full. (Worked through formally in this module's tests.)
//! We then de-duplicate by absolute start offset, since a header sitting
//! inside the overlap region gets scanned twice on purpose.

use crate::carving::signatures::{FileSignature, SIGNATURES};
use crate::error::Result;
use restora_infra::ByteSource;
use std::collections::HashSet;
use std::ops::Range;

#[derive(Debug, Clone)]
pub struct CarvedFile {
    pub format_name: String,
    pub extension: String,
    pub start_offset: u64,
    /// Exclusive end offset.
    pub end_offset: u64,
    /// 0-100. High (85) when the matching footer was actually found;
    /// low (30) when we fell back to the `max_size` cutoff instead —
    /// meaning this is a guess, quite possibly wrong or truncated.
    pub confidence: u8,
}

impl CarvedFile {
    pub fn size(&self) -> u64 {
        self.end_offset - self.start_offset
    }
}

pub trait Carver {
    fn scan(&self, source: &dyn ByteSource, range: Range<u64>) -> Result<Vec<CarvedFile>>;
}

pub struct SignatureCarver {
    signatures: &'static [FileSignature],
    chunk_size: usize,
}

impl Default for SignatureCarver {
    fn default() -> Self {
        // 1MB chunks: large enough for good throughput, small enough to
        // keep memory use sane while scanning a multi-gigabyte drive.
        Self {
            signatures: SIGNATURES,
            chunk_size: 1_000_000,
        }
    }
}

impl SignatureCarver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Exposed mainly for tests, where a small chunk size lets us force
    /// and verify the boundary-overlap logic deterministically instead of
    /// hoping a signature happens to land on a 1MB boundary.
    pub fn with_chunk_size(chunk_size: usize) -> Self {
        Self {
            signatures: SIGNATURES,
            chunk_size,
        }
    }

    fn overlap(&self) -> usize {
        crate::carving::signatures::max_header_len()
    }

    /// Given a header match at `start_offset`, searches forward for the
    /// matching footer (or falls back to `max_size`) and returns the
    /// resolved extent. This is a direct, independent read from the
    /// source — not tied to the chunk-scanning loop at all — so it has no
    /// boundary-overlap concerns of its own.
    fn resolve_extent(
        &self,
        source: &dyn ByteSource,
        sig: &FileSignature,
        start_offset: u64,
    ) -> Result<CarvedFile> {
        let remaining = source.size().saturating_sub(start_offset) as usize;
        let search_len = sig.max_size.min(remaining);
        let window = source.read_vec(start_offset, search_len)?;

        if let Some(footer) = sig.footer {
            if let Some(rel) = find_first(&window, footer) {
                let end_offset = start_offset + rel as u64 + footer.len() as u64;
                return Ok(CarvedFile {
                    format_name: sig.name.to_string(),
                    extension: sig.extension.to_string(),
                    start_offset,
                    end_offset,
                    confidence: 85,
                });
            }
        }

        let end_offset = start_offset + window.len() as u64;
        Ok(CarvedFile {
            format_name: sig.name.to_string(),
            extension: sig.extension.to_string(),
            start_offset,
            end_offset,
            confidence: 30,
        })
    }
}

impl Carver for SignatureCarver {
    fn scan(&self, source: &dyn ByteSource, range: Range<u64>) -> Result<Vec<CarvedFile>> {
        // The trait's plain scan() is just scan_with_progress() with a
        // callback that reports nothing and never asks to stop early —
        // one real implementation, two ways to call it.
        self.scan_with_progress(source, range, |_scanned, _total| true)
    }
}

impl SignatureCarver {
    /// Same scanning logic as `scan()`, but calls `on_progress(scanned,
    /// total)` after every chunk — the natural place to report progress
    /// during a long scan of a real multi-gigabyte drive, and the natural
    /// place to check a cancellation flag. Returning `false` from the
    /// callback stops the scan early (whatever's been found so far is
    /// still returned, not discarded).
    pub fn scan_with_progress<F>(
        &self,
        source: &dyn ByteSource,
        range: Range<u64>,
        mut on_progress: F,
    ) -> Result<Vec<CarvedFile>>
    where
        F: FnMut(u64, u64) -> bool,
    {
        let mut results = Vec::new();
        let mut seen_starts: HashSet<u64> = HashSet::new();

        let overlap = self.overlap();
        let step = if self.chunk_size > overlap {
            self.chunk_size - overlap
        } else {
            self.chunk_size
        };

        let total = range.end - range.start;
        let mut pos = range.start;
        loop {
            if pos >= range.end {
                break;
            }
            let remaining = (range.end - pos) as usize;
            let read_len = self.chunk_size.min(remaining);
            if read_len == 0 {
                break;
            }
            let chunk = source.read_vec(pos, read_len)?;

            for sig in self.signatures {
                for local_offset in find_all(&chunk, sig.header) {
                    let abs_offset = pos + local_offset as u64;
                    if !seen_starts.insert(abs_offset) {
                        continue; // already resolved via a previous overlapping chunk
                    }
                    results.push(self.resolve_extent(source, sig, abs_offset)?);
                }
            }

            let scanned_so_far = (pos + read_len as u64).saturating_sub(range.start);
            if !on_progress(scanned_so_far, total) {
                break; // caller requested cancellation
            }

            let next = pos + step as u64;
            if next <= pos || pos + (read_len as u64) >= range.end {
                break; // reached the end, or step didn't advance (shouldn't happen)
            }
            pos = next;
        }

        results.sort_by_key(|f| f.start_offset);
        Ok(results)
    }
}

/// Every position where `needle` occurs in `haystack`, including
/// overlapping occurrences. Naive O(n*m) — completely fine at the KB/MB
/// chunk sizes here; if this ever needs to scan much bigger chunks, a
/// Boyer-Moore-Horspool or `memchr`-based search is the natural upgrade.
fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return vec![];
    }
    (0..=haystack.len() - needle.len())
        .filter(|&i| &haystack[i..i + needle.len()] == needle)
        .collect()
}

fn find_first(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use restora_infra::ImageFileSource;
    use std::io::Write;

    /// Builds a raw byte blob with a JPEG-like signature and a PDF-like
    /// signature embedded at known offsets, surrounded by zero padding —
    /// deliberately with NO filesystem structure at all. This is the
    /// scenario carving exists for: metadata-free recovery.
    fn build_synthetic_image() -> (tempfile::NamedTempFile, u64, u64, Vec<u8>, Vec<u8>) {
        let mut jpeg = vec![0xFFu8, 0xD8, 0xFF]; // header
        jpeg.extend(std::iter::repeat(0xAA).take(500)); // filler "image data"
        jpeg.extend([0xFF, 0xD9]); // footer

        let pdf = b"%PDF-1.4\nFake pdf body for carving test.\n%%EOF".to_vec();

        let mut buf = Vec::new();
        buf.extend(std::iter::repeat(0u8).take(5000)); // padding before
        let jpeg_offset = buf.len() as u64;
        buf.extend(&jpeg);
        buf.extend(std::iter::repeat(0u8).take(2000)); // gap
        let pdf_offset = buf.len() as u64;
        buf.extend(&pdf);
        buf.extend(std::iter::repeat(0u8).take(3000)); // trailing padding

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&buf).unwrap();
        (tmp, jpeg_offset, pdf_offset, jpeg, pdf)
    }

    #[test]
    fn finds_both_files_with_no_filesystem_metadata_present() {
        let (tmp, jpeg_offset, pdf_offset, jpeg_bytes, pdf_bytes) = build_synthetic_image();
        let source = ImageFileSource::open(tmp.path()).unwrap();

        // Prerequisite for the test to mean anything: confirm this really
        // has no filesystem a metadata-based parser could use instead.
        assert!(
            !restora_domain_fat32_would_detect(&source),
            "test image accidentally looks like a FAT32 volume"
        );

        let carver = SignatureCarver::new();
        let found = carver.scan(&source, 0..source.size()).unwrap();

        assert_eq!(found.len(), 2, "expected exactly JPEG + PDF, found: {found:?}");

        let jpeg_result = found.iter().find(|f| f.format_name == "JPEG").unwrap();
        assert_eq!(jpeg_result.start_offset, jpeg_offset);
        assert_eq!(jpeg_result.confidence, 85);
        let recovered_jpeg = source
            .read_vec(jpeg_result.start_offset, jpeg_result.size() as usize)
            .unwrap();
        assert_eq!(recovered_jpeg, jpeg_bytes);

        let pdf_result = found.iter().find(|f| f.format_name == "PDF").unwrap();
        assert_eq!(pdf_result.start_offset, pdf_offset);
        assert_eq!(pdf_result.confidence, 85);
        let recovered_pdf = source
            .read_vec(pdf_result.start_offset, pdf_result.size() as usize)
            .unwrap();
        assert_eq!(recovered_pdf, pdf_bytes);
    }

    /// The correctness-critical case: force a signature to straddle a
    /// chunk boundary and confirm the overlap logic still finds it,
    /// exactly once (not zero times, not twice).
    #[test]
    fn finds_signature_straddling_a_chunk_boundary() {
        let content = b"%PDF-1.0\n%%EOF"; // 14 bytes, header+footer overlap tightly
        let mut buf = vec![0u8; 30];
        let header_offset = buf.len() as u64; // = 30
        buf.extend_from_slice(content);
        buf.extend(std::iter::repeat(0u8).take(56)); // pad total to 100 bytes

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&buf).unwrap();
        let source = ImageFileSource::open(tmp.path()).unwrap();

        // chunk_size=32, overlap=8 (PNG's header is the longest at 8
        // bytes) => step=24. Chunk 1 covers bytes [0,32) — it can SEE
        // offset 30's first two bytes ('%','P') but the 5-byte "%PDF-"
        // header doesn't fully fit before byte 32, so chunk 1 alone
        // cannot match it. Chunk 2 starts at 24, covering [24,56) —
        // that fully contains the header (30..35), proving the overlap
        // is what rescues this match.
        let carver = SignatureCarver::with_chunk_size(32);
        let found = carver.scan(&source, 0..source.size()).unwrap();

        assert_eq!(found.len(), 1, "expected exactly one match, found: {found:?}");
        let f = &found[0];
        assert_eq!(f.format_name, "PDF");
        assert_eq!(f.start_offset, header_offset);
        assert_eq!(f.confidence, 85);

        let recovered = source.read_vec(f.start_offset, f.size() as usize).unwrap();
        assert_eq!(recovered, content);
    }

    #[test]
    fn falls_back_to_max_size_when_no_footer_present() {
        // A JPEG header with filler bytes but no FF D9 footer anywhere —
        // simulates a truncated or partially-overwritten recovered file.
        let mut buf = vec![0xFFu8, 0xD8, 0xFF];
        buf.extend(std::iter::repeat(0xAA).take(200)); // no footer at all

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&buf).unwrap();
        let source = ImageFileSource::open(tmp.path()).unwrap();

        let carver = SignatureCarver::new();
        let found = carver.scan(&source, 0..source.size()).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].confidence, 30, "no footer found => low confidence expected");
        assert_eq!(found[0].end_offset, source.size(), "should fall back to end of available data");
    }

    // Small helper kept local to this test module: mirrors the real
    // Fat32BootSector signature check, without creating a dependency
    // between the carving tests and the fat32 module.
    fn restora_domain_fat32_would_detect(source: &dyn ByteSource) -> bool {
        let sector = match source.read_vec(0, 512) {
            Ok(s) => s,
            Err(_) => return false,
        };
        sector.len() == 512 && sector[510] == 0x55 && sector[511] == 0xAA
    }
}
