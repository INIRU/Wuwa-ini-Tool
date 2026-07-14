use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid_name: {0}")]
    InvalidName(String),
    #[error("invalid_file_name: {0}")]
    InvalidFileName(String),
    #[error("path_outside_store: {0}")]
    PathOutsideStore(PathBuf),
    #[error("unsupported_schema_version: {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("unknown_profile_key: {0}")]
    UnknownProfileKey(String),
    #[error("invalid_profile: {0}")]
    InvalidProfile(&'static str),
    #[error("profile_not_found: {0}")]
    ProfileNotFound(String),
    #[error("profile_already_exists: {0}")]
    ProfileAlreadyExists(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io_{operation}: {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl PartialEq for ProfileError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

impl Eq for ProfileError {}

impl ProfileError {
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
