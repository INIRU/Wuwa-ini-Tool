#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IniError {
    #[error("unsupported_encoding")]
    UnsupportedEncoding,
    #[error("ambiguous_managed_key: [{section}] {key}")]
    AmbiguousManagedKey { section: String, key: String },
}
