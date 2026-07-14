mod atomic;
mod error;
mod model;
mod retention;

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

use crate::ini_document::MergePreview;

use model::{BackupMetadata, StoredBackup, METADATA_SCHEMA_VERSION};

pub use error::BackupError;
pub use model::{
    ApplyReason, ApplyResult, BackupEntry, BackupRecord, OriginalAttributes, RestoreResult,
};

const METADATA_FILE_NAME: &str = "metadata.json";

#[derive(Clone, Debug)]
pub struct BackupStore {
    root: PathBuf,
}

#[derive(Debug)]
struct SourceSnapshot {
    path: PathBuf,
    bytes: Vec<u8>,
    sha256: String,
    attributes: OriginalAttributes,
}

impl BackupStore {
    pub fn new(app_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: app_data_dir.into().join("backups"),
        }
    }

    pub fn create(&self, source: &Path, reason: ApplyReason) -> Result<BackupEntry, BackupError> {
        let snapshot = self.read_source(source)?;
        self.create_from_snapshot(&snapshot, reason)
    }

    pub fn apply(
        &self,
        source: &Path,
        preview: &MergePreview,
        reason: ApplyReason,
    ) -> Result<ApplyResult, BackupError> {
        let snapshot = self.read_source(source)?;
        let expected = atomic::sha256_hex(&preview.before);
        self.ensure_hash(&snapshot.sha256, &expected)?;

        let backup = self.create_from_snapshot(&snapshot, reason)?;
        self.ensure_source_unchanged(&snapshot.path, &expected)?;
        let applied_sha256 =
            atomic::write_verified(&snapshot.path, &preview.after, &snapshot.attributes)?;

        Ok(ApplyResult {
            backup: backup.backup,
            backup_path: backup.backup_path,
            applied_sha256,
        })
    }

    pub fn restore(&self, source: &Path, backup_id: &str) -> Result<RestoreResult, BackupError> {
        validate_backup_id(backup_id)?;
        let snapshot = self.read_source(source)?;
        let metadata = self.load_metadata(&snapshot.path)?;
        let target = metadata
            .records
            .iter()
            .find(|stored| stored.record.id == backup_id)
            .cloned()
            .ok_or_else(|| BackupError::BackupNotFound(backup_id.to_owned()))?;
        let target_path = self.resolve_backup_path(&snapshot.path, &target.file_name)?;
        let target_bytes = fs::read(&target_path)
            .map_err(|error| BackupError::io("read_backup", &target_path, error))?;
        let actual_target_hash = atomic::sha256_hex(&target_bytes);
        if actual_target_hash != target.record.sha256 {
            return Err(BackupError::HashMismatch {
                path: target_path,
                expected: target.record.sha256,
                actual: actual_target_hash,
            });
        }

        let current_backup = self.create_from_snapshot(&snapshot, ApplyReason::Restore)?;
        self.ensure_source_unchanged(&snapshot.path, &snapshot.sha256)?;
        let applied_sha256 = atomic::write_verified(
            &snapshot.path,
            &target_bytes,
            &target.record.original_attributes,
        )?;

        Ok(RestoreResult {
            backup: current_backup.backup,
            backup_path: current_backup.backup_path,
            restored_from: target.record,
            applied_sha256,
        })
    }

    pub fn list(&self, source: &Path) -> Result<Vec<BackupEntry>, BackupError> {
        let source = validate_source_path(source)?;
        let mut entries = self
            .load_metadata(&source)?
            .records
            .into_iter()
            .map(|stored| {
                let backup_path = self.resolve_backup_path(&source, &stored.file_name)?;
                Ok(BackupEntry {
                    backup: stored.record,
                    backup_path,
                })
            })
            .collect::<Result<Vec<_>, BackupError>>()?;
        entries.reverse();
        Ok(entries)
    }

    pub fn pin(
        &self,
        source: &Path,
        backup_id: &str,
        pinned: bool,
    ) -> Result<BackupRecord, BackupError> {
        validate_backup_id(backup_id)?;
        let source = validate_source_path(source)?;
        let mut metadata = self.load_metadata(&source)?;
        let record = metadata
            .records
            .iter_mut()
            .find(|stored| stored.record.id == backup_id)
            .ok_or_else(|| BackupError::BackupNotFound(backup_id.to_owned()))?;
        record.record.pinned = pinned;
        let result = record.record.clone();
        self.save_metadata(&source, &metadata)?;
        Ok(result)
    }

    fn read_source(&self, source: &Path) -> Result<SourceSnapshot, BackupError> {
        let path = validate_source_path(source)?;
        let bytes =
            fs::read(&path).map_err(|error| BackupError::io("read_source", &path, error))?;
        let attributes = read_attributes(&path)?;
        let sha256 = atomic::sha256_hex(&bytes);
        Ok(SourceSnapshot {
            path,
            bytes,
            sha256,
            attributes,
        })
    }

    fn create_from_snapshot(
        &self,
        snapshot: &SourceSnapshot,
        reason: ApplyReason,
    ) -> Result<BackupEntry, BackupError> {
        let directory = self.source_directory(&snapshot.path);
        fs::create_dir_all(&directory)
            .map_err(|error| BackupError::io("create_backup_directory", &directory, error))?;
        let mut metadata = self.load_metadata(&snapshot.path)?;

        let original_index = metadata
            .records
            .iter()
            .position(|stored| stored.record.reason == ApplyReason::FirstOriginal);
        let original = match original_index {
            Some(index) => metadata.records[index].clone(),
            None => {
                let stored = self.write_backup(snapshot, ApplyReason::FirstOriginal)?;
                metadata.records.push(stored.clone());
                stored
            }
        };

        let requested = if reason == ApplyReason::FirstOriginal {
            original
        } else {
            let stored = self.write_backup(snapshot, reason)?;
            metadata.records.push(stored.clone());
            stored
        };

        let removed = retention::prune(&mut metadata.records);
        self.save_metadata(&snapshot.path, &metadata)?;
        for file_name in removed {
            let path = self.resolve_backup_path(&snapshot.path, &file_name)?;
            if let Err(error) = fs::remove_file(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(BackupError::io("prune_backup", path, error));
                }
            }
        }

        Ok(BackupEntry {
            backup_path: self.resolve_backup_path(&snapshot.path, &requested.file_name)?,
            backup: requested.record,
        })
    }

    fn write_backup(
        &self,
        snapshot: &SourceSnapshot,
        reason: ApplyReason,
    ) -> Result<StoredBackup, BackupError> {
        let id = Uuid::new_v4().to_string();
        let prefix = if reason == ApplyReason::FirstOriginal {
            "original".to_owned()
        } else {
            OffsetDateTime::now_utc().unix_timestamp_nanos().to_string()
        };
        let file_name = format!("{prefix}-{id}.ini");
        let backup_path = self.resolve_backup_path(&snapshot.path, &file_name)?;
        let sha256 = atomic::write_new_verified(&backup_path, &snapshot.bytes)?;
        let created_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .expect("RFC 3339 supports every UTC timestamp");

        Ok(StoredBackup {
            record: BackupRecord {
                id,
                source_path: snapshot.path.clone(),
                created_at,
                sha256,
                reason,
                pinned: false,
                original_attributes: snapshot.attributes.clone(),
            },
            file_name,
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
            detected_game_version: None,
        })
    }

    fn ensure_source_unchanged(&self, source: &Path, expected: &str) -> Result<(), BackupError> {
        let actual = atomic::hash_file(source)?;
        self.ensure_hash(&actual, expected)
    }

    fn ensure_hash(&self, actual: &str, expected: &str) -> Result<(), BackupError> {
        if actual == expected {
            Ok(())
        } else {
            Err(BackupError::SourceConflict {
                expected: expected.to_owned(),
                actual: actual.to_owned(),
            })
        }
    }

    fn source_directory(&self, source: &Path) -> PathBuf {
        self.root.join(source_id(source))
    }

    fn metadata_path(&self, source: &Path) -> PathBuf {
        self.source_directory(source).join(METADATA_FILE_NAME)
    }

    fn load_metadata(&self, source: &Path) -> Result<BackupMetadata, BackupError> {
        let path = self.metadata_path(source);
        if !path.exists() {
            return Ok(BackupMetadata::new(source.to_path_buf()));
        }
        let bytes =
            fs::read(&path).map_err(|error| BackupError::io("read_metadata", &path, error))?;
        let metadata: BackupMetadata = serde_json::from_slice(&bytes)?;
        if metadata.schema_version != METADATA_SCHEMA_VERSION {
            return Err(BackupError::UnsupportedMetadataVersion(
                metadata.schema_version,
            ));
        }
        if metadata.source_path != source {
            return Err(BackupError::InvalidPath {
                path,
                reason: "metadata_source_mismatch",
            });
        }
        for stored in &metadata.records {
            validate_file_name(&stored.file_name)?;
            if stored.record.source_path != source {
                return Err(BackupError::InvalidPath {
                    path: stored.record.source_path.clone(),
                    reason: "record_source_mismatch",
                });
            }
        }
        Ok(metadata)
    }

    fn save_metadata(&self, source: &Path, metadata: &BackupMetadata) -> Result<(), BackupError> {
        let path = self.metadata_path(source);
        let directory = path.parent().expect("metadata path has a parent");
        fs::create_dir_all(directory)
            .map_err(|error| BackupError::io("create_metadata_directory", directory, error))?;
        let bytes = serde_json::to_vec_pretty(metadata)?;
        let attributes = if path.exists() {
            read_attributes(&path)?
        } else {
            OriginalAttributes {
                readonly: false,
                windows_file_attributes: None,
            }
        };
        atomic::write_verified(&path, &bytes, &attributes)?;
        Ok(())
    }

    fn resolve_backup_path(&self, source: &Path, file_name: &str) -> Result<PathBuf, BackupError> {
        validate_file_name(file_name)?;
        Ok(self.source_directory(source).join(file_name))
    }
}

fn validate_source_path(source: &Path) -> Result<PathBuf, BackupError> {
    if !source.is_absolute() {
        return Err(BackupError::InvalidPath {
            path: source.to_path_buf(),
            reason: "source_must_be_absolute",
        });
    }
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| BackupError::io("source_metadata", source, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BackupError::InvalidPath {
            path: source.to_path_buf(),
            reason: "source_must_be_a_regular_file",
        });
    }
    fs::canonicalize(source).map_err(|error| BackupError::io("canonicalize_source", source, error))
}

fn validate_backup_id(backup_id: &str) -> Result<(), BackupError> {
    Uuid::parse_str(backup_id)
        .map(|_| ())
        .map_err(|_| BackupError::InvalidPath {
            path: PathBuf::from(backup_id),
            reason: "backup_id_must_be_a_uuid",
        })
}

fn validate_file_name(file_name: &str) -> Result<(), BackupError> {
    let path = Path::new(file_name);
    let mut components = path.components();
    let valid = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !file_name.is_empty();
    if valid {
        Ok(())
    } else {
        Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "backup_file_name_must_be_a_single_component",
        })
    }
}

fn source_id(source: &Path) -> String {
    #[cfg(windows)]
    let normalized = source.to_string_lossy().to_lowercase();
    #[cfg(not(windows))]
    let normalized = source.to_string_lossy();
    atomic::sha256_hex(normalized.as_bytes())
}

#[cfg(windows)]
fn read_attributes(path: &Path) -> Result<OriginalAttributes, BackupError> {
    use std::os::windows::fs::MetadataExt;

    let metadata = fs::metadata(path).map_err(|error| BackupError::io("metadata", path, error))?;
    Ok(OriginalAttributes {
        readonly: metadata.permissions().readonly(),
        windows_file_attributes: Some(metadata.file_attributes()),
    })
}

#[cfg(not(windows))]
fn read_attributes(path: &Path) -> Result<OriginalAttributes, BackupError> {
    let metadata = fs::metadata(path).map_err(|error| BackupError::io("metadata", path, error))?;
    Ok(OriginalAttributes {
        readonly: metadata.permissions().readonly(),
        windows_file_attributes: None,
    })
}
