//! `RecoverableFile`: the single model the application layer works with,
//! regardless of whether a result came from metadata parsing (FAT32/NTFS)
//! or from carving. This is what lets a UI show one results table instead
//! of two different shapes of data — the same idea as `FilesystemParser`
//! letting the CLI not care which concrete parser it's talking to, one
//! level up the stack.
//!
//! The key design question this type answers: **how do we remember how
//! to actually recover a file later**, potentially in a different process
//! than the one that found it (a persisted, reloaded scan session)? The
//! answer is `RecoveryLocator` — it carries everything needed to redo the
//! recovery from scratch: either a full `DeletedEntry` (metadata path, so
//! `recover_bytes` can be called again through a freshly-opened parser),
//! or a plain byte range (carved path, which never depended on any
//! parser state to begin with).

use crate::carving::CarvedFile;
use crate::parser::DeletedEntry;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoverySource {
    Metadata { filesystem: String },
    Carved { format: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryLocator {
    /// Recovery re-runs through a freshly-opened parser of the named
    /// filesystem, calling `FilesystemParser::recover_bytes` with this
    /// entry. For NTFS specifically, this only gets the *full* fragmented
    /// run-list recovery (better than the contiguous-guess fallback) if
    /// `enumerate_deleted` is called once on the fresh parser first — see
    /// `restora-application`'s `recovery_job` for exactly where that
    /// happens and why.
    Metadata(DeletedEntry),
    /// Recovery is just a direct byte-range read — no parser or
    /// filesystem-specific state involved at all, which is precisely why
    /// carved results survive a session reload with zero loss of
    /// recoverability, unlike (in principle) a metadata-based one.
    CarvedRange { start_offset: u64, end_offset: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverableFile {
    /// Unique within a single scan session — assigned sequentially as
    /// results are found, not a globally unique ID.
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub confidence: u8,
    pub source: RecoverySource,
    pub locator: RecoveryLocator,
}

impl RecoverableFile {
    pub fn from_deleted_entry(id: u64, filesystem: &str, entry: DeletedEntry) -> Self {
        Self {
            id,
            name: entry.name.clone(),
            size: entry.file_size,
            confidence: entry.confidence,
            source: RecoverySource::Metadata {
                filesystem: filesystem.to_string(),
            },
            locator: RecoveryLocator::Metadata(entry),
        }
    }

    pub fn from_carved_file(id: u64, index: usize, carved: &CarvedFile) -> Self {
        // Carved results have no original filename — number them, same
        // convention the Phase 3 CLI `carve` subcommand already uses.
        let name = format!("carved_{:04}.{}", index, carved.extension);
        Self {
            id,
            name,
            size: carved.size(),
            confidence: carved.confidence,
            source: RecoverySource::Carved {
                format: carved.format_name.clone(),
            },
            locator: RecoveryLocator::CarvedRange {
                start_offset: carved.start_offset,
                end_offset: carved.end_offset,
            },
        }
    }
}
