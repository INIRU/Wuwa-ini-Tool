use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("invalid_path: {path}: {reason}")]
    InvalidPath { path: PathBuf, reason: &'static str },
    #[error("io_{operation}: {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("metadata_json: {0}")]
    MetadataJson(#[from] serde_json::Error),
    #[error("unsupported_metadata_version: {0}")]
    UnsupportedMetadataVersion(u32),
    #[error("invalid_metadata: {0}")]
    InvalidMetadata(&'static str),
    #[error("invalid_reason: {0:?}")]
    InvalidReason(crate::backup_store::ApplyReason),
    #[error("source_conflict: expected {expected}, found {actual}")]
    SourceConflict { expected: String, actual: String },
    #[error("backup_not_found: {0}")]
    BackupNotFound(String),
    #[error("hash_mismatch: {path}: expected {expected}, found {actual}")]
    HashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("readback_mismatch: {path}: expected {expected}, found {actual}")]
    ReadbackMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("unrecoverable_transaction: original={original}; rollback={rollback}")]
    Unrecoverable {
        original: Box<BackupError>,
        rollback: Box<BackupError>,
    },
    #[error(
        "replace_reconciliation_required: code={code}, destination={destination}, replacement={replacement}, capture={capture}"
    )]
    ReconciliationRequired {
        code: u32,
        destination: PathBuf,
        replacement: PathBuf,
        capture: PathBuf,
    },
    #[error("durability_unavailable: {path}: {source}")]
    DurabilityUnavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl BackupError {
    pub(crate) fn io(
        operation: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}
