//! restora-desktop-lib
//!
//! Every `#[tauri::command]` here is intentionally thin — one or two
//! lines calling into `restora-api` (Quick scan, recovery, sessions,
//! wipe) or, for live progress, directly into
//! `restora-application::ScanSession::run` with an event channel (Deep
//! scan — needs the `AppHandle` to emit through, which only exists here,
//! not in restora-api). Any logic more complicated than "call the
//! already-tested function and translate the result" belongs in
//! restora-api instead, where it can actually be unit tested.

use restora_application::{ScanEvent, ScanMode, ScanSession, SessionSummary};
use restora_domain::RecoverableFile;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

#[tauri::command]
fn scan_quick(image_path: String) -> Result<ScanSession, String> {
    restora_api::run_scan(&image_path, ScanMode::Quick)
}

/// Deep scan (metadata + carving) with live progress, streamed to the
/// frontend as `scan-event` events — listen for these with
/// `window.__TAURI__.event.listen('scan-event', callback)` on the JS
/// side. Runs the scan on its own thread so this command (itself already
/// off the UI thread, per Tauri's handling of sync commands) can drain
/// the event channel and emit each one as it arrives, rather than
/// blocking until the whole scan finishes before the UI hears anything.
#[tauri::command]
fn scan_deep(app: AppHandle, image_path: String) -> Result<ScanSession, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();

    let id = format!(
        "session-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    let image_path_owned = image_path.clone();
    let cancel_for_thread = cancel.clone();
    let handle = std::thread::spawn(move || {
        ScanSession::run(id, &image_path_owned, ScanMode::Deep, cancel_for_thread, Some(tx))
    });

    for event in rx {
        let _ = app.emit("scan-event", &event);
    }

    handle.join().map_err(|_| "scan thread panicked".to_string())?.map_err(|e| e.to_string())
}

#[tauri::command]
fn recover_selected(image_path: String, files: Vec<RecoverableFile>, destination_dir: String) -> Result<Vec<restora_application::RecoveryResult>, String> {
    restora_api::recover_selected(&image_path, &files, &destination_dir)
}

#[tauri::command]
fn list_sessions(db_path: String) -> Result<Vec<SessionSummary>, String> {
    restora_api::list_sessions(&db_path)
}

#[tauri::command]
fn load_session(db_path: String, session_id: String) -> Result<ScanSession, String> {
    restora_api::load_session(&db_path, &session_id)
}

#[tauri::command]
fn save_session(db_path: String, session: ScanSession) -> Result<(), String> {
    restora_api::save_session(&db_path, &session)
}

#[tauri::command]
fn wipe_free_space(image_path: String, pattern_name: String, assume_ssd: bool, verify: bool) -> Result<restora_application::WipeResult, String> {
    restora_api::wipe_free_space_cmd(&image_path, &pattern_name, assume_ssd, verify)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            scan_quick,
            scan_deep,
            recover_selected,
            list_sessions,
            load_session,
            save_session,
            wipe_free_space,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Restora desktop app");
}
