use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CacheCleanupError {
    #[error("empty_cache_selection")]
    EmptySelection,
    #[error("cache_cleanup_confirmation_required")]
    ConfirmationRequired,
    #[error("game_is_running")]
    GameRunning,
    #[error("unknown_or_expired_cleanup_preview")]
    UnknownPreview,
    #[error("cache_changed_since_preview")]
    CacheChanged,
    #[error("cache_scan_limit_exceeded")]
    ScanLimitExceeded,
    #[error("invalid_game_installation")]
    InvalidGameInstallation,
    #[error("invalid_receipt_store")]
    InvalidReceiptStore,
    #[error("cache_cleanup_state_unavailable")]
    StateUnavailable,
    #[error("unsafe_cache_path: {0}")]
    UnsafePath(PathBuf),
    #[error("cache_cleanup_io_{operation}: {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl CacheCleanupError {
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
