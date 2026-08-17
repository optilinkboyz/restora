//! The interface every filesystem-specific parser implements (FAT32 now,
//! NTFS/ext4 in Phase 4). Matches the architecture's module map exactly —
//! this is what lets `restora-application` treat "find deleted files" the
//! same way regardless of which filesystem is underneath.

use crate::error::Result;
use restora_infra::ByteSource;

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
#[derive(Debug, Clone)]
pub struct DeletedEntry {
    /// Reconstructed name, e.g. "CANARY.TXT". FAT8.3 names only for now.
    pub name: String,
    pub first_cluster: u32,
    pub file_size: u32,
    /// True if the directory entry's data-run pointers are still intact
    /// and not (yet) reused by another file. Doesn't guarantee the actual
    /// cluster bytes weren't overwritten — that's a separate free-space
    /// bitmap check, added when we implement confidence scoring.
    pub metadata_intact: bool,
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
}
