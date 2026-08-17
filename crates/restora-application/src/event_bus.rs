//! The channel a `ScanSession` reports progress through while it runs.
//!
//! Deliberately a plain `std::sync::mpsc::Sender`, not anything
//! async-runtime-specific — matches the workspace's earlier decision to
//! favor a plain thread pool over pulling in tokio for a desktop tool
//! that isn't doing networked I/O. A UI (or, for now, the CLI) holds the
//! matching `Receiver` on another thread and reacts to events as they
//! arrive while the scan runs on its own thread.

use restora_domain::RecoverableFile;
use std::sync::mpsc::Sender;

#[derive(Debug, Clone)]
pub enum ScanEvent {
    PhaseStarted { phase: String },
    Progress { phase: String, scanned_bytes: u64, total_bytes: u64 },
    FileFound { file: RecoverableFile },
    PhaseCompleted { phase: String },
    Cancelled,
    Completed { total_found: usize },
    Failed { message: String },
}

pub type EventSender = Sender<ScanEvent>;
