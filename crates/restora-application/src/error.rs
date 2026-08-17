use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("domain error: {0}")]
    Domain(#[from] restora_domain::DomainError),

    #[error("byte source error: {0}")]
    ByteSource(#[from] restora_infra::ByteSourceError),

    #[error("session store error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("no session found with id '{0}'")]
    SessionNotFound(String),

    #[error("no recognized filesystem found, and scan mode did not include carving")]
    NoFilesystemDetected,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ApplicationError>;
