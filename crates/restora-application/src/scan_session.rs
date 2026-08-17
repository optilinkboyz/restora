//! `ScanSession`: drives a scan end-to-end (metadata pass, then
//! optionally a carving pass), reporting progress through an `EventBus`
//! and producing a `Vec<RecoverableFile>` that a `SessionStore` can
//! persist and a `RecoveryJob` can later act on.
//!
//! **Honest scope note on pause/resume.** The architecture calls for an
//! `Idle -> Scanning -> Paused -> Complete` state machine implying a scan
//! can be paused mid-byte-range and later resumed from exactly where it
//! left off. What's implemented here: the *carving* phase checks a
//! cancellation flag between chunks (so it can stop promptly, and what it
//! found so far is kept, not discarded) — but resuming a cancelled scan
//! currently re-scans from the beginning rather than continuing from the
//! exact byte offset it stopped at. True byte-offset resumability is a
//! reasonable next increment (persist `scanned_bytes` and pass it back in
//! as a starting `range.start`), flagged here rather than silently
//! assumed to already work.

use crate::error::Result;
use crate::event_bus::{EventSender, ScanEvent};
use restora_domain::carving::SignatureCarver;
use restora_domain::{detect_parser, RecoverableFile};
use restora_infra::{ByteSource, ImageFileSource};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanState {
    Idle,
    Scanning,
    Cancelled,
    Complete,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanMode {
    /// Metadata parsing only (fast — this is what Phase 2/4's `scan`
    /// already did).
    Quick,
    /// Metadata parsing, then a full carving pass over the whole image —
    /// slower, but finds files with no surviving metadata at all.
    Deep,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanProgress {
    pub scanned_bytes: u64,
    pub total_bytes: u64,
    pub current_phase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanSession {
    pub id: String,
    pub image_path: String,
    pub device_label: String,
    pub mode: ScanMode,
    pub state: ScanState,
    pub progress: ScanProgress,
    pub filesystem_detected: Option<String>,
    pub error_message: Option<String>,
    pub results: Vec<RecoverableFile>,
}

impl ScanSession {
    /// Runs a full scan to completion (or until cancelled), blocking the
    /// calling thread. A caller that wants live progress should spawn
    /// this on its own thread and read events off the `EventSender`'s
    /// paired `Receiver` on the calling thread — the CLI's `session-scan`
    /// command does exactly this.
    pub fn run(
        id: String,
        image_path: &str,
        mode: ScanMode,
        cancel: Arc<AtomicBool>,
        events: Option<EventSender>,
    ) -> Result<ScanSession> {
        let emit = |e: ScanEvent| {
            if let Some(tx) = &events {
                let _ = tx.send(e); // a dropped receiver just means nobody's listening — not fatal
            }
        };

        let source = ImageFileSource::open(image_path)?;
        let total_bytes = source.size();

        let mut session = ScanSession {
            id,
            image_path: image_path.to_string(),
            device_label: source.label().to_string(),
            mode,
            state: ScanState::Scanning,
            progress: ScanProgress {
                scanned_bytes: 0,
                total_bytes,
                current_phase: "metadata".to_string(),
            },
            filesystem_detected: None,
            error_message: None,
            results: Vec::new(),
        };

        let mut next_id: u64 = 0;

        // --- Phase 1: metadata ---
        emit(ScanEvent::PhaseStarted { phase: "metadata".to_string() });
        if let Some((parser, fs_name)) = detect_parser(&source) {
            session.filesystem_detected = Some(fs_name.to_string());
            match parser.enumerate_deleted(&source) {
                Ok(deleted) => {
                    for entry in deleted {
                        let file = RecoverableFile::from_deleted_entry(next_id, fs_name, entry);
                        next_id += 1;
                        emit(ScanEvent::FileFound { file: file.clone() });
                        session.results.push(file);
                    }
                }
                Err(e) => {
                    // A metadata parse failure doesn't abort the whole
                    // session — carving (if requested) can still proceed
                    // independently, since it never depended on metadata
                    // parsing succeeding in the first place.
                    emit(ScanEvent::Failed { message: format!("metadata phase: {e}") });
                }
            }
        }
        session.progress.scanned_bytes = total_bytes; // metadata pass isn't chunked; treat as instantaneous
        emit(ScanEvent::PhaseCompleted { phase: "metadata".to_string() });

        // --- Phase 2: carving (Deep mode only) ---
        if mode == ScanMode::Deep && !cancel.load(Ordering::Relaxed) {
            session.progress.current_phase = "carving".to_string();
            session.progress.scanned_bytes = 0;
            emit(ScanEvent::PhaseStarted { phase: "carving".to_string() });

            let carver = SignatureCarver::new();
            let carve_result = carver.scan_with_progress(&source, 0..total_bytes, |scanned, total| {
                session.progress.scanned_bytes = scanned;
                emit(ScanEvent::Progress {
                    phase: "carving".to_string(),
                    scanned_bytes: scanned,
                    total_bytes: total,
                });
                !cancel.load(Ordering::Relaxed) // false => stop early
            });

            match carve_result {
                Ok(carved_files) => {
                    for (i, carved) in carved_files.iter().enumerate() {
                        let file = RecoverableFile::from_carved_file(next_id, i, carved);
                        next_id += 1;
                        emit(ScanEvent::FileFound { file: file.clone() });
                        session.results.push(file);
                    }
                    emit(ScanEvent::PhaseCompleted { phase: "carving".to_string() });
                }
                Err(e) => {
                    session.state = ScanState::Failed;
                    session.error_message = Some(format!("carving phase: {e}"));
                    emit(ScanEvent::Failed { message: session.error_message.clone().unwrap() });
                    return Ok(session);
                }
            }
        }

        if cancel.load(Ordering::Relaxed) {
            session.state = ScanState::Cancelled;
            emit(ScanEvent::Cancelled);
        } else {
            session.state = ScanState::Complete;
            session.progress.current_phase = "done".to_string();
            emit(ScanEvent::Completed { total_found: session.results.len() });
        }

        Ok(session)
    }
}
