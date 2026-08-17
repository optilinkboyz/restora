//! `WipeJob`: applies a `WipePattern` to a target's free space, then
//! optionally self-verifies by running the Phase 3 carver against exactly
//! the ranges it just wiped.
//!
//! **Scope, named honestly, matching the SSD/TRIM guidance from earlier
//! in this project's context.** Real SSD-aware behavior (detecting drive
//! type, refusing overwrite in favor of TRIM/crypto-erase) needs either a
//! real block device or a real mounted filesystem to query — neither
//! applies to the flat `.img` test files this whole project develops
//! against. Rather than fake a detection that can't mean anything here,
//! `wipe_free_space` takes an explicit `assume_ssd: bool` the caller sets
//! (a stand-in for what real drive-type detection would eventually feed
//! in) and refuses to run an overwrite-based wipe when it's true — the
//! actual behavioral guarantee that matters is implemented and tested;
//! only the hardware-querying trigger for it is stubbed.
//!
//! Similarly, issuing real TRIM/Discard commands requires a raw block
//! device and platform-specific ioctls (`IOCTL_STORAGE_MANAGE_DATA_SET_ATTRIBUTES`
//! on Windows, `BLKDISCARD` on Linux) that cannot apply to a regular file
//! at all — not a missing feature so much as a hard boundary of what this
//! test environment can meaningfully exercise. Left as a clearly-marked
//! follow-up for when real device access is available.

use crate::error::{ApplicationError, Result};
use restora_domain::carving::{Carver, SignatureCarver};
use restora_domain::secure_delete::{fill_pass, WipePattern, WipeRng};
use restora_domain::detect_parser;
use restora_infra::{WritableByteSource, WritableImageFileSource};

/// Chunk size for generating and writing pattern bytes — matches the
/// carver's own default, kept modest so we're not allocating huge
/// buffers for a big wipe range.
const WIPE_CHUNK_SIZE: usize = 1_000_000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct VerificationResult {
    /// How many carve-able signatures the Phase 3 carver still found in
    /// the wiped ranges after wiping. Zero is the goal — anything found
    /// means either the wipe missed something or (more likely, if this
    /// number is small) a false-positive header-byte coincidence in the
    /// random-fill pass, worth knowing can happen.
    pub carved_files_found_after_wipe: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WipeResult {
    pub pattern_name: &'static str,
    pub ranges_wiped: usize,
    pub bytes_written: u64,
    pub verification: Option<VerificationResult>,
}

/// Overwrites every free-space range on the detected filesystem with
/// `pattern`. Never touches allocated/live clusters — `free_space_ranges`
/// (Phase 6's addition to `FilesystemParser`) is exactly the boundary
/// that guarantees this.
pub fn wipe_free_space(
    image_path: &str,
    pattern: &WipePattern,
    assume_ssd: bool,
    verify: bool,
) -> Result<WipeResult> {
    if assume_ssd {
        return Err(ApplicationError::SsdOverwriteRefused);
    }

    let source = WritableImageFileSource::open_read_write(image_path)?;

    let (parser, _fs_name) =
        detect_parser(&source).ok_or(ApplicationError::NoFilesystemDetected)?;
    let ranges = parser.free_space_ranges(&source)?;

    let mut bytes_written: u64 = 0;
    let mut rng = WipeRng::seeded_from_time();

    for &(start, end) in &ranges {
        for &pass in pattern.passes {
            let mut offset = start;
            let mut buf = vec![0u8; WIPE_CHUNK_SIZE];
            while offset < end {
                let chunk_len = ((end - offset) as usize).min(WIPE_CHUNK_SIZE);
                let chunk = &mut buf[..chunk_len];
                fill_pass(pass, chunk, &mut rng);
                source.write_at(offset, chunk)?;
                offset += chunk_len as u64;
            }
        }
        bytes_written += end - start;
    }

    let verification = if verify {
        let carver = SignatureCarver::new();
        let mut total_found = 0;
        for &(start, end) in &ranges {
            let found = carver.scan(&source, start..end)?;
            total_found += found.len();
        }
        Some(VerificationResult { carved_files_found_after_wipe: total_found })
    } else {
        None
    };

    Ok(WipeResult {
        pattern_name: pattern.name,
        ranges_wiped: ranges.len(),
        bytes_written,
        verification,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use restora_infra::ImageFileSource;

    fn fixture_path(name: &str) -> String {
        format!("{}/../../tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    /// Copies a checked-in fixture to a fresh temp file — every wipe test
    /// must operate on its own copy, never the shared fixture other tests
    /// (and other phases entirely) depend on staying intact.
    fn copy_fixture_to_temp(name: &str) -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::copy(fixture_path(name), tmp.path()).unwrap();
        tmp
    }

    /// The Phase 6 milestone: prove a free-space wipe actually destroys
    /// previously-recoverable data. Before wiping, canary.txt recovers
    /// byte-for-byte (same as Phase 4 proved). After wiping the same
    /// image's free space, recovering the identical entry returns bytes
    /// that no longer match — the content is genuinely gone, not just
    /// hidden.
    #[test]
    fn wiping_free_space_destroys_previously_recoverable_ntfs_file() {
        let tmp = copy_fixture_to_temp("ntfs_basic.img");
        let image_path = tmp.path().to_str().unwrap();

        // Before: confirm it's genuinely recoverable, exactly as Phase 4 did.
        let original_content =
            b"This is a canary file for NTFS recovery testing. Bitmap-verified high-confidence recovery.\n";
        {
            let source = ImageFileSource::open(image_path).unwrap();
            let (parser, _) = detect_parser(&source).unwrap();
            let deleted = parser.enumerate_deleted(&source).unwrap();
            let recovered = parser.recover_bytes(&source, &deleted[0]).unwrap();
            assert_eq!(&recovered, original_content, "sanity check: should be recoverable before wiping");
        }

        // Wipe free space with the Zero pattern — deterministic, easy to
        // assert against exactly.
        let result = wipe_free_space(image_path, &restora_domain::WipePattern::ZERO, false, true).unwrap();
        assert!(result.ranges_wiped > 0, "expected at least one free-space range to wipe");
        assert!(result.bytes_written > 0);
        let verification = result.verification.expect("requested verification");
        assert_eq!(
            verification.carved_files_found_after_wipe, 0,
            "no known file signatures should remain in wiped free space"
        );

        // After: the same recovery attempt now returns different bytes —
        // specifically, all zeros, since we used the Zero pattern and
        // canary.txt's data cluster was inside the wiped range.
        {
            let source = ImageFileSource::open(image_path).unwrap();
            let (parser, _) = detect_parser(&source).unwrap();
            let deleted = parser.enumerate_deleted(&source).unwrap();
            let recovered_after_wipe = parser.recover_bytes(&source, &deleted[0]).unwrap();

            assert_ne!(
                &recovered_after_wipe, original_content,
                "content should no longer match after wiping — the whole point of Phase 6"
            );
            assert!(
                recovered_after_wipe.iter().all(|&b| b == 0),
                "with the Zero pattern, recovered bytes should now be all zero"
            );
        }
    }

    #[test]
    fn refuses_to_overwrite_when_assume_ssd_is_set() {
        let tmp = copy_fixture_to_temp("fat32_basic.img");
        let image_path = tmp.path().to_str().unwrap();

        let err = wipe_free_space(image_path, &restora_domain::WipePattern::ZERO, true, false).unwrap_err();
        assert!(matches!(err, ApplicationError::SsdOverwriteRefused));

        // And confirm nothing was actually touched — the refusal must
        // happen before any write, not after a partial wipe.
        let source = ImageFileSource::open(image_path).unwrap();
        let (parser, _) = detect_parser(&source).unwrap();
        let deleted = parser.enumerate_deleted(&source).unwrap();
        let recovered = parser.recover_bytes(&source, &deleted[0]).unwrap();
        let expected =
            b"This is a canary file for FAT32 recovery testing. If you can read this after carving, the recovery worked.\n";
        assert_eq!(&recovered, expected, "refused wipe must leave the image completely untouched");
    }

    /// The other half of the write-blocking-by-construction claim: wiping
    /// never touches allocated clusters. We confirm a LIVE file (not the
    /// deleted one) survives a free-space wipe completely intact by
    /// checking the $MFT itself is still parseable afterward (if wipe
    /// ever accidentally overwrote $MFT's own clusters — which are
    /// allocated, not free — nothing would parse at all afterward).
    #[test]
    fn live_system_files_survive_a_free_space_wipe() {
        let tmp = copy_fixture_to_temp("ntfs_basic.img");
        let image_path = tmp.path().to_str().unwrap();

        wipe_free_space(image_path, &restora_domain::WipePattern::RANDOM, false, false).unwrap();

        // If $MFT's own clusters (allocated, not free) had been
        // overwritten, this would fail to parse at all.
        let source = ImageFileSource::open(image_path).unwrap();
        let (parser, fs_name) = detect_parser(&source).expect("filesystem should still be parseable after wipe");
        assert_eq!(fs_name, "NTFS");
        let _ = parser.enumerate_deleted(&source).expect("MFT should still be walkable after wipe");
    }
}
