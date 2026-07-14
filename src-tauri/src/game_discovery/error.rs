use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("unsupported_platform")]
    UnsupportedPlatform,
    #[error("invalid_executable_path: {0}")]
    InvalidExecutablePath(PathBuf),
    #[error("missing_executable: {0}")]
    MissingExecutable(PathBuf),
    #[error("path_is_not_a_file: {0}")]
    NotAFile(PathBuf),
    #[error("unsafe_path_alias: {0}")]
    UnsafePathAlias(PathBuf),
    #[error("invalid_config_path: {0}")]
    InvalidConfigPath(PathBuf),
    #[error("invalid_keyvalues: {0}")]
    InvalidKeyValues(&'static str),
    #[error("input_too_large: {actual} > {maximum}")]
    InputTooLarge { actual: usize, maximum: usize },
    #[error("io_{operation}: {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl DiscoveryError {
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
