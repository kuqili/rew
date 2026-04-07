//! Error types for rew.

use thiserror::Error;

/// Unified error type covering all rew error categories.
#[derive(Error, Debug)]
pub enum RewError {
    /// I/O errors (file system operations, process execution)
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Database errors (SQLite operations)
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Snapshot errors (creation, restoration, deletion)
    #[error("Snapshot error: {0}")]
    Snapshot(String),

    /// Configuration errors (parsing, validation)
    #[error("Config error: {0}")]
    Config(String),

    /// Serialization/deserialization errors
    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<toml::de::Error> for RewError {
    fn from(e: toml::de::Error) -> Self {
        RewError::Config(e.to_string())
    }
}

impl From<toml::ser::Error> for RewError {
    fn from(e: toml::ser::Error) -> Self {
        RewError::Serialization(e.to_string())
    }
}

impl From<serde_json::Error> for RewError {
    fn from(e: serde_json::Error) -> Self {
        RewError::Serialization(e.to_string())
    }
}

pub type RewResult<T> = Result<T, RewError>;
