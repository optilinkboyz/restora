//! Parses the NTFS boot sector — the first 512 bytes of an NTFS volume.
//!
//! NTFS's boot sector is structurally different from FAT32's in one
//! important way worth understanding up front: it doesn't describe the
//! filesystem's *layout* directly (no root cluster, no FAT size). Instead
//! it tells you exactly one crucial thing — where the **Master File
//! Table ($MFT)** starts — and the MFT is itself a file, described by its
//! own first record, whose data runs tell you everything else. Every
//! other piece of metadata in NTFS (directories, the free-space bitmap,
//! even the boot sector's backup copy) is *also* just a file with its own
//! MFT record. This is the core design idea worth carrying into the
//! parser code: almost everything routes through "read an MFT record,
//! parse its $DATA attribute's data runs."

use crate::error::{DomainError, Result};
use restora_infra::ByteSource;

#[derive(Debug, Clone)]
pub struct NtfsBootSector {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub total_sectors: u64,
    /// Cluster number where the $MFT's own first record (record 0) sits.
    pub mft_cluster_number: u64,
    /// Size in bytes of one MFT record — usually 1024, but the on-disk
    /// field encodes it in a slightly indirect way (see `parse` below).
    pub mft_record_size: u32,
}

impl NtfsBootSector {
    pub fn parse(source: &dyn ByteSource) -> Result<Self> {
        let sector = source.read_vec(0, 512)?;

        if sector[510] != 0x55 || sector[511] != 0xAA {
            return Err(DomainError::NotFat32(
                "missing 0x55AA boot sector signature".into(),
            ));
        }
        let oem_id = &sector[0x03..0x03 + 8];
        if oem_id != b"NTFS    " {
            return Err(DomainError::NotFat32(format!(
                "OEM ID is not 'NTFS    ': {:?}",
                String::from_utf8_lossy(oem_id)
            )));
        }

        let bytes_per_sector = u16::from_le_bytes([sector[0x0B], sector[0x0C]]);
        let sectors_per_cluster = sector[0x0D];

        let total_sectors = u64::from_le_bytes(sector[0x28..0x30].try_into().unwrap());
        let mft_cluster_number = u64::from_le_bytes(sector[0x30..0x38].try_into().unwrap());

        // Clusters-per-MFT-record is stored as a SIGNED byte, and its
        // meaning flips depending on sign — a quirk worth knowing rather
        // than just copying: if positive, it's literally "this many
        // clusters make one MFT record." If negative, the actual record
        // size is 2^|value| bytes instead (this is how NTFS supports MFT
        // record sizes smaller than one cluster, e.g. the common case of
        // 1024-byte records on a 4096-byte-cluster volume).
        let raw = sector[0x40] as i8;
        let mft_record_size = if raw > 0 {
            raw as u32 * sectors_per_cluster as u32 * bytes_per_sector as u32
        } else {
            1u32 << (-(raw as i32))
        };

        Ok(Self {
            bytes_per_sector,
            sectors_per_cluster,
            total_sectors,
            mft_cluster_number,
            mft_record_size,
        })
    }

    pub fn cluster_size_bytes(&self) -> u64 {
        self.sectors_per_cluster as u64 * self.bytes_per_sector as u64
    }

    /// Byte offset of the start of a given LCN (Logical Cluster Number).
    /// Unlike FAT32, NTFS numbers clusters from 0, not 2 — a small but
    /// easy detail to get wrong if you're used to the FAT convention.
    pub fn cluster_offset(&self, lcn: u64) -> u64 {
        lcn * self.cluster_size_bytes()
    }

    /// Byte offset of MFT record 0 — the very first thing we need to
    /// locate before anything else in NTFS parsing can proceed.
    pub fn mft_start_offset(&self) -> u64 {
        self.cluster_offset(self.mft_cluster_number)
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

    #[test]
    fn parses_real_fixture_boot_sector() {
        let source = ImageFileSource::open(fixture_path())
            .expect("fixture image missing — run scripts/make_ntfs_fixture.py first");
        let bs = NtfsBootSector::parse(&source).unwrap();

        assert_eq!(bs.bytes_per_sector, 512);
        assert_eq!(bs.sectors_per_cluster, 1); // matches the fixture generator
        assert_eq!(bs.mft_record_size, 1024);
        assert!(bs.mft_start_offset() > 0);
    }
}
