//! restora-domain
//!
//! Pure parsing/carving logic: FilesystemParser and Carver traits and their
//! implementations. No direct I/O — everything reads through a
//! `&dyn ByteSource` from restora-infra. This is what makes the hardest
//! logic in the project unit-testable without a real disk anywhere in sight.
//!
//! Phase 2: FAT32Parser — metadata-based recovery (root dir + subdirs).
//! Phase 3: SignatureCarver — metadata-free recovery.
//! Phase 4 (this): NtfsParser — MFT-based recovery with $Bitmap confidence
//! scoring.

pub mod carving;
pub mod error;
pub mod fat32;
pub mod ntfs;
pub mod parser;

pub use carving::{CarvedFile, Carver, SignatureCarver};
pub use error::{DomainError, Result};
pub use parser::{ClusterRange, DeletedEntry, FilesystemParser};
