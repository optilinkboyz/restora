//! restora-cli
//!
//! Phase 0/1 milestone: prove the workspace wires together by opening an
//! image file through ByteSource and dumping its boot sector in hex.
//! From Phase 2 onward this gains real subcommands (`scan`, `recover`, `wipe`)
//! before the Tauri UI (Phase 7) replaces it as the primary interface.

use anyhow::{Context, Result};
use restora_infra::{ByteSource, ImageFileSource};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let path = std::env::args()
        .nth(1)
        .context("usage: restora-cli <path-to-disk-image>")?;

    let source = ImageFileSource::open(&path)
        .with_context(|| format!("failed to open image: {path}"))?;

    println!("Opened: {}", source.label());
    println!("Size:   {} bytes", source.size());

    let boot_sector = source
        .read_vec(0, 512.min(source.size() as usize))
        .context("failed to read boot sector")?;

    println!("\nFirst 512 bytes (hex):");
    for (i, chunk) in boot_sector.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        println!("{:04x}  {:<47}  {}", i * 16, hex.join(" "), ascii);
    }

    Ok(())
}
