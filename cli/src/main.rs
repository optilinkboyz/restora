//! restora-cli
//!
//! Phase 2 milestone: real subcommands backed by the FAT32 parser.
//!
//!   restora-cli scan <image>                    — list deleted files found
//!   restora-cli recover <image> <name> <outdir>  — recover one file's bytes
//!
//! From Phase 7 onward the Tauri UI replaces this as the primary interface,
//! but this stays useful as a scriptable/debuggable entry point into the
//! same restora-domain logic.

use anyhow::{bail, Context, Result};
use restora_domain::carving::{Carver, SignatureCarver};
use restora_domain::fat32::Fat32Parser;
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
            eprintln!("  restora-cli scan <image>                       (FAT32 metadata scan)");
            eprintln!("  restora-cli recover <image> <name> <outdir>    (FAT32 metadata recovery)");
            eprintln!("  restora-cli carve <image> <outdir>             (signature-based carving)");
            std::process::exit(1);
        }
    }
}

fn open_and_parse(image_path: &str) -> Result<(ImageFileSource, Fat32Parser)> {
    let source = ImageFileSource::open(image_path)
        .with_context(|| format!("failed to open image: {image_path}"))?;
    let parser = Fat32Parser::new(&source)
        .context("failed to parse boot sector — is this a FAT32 image?")?;
    Ok((source, parser))
}

fn cmd_scan(args: &[String]) -> Result<()> {
    let image_path = args.first().context("usage: scan <image>")?;
    let (source, parser) = open_and_parse(image_path)?;

    let deleted = parser
        .enumerate_deleted(&source)
        .context("enumerate_deleted failed")?;

    if deleted.is_empty() {
        println!("No deleted files found in root directory.");
        return Ok(());
    }

    println!("{:<20} {:>10}  {:<12}  {}", "NAME", "SIZE", "1ST CLUSTER", "METADATA");
    for entry in &deleted {
        println!(
            "{:<20} {:>10}  {:<12}  {}",
            entry.name,
            entry.file_size,
            entry.first_cluster,
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

    let (source, parser) = open_and_parse(image_path)?;
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

    let out_path = PathBuf::from(outdir).join(&entry.name);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, &recovered)
        .with_context(|| format!("failed to write recovered file to {}", out_path.display()))?;

    println!(
        "Recovered {} bytes -> {}",
        recovered.len(),
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
