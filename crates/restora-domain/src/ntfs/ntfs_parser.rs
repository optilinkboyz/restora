//! `NtfsParser`: the NTFS implementation of `FilesystemParser`.
//!
//! The high-level flow, worth having in your head before reading the
//! code: locate `$MFT` (from the boot sector) → read its own record 0 to
//! learn `$MFT`'s *own* data runs (it's a file, remember) → that gives us
//! a `RunListFile` we can use to read *any* MFT record by index → read
//! record 6 (`$Bitmap`) the same way, to get the volume's free/used
//! cluster map → now scan every record for ones marked "not in use" with
//! a `$FILE_NAME` attribute, and cross-check each one's data clusters
//! against the bitmap for a real confidence score.
//!
//! **Scope for this phase**, named honestly:
//!  - We scan the flat MFT record table directly rather than walking
//!    directory `$INDEX_ROOT` structures, so recovered entries report
//!    just a filename, not a full path (real NTFS builds paths by
//!    following each `$FILE_NAME`'s parent reference up the tree — a
//!    reasonable next increment, same spirit as FAT's Phase 2.5).
//!  - Deleted directories aren't recursed into, matching FAT's stance.
//!  - Resident `$DATA` (small files whose content is stored directly
//!    inside the MFT record, no separate clusters at all) is skipped for
//!    now — real recovery of these is actually *easier* than
//!    non-resident files since the bytes are already in hand, but it
//!    needs `DeletedEntry` to carry inline payload bytes, which the
//!    current FS-agnostic shape doesn't support. Documented gap, not a
//!    silent one.

use crate::error::{DomainError, Result};
use crate::ntfs::boot_sector::NtfsBootSector;
use crate::ntfs::data_runs::DataRun;
use crate::ntfs::fixup::apply_fixups;
use crate::ntfs::mft_record::{parse_attributes, parse_header, DataLocation, MftRecordHeader, ParsedAttributes};
use crate::ntfs::run_list_file::RunListFile;
use crate::parser::{ClusterRange, DeletedEntry, FilesystemParser};
use restora_infra::ByteSource;
use std::cell::RefCell;
use std::collections::HashMap;

const MFT_RECORD_INDEX_BITMAP: u64 = 6;
/// Sanity cap on how many MFT records we'll ever scan, protecting against
/// a corrupted `$MFT` data-run claiming an absurd size.
const MAX_RECORDS_SCANNED: u64 = 200_000;

enum ClusterBitmap {
    Resident(Vec<u8>),
    NonResident(RunListFile),
}

impl ClusterBitmap {
    fn is_allocated(&self, source: &dyn ByteSource, cluster: u64) -> Result<bool> {
        let byte_index = (cluster / 8) as usize;
        let bit = (cluster % 8) as u8;
        let byte = match self {
            ClusterBitmap::Resident(bytes) => *bytes.get(byte_index).unwrap_or(&0),
            ClusterBitmap::NonResident(rlf) => {
                let b = rlf.read_at(source, byte_index as u64, 1)?;
                b[0]
            }
        };
        Ok((byte >> bit) & 1 == 1)
    }
}

pub struct NtfsParser {
    boot_sector: NtfsBootSector,
    mft_table: RunListFile,
    bitmap: ClusterBitmap,
    /// Populated during `enumerate_deleted`, consulted by `recover_bytes`
    /// so we can re-read a file's *complete* (possibly fragmented) run
    /// list instead of falling back to a single-contiguous-range guess —
    /// this is the concrete advantage NTFS recovery has over FAT32's
    /// approach, made possible because NTFS's chain metadata (the data
    /// runs) genuinely survives deletion, unlike FAT's chain in the FAT
    /// table.
    record_index_by_name: RefCell<HashMap<String, u64>>,
}

impl NtfsParser {
    pub fn new(source: &dyn ByteSource) -> Result<Self> {
        let boot_sector = NtfsBootSector::parse(source)?;
        let cluster_size = boot_sector.cluster_size_bytes();
        let bps = boot_sector.bytes_per_sector as usize;
        let record_size = boot_sector.mft_record_size as usize;

        // Bootstrap step: read $MFT's own record 0 directly by its known
        // disk offset (the one place in this whole parser where we don't
        // yet have a RunListFile to help us).
        let mut record0 = source.read_vec(boot_sector.mft_start_offset(), record_size)?;
        apply_fixups(&mut record0, bps)?;
        let header0 = parse_header(&record0);
        if !header0.is_valid_file_record {
            return Err(DomainError::NotFat32("MFT record 0 has no valid FILE signature".into()));
        }
        let attrs0 = parse_attributes(&record0, &header0)?;
        let mft_runs = match attrs0.data {
            Some(DataLocation::NonResident(runs)) => runs,
            _ => {
                return Err(DomainError::DirEntry(
                    "$MFT's own $DATA attribute was not non-resident — unexpected".into(),
                ))
            }
        };
        let mft_table = RunListFile::new(mft_runs, cluster_size);

        // Now that we can read arbitrary MFT records via mft_table, read
        // record 6 ($Bitmap) the normal way.
        let (_header6, attrs6) = read_record_via(source, &mft_table, record_size, bps, MFT_RECORD_INDEX_BITMAP)?;
        let bitmap = match attrs6.data {
            Some(DataLocation::Resident(bytes)) => ClusterBitmap::Resident(bytes),
            Some(DataLocation::NonResident(runs)) => {
                ClusterBitmap::NonResident(RunListFile::new(runs, cluster_size))
            }
            None => return Err(DomainError::DirEntry("$Bitmap record has no $DATA attribute".into())),
        };

        Ok(Self {
            boot_sector,
            mft_table,
            bitmap,
            record_index_by_name: RefCell::new(HashMap::new()),
        })
    }

    fn read_record(&self, source: &dyn ByteSource, index: u64) -> Result<(MftRecordHeader, ParsedAttributes)> {
        read_record_via(
            source,
            &self.mft_table,
            self.boot_sector.mft_record_size as usize,
            self.boot_sector.bytes_per_sector as usize,
            index,
        )
    }

    /// Cross-checks a file's data-run clusters against `$Bitmap` — the
    /// real signal for whether recovery will actually succeed. If every
    /// cluster the file used is still marked free, nothing has claimed
    /// that space since deletion, and recovery is very likely
    /// byte-exact. If any cluster shows allocated, something has already
    /// been written there — recovery would return corrupted or unrelated
    /// data.
    fn confidence_for_runs(&self, source: &dyn ByteSource, runs: &[DataRun]) -> u8 {
        let mut any_checked = false;
        for run in runs {
            for cluster in run.start_lcn..run.start_lcn + run.length_clusters {
                any_checked = true;
                match self.bitmap.is_allocated(source, cluster) {
                    Ok(true) => return 25,  // still allocated => likely overwritten already
                    Ok(false) => {}         // free => good sign, keep checking the rest
                    Err(_) => return 40,    // couldn't verify — moderate, uncertain confidence
                }
            }
        }
        if any_checked {
            90
        } else {
            30
        }
    }
}

/// Free function (not a method) so the bootstrap path in `new()` — before
/// `Self` fully exists — can reuse the exact same record-reading logic
/// rather than duplicating it.
fn read_record_via(
    source: &dyn ByteSource,
    mft_table: &RunListFile,
    record_size: usize,
    bytes_per_sector: usize,
    index: u64,
) -> Result<(MftRecordHeader, ParsedAttributes)> {
    let logical_offset = index * record_size as u64;
    let mut record = mft_table.read_at(source, logical_offset, record_size)?;
    apply_fixups(&mut record, bytes_per_sector)?;
    let header = parse_header(&record);
    let attrs = if header.is_valid_file_record {
        parse_attributes(&record, &header)?
    } else {
        ParsedAttributes::default()
    };
    Ok((header, attrs))
}

impl FilesystemParser for NtfsParser {
    fn detect(source: &dyn ByteSource) -> bool {
        NtfsBootSector::parse(source).is_ok()
    }

    fn enumerate_deleted(&self, source: &dyn ByteSource) -> Result<Vec<DeletedEntry>> {
        let mut results = Vec::new();
        let total_records = (self.mft_table.total_len() / self.boot_sector.mft_record_size as u64)
            .min(MAX_RECORDS_SCANNED);

        let mut name_map = self.record_index_by_name.borrow_mut();

        for index in 0..total_records {
            let (header, attrs) = match self.read_record(source, index) {
                Ok(r) => r,
                Err(_) => continue, // unreadable/corrupted record — skip, don't abort the whole scan
            };

            if !header.is_valid_file_record || header.in_use || header.is_directory {
                continue;
            }

            let Some(file_name) = attrs.file_name else { continue };
            let Some(DataLocation::NonResident(runs)) = attrs.data else {
                continue; // resident-data files: documented gap, see module docs
            };
            if runs.is_empty() {
                continue;
            }

            let confidence = self.confidence_for_runs(source, &runs);
            name_map.insert(file_name.name.clone(), index);

            results.push(DeletedEntry {
                name: file_name.name,
                first_cluster: runs[0].start_lcn,
                file_size: file_name.real_size,
                metadata_intact: true,
                confidence,
            });
        }

        Ok(results)
    }

    fn resolve_data_runs(&self, entry: &DeletedEntry) -> Vec<ClusterRange> {
        // Best-effort single-range approximation for callers that only
        // have the entry and no parser context — `recover_bytes` below
        // does better by re-reading the record's full run list instead.
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
        let cluster_size = self.boot_sector.cluster_size_bytes();

        // Preferred path: we remember which record this entry came from,
        // so re-read it and recover using its FULL (possibly fragmented)
        // run list.
        if let Some(&index) = self.record_index_by_name.borrow().get(&entry.name) {
            if let Ok((header, attrs)) = self.read_record(source, index) {
                if header.is_valid_file_record {
                    if let Some(DataLocation::NonResident(runs)) = attrs.data {
                        let file = RunListFile::new(runs, cluster_size);
                        let bytes = file.read_at(source, 0, entry.file_size as usize)?;
                        return Ok(bytes);
                    }
                }
            }
        }

        // Fallback: single-contiguous-range guess, same approach FAT32
        // uses. Only reached if the name lookup failed (e.g. this entry
        // came from somewhere other than this parser's own scan).
        let runs = self.resolve_data_runs(entry);
        let mut out = Vec::with_capacity(entry.file_size as usize);
        for run in runs {
            let offset = self.boot_sector.cluster_offset(run.start_cluster as u64);
            let len = run.cluster_count as u64 * cluster_size;
            out.extend_from_slice(&source.read_vec(offset, len as usize)?);
        }
        out.truncate(entry.file_size as usize);
        Ok(out)
    }

    fn free_space_ranges(&self, source: &dyn ByteSource) -> Result<Vec<(u64, u64)>> {
        // Same coalesce-consecutive-free-clusters approach as FAT32's
        // implementation, using $Bitmap instead of FAT entries as the
        // free/used signal. Same honest performance caveat applies too:
        // one bitmap-bit check per cluster is fine for our small test
        // volumes, but a real multi-terabyte drive has billions of
        // clusters — production code would read the bitmap in bulk
        // chunks and check bits in memory, not one at a time through
        // `ClusterBitmap::is_allocated`'s per-call disk read.
        let total_clusters = self.boot_sector.total_sectors / self.boot_sector.sectors_per_cluster as u64;
        let mut ranges = Vec::new();
        let mut run_start: Option<u64> = None;

        for cluster in 0..total_clusters {
            let is_free = !self.bitmap.is_allocated(source, cluster).unwrap_or(true);

            match (is_free, run_start) {
                (true, None) => run_start = Some(cluster),
                (false, Some(start)) => {
                    ranges.push((self.boot_sector.cluster_offset(start), self.boot_sector.cluster_offset(cluster)));
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run_start {
            ranges.push((self.boot_sector.cluster_offset(start), self.boot_sector.cluster_offset(total_clusters)));
        }

        Ok(ranges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use restora_infra::ImageFileSource;
    use std::path::PathBuf;

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/ntfs_basic.img")
    }

    /// The Phase 4 milestone: find canary.txt via the MFT (not-in-use
    /// flag, correctly fixed-up and attribute-parsed), confirm the
    /// $Bitmap cross-check reports high confidence (its cluster really is
    /// marked free in our hand-built fixture), and recover its bytes
    /// byte-for-byte — using the SAME record-index-based path that would
    /// also correctly handle a fragmented file, unlike FAT32's
    /// single-contiguous-guess approach.
    #[test]
    fn finds_and_recovers_deleted_ntfs_file_with_high_confidence() {
        let source = ImageFileSource::open(fixture_path())
            .expect("fixture image missing — run scripts/make_ntfs_fixture.py first");

        let parser = NtfsParser::new(&source).expect("NTFS boot sector + $MFT + $Bitmap should parse");

        let deleted = parser.enumerate_deleted(&source).expect("enumerate_deleted failed");
        assert_eq!(deleted.len(), 1, "expected exactly one deleted entry (canary.txt), found: {deleted:?}");

        let entry = &deleted[0];
        assert_eq!(entry.name, "canary.txt");
        assert_eq!(entry.file_size, 91);
        assert_eq!(
            entry.confidence, 90,
            "canary.txt's data cluster is marked FREE in $Bitmap in the fixture — should be high confidence"
        );

        let recovered = parser.recover_bytes(&source, entry).expect("recover_bytes failed");
        let expected =
            b"This is a canary file for NTFS recovery testing. Bitmap-verified high-confidence recovery.\n";
        assert_eq!(recovered.len(), expected.len());
        assert_eq!(&recovered, expected);
    }

    /// Same Phase 6 prerequisite check as FAT32's: canary.txt's data
    /// cluster (deliberately left free in the hand-built $Bitmap fixture)
    /// must be reported by free_space_ranges.
    #[test]
    fn deleted_file_cluster_appears_in_free_space_ranges() {
        let source = ImageFileSource::open(fixture_path())
            .expect("fixture image missing — run scripts/make_ntfs_fixture.py first");
        let parser = NtfsParser::new(&source).unwrap();

        let deleted = parser.enumerate_deleted(&source).unwrap();
        let entry = &deleted[0];

        let free_ranges = parser.free_space_ranges(&source).unwrap();
        let cluster_offset = parser.boot_sector.cluster_offset(entry.first_cluster);

        let covered = free_ranges.iter().any(|&(start, end)| cluster_offset >= start && cluster_offset < end);
        assert!(
            covered,
            "canary.txt's cluster at offset {cluster_offset} should be inside a free-space range, \
             free ranges were: {free_ranges:?}"
        );
    }
}
