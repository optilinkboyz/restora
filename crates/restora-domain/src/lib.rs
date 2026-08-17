//! restora-domain
//!
//! Pure parsing/carving logic: FilesystemParser and Carver traits and their
//! implementations. No direct I/O — everything reads through a
//! `&dyn ByteSource` from restora-infra. This is what makes the hardest
//! logic in the project unit-testable without a real disk anywhere in sight.
//!
//! Populated starting Phase 2 (FAT32Parser), Phase 3 (SignatureCarver),
//! Phase 4 (NtfsParser).
