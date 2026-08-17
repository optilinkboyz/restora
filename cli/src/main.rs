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
use restora_domain::carving::{Carver, SignatureCarver};
use restora_domain::fat32::Fat32Parser;
use restora_domain::ntfs::NtfsParser;
use restora_domain::FilesystemParser;
use restora_infra::{ByteSource, ImageFileSource};
use std::path::PathBuf;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("scan") => cmd_scan(&args[2..]),
        Some("recover") => cmd_recover(&args[2..]),
        Some("carve") => cmd_carve(&args[2..]),
        _ => {
            eprintln!("usage:");
            eprintln!("  restora-cli scan <image>                       (auto-detects FAT32/NTFS)");
            eprintln!("  restora-cli recover <image> <name> <outdir>    (auto-detects FAT32/NTFS)");
            eprintln!("  restora-cli carve <image> <outdir>             (signature-based carving)");
            std::process::exit(1);
        }
    }
}

/// Tries each known metadata-based parser in turn. This is the concrete
/// payoff of the FilesystemParser trait: every parser implements the same
/// three methods, so once we know which one applies, the rest of the CLI
/// code is completely filesystem-agnostic.
fn detect_and_open(image_path: &str) -> Result<(ImageFileSource, Box<dyn FilesystemParser>, &'static str)> {
    let source = ImageFileSource::open(image_path)
        .with_context(|| format!("failed to open image: {image_path}"))?;

    if Fat32Parser::detect(&source) {
        let parser = Fat32Parser::new(&source).context("detected FAT32 but failed to parse it")?;
        return Ok((source, Box::new(parser), "FAT32"));
    }
    if NtfsParser::detect(&source) {
        let parser = NtfsParser::new(&source).context("detected NTFS but failed to parse it")?;
        return Ok((source, Box::new(parser), "NTFS"));
    }

    bail!(
        "no recognized filesystem found in {image_path} (tried FAT32, NTFS) — \
         if this image genuinely has no filesystem, try `carve` instead"
    )
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
