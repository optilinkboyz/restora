//! `Fat32Parser`: the FAT32 implementation of `FilesystemParser`.
//!
//! This version closes the two gaps left after the first pass:
//!   1. Directories are now read by properly following their FAT cluster
//!      chain (`fat_table::follow_chain`), not by assuming they fit in a
//!      handful of clusters read linearly.
//!   2. `enumerate_deleted` recurses into live subdirectories, so deleted
//!      files anywhere in the tree are found — not just ones sitting
//!      directly in the root. Each entry's `name` field is now a
//!      root-relative path, e.g. `"Documents/_ANARY.TXT"`.
//!
//! Recovery of a deleted file's *data* still uses the Phase 2 contiguous-
//! cluster assumption — that part is unrelated to directory traversal and
//! is explained in fat32_parser's original docs / Phase 3's carver.
//!
//! One deliberate, documented limitation that remains: we only recurse
//! into *live* (non-deleted) subdirectories. A deleted subdirectory's own
//! chain is usually zeroed by the delete, same as a deleted file's — and
//! unlike a file, a directory has no reliable size field to fall back on
//! for a contiguous-read guess. So deleted subdirectories show up as a
//! single deleted entry (the folder itself), but we don't attempt to
//! recover what was inside them. That's real forensic-carving territory,
//! which is exactly what Phase 3 exists for.

use crate::error::Result;
use crate::fat32::boot_sector::Fat32BootSector;
use crate::fat32::dir_entry::{format_name, parse_entry, EntrySlot, ATTR_DIRECTORY};
use crate::fat32::fat_table::follow_chain;
use crate::parser::{ClusterRange, DeletedEntry, FilesystemParser};
use restora_infra::ByteSource;
use std::collections::HashSet;

pub struct Fat32Parser {
    boot_sector: Fat32BootSector,
}

impl Fat32Parser {
    pub fn new(source: &dyn ByteSource) -> Result<Self> {
        let boot_sector = Fat32BootSector::parse(source)?;
        Ok(Self { boot_sector })
    }

    /// Reads a directory's full contents by following its FAT chain from
    /// `start_cluster`, concatenating every cluster in order. Works
    /// identically for the root directory and any subdirectory.
    fn read_directory_bytes(&self, source: &dyn ByteSource, start_cluster: u32) -> Result<Vec<u8>> {
        let chain = follow_chain(source, &self.boot_sector, start_cluster)?;
        let mut bytes = Vec::with_capacity(chain.len() * self.boot_sector.cluster_size_bytes() as usize);
        for cluster in chain {
            let offset = self.boot_sector.cluster_offset(cluster);
            let chunk = source.read_vec(offset, self.boot_sector.cluster_size_bytes() as usize)?;
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    /// Depth-first walk of the directory tree starting at `cluster`,
    /// pushing every deleted entry found (with a root-relative path
    /// prefix) into `out`. `visited_dirs` guards against cycles in a
    /// corrupted image (e.g. a subdirectory pointing back at an ancestor).
    fn walk_directory(
        &self,
        source: &dyn ByteSource,
        cluster: u32,
        prefix: &str,
        out: &mut Vec<DeletedEntry>,
        visited_dirs: &mut HashSet<u32>,
    ) -> Result<()> {
        if !visited_dirs.insert(cluster) {
            return Ok(()); // already walked this cluster — cycle, stop here
        }

        let dir_bytes = self.read_directory_bytes(source, cluster)?;

        for chunk in dir_bytes.chunks_exact(32) {
            let mut entry_bytes = [0u8; 32];
            entry_bytes.copy_from_slice(chunk);

            let raw = match parse_entry(&entry_bytes) {
                EntrySlot::EndOfDirectory => break,
                EntrySlot::Skip => continue,
                EntrySlot::Entry(raw) => raw,
            };

            // Skip "." and ".." self/parent-reference entries — recursing
            // into these would immediately re-walk the current or parent
            // directory and defeat the cycle guard's purpose.
            if raw.raw_name[0] == b'.' {
                continue;
            }

            if raw.is_deleted {
                out.push(DeletedEntry {
                    name: format!("{prefix}{}", format_name(&raw)),
                    first_cluster: raw.first_cluster as u64,
                    file_size: raw.file_size as u64,
                    metadata_intact: raw.first_cluster != 0,
                    // FAT32's parser doesn't cross-check the FAT's own
                    // free/used state for these clusters (unlike NTFS's
                    // $Bitmap check) — this is a coarser estimate based
                    // only on whether the directory entry itself parsed
                    // sensibly.
                    confidence: if raw.first_cluster != 0 { 60 } else { 10 },
                });
                // Deliberately not recursing into deleted subdirectories —
                // see module docs above for why.
                continue;
            }

            if raw.attr & ATTR_DIRECTORY != 0 && raw.first_cluster >= 2 {
                let sub_prefix = format!("{prefix}{}/", format_name(&raw));
                self.walk_directory(source, raw.first_cluster, &sub_prefix, out, visited_dirs)?;
            }
        }

        Ok(())
    }
}

impl FilesystemParser for Fat32Parser {
    fn detect(source: &dyn ByteSource) -> bool {
        Fat32BootSector::parse(source).is_ok()
    }

    fn enumerate_deleted(&self, source: &dyn ByteSource) -> Result<Vec<DeletedEntry>> {
        let mut results = Vec::new();
        let mut visited_dirs = HashSet::new();
        self.walk_directory(source, self.boot_sector.root_cluster, "", &mut results, &mut visited_dirs)?;
        Ok(results)
    }

    fn resolve_data_runs(&self, entry: &DeletedEntry) -> Vec<ClusterRange> {
        if entry.file_size == 0 {
            return vec![];
        }
        let cluster_size = self.boot_sector.cluster_size_bytes();
        let cluster_count = entry.file_size.div_ceil(cluster_size);

        vec![ClusterRange {
            start_cluster: entry.first_cluster as u32,
            cluster_count: cluster_count as u32,
        }]
    }

    fn recover_bytes(&self, source: &dyn ByteSource, entry: &DeletedEntry) -> Result<Vec<u8>> {
        let runs = self.resolve_data_runs(entry);
        let mut out = Vec::with_capacity(entry.file_size as usize);

        for run in runs {
            let offset = self.boot_sector.cluster_offset(run.start_cluster);
            let len = run.cluster_count as u64 * self.boot_sector.cluster_size_bytes();
            let bytes = source.read_vec(offset, len as usize)?;
            out.extend_from_slice(&bytes);
        }

        out.truncate(entry.file_size as usize);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use restora_infra::ImageFileSource;
    use std::path::PathBuf;

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/fat32_basic.img")
    }

    fn nested_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/fat32_nested.img")
    }

    /// The Phase 2 milestone test, unchanged in behavior: a root-level
    /// deleted file still recovers byte-for-byte. This confirms the
    /// rewrite to chain-following didn't regress the simple case.
    #[test]
    fn recovers_deleted_canary_file_byte_for_byte() {
        let source = ImageFileSource::open(fixture_path())
            .expect("fixture image missing — run scripts/make_fat32_fixture.sh first");

        let parser = Fat32Parser::new(&source).expect("boot sector should parse as FAT32");

        let deleted = parser.enumerate_deleted(&source).expect("enumerate_deleted failed");
        assert_eq!(deleted.len(), 1, "expected exactly one deleted entry (canary.txt)");

        let entry = &deleted[0];
        assert_eq!(entry.name, "_ANARY.TXT");
        assert_eq!(entry.file_size, 107);

        let recovered = parser.recover_bytes(&source, entry).expect("recover_bytes failed");

        let expected =
            b"This is a canary file for FAT32 recovery testing. If you can read this after carving, the recovery worked.\n";
        assert_eq!(recovered.len(), expected.len());
        assert_eq!(&recovered, expected);
    }

    /// The new Phase 2.5 milestone: a deleted file sitting inside a live
    /// subdirectory is found via recursion, with a correctly built
    /// relative path, and still recovers byte-for-byte.
    #[test]
    fn recovers_deleted_file_inside_subdirectory() {
        let source = ImageFileSource::open(nested_fixture_path()).expect(
            "nested fixture missing — run scripts/make_fat32_nested_fixture.sh first",
        );

        let parser = Fat32Parser::new(&source).expect("boot sector should parse as FAT32");
        let deleted = parser.enumerate_deleted(&source).expect("enumerate_deleted failed");

        let entry = deleted
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("SUBDIR/_EEP.TXT"))
            .expect("expected to find SUBDIR/_EEP.TXT among deleted entries");

        let recovered = parser.recover_bytes(&source, entry).expect("recover_bytes failed");
        let expected = b"Nested file inside a subdirectory, for testing recursive directory traversal.\n";
        assert_eq!(&recovered, expected);
    }
}

