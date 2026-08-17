use restora_infra::ByteSourceError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("underlying byte source error: {0}")]
    ByteSource(#[from] ByteSourceError),

    #[error("not a valid FAT32 volume: {0}")]
    NotFat32(String),

    #[error("directory entry parse error: {0}")]
    DirEntry(String),
}

pub type Result<T> = std::result::Result<T, DomainError>;
