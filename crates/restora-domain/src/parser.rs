//! The interface every filesystem-specific parser implements (FAT32 now,
//! NTFS/ext4 in Phase 4). Matches the architecture's module map exactly —
//! this is what lets `restora-application` treat "find deleted files" the
//! same way regardless of which filesystem is underneath.

use crate::error::Result;
use restora_infra::ByteSource;

use serde::{Deserialize, Serialize};

/// A contiguous run of clusters. Deleted files that are still contiguous
/// on disk (the common case for small/unfragmented files) resolve to a
/// single range; fragmented files would need several — Phase 2 assumes
/// contiguity (see `Fat32Parser::resolve_data_runs` docs for why), full
/// fragment-chain recovery lands with the NTFS parser in Phase 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterRange {
    pub start_cluster: u32,
    pub cluster_count: u32,
}

/// A deleted file found by walking filesystem metadata (as opposed to one
/// found by carving, which has no metadata at all).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedEntry {
    /// Reconstructed name, e.g. "CANARY.TXT" (FAT) or "canary.txt" (NTFS).
    pub name: String,
    /// Starting cluster (FAT) or LCN (NTFS) of the file's data. Widened to
    /// u64 to accommodate NTFS's 64-bit cluster addressing on very large
    /// volumes — FAT32 values always fit comfortably within this.
    pub first_cluster: u64,
    pub file_size: u64,
    /// True if the directory entry's data-run pointers are still intact
    /// and not (yet) reused by another file. Doesn't guarantee the actual
    /// cluster bytes weren't overwritten — that's a separate free-space
    /// bitmap check, added when we implement confidence scoring.
    pub metadata_intact: bool,
    /// 0-100. How likely a byte-exact recovery is. FAT32's parser can only
    /// estimate this loosely (it doesn't cross-check the FAT's own
    /// free/used state yet). NTFS's parser computes this properly by
    /// checking whether the file's clusters are still marked free in
    /// `$Bitmap` — the same signal a real forensic tool relies on.
    pub confidence: u8,
}

/// Implemented once per filesystem type. `restora-application` never
/// needs to know whether it's talking to FAT32, NTFS, or ext4 — it just
/// calls these three methods.
pub trait FilesystemParser {
    /// Cheap check: does the boot sector / superblock look like this FS?
    fn detect(source: &dyn ByteSource) -> bool
    where
        Self: Sized;

    /// Walk directory/metadata structures and yield every entry marked
    /// deleted.
    fn enumerate_deleted(&self, source: &dyn ByteSource) -> Result<Vec<DeletedEntry>>;

    /// Given a deleted entry, resolve where its data actually lives on
    /// disk.
    fn resolve_data_runs(&self, entry: &DeletedEntry) -> Vec<ClusterRange>;

    /// Convenience: resolve + read + return the recovered bytes, truncated
    /// to the entry's recorded file size.
    fn recover_bytes(&self, source: &dyn ByteSource, entry: &DeletedEntry) -> Result<Vec<u8>>;

    /// Every byte range on disk this filesystem currently considers
    /// unallocated — the set of places safe to overwrite without touching
    /// any live file. This is exactly what a free-space wipe (Phase 6)
    /// needs, and it's exactly the same underlying data
    /// (`$Bitmap`/the FAT) each parser already reads for confidence
    /// scoring — just walked exhaustively instead of only for specific
    /// entries. Default implementation returns nothing, so any future
    /// `FilesystemParser` (ext4, APFS — still unimplemented) doesn't
    /// break the trait; both current implementors override it properly.
    fn free_space_ranges(&self, _source: &dyn ByteSource) -> Result<Vec<(u64, u64)>> {
        Ok(vec![])
    }
}
