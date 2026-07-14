use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) const METADATA_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyReason {
    FirstOriginal,
    Preset,
    RawEditor,
    Restore,
    Manual,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OriginalAttributes {
    pub readonly: bool,
    pub windows_file_attributes: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackupRecord {
    pub id: String,
    pub source_path: PathBuf,
    pub created_at: String,
    pub sha256: String,
    pub reason: ApplyReason,
    pub pinned: bool,
    pub original_attributes: OriginalAttributes,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupEntry {
    pub backup: BackupRecord,
    pub backup_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyResult {
    pub backup: BackupRecord,
    pub backup_path: PathBuf,
    pub applied_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreResult {
    pub backup: BackupRecord,
    pub backup_path: PathBuf,
    pub restored_from: BackupRecord,
    pub applied_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct StoredBackup {
    pub record: BackupRecord,
    pub file_name: String,
    pub application_version: String,
    pub detected_game_version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct BackupMetadata {
    pub schema_version: u32,
    pub source_path: PathBuf,
    pub records: Vec<StoredBackup>,
}

impl BackupMetadata {
    pub(crate) fn new(source_path: PathBuf) -> Self {
        Self {
            schema_version: METADATA_SCHEMA_VERSION,
            source_path,
            records: Vec::new(),
        }
    }
}
