//! restora-domain
//!
//! Pure parsing/carving logic: FilesystemParser and Carver traits and their
//! implementations. No direct I/O — everything reads through a
//! `&dyn ByteSource` from restora-infra. This is what makes the hardest
//! logic in the project unit-testable without a real disk anywhere in sight.
//!
//! Phase 2: FAT32Parser — metadata-based recovery (root dir + subdirs).
//! Phase 3: SignatureCarver — metadata-free recovery.
//! Phase 4: NtfsParser — MFT-based recovery with $Bitmap confidence
//! scoring.
//! Phase 5 (this): RecoverableFile — unified model + shared filesystem
//! detection, consumed by restora-application's ScanSession.

pub mod carving;
pub mod error;
pub mod fat32;
pub mod ntfs;
pub mod parser;
pub mod recoverable;

pub use carving::{CarvedFile, Carver, SignatureCarver};
pub use error::{DomainError, Result};
pub use parser::{ClusterRange, DeletedEntry, FilesystemParser};
pub use recoverable::{RecoverableFile, RecoveryLocator, RecoverySource};

/// Tries each known metadata-based parser in turn and returns the first
/// one that recognizes the image. This is the shared home for filesystem
/// dispatch logic — both the CLI and the application layer's ScanSession
/// use this instead of each hand-rolling their own try-FAT32-then-NTFS
/// chain, which is exactly the kind of duplication worth eliminating once
/// a second caller shows up needing the same logic.
///
/// Returns `None` (not an error) if no known filesystem is recognized —
/// that's a legitimate outcome (e.g. a raw carving-only image), not a
/// failure.
pub fn detect_parser(source: &dyn restora_infra::ByteSource) -> Option<(Box<dyn FilesystemParser>, &'static str)> {
    if fat32::Fat32Parser::detect(source) {
        if let Ok(parser) = fat32::Fat32Parser::new(source) {
            return Some((Box::new(parser), "FAT32"));
        }
    }
    if ntfs::NtfsParser::detect(source) {
        if let Ok(parser) = ntfs::NtfsParser::new(source) {
            return Some((Box::new(parser), "NTFS"));
        }
    }
    None
}
