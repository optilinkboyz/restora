//! `Fat32Parser`: the FAT32 implementation of `FilesystemParser`.
//!
//! Phase 2 scope, deliberately: root directory only (no subdirectory
//! recursion yet), and a **contiguity assumption** for recovery — meaning
//! we assume a deleted file's clusters are still laid out sequentially
//! starting at `first_cluster`, rather than following the FAT chain.
//!
//! Why not follow the FAT chain like a normal reader would? Because on
//! delete, most FAT implementations *clear the chain's cluster entries*
//! back to 0x00000000 (free) — the directory entry keeps `first_cluster`
//! and `file_size`, but the linkage `cluster 3 -> cluster 4 -> cluster 9`
//! is gone. This is precisely why undelete tools have always relied on
//! the contiguous-file assumption for FAT: it works great for small,
//! unfragmented files (the common case) and simply won't recover
//! fragmented ones without a carving fallback — which is exactly why
//! Phase 3 (the carver) exists as a complement, not a replacement.

use crate::error::Result;
use crate::fat32::boot_sector::Fat32BootSector;
use crate::fat32::dir_entry::{format_name, parse_entry, EntrySlot};
use crate::parser::{ClusterRange, DeletedEntry, FilesystemParser};
use restora_infra::ByteSource;

pub struct Fat32Parser {
    boot_sector: Fat32BootSector,
}

impl Fat32Parser {
    pub fn new(source: &dyn ByteSource) -> Result<Self> {
        let boot_sector = Fat32BootSector::parse(source)?;
        Ok(Self { boot_sector })
    }

    /// Root directory clusters, contiguity-assumed for the same reason
    /// described above — fine for Phase 2's tiny test fixtures, and a
    /// named limitation to revisit once subdirectory + fragmentation
    /// support is needed.
    fn read_root_directory_bytes(&self, source: &dyn ByteSource) -> Result<Vec<u8>> {
        let offset = self.boot_sector.cluster_offset(self.boot_sector.root_cluster);
        // Read a handful of clusters worth — enough for any small test
        // volume's root directory. Real multi-cluster traversal via the
        // FAT chain is the natural next step once this baseline works.
        let len = (self.boot_sector.cluster_size_bytes() * 4) as usize;
        Ok(source.read_vec(offset, len)?)
    }
}

impl FilesystemParser for Fat32Parser {
    fn detect(source: &dyn ByteSource) -> bool {
        Fat32BootSector::parse(source).is_ok()
    }

    fn enumerate_deleted(&self, source: &dyn ByteSource) -> Result<Vec<DeletedEntry>> {
        let dir_bytes = self.read_root_directory_bytes(source)?;
        let mut results = Vec::new();

        for chunk in dir_bytes.chunks_exact(32) {
            let mut entry_bytes = [0u8; 32];
            entry_bytes.copy_from_slice(chunk);

            match parse_entry(&entry_bytes) {
                EntrySlot::EndOfDirectory => break,
                EntrySlot::Skip => continue,
                EntrySlot::Entry(raw) if raw.is_deleted => {
                    results.push(DeletedEntry {
                        name: format_name(&raw),
                        first_cluster: raw.first_cluster,
                        file_size: raw.file_size,
                        // Phase 2 doesn't yet cross-check the FAT's
                        // free/used state for these clusters (that's the
                        // confidence-scoring work in Phase 4) — for now,
                        // "metadata_intact" just means we successfully
                        // parsed a first_cluster/file_size pair at all.
                        metadata_intact: raw.first_cluster != 0,
                    });
                }
                EntrySlot::Entry(_) => continue, // a live, non-deleted file
            }
        }

        Ok(results)
    }

    fn resolve_data_runs(&self, entry: &DeletedEntry) -> Vec<ClusterRange> {
        if entry.file_size == 0 {
            return vec![];
        }
        let cluster_size = self.boot_sector.cluster_size_bytes() as u32;
        let cluster_count = entry.file_size.div_ceil(cluster_size);

        vec![ClusterRange {
            start_cluster: entry.first_cluster,
            cluster_count,
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

    /// The end-to-end Phase 2 milestone: find the deleted canary.txt in a
    /// real FAT32 image, recover its bytes, and byte-compare against the
    /// original content we know we wrote before deleting it.
    #[test]
    fn recovers_deleted_canary_file_byte_for_byte() {
        let source = ImageFileSource::open(fixture_path())
            .expect("fixture image missing — run scripts/make_fat32_fixture.sh first");

        let parser = Fat32Parser::new(&source).expect("boot sector should parse as FAT32");

        let deleted = parser.enumerate_deleted(&source).expect("enumerate_deleted failed");
        assert_eq!(deleted.len(), 1, "expected exactly one deleted entry (canary.txt)");

        let entry = &deleted[0];
        // First character is unrecoverable from the 0xE5 marker alone —
        // this assertion documents that real, expected limitation.
        assert_eq!(entry.name, "_ANARY.TXT");
        assert_eq!(entry.file_size, 107);

        let recovered = parser.recover_bytes(&source, entry).expect("recover_bytes failed");

        let expected =
            b"This is a canary file for FAT32 recovery testing. If you can read this after carving, the recovery worked.\n";
        assert_eq!(recovered.len(), expected.len());
        assert_eq!(&recovered, expected);
    }
}
