//! restora-cli
//!
//! Phase 4 milestone: filesystem auto-detection. `scan`/`recover` now work
//! on either FAT32 or NTFS images — this is the FilesystemParser trait
//! design paying off directly: the CLI code doesn't need to know or care
//! which parser it ends up using.
//!
//!   restora-cli scan <image>                       — list deleted files
//!   restora-cli recover <image> <name> <outdir>     — recover one file
//!   restora-cli carve <image> <outdir>              — signature-based carving
//!
//! From Phase 7 onward the Tauri UI replaces this as the primary interface,
//! but this stays useful as a scriptable/debuggable entry point into the
//! same restora-domain logic.

use anyhow::{bail, Context, Result};
use restora_application::{recover_files, ScanEvent, ScanMode, ScanSession, SessionStore};
use restora_domain::carving::{Carver, SignatureCarver};
use restora_domain::FilesystemParser;
use restora_infra::{ByteSource, ImageFileSource};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("scan") => cmd_scan(&args[2..]),
        Some("recover") => cmd_recover(&args[2..]),
        Some("carve") => cmd_carve(&args[2..]),
        Some("session-scan") => cmd_session_scan(&args[2..]),
        Some("session-list") => cmd_session_list(&args[2..]),
        Some("session-recover") => cmd_session_recover(&args[2..]),
        _ => {
            eprintln!("usage:");
            eprintln!("  restora-cli scan <image>                             (one-shot, auto-detects FAT32/NTFS)");
            eprintln!("  restora-cli recover <image> <name> <outdir>          (one-shot recovery)");
            eprintln!("  restora-cli carve <image> <outdir>                   (one-shot carving)");
            eprintln!("  restora-cli session-scan <image> <db> [quick|deep]   (persisted, resumable-session scan)");
            eprintln!("  restora-cli session-list <db>                       (list persisted sessions)");
            eprintln!("  restora-cli session-recover <db> <id> <name> <outdir>  (recover from a persisted session)");
            std::process::exit(1);
        }
    }
}

/// Tries each known metadata-based parser in turn. This now delegates to
/// `restora_domain::detect_parser` — the shared dispatch logic added in
/// Phase 5 so the CLI and the application layer's ScanSession don't each
/// maintain their own copy.
fn detect_and_open(image_path: &str) -> Result<(ImageFileSource, Box<dyn FilesystemParser>, &'static str)> {
    let source = ImageFileSource::open(image_path)
        .with_context(|| format!("failed to open image: {image_path}"))?;
    let (parser, fs_name) = restora_domain::detect_parser(&source).with_context(|| {
        format!(
            "no recognized filesystem found in {image_path} (tried FAT32, NTFS) — \
             if this image genuinely has no filesystem, try `carve` instead"
        )
    })?;
    Ok((source, parser, fs_name))
}

fn cmd_scan(args: &[String]) -> Result<()> {
    let image_path = args.first().context("usage: scan <image>")?;
    let (source, parser, fs_name) = detect_and_open(image_path)?;

    let deleted = parser
        .enumerate_deleted(&source)
        .context("enumerate_deleted failed")?;

    println!("Detected filesystem: {fs_name}\n");

    if deleted.is_empty() {
        println!("No deleted files found.");
        return Ok(());
    }

    println!("{:<24} {:>10}  {:>6}  {}", "NAME", "SIZE", "CONF", "METADATA");
    for entry in &deleted {
        println!(
            "{:<24} {:>10}  {:>5}%  {}",
            entry.name,
            entry.file_size,
            entry.confidence,
            if entry.metadata_intact { "intact" } else { "damaged" }
        );
    }
    println!("\n{} deleted file(s) found.", deleted.len());
    Ok(())
}

fn cmd_recover(args: &[String]) -> Result<()> {
    let image_path = args.first().context("usage: recover <image> <name> <outdir>")?;
    let name = args.get(1).context("usage: recover <image> <name> <outdir>")?;
    let outdir = args.get(2).context("usage: recover <image> <name> <outdir>")?;

    let (source, parser, fs_name) = detect_and_open(image_path)?;
    println!("Detected filesystem: {fs_name}");

    let deleted = parser
        .enumerate_deleted(&source)
        .context("enumerate_deleted failed")?;

    let entry = deleted
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(name))
        .with_context(|| format!("no deleted entry named '{name}' found — run `scan` first"))?;

    let recovered = parser
        .recover_bytes(&source, entry)
        .context("recover_bytes failed")?;

    // NTFS names can contain path-hostile characters we don't sanitize
    // yet, and FAT names sometimes carry the unrecoverable-first-char '_'
    // placeholder — file_name() below just uses the reconstructed name
    // as-is, which is fine for this CLI-scale tool.
    let out_path = PathBuf::from(outdir).join(&entry.name);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, &recovered)
        .with_context(|| format!("failed to write recovered file to {}", out_path.display()))?;

    println!(
        "Recovered {} bytes (confidence {}%) -> {}",
        recovered.len(),
        entry.confidence,
        out_path.display()
    );

    if recovered.is_empty() {
        bail!("recovered 0 bytes — this file's data may already be overwritten");
    }
    Ok(())
}

fn cmd_carve(args: &[String]) -> Result<()> {
    let image_path = args.first().context("usage: carve <image> <outdir>")?;
    let outdir = args.get(1).context("usage: carve <image> <outdir>")?;

    let source = ImageFileSource::open(image_path)
        .with_context(|| format!("failed to open image: {image_path}"))?;

    let carver = SignatureCarver::new();
    let found = carver
        .scan(&source, 0..source.size())
        .context("carve scan failed")?;

    if found.is_empty() {
        println!("No known file signatures found.");
        return Ok(());
    }

    std::fs::create_dir_all(outdir)?;

    println!(
        "{:<6} {:<6} {:>14} {:>10} {:>6}  {}",
        "INDEX", "TYPE", "OFFSET", "SIZE", "CONF", "OUTPUT"
    );
    for (i, file) in found.iter().enumerate() {
        let bytes = source.read_vec(file.start_offset, file.size() as usize)?;
        let out_path = PathBuf::from(outdir).join(format!("carved_{:04}.{}", i, file.extension));
        std::fs::write(&out_path, &bytes)?;
        println!(
            "{:<6} {:<6} {:>14} {:>10} {:>5}%  {}",
            i,
            file.format_name,
            file.start_offset,
            file.size(),
            file.confidence,
            out_path.display()
        );
    }
    println!("\n{} file(s) carved.", found.len());
    Ok(())
}

fn cmd_session_scan(args: &[String]) -> Result<()> {
    let image_path = args.first().context("usage: session-scan <image> <db> [quick|deep]")?;
    let db_path = args.get(1).context("usage: session-scan <image> <db> [quick|deep]")?;
    let mode = match args.get(2).map(String::as_str) {
        Some("deep") => ScanMode::Deep,
        _ => ScanMode::Quick, // default — matches "quick" being the safer, faster default
    };

    let session_id = format!(
        "session-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );

    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();

    // Run the scan on its own thread so this thread is free to print
    // events as they arrive — the same pattern a real UI would use to
    // stay responsive during a long scan.
    let image_path_owned = image_path.clone();
    let cancel_for_thread = cancel.clone();
    let session_id_for_thread = session_id.clone();
    let handle = std::thread::spawn(move || {
        ScanSession::run(session_id_for_thread, &image_path_owned, mode, cancel_for_thread, Some(tx))
    });

    for event in rx {
        match event {
            ScanEvent::PhaseStarted { phase } => println!("[phase started] {phase}"),
            ScanEvent::Progress { phase, scanned_bytes, total_bytes } => {
                let pct = if total_bytes > 0 { scanned_bytes * 100 / total_bytes } else { 0 };
                println!("[progress] {phase}: {scanned_bytes}/{total_bytes} bytes ({pct}%)");
            }
            ScanEvent::FileFound { file } => {
                println!("[found] {} ({} bytes, {}% confidence)", file.name, file.size, file.confidence);
            }
            ScanEvent::PhaseCompleted { phase } => println!("[phase completed] {phase}"),
            ScanEvent::Cancelled => println!("[cancelled]"),
            ScanEvent::Completed { total_found } => println!("[completed] {total_found} file(s) found"),
            ScanEvent::Failed { message } => println!("[failed] {message}"),
        }
    }

    let session = handle.join().expect("scan thread panicked")?;

    let store = SessionStore::open(db_path)?;
    store.save(&session)?;

    println!(
        "\nSession '{}' saved to {db_path} — state: {:?}, {} file(s).",
        session.id,
        session.state,
        session.results.len()
    );
    Ok(())
}

fn cmd_session_list(args: &[String]) -> Result<()> {
    let db_path = args.first().context("usage: session-list <db>")?;
    let store = SessionStore::open(db_path)?;
    let summaries = store.list_summaries()?;

    if summaries.is_empty() {
        println!("No sessions found in {db_path}.");
        return Ok(());
    }

    println!("{:<24} {:<24} {:<12} {:<10} {}", "ID", "UPDATED", "DEVICE", "STATE", "FILES");
    for s in summaries {
        println!(
            "{:<24} {:<24} {:<12} {:<10?} {}",
            s.id, s.updated_at, s.device_label, s.state, s.files_found
        );
    }
    Ok(())
}

fn cmd_session_recover(args: &[String]) -> Result<()> {
    let db_path = args.first().context("usage: session-recover <db> <session_id> <name> <outdir>")?;
    let session_id = args.get(1).context("usage: session-recover <db> <session_id> <name> <outdir>")?;
    let name = args.get(2).context("usage: session-recover <db> <session_id> <name> <outdir>")?;
    let outdir = args.get(3).context("usage: session-recover <db> <session_id> <name> <outdir>")?;

    let store = SessionStore::open(db_path)?;
    let session = store
        .load(session_id)?
        .with_context(|| format!("no session found with id '{session_id}' in {db_path}"))?;

    let file = session
        .results
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(name))
        .with_context(|| format!("no file named '{name}' in session '{session_id}'"))?;

    // Note: this reopens session.image_path fresh — proving recovery
    // works from nothing but what was persisted, in a process that never
    // ran the original scan.
    let results = recover_files(&session.image_path, std::slice::from_ref(file), outdir)?;
    let result = &results[0];

    if result.success {
        println!(
            "Recovered {} bytes -> {}",
            result.bytes_written,
            result.output_path.as_ref().unwrap().display()
        );
    } else {
        bail!("recovery failed: {}", result.error.clone().unwrap_or_default());
    }
    Ok(())
}
