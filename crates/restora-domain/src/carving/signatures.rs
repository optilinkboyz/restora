//! Known file-format signatures used for carving: byte patterns that mark
//! the start (and often the end) of a file, independent of any filesystem
//! metadata whatsoever. This is what carving depends on instead of a
//! directory entry — which is exactly why it still works on a formatted
//! drive, in a deleted subdirectory, or on a filesystem we haven't even
//! written a parser for.

#[derive(Debug, Clone, Copy)]
pub struct FileSignature {
    pub name: &'static str,
    pub extension: &'static str,
    pub header: &'static [u8],
    /// If present, carving searches for this exact byte sequence after
    /// the header to find the file's true end — this is what gives a
    /// carved result high confidence. If `None`, or if no footer is found
    /// within `max_size`, we fall back to a `max_size` cutoff instead,
    /// which is a much lower-confidence guess (likely truncated, or not
    /// really a complete file of this type at all).
    pub footer: Option<&'static [u8]>,
    /// Safety cap on how far past the header we'll search for a footer.
    pub max_size: usize,
}

pub const SIGNATURES: &[FileSignature] = &[
    FileSignature {
        name: "JPEG",
        extension: "jpg",
        header: &[0xFF, 0xD8, 0xFF],
        footer: Some(&[0xFF, 0xD9]),
        max_size: 20 * 1024 * 1024,
    },
    FileSignature {
        name: "PNG",
        extension: "png",
        header: &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        // IEND chunk type + its fixed CRC32 — this exact 8-byte sequence
        // always closes a well-formed PNG.
        footer: Some(&[0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82]),
        max_size: 20 * 1024 * 1024,
    },
    FileSignature {
        name: "PDF",
        extension: "pdf",
        header: b"%PDF-",
        footer: Some(b"%%EOF"),
        max_size: 50 * 1024 * 1024,
    },
    FileSignature {
        name: "ZIP",
        extension: "zip",
        header: &[0x50, 0x4B, 0x03, 0x04],
        // End Of Central Directory record — the real end-of-archive marker.
        footer: Some(&[0x50, 0x4B, 0x05, 0x06]),
        max_size: 200 * 1024 * 1024,
    },
];

/// Longest header among all known signatures. This becomes the required
/// overlap between scan chunks — see `carver.rs` docs for why that's
/// exactly the right amount, not just a safe-ish guess.
pub fn max_header_len() -> usize {
    SIGNATURES.iter().map(|s| s.header.len()).max().unwrap_or(0)
}
