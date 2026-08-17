//! Reads the File Allocation Table itself and follows cluster chains.
//!
//! This is the piece Phase 2 skipped: instead of assuming a directory
//! fits in a fixed number of clusters, we now read the FAT — a flat array
//! of 4-byte entries, one per cluster, starting right after the reserved
//! sectors — and follow the linked list of "next cluster" pointers until
//! we hit an end-of-chain marker. This is exactly how a real FAT32 driver
//! finds all of a directory's (or file's) clusters, live ones anyway.
//!
//! Reminder from Phase 2's parser docs: this chain-following is safe and
//! correct for *live* directories (which is what we need now, to walk the
//! real directory tree) — but on a *deleted* file, this same chain is
//! usually zeroed out by the delete operation, which is why file recovery
//! itself still uses the Phase 2 contiguous-cluster assumption rather than
//! chain-following.

use crate::error::Result;
use crate::fat32::boot_sector::Fat32BootSector;
use restora_infra::ByteSource;
use std::collections::HashSet;

/// FAT32 entries are 32 bits, but only the low 28 are meaningful — the top
/// 4 are reserved and must be masked off before checking values.
const FAT32_ENTRY_MASK: u32 = 0x0FFF_FFFF;

/// Any value at or above this means "this was the last cluster in the
/// file/directory."
const END_OF_CHAIN_MIN: u32 = 0x0FFF_FFF8;

/// This exact value marks a cluster the filesystem has flagged as bad
/// (unreadable media) — treat it like end-of-chain for traversal purposes,
/// we just can't read past it.
const BAD_CLUSTER: u32 = 0x0FFF_FFF7;

/// Hard cap on chain length as corruption protection — a well-formed
/// volume never needs anywhere near this many clusters in one chain, and
/// without a cap a corrupted or maliciously crafted image could walk into
/// a near-infinite loop before the cycle-detection (via `visited`) even
/// gets a chance to kick in on some patterns.
const MAX_CHAIN_CLUSTERS: usize = 1_000_000;

pub fn read_fat_entry(
    source: &dyn ByteSource,
    boot_sector: &Fat32BootSector,
    cluster: u32,
) -> Result<u32> {
    let offset = boot_sector.fat_start_offset() + (cluster as u64 * 4);
    let bytes = source.read_vec(offset, 4)?;
    let raw = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    Ok(raw & FAT32_ENTRY_MASK)
}

fn is_end_of_chain(entry: u32) -> bool {
    entry >= END_OF_CHAIN_MIN
}

fn is_free(entry: u32) -> bool {
    entry == 0
}

fn is_bad(entry: u32) -> bool {
    entry == BAD_CLUSTER
}

/// Returns every cluster number in the chain starting at `start_cluster`,
/// in order. Stops at end-of-chain, a free/bad entry (shouldn't happen on
/// a well-formed live chain, but corrupted images exist), a cycle back to
/// an already-visited cluster, or the safety cap — whichever comes first.
pub fn follow_chain(
    source: &dyn ByteSource,
    boot_sector: &Fat32BootSector,
    start_cluster: u32,
) -> Result<Vec<u32>> {
    let mut clusters = Vec::new();
    let mut visited = HashSet::new();
    let mut current = start_cluster;

    loop {
        if current < 2 {
            break; // 0 and 1 are reserved, never valid data clusters
        }
        if !visited.insert(current) {
            break; // cycle — corrupted FAT, stop rather than loop forever
        }
        clusters.push(current);
        if clusters.len() >= MAX_CHAIN_CLUSTERS {
            break;
        }

        let entry = read_fat_entry(source, boot_sector, current)?;
        if is_end_of_chain(entry) || is_free(entry) || is_bad(entry) {
            break;
        }
        current = entry;
    }

    Ok(clusters)
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

    #[test]
    fn root_directory_chain_starts_at_configured_root_cluster() {
        let source = ImageFileSource::open(fixture_path())
            .expect("fixture image missing — run scripts/make_fat32_fixture.sh first");
        let bs = Fat32BootSector::parse(&source).unwrap();

        let chain = follow_chain(&source, &bs, bs.root_cluster).unwrap();
        assert!(!chain.is_empty());
        assert_eq!(chain[0], bs.root_cluster);
    }
}
