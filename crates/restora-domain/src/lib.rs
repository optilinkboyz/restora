//! restora-domain
//!
//! Pure parsing/carving logic: FilesystemParser and Carver traits and their
//! implementations. No direct I/O — everything reads through a
//! `&dyn ByteSource` from restora-infra. This is what makes the hardest
//! logic in the project unit-testable without a real disk anywhere in sight.
//!
//! Phase 2 (this): FAT32Parser — root-directory deleted-entry recovery.
//! Coming later: SignatureCarver (Phase 3), NtfsParser (Phase 4).

pub mod error;
pub mod fat32;
pub mod parser;

pub use error::{DomainError, Result};
pub use parser::{ClusterRange, DeletedEntry, FilesystemParser};
