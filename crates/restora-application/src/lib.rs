//! restora-application
//!
//! Orchestration: ScanSession, RecoveryJob, WipeJob, EventBus, SessionStore.
//! Phase 5: ScanSession, EventBus, SessionStore, RecoveryJob.
//! Phase 6 (this): WipeJob — free-space wiping with WipePattern + self-verification.

pub mod error;
pub mod event_bus;
pub mod recovery_job;
pub mod scan_session;
pub mod session_store;
pub mod wipe_job;

pub use error::{ApplicationError, Result};
pub use event_bus::{EventSender, ScanEvent};
pub use recovery_job::{recover_files, RecoveryResult};
pub use scan_session::{ScanMode, ScanProgress, ScanSession, ScanState};
pub use session_store::{SessionStore, SessionSummary};
pub use wipe_job::{wipe_free_space, VerificationResult, WipeResult};
