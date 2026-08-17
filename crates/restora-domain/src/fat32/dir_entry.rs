//! Parses 32-byte FAT directory entries and identifies deleted ones.
//!
//! The core trick you need to understand for FAT recovery: deleting a file
//! does NOT erase its directory entry. It only overwrites the entry's
//! first byte with 0xE5. Every other field — the rest of the filename, the
//! starting cluster number, the file size — is left completely intact.
//! That's the entire reason FAT recovery is possible at all.
//!
//! This module only handles short (8.3) names. Long filenames (VFAT LFN)
//! are stored in extra preceding entries with attribute 0x0F — real-world
//! support for those is a good Phase 2.5 follow-up once this baseline
//! works, since it mostly changes name reconstruction, not the recovery
//! logic itself.

pub const DELETED_MARKER: u8 = 0xE5;
const END_OF_DIRECTORY: u8 = 0x00;
const ATTR_LONG_NAME: u8 = 0x0F;
const ATTR_VOLUME_ID: u8 = 0x08;

#[derive(Debug, Clone)]
pub struct RawDirEntry {
    pub raw_name: [u8; 11],
    pub attr: u8,
    pub first_cluster: u32,
    pub file_size: u32,
    pub is_deleted: bool,
}

/// What `parse_entry` can tell us about a raw 32-byte slot.
pub enum EntrySlot {
    /// 0x00 as the first byte — everything from here to the end of the
    /// directory's clusters is unused. Callers should stop iterating.
    EndOfDirectory,
    /// A long-filename fragment or volume-label entry — not a real file,
    /// skip it.
    Skip,
    Entry(RawDirEntry),
}

/// Parses one 32-byte directory entry slot.
pub fn parse_entry(bytes: &[u8; 32]) -> EntrySlot {
    if bytes[0] == END_OF_DIRECTORY {
        return EntrySlot::EndOfDirectory;
    }

    let attr = bytes[11];
    if attr == ATTR_LONG_NAME || attr == ATTR_VOLUME_ID {
        return EntrySlot::Skip;
    }

    let is_deleted = bytes[0] == DELETED_MARKER;

    let mut raw_name = [0u8; 11];
    raw_name.copy_from_slice(&bytes[0..11]);

    let cluster_hi = u16::from_le_bytes([bytes[20], bytes[21]]);
    let cluster_lo = u16::from_le_bytes([bytes[26], bytes[27]]);
    let first_cluster = ((cluster_hi as u32) << 16) | (cluster_lo as u32);

    let file_size = u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]);

    EntrySlot::Entry(RawDirEntry {
        raw_name,
        attr,
        first_cluster,
        file_size,
        is_deleted,
    })
}

/// Reconstructs a human-readable "NAME.EXT" from the raw 11-byte 8.3 field.
///
/// Note the first byte may be the 0xE5 deleted marker — real FAT recovery
/// tools conventionally render that as `_` since the original first
/// character is unrecoverable from the marker alone (it's genuinely gone;
/// only the *rest* of the name survives).
pub fn format_name(entry: &RawDirEntry) -> String {
    let mut name_part: Vec<u8> = entry.raw_name[0..8].to_vec();
    if entry.is_deleted {
        name_part[0] = b'_';
    }
    let ext_part = &entry.raw_name[8..11];

    let name = String::from_utf8_lossy(&name_part).trim_end().to_string();
    let ext = String::from_utf8_lossy(ext_part).trim_end().to_string();

    if ext.is_empty() {
        name
    } else {
        format!("{name}.{ext}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_deleted_entry_and_preserves_other_fields() {
        // A live "CANARY.TXT" entry as FAT would actually lay it out on
        // disk, sized 107 bytes at cluster 3, then deleted (first byte set
        // to 0xE5).
        let mut raw = [0u8; 32];
        raw[0..11].copy_from_slice(b"CANARY  TXT");
        raw[11] = 0x20; // ARCHIVE attribute — a normal file
        raw[26..28].copy_from_slice(&3u16.to_le_bytes()); // cluster_lo = 3
        raw[28..32].copy_from_slice(&107u32.to_le_bytes()); // size = 107

        // Undeleted first
        if let EntrySlot::Entry(e) = parse_entry(&raw) {
            assert!(!e.is_deleted);
            assert_eq!(format_name(&e), "CANARY.TXT");
            assert_eq!(e.first_cluster, 3);
            assert_eq!(e.file_size, 107);
        } else {
            panic!("expected Entry");
        }

        // Now simulate deletion: only byte 0 changes.
        raw[0] = DELETED_MARKER;
        if let EntrySlot::Entry(e) = parse_entry(&raw) {
            assert!(e.is_deleted);
            assert_eq!(format_name(&e), "_ANARY.TXT"); // first char unrecoverable
            assert_eq!(e.first_cluster, 3); // cluster survives intact
            assert_eq!(e.file_size, 107); // size survives intact
        } else {
            panic!("expected Entry");
        }
    }

    #[test]
    fn end_of_directory_marker_stops_iteration() {
        let raw = [0u8; 32];
        assert!(matches!(parse_entry(&raw), EntrySlot::EndOfDirectory));
    }

    #[test]
    fn long_name_entries_are_skipped() {
        let mut raw = [0u8; 32];
        // byte[0] must be non-zero here, or parse_entry reads it as the
        // 0x00 end-of-directory marker before ever checking the attr byte.
        raw[0] = 0x41; // arbitrary non-zero, non-0xE5 name byte
        raw[11] = ATTR_LONG_NAME;
        assert!(matches!(parse_entry(&raw), EntrySlot::Skip));
    }
}
