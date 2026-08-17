//! Parses the FAT32 Boot Sector / BIOS Parameter Block (BPB) — the first
//! 512 bytes of any FAT32 volume, and the thing that tells us where
//! everything else lives (the FAT tables, the root directory, the data
//! area).
//!
//! Field offsets below are the official Microsoft FAT32 spec offsets. If
//! you want to sanity-check these by eye against a real image, the boot
//! sector dump you already ran through `restora-cli` shows every one of
//! these fields: bytes_per_sector at 0x0B, sectors_per_cluster at 0x0D,
//! the "FAT32   " signature string at 0x52, and the 0x55AA boot signature
//! at the very last two bytes of the sector.

use crate::error::{DomainError, Result};
use restora_infra::ByteSource;

#[derive(Debug, Clone)]
pub struct Fat32BootSector {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sector_count: u16,
    pub num_fats: u8,
    pub fat_size_32: u32,
    pub root_cluster: u32,
    pub total_sectors: u32,
}

impl Fat32BootSector {
    pub fn parse(source: &dyn ByteSource) -> Result<Self> {
        let sector = source.read_vec(0, 512)?;

        // Boot signature must be present regardless of filesystem type.
        if sector[510] != 0x55 || sector[511] != 0xAA {
            return Err(DomainError::NotFat32(
                "missing 0x55AA boot sector signature".into(),
            ));
        }

        // "FAT32   " (8 bytes, space-padded) lives at offset 0x52 when
        // BootSig (offset 0x42) == 0x29, indicating the extended BPB
        // fields (VolID/VolLab/FilSysType) are present.
        let boot_sig = sector[0x42];
        if boot_sig != 0x29 {
            return Err(DomainError::NotFat32(format!(
                "unexpected boot signature byte: 0x{boot_sig:02x}"
            )));
        }
        let fs_type = &sector[0x52..0x52 + 8];
        if fs_type != b"FAT32   " {
            return Err(DomainError::NotFat32(format!(
                "FilSysType field is not 'FAT32   ': {:?}",
                String::from_utf8_lossy(fs_type)
            )));
        }

        let bytes_per_sector = u16::from_le_bytes([sector[0x0B], sector[0x0C]]);
        let sectors_per_cluster = sector[0x0D];
        let reserved_sector_count = u16::from_le_bytes([sector[0x0E], sector[0x0F]]);
        let num_fats = sector[0x10];

        let total_sectors_16 = u16::from_le_bytes([sector[0x13], sector[0x14]]);
        let total_sectors_32 = u32::from_le_bytes([
            sector[0x20],
            sector[0x21],
            sector[0x22],
            sector[0x23],
        ]);
        // Spec: if the volume fits in 16 bits of sectors, TotSec16 holds
        // it and TotSec32 is 0. Otherwise the reverse. Use whichever is
        // non-zero.
        let total_sectors = if total_sectors_16 != 0 {
            total_sectors_16 as u32
        } else {
            total_sectors_32
        };

        let fat_size_32 = u32::from_le_bytes([
            sector[0x24],
            sector[0x25],
            sector[0x26],
            sector[0x27],
        ]);
        let root_cluster = u32::from_le_bytes([
            sector[0x2C],
            sector[0x2D],
            sector[0x2E],
            sector[0x2F],
        ]);

        Ok(Self {
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sector_count,
            num_fats,
            fat_size_32,
            root_cluster,
            total_sectors,
        })
    }

    /// Byte offset where the first FAT table begins.
    pub fn fat_start_offset(&self) -> u64 {
        self.reserved_sector_count as u64 * self.bytes_per_sector as u64
    }

    /// Byte offset where the data area (cluster 2 onward) begins — this is
    /// past the reserved region *and* all `num_fats` copies of the FAT.
    pub fn data_area_offset(&self) -> u64 {
        self.fat_start_offset()
            + (self.num_fats as u64 * self.fat_size_32 as u64 * self.bytes_per_sector as u64)
    }

    /// Size in bytes of one cluster.
    pub fn cluster_size_bytes(&self) -> u64 {
        self.sectors_per_cluster as u64 * self.bytes_per_sector as u64
    }

    /// Byte offset of the start of a given cluster number. Cluster
    /// numbering starts at 2 (0 and 1 are reserved) — this is a classic
    /// off-by-one trap, worth remembering explicitly.
    pub fn cluster_offset(&self, cluster: u32) -> u64 {
        self.data_area_offset() + (cluster as u64 - 2) * self.cluster_size_bytes()
    }
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
    fn parses_real_fixture_boot_sector() {
        let source = ImageFileSource::open(fixture_path()).expect("fixture image missing — run scripts/make_fat32_fixture.sh first");
        let bs = Fat32BootSector::parse(&source).unwrap();

        assert_eq!(bs.bytes_per_sector, 512);
        assert_eq!(bs.root_cluster, 2);
        assert_eq!(bs.num_fats, 2);
        // Sanity: data area must start after reserved + both FAT copies,
        // and must be well within the 16MB image.
        assert!(bs.data_area_offset() > bs.fat_start_offset());
        assert!(bs.data_area_offset() < 16 * 1024 * 1024);
    }
}
