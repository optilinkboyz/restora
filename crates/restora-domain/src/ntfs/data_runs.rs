//! Decodes NTFS "data runs" — the compact, variable-length encoding a
//! non-resident attribute uses to describe which clusters hold its data.
//!
//! **The encoding.** A data run list is a sequence of runs, each shaped:
//!
//! ```text
//! [header byte] [length bytes...] [offset bytes...]
//! ```
//!
//! The header byte packs two nibbles: the low nibble says how many bytes
//! encode the run's *length* (in clusters), the high nibble says how many
//! bytes encode the run's *offset*. Both length and offset are stored
//! little-endian, and — this is the part that catches people off guard —
//! **the offset is a signed delta from the previous run's LCN**, not an
//! absolute cluster number. The very first run's offset is a delta from
//! LCN 0, i.e. effectively absolute. The list ends at a header byte of
//! 0x00.
//!
//! Why signed deltas instead of absolute numbers? It keeps highly
//! fragmented files' run lists compact — nearby fragments only need a
//! couple of offset bytes instead of a full 8-byte LCN every time.

use crate::error::{DomainError, Result};

/// One decoded run: `length` clusters starting at `start_lcn`, already
/// resolved from the raw signed-delta encoding to an absolute LCN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataRun {
    pub start_lcn: u64,
    pub length_clusters: u64,
}

/// Parses a full data run list starting at `bytes[0]`, resolving deltas
/// into absolute LCNs as it goes. Stops at the terminating 0x00 header
/// byte, or at the end of the slice if no terminator is found (tolerated,
/// since a fixup-corrupted or truncated record shouldn't panic a
/// recovery tool).
pub fn parse_data_runs(bytes: &[u8]) -> Result<Vec<DataRun>> {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut current_lcn: i64 = 0; // running absolute position, deltas accumulate onto this

    while pos < bytes.len() {
        let header = bytes[pos];
        if header == 0x00 {
            break; // list terminator
        }
        pos += 1;

        let length_size = (header & 0x0F) as usize;
        let offset_size = ((header >> 4) & 0x0F) as usize;

        if pos + length_size + offset_size > bytes.len() {
            return Err(DomainError::DirEntry(
                "data run header claims more bytes than remain in the attribute".into(),
            ));
        }

        let length = read_unsigned_le(&bytes[pos..pos + length_size]);
        pos += length_size;

        // A "sparse" run (offset_size == 0) has no physical location at
        // all — it represents a hole full of implicit zeros. We skip
        // these; there's nothing on disk to recover.
        if offset_size > 0 {
            let delta = read_signed_le(&bytes[pos..pos + offset_size]);
            current_lcn += delta;

            if current_lcn < 0 {
                return Err(DomainError::DirEntry(
                    "data run resolved to a negative LCN — corrupted attribute".into(),
                ));
            }

            runs.push(DataRun {
                start_lcn: current_lcn as u64,
                length_clusters: length,
            });
        }
        pos += offset_size;
    }

    Ok(runs)
}

fn read_unsigned_le(bytes: &[u8]) -> u64 {
    let mut value: u64 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        value |= (b as u64) << (8 * i);
    }
    value
}

/// Data run offsets are little-endian, arbitrary-width, two's-complement
/// signed integers — meaning we have to sign-extend based on the top bit
/// of the *last* byte actually present, not assume a fixed width like i32
/// or i64.
fn read_signed_le(bytes: &[u8]) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    let mut value: i64 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        value |= (b as i64) << (8 * i);
    }
    // Sign-extend: if the highest bit of the most significant byte we
    // read is set, the value is negative, and everything above the bytes
    // we have needs to become 1s.
    let top_byte = bytes[bytes.len() - 1];
    if top_byte & 0x80 != 0 {
        let shift = 8 * bytes.len();
        if shift < 64 {
            value |= -1i64 << shift;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_contiguous_run() {
        // header 0x21: offset_size=2 (high nibble), length_size=1 (low nibble)
        // length = 0x0A (10 clusters), offset = 0x0064 (delta = +100, so LCN 100)
        let bytes = [0x21, 0x0A, 0x64, 0x00, 0x00]; // trailing 0x00 = terminator
        let runs = parse_data_runs(&bytes).unwrap();

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0], DataRun { start_lcn: 100, length_clusters: 10 });
    }

    #[test]
    fn parses_multiple_runs_with_negative_delta() {
        // Run 1: header 0x21, length=5, offset delta=+200 => LCN 200
        // Run 2: header 0x21, length=3, offset delta=-50  => LCN 150
        //   (delta encoded as 0xFFCE = -50 in 16-bit two's complement)
        let mut bytes = Vec::new();
        bytes.extend([0x21, 5, 200, 0]); // run 1: +200
        bytes.extend([0x21, 3, 0xCE, 0xFF]); // run 2: -50 (0xFFCE)
        bytes.push(0x00); // terminator

        let runs = parse_data_runs(&bytes).unwrap();

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0], DataRun { start_lcn: 200, length_clusters: 5 });
        assert_eq!(runs[1], DataRun { start_lcn: 150, length_clusters: 3 }); // 200-50
    }

    #[test]
    fn stops_at_terminator_and_ignores_trailing_bytes() {
        let bytes = [0x11, 4, 10, 0x00, 0xFF, 0xFF, 0xFF]; // junk after terminator, ignored
        let runs = parse_data_runs(&bytes).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0], DataRun { start_lcn: 10, length_clusters: 4 });
    }
}
