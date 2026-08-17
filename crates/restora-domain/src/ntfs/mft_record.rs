//! Parses a single MFT record's header and walks its attribute list.
//!
//! A record's layout, after fixups have already been applied (see
//! `fixup.rs` — this module assumes that's already happened):
//!
//! ```text
//! offset 0:  "FILE" signature (4 bytes) — or "BAAD" if corrupted
//! offset 22: flags (2 bytes) — bit 0 = in-use, bit 1 = is-directory
//! offset 20: offset to first attribute (2 bytes)
//! ...then a sequence of attributes, each self-describing its own length,
//!    ending with a 4-byte 0xFFFFFFFF terminator.
//! ```
//!
//! We only decode the two attribute types that matter for finding and
//! recovering a deleted file: `$FILE_NAME` (0x30, gives us the name) and
//! `$DATA` (0x80, gives us where the actual bytes live). Real NTFS has a
//! dozen more attribute types (`$STANDARD_INFORMATION`, `$INDEX_ROOT`,
//! security descriptors, reparse points...) — skipping the ones we don't
//! need is exactly what the generic attribute-walking loop below makes
//! easy: each attribute carries its own length, so skipping an unknown
//! one is just "add its length to the cursor and keep going."

use crate::error::Result;
use crate::ntfs::data_runs::{parse_data_runs, DataRun};

const ATTR_TYPE_FILE_NAME: u32 = 0x30;
const ATTR_TYPE_DATA: u32 = 0x80;
const ATTR_TYPE_END: u32 = 0xFFFF_FFFF;

const FLAG_IN_USE: u16 = 0x0001;
const FLAG_DIRECTORY: u16 = 0x0002;

#[derive(Debug, Clone)]
pub struct MftRecordHeader {
    pub is_valid_file_record: bool,
    pub in_use: bool,
    pub is_directory: bool,
    pub first_attribute_offset: u16,
}

pub fn parse_header(record: &[u8]) -> MftRecordHeader {
    let is_valid_file_record = record.len() >= 24 && &record[0..4] == b"FILE";
    if !is_valid_file_record {
        return MftRecordHeader {
            is_valid_file_record: false,
            in_use: false,
            is_directory: false,
            first_attribute_offset: 0,
        };
    }

    let flags = u16::from_le_bytes([record[22], record[23]]);
    let first_attribute_offset = u16::from_le_bytes([record[20], record[21]]);

    MftRecordHeader {
        is_valid_file_record,
        in_use: flags & FLAG_IN_USE != 0,
        is_directory: flags & FLAG_DIRECTORY != 0,
        first_attribute_offset,
    }
}

/// What we extract from a `$FILE_NAME` attribute — enough to display and
/// identify the file. Real NTFS stores several timestamps here too;
/// omitted for now as not needed for the recovery path itself.
#[derive(Debug, Clone)]
pub struct FileNameInfo {
    pub name: String,
    pub real_size: u64,
}

/// Either a resident attribute's raw content bytes, or a non-resident
/// attribute's decoded data runs — the two fundamentally different ways
/// NTFS can store `$DATA`.
#[derive(Debug, Clone)]
pub enum DataLocation {
    Resident(Vec<u8>),
    NonResident(Vec<DataRun>),
}

#[derive(Debug, Clone, Default)]
pub struct ParsedAttributes {
    pub file_name: Option<FileNameInfo>,
    pub data: Option<DataLocation>,
}

/// Walks every attribute in a (fixup-applied) MFT record and extracts the
/// `$FILE_NAME` and `$DATA` attributes if present.
pub fn parse_attributes(record: &[u8], header: &MftRecordHeader) -> Result<ParsedAttributes> {
    let mut result = ParsedAttributes::default();
    let mut pos = header.first_attribute_offset as usize;

    while pos + 4 <= record.len() {
        let attr_type = u32::from_le_bytes(record[pos..pos + 4].try_into().unwrap());
        if attr_type == ATTR_TYPE_END {
            break;
        }
        if pos + 8 > record.len() {
            break; // truncated record, stop rather than read out of bounds
        }
        let attr_len = u32::from_le_bytes(record[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if attr_len == 0 || pos + attr_len > record.len() {
            break; // corrupted length — stop parsing this record defensively
        }

        let non_resident = record[pos + 8] != 0;

        match attr_type {
            ATTR_TYPE_FILE_NAME if !non_resident => {
                if let Some(info) = parse_file_name_attribute(&record[pos..pos + attr_len]) {
                    // A file can have multiple $FILE_NAME attributes (one
                    // per hard link, or a short 8.3 alias alongside the
                    // long name) — we keep the first one found, which is
                    // sufficient for Phase 4's scope.
                    if result.file_name.is_none() {
                        result.file_name = Some(info);
                    }
                }
            }
            ATTR_TYPE_DATA => {
                if result.data.is_none() {
                    result.data = Some(parse_data_attribute(&record[pos..pos + attr_len], non_resident)?);
                }
            }
            _ => {} // any other attribute type: skip, we don't need it
        }

        pos += attr_len;
    }

    Ok(result)
}

fn parse_file_name_attribute(attr: &[u8]) -> Option<FileNameInfo> {
    // Resident attribute header: content_length @ offset 16, content_offset @ offset 20
    if attr.len() < 24 {
        return None;
    }
    let content_offset = u16::from_le_bytes([attr[20], attr[21]]) as usize;
    let content = &attr[content_offset..];
    if content.len() < 66 {
        return None;
    }

    let real_size = u64::from_le_bytes(content[48..56].try_into().unwrap());
    let name_len_chars = content[64] as usize;
    let name_bytes_len = name_len_chars * 2;
    if 66 + name_bytes_len > content.len() {
        return None;
    }

    let name_utf16: Vec<u16> = content[66..66 + name_bytes_len]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let name = String::from_utf16_lossy(&name_utf16);

    Some(FileNameInfo { name, real_size })
}

fn parse_data_attribute(attr: &[u8], non_resident: bool) -> Result<DataLocation> {
    if non_resident {
        // Non-resident header: data_run_offset @ offset 32 (2 bytes)
        let run_offset = u16::from_le_bytes([attr[32], attr[33]]) as usize;
        let runs = parse_data_runs(&attr[run_offset..])?;
        Ok(DataLocation::NonResident(runs))
    } else {
        // Resident header: content_length @ offset 16, content_offset @ offset 20
        let content_length = u32::from_le_bytes(attr[16..20].try_into().unwrap()) as usize;
        let content_offset = u16::from_le_bytes([attr[20], attr[21]]) as usize;
        let content = attr[content_offset..content_offset + content_length].to_vec();
        Ok(DataLocation::Resident(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_file_signature() {
        let record = vec![0u8; 64]; // all zeros — no "FILE" signature
        let header = parse_header(&record);
        assert!(!header.is_valid_file_record);
    }

    #[test]
    fn parses_in_use_and_directory_flags() {
        let mut record = vec![0u8; 64];
        record[0..4].copy_from_slice(b"FILE");
        record[20..22].copy_from_slice(&48u16.to_le_bytes()); // first_attribute_offset
        record[22..24].copy_from_slice(&0x0003u16.to_le_bytes()); // in_use | directory

        let header = parse_header(&record);
        assert!(header.is_valid_file_record);
        assert!(header.in_use);
        assert!(header.is_directory);
        assert_eq!(header.first_attribute_offset, 48);
    }

    #[test]
    fn parses_resident_file_name_attribute() {
        // Build a minimal record with a single resident $FILE_NAME
        // attribute containing the name "HI.TXT" (6 chars).
        let name = "HI.TXT";
        let name_utf16: Vec<u8> = name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();

        let content_offset: u16 = 24; // right after the resident attr header
        let mut content = vec![0u8; 66];
        content[48..56].copy_from_slice(&1234u64.to_le_bytes()); // real_size
        content[64] = name.chars().count() as u8; // name length in chars
        content.extend_from_slice(&name_utf16);

        let mut attr = Vec::new();
        attr.extend((ATTR_TYPE_FILE_NAME).to_le_bytes()); // type
        let attr_len = (24 + content.len()) as u32;
        attr.extend(attr_len.to_le_bytes()); // length
        attr.push(0); // non_resident = false
        attr.push(0); // name_length
        attr.extend(0u16.to_le_bytes()); // name_offset
        attr.extend(0u16.to_le_bytes()); // flags
        attr.extend(0u16.to_le_bytes()); // attribute_id
        attr.extend((content.len() as u32).to_le_bytes()); // content_length
        attr.extend(content_offset.to_le_bytes()); // content_offset
        attr.push(0); // indexed flag
        attr.push(0); // padding
        attr.extend(&content);

        let mut record = vec![0u8; 32];
        record[0..4].copy_from_slice(b"FILE");
        record[20..22].copy_from_slice(&32u16.to_le_bytes());
        record[22..24].copy_from_slice(&1u16.to_le_bytes()); // in_use
        record.extend(&attr);
        record.extend(ATTR_TYPE_END.to_le_bytes()); // terminator

        let header = parse_header(&record);
        let parsed = parse_attributes(&record, &header).unwrap();

        let fname = parsed.file_name.expect("expected a parsed $FILE_NAME");
        assert_eq!(fname.name, "HI.TXT");
        assert_eq!(fname.real_size, 1234);
    }
}
