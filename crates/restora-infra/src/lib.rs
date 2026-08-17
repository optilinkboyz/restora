//! restora-infra
//!
//! The ONLY crate in the workspace allowed to touch raw devices or open
//! file handles directly. Everything above this layer speaks in terms of
//! `ByteSource` and never knows whether it's reading a live disk or a
//! `.img` fixture file.

pub mod byte_source;
pub mod image_file_source;
pub mod writable_byte_source;
pub mod writable_image_file;

pub use byte_source::{ByteSource, ByteSourceError};
pub use image_file_source::ImageFileSource;
pub use writable_byte_source::WritableByteSource;
pub use writable_image_file::WritableImageFileSource;

// Coming in later phases from this same crate:
//   - raw_disk_source.rs   (RawDiskSource: \\.\PhysicalDriveN / /dev/sdX)
//   - sector_cache.rs      (LRU-cached wrapper around any ByteSource)
//   - privilege_broker.rs  (elevation requests, capability checks)
//   - trim_issuer.rs       (real block-device TRIM — see Phase 6's
//     wipe_job.rs for why this is scoped out for now)
