//! Applies NTFS "fixups" to a raw MFT record — the single most common
//! thing that silently corrupts a hand-rolled NTFS parser's output if
//! it's skipped.
//!
//! **The problem fixups solve.** NTFS needs to detect torn writes — a
//! power loss mid-write to a multi-sector structure. Its solution: before
//! writing an MFT record to disk, NTFS saves the last 2 bytes of *every*
//! 512-byte sector within the record, replaces those same 2 bytes with a
//! shared "Update Sequence Number" (USN), and stores the real saved bytes
//! in a small array elsewhere in the record (the "Update Sequence
//! Array"). On read, a driver checks that every sector-ending 2 bytes
//! equals the USN (proof the whole record was written together) and then
//! restores the original bytes before parsing anything else.
//!
//! **What this means for us**: if you parse a raw MFT record's bytes
//! without doing this restoration first, the last 2 bytes of every
//! 512-byte chunk inside the record are corrupted stand-in values, not
//! real data — for a typical 1024-byte record (2 sectors), that's bytes
//! at offsets 510-511 and 1022-1023, right in the middle of attribute
//! data if you're unlucky. Fixups must be applied before any attribute
//! parsing happens, always.

use crate::error::{DomainError, Result};

/// Applies fixups to `record` **in place**. `bytes_per_sector` matches the
/// volume's actual sector size (usually 512) — that's the unit fixups are
/// applied per, not the MFT record size itself.
pub fn apply_fixups(record: &mut [u8], bytes_per_sector: usize) -> Result<()> {
    if record.len() < 8 {
        return Err(DomainError::DirEntry("MFT record too short for a header".into()));
    }

    let usa_offset = u16::from_le_bytes([record[4], record[5]]) as usize;
    let usa_count = u16::from_le_bytes([record[6], record[7]]) as usize;

    // usa_count includes the USN value itself plus one entry per sector,
    // so a 2-sector (1024-byte) record has usa_count == 3.
    if usa_count == 0 {
        return Ok(()); // nothing to fix up — some tools write this for empty records
    }

    if usa_offset + usa_count * 2 > record.len() {
        return Err(DomainError::DirEntry(
            "update sequence array runs past end of record".into(),
        ));
    }

    let usn = [record[usa_offset], record[usa_offset + 1]];

    for i in 1..usa_count {
        let sector_end = i * bytes_per_sector - 2;
        if sector_end + 2 > record.len() {
            break; // record is shorter than usa_count implies — be lenient
        }

        // The check every real NTFS driver performs: the bytes currently
        // sitting at the sector boundary should equal the USN. If they
        // don't, the sector's fixup marker doesn't match — a sign of a
        // torn write or, in our recovery context, of a record that's
        // partially overwritten by something else. We don't hard-fail on
        // mismatch (a recovery tool needs to be tolerant of exactly this
        // kind of damage), but it's worth knowing this check exists.
        let found = &record[sector_end..sector_end + 2];
        if found != usn {
            // Best-effort recovery tool stance: proceed anyway, since the
            // rest of the sector may still be perfectly readable. A
            // stricter tool could return an error here instead.
        }

        let original_offset = usa_offset + i * 2;
        record[sector_end] = record[original_offset];
        record[sector_end + 1] = record[original_offset + 1];
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_sector_boundary_bytes() {
        // Build a minimal 1024-byte (2-sector) record with a fixup array
        // at offset 0x30 (arbitrary, just needs usa_offset to point at it).
        let mut record = vec![0u8; 1024];

        let usa_offset: u16 = 0x30;
        let usa_count: u16 = 3; // USN + 2 sector entries
        record[4..6].copy_from_slice(&usa_offset.to_le_bytes());
        record[6..8].copy_from_slice(&usa_count.to_le_bytes());

        let usn: [u8; 2] = [0xAB, 0xCD];
        let original_sector1_end: [u8; 2] = [0x11, 0x22]; // real data that belongs at 510..512
        let original_sector2_end: [u8; 2] = [0x33, 0x44]; // real data that belongs at 1022..1024

        // Write the USA: [usn, original_sector1_end, original_sector2_end]
        record[0x30..0x32].copy_from_slice(&usn);
        record[0x32..0x34].copy_from_slice(&original_sector1_end);
        record[0x34..0x36].copy_from_slice(&original_sector2_end);

        // Simulate what's actually on disk: sector-ending bytes replaced
        // with the USN (this is what apply_fixups must undo).
        record[510..512].copy_from_slice(&usn);
        record[1022..1024].copy_from_slice(&usn);

        apply_fixups(&mut record, 512).unwrap();

        assert_eq!(&record[510..512], &original_sector1_end);
        assert_eq!(&record[1022..1024], &original_sector2_end);
    }
}
