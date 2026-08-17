//! restora-application
//!
//! Orchestration: ScanSession, RecoveryJob, WipeJob, EventBus, SessionStore.
//! Phase 5 (this): ScanSession, EventBus, SessionStore, RecoveryJob.
//! Coming later: WipeJob (Phase 6).

pub mod error;
pub mod event_bus;
pub mod recovery_job;
pub mod scan_session;
pub mod session_store;

pub use error::{ApplicationError, Result};
pub use event_bus::{EventSender, ScanEvent};
pub use recovery_job::{recover_files, RecoveryResult};
pub use scan_session::{ScanMode, ScanProgress, ScanSession, ScanState};
pub use session_store::{SessionStore, SessionSummary};
