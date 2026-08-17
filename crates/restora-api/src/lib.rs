//! restora-api
//!
//! The typed boundary a UI calls into — Phase 7's Tauri shell wraps each
//! of these in a one-line `#[tauri::command]` function, but everything
//! genuinely meaningful (error mapping, pattern-name parsing, DTO shape)
//! lives here, where it can be unit tested without needing a real Tauri
//! runtime, a webview, or a display. That split is deliberate: this crate
//! only depends on restora-application/-domain/-infra, never on `tauri`
//! itself, so it stays buildable and testable in any plain Rust
//! environment — including this project's CI, and this very sandbox,
//! neither of which can build the actual desktop shell (see Phase 7's
//! notes on why).
//!
//! Every function here returns `Result<T, String>` rather than a typed
//! error — JS/TS on the other side of a Tauri IPC call doesn't know
//! Rust's error types, so collapsing to a message string at this
//! boundary is the right place to do it, once, rather than in every
//! individual `#[tauri::command]`.

use restora_application::{
    recover_files, wipe_free_space, RecoveryResult, ScanMode, ScanSession, SessionStore, SessionSummary,
    WipeResult,
};
use restora_domain::{RecoverableFile, WipePattern};

fn map_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// One-shot scan with no live progress — suitable for a UI that just
/// wants a completed result (e.g. a "Quick Scan" button that shows a
/// spinner, not a progress bar). For live progress during a Deep scan,
/// the Tauri shell calls `restora_application::ScanSession::run`
/// directly with an event channel, the same pattern the CLI's
/// `session-scan` command already uses — that needs an `AppHandle` to
/// emit events through, which only exists inside the Tauri runtime, so
/// it isn't something this crate can usefully wrap.
pub fn run_scan(image_path: &str, mode: ScanMode) -> Result<ScanSession, String> {
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let id = format!(
        "session-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    ScanSession::run(id, image_path, mode, cancel, None).map_err(map_err)
}

pub fn recover_selected(image_path: &str, files: &[RecoverableFile], destination_dir: &str) -> Result<Vec<RecoveryResult>, String> {
    recover_files(image_path, files, destination_dir).map_err(map_err)
}

pub fn list_sessions(db_path: &str) -> Result<Vec<SessionSummary>, String> {
    let store = SessionStore::open(db_path).map_err(map_err)?;
    store.list_summaries().map_err(map_err)
}

pub fn load_session(db_path: &str, session_id: &str) -> Result<ScanSession, String> {
    let store = SessionStore::open(db_path).map_err(map_err)?;
    store
        .load(session_id)
        .map_err(map_err)?
        .ok_or_else(|| format!("no session found with id '{session_id}'"))
}

pub fn save_session(db_path: &str, session: &ScanSession) -> Result<(), String> {
    let store = SessionStore::open(db_path).map_err(map_err)?;
    store.save(session).map_err(map_err)
}

/// Parses a UI-facing pattern name ("zero" / "random" / "dod3") into the
/// domain's `WipePattern` — centralized here so both the CLI and the
/// Tauri shell parse the same three strings the same way, rather than
/// each maintaining their own copy of this mapping.
pub fn parse_wipe_pattern(name: &str) -> Result<WipePattern, String> {
    match name {
        "zero" => Ok(WipePattern::ZERO),
        "random" => Ok(WipePattern::RANDOM),
        "dod3" => Ok(WipePattern::DOD_3PASS),
        other => Err(format!("unknown wipe pattern '{other}' — expected one of: zero, random, dod3")),
    }
}

pub fn wipe_free_space_cmd(image_path: &str, pattern_name: &str, assume_ssd: bool, verify: bool) -> Result<WipeResult, String> {
    let pattern = parse_wipe_pattern(pattern_name)?;
    wipe_free_space(image_path, &pattern, assume_ssd, verify).map_err(map_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> String {
        format!("{}/../../tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn run_scan_returns_a_completed_session_for_ntfs_fixture() {
        let path = fixture_path("ntfs_basic.img");
        let session = run_scan(&path, ScanMode::Quick).expect("scan should succeed");

        assert_eq!(session.filesystem_detected, Some("NTFS".to_string()));
        assert_eq!(session.results.len(), 1);
        assert_eq!(session.results[0].name, "canary.txt");
    }

    #[test]
    fn run_scan_reports_a_clear_error_for_a_missing_file() {
        let err = run_scan("/nonexistent/path/does/not/exist.img", ScanMode::Quick).unwrap_err();
        assert!(!err.is_empty(), "error message should not be empty");
    }

    #[test]
    fn parse_wipe_pattern_rejects_unknown_names() {
        assert!(parse_wipe_pattern("zero").is_ok());
        assert!(parse_wipe_pattern("random").is_ok());
        assert!(parse_wipe_pattern("dod3").is_ok());
        let err = parse_wipe_pattern("nonsense").unwrap_err();
        assert!(err.contains("nonsense"));
    }

    /// The full round trip a UI would actually drive: scan, save, list,
    /// load, recover — every restora-api function exercised together.
    #[test]
    fn full_session_round_trip_through_the_api_layer() {
        let image_path = fixture_path("ntfs_basic.img");
        let db = tempfile::NamedTempFile::new().unwrap();
        let db_path = db.path().to_str().unwrap();

        let session = run_scan(&image_path, ScanMode::Quick).unwrap();
        save_session(db_path, &session).unwrap();

        let summaries = list_sessions(db_path).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, session.id);

        let reloaded = load_session(db_path, &session.id).unwrap();
        assert_eq!(reloaded.results.len(), 1);

        let tmp_dir = tempfile::tempdir().unwrap();
        let results = recover_selected(&image_path, &reloaded.results, tmp_dir.path().to_str().unwrap()).unwrap();
        assert!(results[0].success);
    }
}
