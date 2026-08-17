//! Recovers a set of previously-found `RecoverableFile`s to a destination
//! directory — usable immediately after a scan, or hours later against a
//! session reloaded from `SessionStore`, since everything needed is
//! carried in each file's `RecoveryLocator`.
//!
//! **Why `enumerate_deleted` is called once up front for the metadata
//! path.** NTFS's parser remembers which MFT record each name came from
//! *during* `enumerate_deleted` (see Phase 4's `ntfs_parser.rs`), and
//! `recover_bytes` uses that to re-read the file's complete, possibly
//! fragmented run list — better than falling back to a single-contiguous
//! guess. A freshly-opened parser (exactly the situation after a session
//! reload) doesn't have that memory yet. Calling `enumerate_deleted` once
//! before recovering anything "warms" it back up, so a reloaded session
//! gets the same quality of recovery as the original scan did — this one
//! extra call is what makes that guarantee hold.

use crate::error::Result;
use restora_domain::{detect_parser, RecoverableFile, RecoveryLocator};
use restora_infra::{ByteSource, ImageFileSource};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct RecoveryResult {
    pub name: String,
    pub success: bool,
    pub bytes_written: u64,
    pub output_path: Option<PathBuf>,
    pub error: Option<String>,
}

pub fn recover_files(image_path: &str, files: &[RecoverableFile], destination_dir: &str) -> Result<Vec<RecoveryResult>> {
    let source = ImageFileSource::open(image_path)?;
    std::fs::create_dir_all(destination_dir)?;

    // Warm up the metadata parser (see module docs) — ignored if no
    // known filesystem is present, since carved-only recoveries don't
    // need this at all.
    let parser = detect_parser(&source);
    if let Some((parser, _fs_name)) = &parser {
        let _ = parser.enumerate_deleted(&source); // best-effort warm-up; failures handled per-file below
    }

    let mut results = Vec::new();

    for file in files {
        let result = recover_one(&source, parser.as_ref().map(|(p, _)| p.as_ref()), file, destination_dir);
        results.push(result);
    }

    Ok(results)
}

fn recover_one(
    source: &ImageFileSource,
    parser: Option<&dyn restora_domain::FilesystemParser>,
    file: &RecoverableFile,
    destination_dir: &str,
) -> RecoveryResult {
    let attempt = || -> Result<(Vec<u8>, PathBuf)> {
        let bytes = match &file.locator {
            RecoveryLocator::Metadata(entry) => {
                let parser = parser.ok_or(crate::error::ApplicationError::NoFilesystemDetected)?;
                parser.recover_bytes(source, entry)?
            }
            RecoveryLocator::CarvedRange { start_offset, end_offset } => {
                let len = (end_offset - start_offset) as usize;
                source.read_vec(*start_offset, len)?
            }
        };

        let out_path = Path::new(destination_dir).join(&file.name);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out_path, &bytes)?;
        Ok((bytes, out_path))
    };

    match attempt() {
        Ok((bytes, out_path)) => RecoveryResult {
            name: file.name.clone(),
            success: !bytes.is_empty(),
            bytes_written: bytes.len() as u64,
            output_path: Some(out_path),
            error: if bytes.is_empty() {
                Some("recovered 0 bytes — data may already be overwritten".to_string())
            } else {
                None
            },
        },
        Err(e) => RecoveryResult {
            name: file.name.clone(),
            success: false,
            bytes_written: 0,
            output_path: None,
            error: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan_session::{ScanMode, ScanSession};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn fixture_path(name: &str) -> String {
        format!(
            "{}/../../tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    /// The Phase 5 milestone that matters most: run a scan, throw away
    /// the in-memory parser entirely, and recover using ONLY what's in
    /// the resulting `RecoverableFile` — simulating exactly what happens
    /// after a session is reloaded from `SessionStore` in a new process.
    #[test]
    fn recovers_ntfs_file_using_only_the_persisted_locator() {
        let image_path = fixture_path("ntfs_basic.img");
        let cancel = Arc::new(AtomicBool::new(false));

        let session = ScanSession::run("test-session".to_string(), &image_path, ScanMode::Quick, cancel, None)
            .expect("scan should succeed");

        assert_eq!(session.results.len(), 1);
        let file = &session.results[0];
        assert_eq!(file.name, "canary.txt");
        assert_eq!(file.confidence, 90);

        // Round-trip through JSON, exactly as SessionStore would, to
        // prove there's no reliance on anything not actually serialized.
        let json = serde_json::to_string(file).unwrap();
        let reloaded: RecoverableFile = serde_json::from_str(&json).unwrap();

        let tmp_dir = tempfile::tempdir().unwrap();
        let results = recover_files(&image_path, &[reloaded], tmp_dir.path().to_str().unwrap()).unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success, "recovery failed: {:?}", results[0].error);

        let recovered_bytes = std::fs::read(results[0].output_path.as_ref().unwrap()).unwrap();
        let expected =
            b"This is a canary file for NTFS recovery testing. Bitmap-verified high-confidence recovery.\n";
        assert_eq!(&recovered_bytes, expected);
    }

    #[test]
    fn recovers_carved_file_with_no_filesystem_at_all() {
        let image_path = fixture_path("carve_test.img");
        let cancel = Arc::new(AtomicBool::new(false));

        let session = ScanSession::run("carve-session".to_string(), &image_path, ScanMode::Deep, cancel, None)
            .expect("scan should succeed");

        // No filesystem on this image, so only the carving phase
        // contributes results.
        assert!(session.filesystem_detected.is_none());
        assert_eq!(session.results.len(), 2); // JPEG + PDF, per Phase 3's fixture

        let pdf_file = session.results.iter().find(|f| f.name.ends_with(".pdf")).unwrap();

        let tmp_dir = tempfile::tempdir().unwrap();
        let results = recover_files(&image_path, &[pdf_file.clone()], tmp_dir.path().to_str().unwrap()).unwrap();

        assert!(results[0].success);
        let recovered = std::fs::read(results[0].output_path.as_ref().unwrap()).unwrap();
        assert!(recovered.starts_with(b"%PDF-1.4"));
        assert!(recovered.ends_with(b"%%EOF"));
    }
}
