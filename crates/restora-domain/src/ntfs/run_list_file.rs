//! A file whose bytes are described by a list of `DataRun`s — possibly
//! fragmented across the disk — presented as one continuous logical byte
//! stream.
//!
//! Why this is worth its own type: in NTFS, `$MFT` (the master file
//! table itself) and `$Bitmap` (the free-space bitmap) are both just
//! ordinary files, each described by data runs exactly like a user file
//! would be. Both need the same operation — "give me bytes N..M of this
//! file's logical content" — translated through however many
//! fragmented physical extents that spans. Writing this once and reusing
//! it for both (instead of two near-duplicate implementations) is the
//! same instinct as the workspace's shared `ByteSource` trait: one
//! correct implementation of a tricky piece of arithmetic, used
//! everywhere it's needed.

use crate::error::Result;
use crate::ntfs::data_runs::DataRun;
use restora_infra::ByteSource;

pub struct RunListFile {
    runs: Vec<DataRun>,
    cluster_size: u64,
}

impl RunListFile {
    pub fn new(runs: Vec<DataRun>, cluster_size: u64) -> Self {
        Self { runs, cluster_size }
    }

    /// Total logical length available, in bytes — the sum of every run's
    /// cluster count times cluster size. Real files are usually shorter
    /// than this (the last cluster is padding), but for our purposes
    /// (reading MFT records or bitmap bytes, both cluster-aligned
    /// structures) that's fine.
    pub fn total_len(&self) -> u64 {
        self.runs.iter().map(|r| r.length_clusters * self.cluster_size).sum()
    }

    /// Reads `len` bytes starting at logical offset `offset` (position
    /// within this file's own content, not a disk offset), translating
    /// through however many physical extents that range touches.
    pub fn read_at(&self, source: &dyn ByteSource, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        let mut remaining = len as u64;
        let mut logical_pos = offset;
        let mut run_base: u64 = 0; // logical offset where the current run begins

        for run in &self.runs {
            let run_len_bytes = run.length_clusters * self.cluster_size;
            let run_end = run_base + run_len_bytes;

            if remaining == 0 {
                break;
            }
            if logical_pos < run_end && logical_pos + remaining > run_base {
                // This run overlaps the requested range — read the
                // overlapping slice from it.
                let start_in_run = logical_pos.saturating_sub(run_base);
                let bytes_available_in_run = run_len_bytes - start_in_run;
                let take = remaining.min(bytes_available_in_run);

                let disk_offset = run.start_lcn * self.cluster_size + start_in_run;
                let chunk = source.read_vec(disk_offset, take as usize)?;
                out.extend_from_slice(&chunk);

                logical_pos += take;
                remaining -= take;
            }

            run_base = run_end;
        }

        // If we ran out of runs before satisfying the full request (e.g.
        // requested past the end of a truncated/corrupted file), pad with
        // zeros rather than erroring — consistent with this being a
        // best-effort recovery tool, not a strict filesystem driver.
        if (out.len() as u64) < len as u64 {
            out.resize(len, 0);
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use restora_infra::ImageFileSource;
    use std::io::Write;

    #[test]
    fn reads_across_two_fragmented_runs() {
        // Physical disk layout (cluster_size = 16 bytes for an easy test):
        //   LCN 0-1: "AAAAAAAAAAAAAAAABBBBBBBBBBBBBBBB" (run A, 2 clusters)
        //   LCN 5:   "CCCCCCCCCCCCCCCC"                 (run B, 1 cluster)
        // Logical file = run A's 32 bytes followed by run B's 16 bytes,
        // i.e. 48 bytes total, even though physically fragmented.
        let mut disk = vec![0u8; 16 * 6];
        disk[0..32].copy_from_slice(&[b'A'; 16].iter().chain(&[b'B'; 16]).copied().collect::<Vec<u8>>());
        disk[16 * 5..16 * 6].copy_from_slice(&[b'C'; 16]);

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&disk).unwrap();
        let source = ImageFileSource::open(tmp.path()).unwrap();

        let runs = vec![
            DataRun { start_lcn: 0, length_clusters: 2 },
            DataRun { start_lcn: 5, length_clusters: 1 },
        ];
        let file = RunListFile::new(runs, 16);

        assert_eq!(file.total_len(), 48);

        // Read spanning exactly across the run boundary (bytes 24..40 of
        // the logical file: last 8 of run A's B-half, then first 8 of run B).
        let bytes = file.read_at(&source, 24, 16).unwrap();
        assert_eq!(&bytes, b"BBBBBBBBCCCCCCCC");
    }
}
