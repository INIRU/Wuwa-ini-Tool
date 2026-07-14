mod atomic;
mod error;
mod model;
mod retention;

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

#[cfg(windows)]
use std::fs::OpenOptions;

use crate::ini_document::MergePreview;

use model::{BackupMetadata, StoredBackup, METADATA_SCHEMA_VERSION};

pub use error::{BackupError, ReconciliationState};
pub use model::{
    ApplyReason, ApplyResult, BackupEntry, BackupIntegrity, BackupRecord, OriginalAttributes,
    RestoreResult, SourceExpectation,
};

const METADATA_FILE_NAME: &str = "metadata.json";
static SOURCE_LOCKS: OnceLock<Mutex<HashMap<String, Weak<Mutex<()>>>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct BackupStore {
    root: PathBuf,
}

#[derive(Clone, Debug)]
struct SourceIdentity {
    path: PathBuf,
    lock_key: String,
}

#[derive(Debug)]
struct SourceSnapshot {
    path: PathBuf,
    bytes: Vec<u8>,
    sha256: String,
    attributes: OriginalAttributes,
    file_identity: FileIdentity,
    handle: Option<File>,
}

impl SourceSnapshot {
    fn release_write_exclusion(&mut self) {
        self.handle.take();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    first: u64,
    second: u64,
}

impl BackupStore {
    pub fn new(app_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: app_data_dir.into().join("backups"),
        }
    }

    pub fn create(&self, source: &Path, reason: ApplyReason) -> Result<BackupEntry, BackupError> {
        validate_operation_reason(reason)?;
        let identity = resolve_source_identity(source)?;
        with_source_lock(&identity.lock_key, || {
            let snapshot = self.read_source(&identity)?;
            self.create_from_snapshot(&snapshot, reason)
        })
    }

    pub fn apply(
        &self,
        source: &Path,
        preview: &MergePreview,
        reason: ApplyReason,
    ) -> Result<ApplyResult, BackupError> {
        validate_operation_reason(reason)?;
        let identity = resolve_source_identity(source)?;
        with_source_lock(&identity.lock_key, || {
            let mut snapshot = self.read_source(&identity)?;
            let expected = atomic::sha256_hex(&preview.before);
            ensure_hash(&snapshot.sha256, &expected)?;

            let backup = self.create_from_snapshot(&snapshot, reason)?;
            self.ensure_present_unchanged(&identity, &snapshot, &expected)?;
            // ReplaceFileW needs write access to the source. Release the Windows
            // no-write-sharing handle only after the final pre-commit check; the
            // atomic capture fingerprint reconciles any race after this point.
            snapshot.release_write_exclusion();
            let applied = atomic::write_verified_from_backup(
                &snapshot.path,
                &preview.after,
                &snapshot.attributes,
                &backup.backup_path,
                &backup.backup.sha256,
                (snapshot.file_identity.first, snapshot.file_identity.second),
                &backup.backup.original_attributes,
            )?;
            let cleanup_pending =
                aggregate_cleanup_pending(backup.cleanup_pending.clone(), applied.cleanup_pending);

            Ok(ApplyResult {
                backup: backup.backup,
                backup_path: backup.backup_path,
                applied_sha256: applied.sha256,
                cleanup_pending,
            })
        })
    }

    pub fn restore(
        &self,
        source: &Path,
        backup_id: &str,
        expectation: SourceExpectation,
    ) -> Result<RestoreResult, BackupError> {
        validate_backup_id(backup_id)?;
        validate_expectation(&expectation)?;
        let identity = resolve_source_identity(source)?;
        with_source_lock(&identity.lock_key, || {
            let metadata = self.load_metadata(&identity.path)?;
            let target = metadata
                .records
                .iter()
                .find(|stored| stored.record.id == backup_id)
                .cloned()
                .ok_or_else(|| BackupError::BackupNotFound(backup_id.to_owned()))?;
            let target_path = self.resolve_backup_path(&identity.path, &target.file_name)?;
            validate_regular_owned_file(&target_path, "backup_must_be_a_regular_file")?;
            let target_bytes = atomic::read_verified_bytes(&target_path, &target.record.sha256)?;

            let mut current = self.read_expected_source(&identity, &expectation)?;
            let operation_backup = current
                .as_ref()
                .map(|snapshot| self.create_from_snapshot(snapshot, ApplyReason::Restore))
                .transpose()?;
            self.ensure_expectation_unchanged(&identity, current.as_ref(), &expectation)?;
            if let Some(snapshot) = current.as_mut() {
                snapshot.release_write_exclusion();
            }

            let applied = match &operation_backup {
                Some(backup) => atomic::write_verified_from_backup(
                    &identity.path,
                    &target_bytes,
                    &target.record.original_attributes,
                    &backup.backup_path,
                    &backup.backup.sha256,
                    {
                        let snapshot = current
                            .as_ref()
                            .expect("present restore has a source snapshot");
                        (snapshot.file_identity.first, snapshot.file_identity.second)
                    },
                    &backup.backup.original_attributes,
                )?,
                None => atomic::write_verified_if_missing(
                    &identity.path,
                    &target_bytes,
                    &target.record.original_attributes,
                )?,
            };
            let cleanup_pending = aggregate_cleanup_pending(
                operation_backup
                    .as_ref()
                    .map(|entry| entry.cleanup_pending.clone())
                    .unwrap_or_default(),
                applied.cleanup_pending,
            );

            Ok(RestoreResult {
                backup: operation_backup.as_ref().map(|entry| entry.backup.clone()),
                backup_path: operation_backup.map(|entry| entry.backup_path),
                restored_from: target.record,
                applied_sha256: applied.sha256,
                cleanup_pending,
            })
        })
    }

    pub fn list(&self, source: &Path) -> Result<Vec<BackupEntry>, BackupError> {
        let identity = resolve_source_identity(source)?;
        with_source_lock(&identity.lock_key, || {
            let mut entries = self
                .load_metadata(&identity.path)?
                .records
                .into_iter()
                .map(|stored| {
                    let backup_path =
                        self.resolve_backup_path(&identity.path, &stored.file_name)?;
                    let integrity = backup_integrity(&backup_path, &stored.record.sha256);
                    Ok(BackupEntry {
                        backup: stored.record,
                        backup_path,
                        integrity,
                        cleanup_pending: Vec::new(),
                    })
                })
                .collect::<Result<Vec<_>, BackupError>>()?;
            entries.reverse();
            Ok(entries)
        })
    }

    pub fn pin(
        &self,
        source: &Path,
        backup_id: &str,
        pinned: bool,
    ) -> Result<BackupRecord, BackupError> {
        validate_backup_id(backup_id)?;
        let identity = resolve_source_identity(source)?;
        with_source_lock(&identity.lock_key, || {
            let mut metadata = self.load_metadata(&identity.path)?;
            let record = metadata
                .records
                .iter_mut()
                .find(|stored| stored.record.id == backup_id)
                .ok_or_else(|| BackupError::BackupNotFound(backup_id.to_owned()))?;
            record.record.pinned = pinned;
            let result = record.record.clone();
            if let Some(path) = self.save_metadata(&identity.path, &metadata)? {
                return Err(BackupError::CleanupPending { path });
            }
            Ok(result)
        })
    }

    fn read_expected_source(
        &self,
        identity: &SourceIdentity,
        expectation: &SourceExpectation,
    ) -> Result<Option<SourceSnapshot>, BackupError> {
        match expectation {
            SourceExpectation::Present(expected) => {
                let snapshot = self.read_source(identity)?;
                ensure_hash(&snapshot.sha256, expected)?;
                Ok(Some(snapshot))
            }
            SourceExpectation::Missing => match fs::symlink_metadata(&identity.path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(BackupError::io("source_metadata", &identity.path, error)),
                Ok(_) => Err(BackupError::SourceConflict {
                    expected: "missing".to_owned(),
                    actual: "present".to_owned(),
                }),
            },
        }
    }

    fn read_source(&self, identity: &SourceIdentity) -> Result<SourceSnapshot, BackupError> {
        validate_regular_source(&identity.path)?;
        let mut handle = open_source_locked(&identity.path)?;
        let handle_metadata = handle
            .metadata()
            .map_err(|error| BackupError::io("source_handle_metadata", &identity.path, error))?;
        validate_link_count_and_reparse(&identity.path, &handle_metadata)?;
        let file_identity = file_identity(&handle_metadata)?;
        let mut bytes = Vec::new();
        handle
            .read_to_end(&mut bytes)
            .map_err(|error| BackupError::io("read_source", &identity.path, error))?;
        let attributes = attributes_from_metadata(&handle_metadata);
        Ok(SourceSnapshot {
            path: identity.path.clone(),
            sha256: atomic::sha256_hex(&bytes),
            bytes,
            attributes,
            file_identity,
            handle: Some(handle),
        })
    }

    fn ensure_present_unchanged(
        &self,
        identity: &SourceIdentity,
        snapshot: &SourceSnapshot,
        expected: &str,
    ) -> Result<(), BackupError> {
        validate_regular_source(&identity.path)?;
        let metadata = fs::metadata(&identity.path)
            .map_err(|error| BackupError::io("source_metadata", &identity.path, error))?;
        if file_identity(&metadata)? != snapshot.file_identity {
            return Err(BackupError::SourceConflict {
                expected: "same_file_identity".to_owned(),
                actual: "replaced_file_identity".to_owned(),
            });
        }
        let actual = atomic::hash_file(&identity.path)?;
        ensure_hash(&actual, expected)
    }

    fn ensure_expectation_unchanged(
        &self,
        identity: &SourceIdentity,
        snapshot: Option<&SourceSnapshot>,
        expectation: &SourceExpectation,
    ) -> Result<(), BackupError> {
        match (snapshot, expectation) {
            (Some(snapshot), SourceExpectation::Present(expected)) => {
                self.ensure_present_unchanged(identity, snapshot, expected)
            }
            (None, SourceExpectation::Missing) => match fs::symlink_metadata(&identity.path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(BackupError::io("source_metadata", &identity.path, error)),
                Ok(_) => Err(BackupError::SourceConflict {
                    expected: "missing".to_owned(),
                    actual: "present".to_owned(),
                }),
            },
            _ => Err(BackupError::InvalidMetadata("expectation_state_mismatch")),
        }
    }

    fn create_from_snapshot(
        &self,
        snapshot: &SourceSnapshot,
        reason: ApplyReason,
    ) -> Result<BackupEntry, BackupError> {
        let directory = self.source_directory(&snapshot.path, true)?;
        let mut metadata = self.load_metadata(&snapshot.path)?;
        self.retry_pending_deletions(&snapshot.path, &mut metadata)?;

        let original = match metadata
            .records
            .iter()
            .find(|stored| stored.record.reason == ApplyReason::FirstOriginal)
            .cloned()
        {
            Some(stored) => stored,
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

        metadata
            .pending_deletions
            .extend(retention::prune(&mut metadata.records));
        let metadata_cleanup_pending = self.save_metadata(&snapshot.path, &metadata)?;
        self.cleanup_pending_nonfatal(&directory, &metadata.pending_deletions);

        Ok(BackupEntry {
            backup_path: self.resolve_backup_path(&snapshot.path, &requested.file_name)?,
            backup: requested.record,
            integrity: BackupIntegrity::Verified,
            cleanup_pending: metadata_cleanup_pending.into_iter().collect(),
        })
    }

    fn retry_pending_deletions(
        &self,
        source: &Path,
        metadata: &mut BackupMetadata,
    ) -> Result<(), BackupError> {
        if metadata.pending_deletions.is_empty() {
            return Ok(());
        }
        let directory = self.source_directory(source, false)?;
        let mut remaining = Vec::new();
        for file_name in &metadata.pending_deletions {
            let path = directory.join(file_name);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => remaining.push(file_name.clone()),
            }
        }
        if remaining != metadata.pending_deletions {
            atomic::sync_parent_directory(&directory.join(METADATA_FILE_NAME))?;
            metadata.pending_deletions = remaining;
            if let Some(path) = self.save_metadata(source, metadata)? {
                return Err(BackupError::CleanupPending { path });
            }
        }
        Ok(())
    }

    fn cleanup_pending_nonfatal(&self, directory: &Path, pending: &[String]) {
        for file_name in pending {
            let path = directory.join(file_name);
            if let Ok(metadata) = fs::symlink_metadata(&path) {
                if metadata.is_file() && !metadata.file_type().is_symlink() {
                    let _ = fs::remove_file(path);
                }
            }
        }
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

    fn backup_root(&self) -> Result<PathBuf, BackupError> {
        fs::create_dir_all(&self.root)
            .map_err(|error| BackupError::io("create_backup_root", &self.root, error))?;
        let metadata = fs::symlink_metadata(&self.root)
            .map_err(|error| BackupError::io("backup_root_metadata", &self.root, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(BackupError::InvalidPath {
                path: self.root.clone(),
                reason: "backup_root_must_be_a_real_directory",
            });
        }
        fs::canonicalize(&self.root)
            .map_err(|error| BackupError::io("canonicalize_backup_root", &self.root, error))
    }

    fn source_directory(&self, source: &Path, create: bool) -> Result<PathBuf, BackupError> {
        let root = self.backup_root()?;
        let directory = root.join(source_id(source));
        if create {
            fs::create_dir_all(&directory)
                .map_err(|error| BackupError::io("create_backup_directory", &directory, error))?;
        }
        if directory.exists() {
            let metadata = fs::symlink_metadata(&directory)
                .map_err(|error| BackupError::io("backup_directory_metadata", &directory, error))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(BackupError::InvalidPath {
                    path: directory,
                    reason: "backup_directory_must_be_a_real_directory",
                });
            }
            let canonical = fs::canonicalize(&directory).map_err(|error| {
                BackupError::io("canonicalize_backup_directory", &directory, error)
            })?;
            if !canonical.starts_with(&root) {
                return Err(BackupError::InvalidPath {
                    path: canonical,
                    reason: "backup_directory_outside_root",
                });
            }
            Ok(canonical)
        } else {
            Ok(directory)
        }
    }

    fn metadata_path(&self, source: &Path) -> Result<PathBuf, BackupError> {
        Ok(self
            .source_directory(source, false)?
            .join(METADATA_FILE_NAME))
    }

    fn load_metadata(&self, source: &Path) -> Result<BackupMetadata, BackupError> {
        let path = self.metadata_path(source)?;
        if !path.exists() {
            return Ok(BackupMetadata::new(source.to_path_buf()));
        }
        validate_regular_owned_file(&path, "metadata_must_be_a_regular_file")?;
        let bytes =
            fs::read(&path).map_err(|error| BackupError::io("read_metadata", &path, error))?;
        let metadata: BackupMetadata = serde_json::from_slice(&bytes)?;
        validate_metadata(source, &metadata)?;
        Ok(metadata)
    }

    fn save_metadata(
        &self,
        source: &Path,
        metadata: &BackupMetadata,
    ) -> Result<Option<PathBuf>, BackupError> {
        validate_metadata(source, metadata)?;
        let path = self
            .source_directory(source, true)?
            .join(METADATA_FILE_NAME);
        let bytes = serde_json::to_vec_pretty(metadata)?;
        let attributes = if path.exists() {
            read_attributes(&path)?
        } else {
            OriginalAttributes {
                readonly: false,
                windows_file_attributes: None,
            }
        };
        Ok(atomic::write_verified(&path, &bytes, &attributes)?.cleanup_pending)
    }

    fn resolve_backup_path(&self, source: &Path, file_name: &str) -> Result<PathBuf, BackupError> {
        validate_file_name(file_name)?;
        let directory = self.source_directory(source, true)?;
        let path = directory.join(file_name);
        if !path.starts_with(&directory) {
            return Err(BackupError::InvalidPath {
                path,
                reason: "backup_path_outside_source_directory",
            });
        }
        Ok(path)
    }
}

fn backup_integrity(path: &Path, expected: &str) -> BackupIntegrity {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BackupIntegrity::Missing,
        Err(_) => BackupIntegrity::Corrupt,
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            BackupIntegrity::Corrupt
        }
        Ok(_) => match atomic::hash_file(path) {
            Ok(actual) if actual == expected => BackupIntegrity::Verified,
            Ok(_) | Err(_) => BackupIntegrity::Corrupt,
        },
    }
}

fn aggregate_cleanup_pending(
    mut metadata: Vec<PathBuf>,
    operation: Option<PathBuf>,
) -> Vec<PathBuf> {
    if let Some(path) = operation {
        if !metadata.contains(&path) {
            metadata.push(path);
        }
    }
    metadata
}

fn validate_metadata(source: &Path, metadata: &BackupMetadata) -> Result<(), BackupError> {
    if metadata.schema_version != METADATA_SCHEMA_VERSION {
        return Err(BackupError::UnsupportedMetadataVersion(
            metadata.schema_version,
        ));
    }
    if !paths_equal(&metadata.source_path, source) {
        return Err(BackupError::InvalidMetadata("metadata_source_mismatch"));
    }
    if metadata.records.is_empty() {
        return Err(BackupError::InvalidMetadata("metadata_records_empty"));
    }
    let mut ids = HashSet::new();
    let mut originals = 0;
    for stored in &metadata.records {
        validate_stored_backup(source, stored)?;
        if !ids.insert(stored.record.id.as_str()) {
            return Err(BackupError::InvalidMetadata("duplicate_backup_id"));
        }
        if stored.record.reason == ApplyReason::FirstOriginal {
            originals += 1;
        }
    }
    if originals != 1 {
        return Err(BackupError::InvalidMetadata(
            "exactly_one_first_original_required",
        ));
    }
    let mut retired_names = HashSet::new();
    for file_name in &metadata.pending_deletions {
        validate_retired_file_name(file_name)?;
        if !retired_names.insert(file_name) {
            return Err(BackupError::InvalidMetadata("duplicate_pending_deletion"));
        }
        if metadata
            .records
            .iter()
            .any(|stored| stored.file_name == *file_name)
        {
            return Err(BackupError::InvalidMetadata(
                "pending_deletion_is_still_reachable",
            ));
        }
    }
    Ok(())
}

fn validate_stored_backup(source: &Path, stored: &StoredBackup) -> Result<(), BackupError> {
    if !paths_equal(&stored.record.source_path, source) {
        return Err(BackupError::InvalidMetadata("record_source_mismatch"));
    }
    let parsed = Uuid::parse_str(&stored.record.id)
        .map_err(|_| BackupError::InvalidMetadata("record_id_must_be_uuid"))?;
    if parsed.to_string() != stored.record.id {
        return Err(BackupError::InvalidMetadata("record_id_must_be_canonical"));
    }
    if stored.record.sha256.len() != 64
        || !stored
            .record
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(BackupError::InvalidMetadata("sha256_must_be_lowercase_hex"));
    }
    OffsetDateTime::parse(&stored.record.created_at, &Rfc3339)
        .map_err(|_| BackupError::InvalidMetadata("created_at_must_be_rfc3339"))?;
    validate_file_name(&stored.file_name)?;
    let expected_suffix = format!("-{}.ini", stored.record.id);
    let prefix = stored
        .file_name
        .strip_suffix(&expected_suffix)
        .ok_or(BackupError::InvalidMetadata("file_name_id_mismatch"))?;
    let valid_prefix = if stored.record.reason == ApplyReason::FirstOriginal {
        prefix == "original"
    } else {
        !prefix.is_empty() && prefix.bytes().all(|byte| byte.is_ascii_digit())
    };
    if !valid_prefix {
        return Err(BackupError::InvalidMetadata("invalid_backup_file_name"));
    }
    if stored.application_version.is_empty() {
        return Err(BackupError::InvalidMetadata("application_version_empty"));
    }
    Ok(())
}

fn validate_operation_reason(reason: ApplyReason) -> Result<(), BackupError> {
    if matches!(reason, ApplyReason::FirstOriginal | ApplyReason::Restore) {
        Err(BackupError::InvalidReason(reason))
    } else {
        Ok(())
    }
}

fn validate_expectation(expectation: &SourceExpectation) -> Result<(), BackupError> {
    if let SourceExpectation::Present(hash) = expectation {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(BackupError::InvalidMetadata("expected_hash_invalid"));
        }
    }
    Ok(())
}

fn resolve_source_identity(source: &Path) -> Result<SourceIdentity, BackupError> {
    if !source.is_absolute() {
        return Err(BackupError::InvalidPath {
            path: source.to_path_buf(),
            reason: "source_must_be_absolute",
        });
    }
    let file_name = source.file_name().ok_or_else(|| BackupError::InvalidPath {
        path: source.to_path_buf(),
        reason: "source_must_have_a_file_name",
    })?;
    if file_name.to_string_lossy().contains(':') {
        return Err(BackupError::InvalidPath {
            path: source.to_path_buf(),
            reason: "source_ads_is_not_allowed",
        });
    }
    let parent = source.parent().ok_or_else(|| BackupError::InvalidPath {
        path: source.to_path_buf(),
        reason: "source_must_have_a_parent",
    })?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| BackupError::io("canonicalize_source_parent", parent, error))?;
    validate_source_directory_case_policy(&canonical_parent)?;
    let path = canonical_parent.join(file_name);
    Ok(SourceIdentity {
        lock_key: source_lock_key(&path),
        path,
    })
}

fn validate_regular_source(path: &Path) -> Result<(), BackupError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| BackupError::io("source_metadata", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "source_must_be_a_regular_file",
        });
    }
    validate_link_count_and_reparse(path, &metadata)
}

fn validate_regular_owned_file(path: &Path, reason: &'static str) -> Result<(), BackupError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| BackupError::io("owned_file_metadata", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason,
        });
    }
    validate_link_count_and_reparse(path, &metadata)
}

fn validate_backup_id(backup_id: &str) -> Result<(), BackupError> {
    match Uuid::parse_str(backup_id) {
        Ok(id) if id.to_string() == backup_id => Ok(()),
        _ => Err(BackupError::InvalidPath {
            path: PathBuf::from(backup_id),
            reason: "backup_id_must_be_a_canonical_uuid",
        }),
    }
}

fn validate_file_name(file_name: &str) -> Result<(), BackupError> {
    let path = Path::new(file_name);
    let mut components = path.components();
    let valid = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && file_name.is_ascii()
        && !file_name.contains(':')
        && !file_name.is_empty();
    if valid {
        Ok(())
    } else {
        Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "backup_file_name_must_be_a_strict_single_component",
        })
    }
}

fn validate_retired_file_name(file_name: &str) -> Result<(), BackupError> {
    validate_file_name(file_name)?;
    let stem = file_name
        .strip_suffix(".ini")
        .ok_or(BackupError::InvalidMetadata(
            "retired_backup_must_end_in_ini",
        ))?;
    let (timestamp, id) = stem
        .split_once('-')
        .ok_or(BackupError::InvalidMetadata("retired_backup_name_invalid"))?;
    if timestamp.is_empty() || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(BackupError::InvalidMetadata(
            "retired_backup_timestamp_invalid",
        ));
    }
    let parsed = Uuid::parse_str(id)
        .map_err(|_| BackupError::InvalidMetadata("retired_backup_id_invalid"))?;
    if parsed.to_string() != id {
        return Err(BackupError::InvalidMetadata(
            "retired_backup_id_not_canonical",
        ));
    }
    Ok(())
}

fn source_id(source: &Path) -> String {
    atomic::sha256_hex(&normalized_path_bytes(source, cfg!(windows)))
}

#[cfg(test)]
fn source_id_from_raw_bytes_for_test(bytes: &[u8]) -> String {
    atomic::sha256_hex(bytes)
}

#[cfg(unix)]
fn normalized_path_bytes(path: &Path, case_insensitive: bool) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    path.as_os_str()
        .as_bytes()
        .iter()
        .map(|byte| {
            if case_insensitive {
                byte.to_ascii_lowercase()
            } else {
                *byte
            }
        })
        .collect()
}

#[cfg(windows)]
fn normalized_path_bytes(path: &Path, case_insensitive: bool) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .flat_map(|unit| {
            let normalized = if case_insensitive && (b'A' as u16..=b'Z' as u16).contains(&unit) {
                unit + 32
            } else {
                unit
            };
            normalized.to_le_bytes()
        })
        .collect()
}

fn ensure_hash(actual: &str, expected: &str) -> Result<(), BackupError> {
    if actual == expected {
        Ok(())
    } else {
        Err(BackupError::SourceConflict {
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    return normalized_path_bytes(left, true) == normalized_path_bytes(right, true);
    #[cfg(not(windows))]
    return left == right;
}

fn with_source_lock<T>(lock_key: &str, operation: impl FnOnce() -> T) -> T {
    let registry = SOURCE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let lock = {
        let mut locks = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        locks
            .entry(lock_key.to_owned())
            .or_default()
            .upgrade()
            .unwrap_or_else(|| {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(lock_key.to_owned(), Arc::downgrade(&lock));
                lock
            })
    };
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    operation()
}

fn source_lock_key_for(path: &Path, case_insensitive: bool) -> String {
    atomic::sha256_hex(&normalized_path_bytes(path, case_insensitive))
}

fn source_lock_key(path: &Path) -> String {
    source_lock_key_for(path, cfg!(windows))
}

#[cfg(any(windows, test))]
fn validate_windows_case_sensitive_flag(path: &Path, flags: u32) -> Result<(), BackupError> {
    const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 0x0000_0001;
    if flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR == 0 {
        Ok(())
    } else {
        Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "case_sensitive_source_directories_are_not_supported",
        })
    }
}

#[cfg(not(windows))]
fn validate_source_directory_case_policy(_path: &Path) -> Result<(), BackupError> {
    Ok(())
}

#[cfg(windows)]
fn validate_source_directory_case_policy(path: &Path) -> Result<(), BackupError> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};

    const FILE_CASE_SENSITIVE_INFO_CLASS: u32 = 23;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;

    #[repr(C)]
    struct FileCaseSensitiveInfo {
        flags: u32,
    }
    #[link(name = "Kernel32")]
    extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut std::ffi::c_void,
            info_class: u32,
            info: *mut std::ffi::c_void,
            size: u32,
        ) -> i32;
    }

    // Source IDs intentionally use case-folded raw UTF-16 only after the parent
    // directory confirms standard Windows case-insensitive name semantics.
    let directory = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| BackupError::io("open_source_parent", path, error))?;
    let mut info = FileCaseSensitiveInfo { flags: 0 };
    // SAFETY: directory is an open directory handle and info is writable storage
    // of the exact structure required by FileCaseSensitiveInfo.
    let read = unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle(),
            FILE_CASE_SENSITIVE_INFO_CLASS,
            (&mut info as *mut FileCaseSensitiveInfo).cast(),
            std::mem::size_of::<FileCaseSensitiveInfo>() as u32,
        )
    };
    if read == 0 {
        return Err(BackupError::io(
            "query_source_parent_case_sensitivity",
            path,
            std::io::Error::last_os_error(),
        ));
    }
    validate_windows_case_sensitive_flag(path, info.flags)
}

#[cfg(windows)]
fn open_source_locked(path: &Path) -> Result<File, BackupError> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(path)
        .map_err(|error| BackupError::io("open_source_locked", path, error))
}

#[cfg(not(windows))]
fn open_source_locked(path: &Path) -> Result<File, BackupError> {
    File::open(path).map_err(|error| BackupError::io("open_source_locked", path, error))
}

#[cfg(unix)]
fn validate_link_count_and_reparse(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), BackupError> {
    use std::os::unix::fs::MetadataExt;

    if metadata.nlink() != 1 {
        return Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "hardlinks_are_not_allowed",
        });
    }
    Ok(())
}

#[cfg(windows)]
fn validate_link_count_and_reparse(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), BackupError> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    if metadata.number_of_links() != Some(1)
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "hardlinks_and_reparse_points_are_not_allowed",
        });
    }
    Ok(())
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> Result<FileIdentity, BackupError> {
    use std::os::unix::fs::MetadataExt;

    Ok(FileIdentity {
        first: metadata.dev(),
        second: metadata.ino(),
    })
}

#[cfg(windows)]
fn file_identity(metadata: &fs::Metadata) -> Result<FileIdentity, BackupError> {
    use std::os::windows::fs::MetadataExt;

    let first = metadata
        .volume_serial_number()
        .ok_or(BackupError::InvalidMetadata(
            "source_volume_identity_unavailable",
        ))?;
    let second = metadata.file_index().ok_or(BackupError::InvalidMetadata(
        "source_file_identity_unavailable",
    ))?;
    Ok(FileIdentity {
        first: u64::from(first),
        second,
    })
}

#[cfg(windows)]
fn attributes_from_metadata(metadata: &fs::Metadata) -> OriginalAttributes {
    use std::os::windows::fs::MetadataExt;

    OriginalAttributes {
        readonly: metadata.permissions().readonly(),
        windows_file_attributes: Some(metadata.file_attributes()),
    }
}

#[cfg(not(windows))]
fn attributes_from_metadata(metadata: &fs::Metadata) -> OriginalAttributes {
    OriginalAttributes {
        readonly: metadata.permissions().readonly(),
        windows_file_attributes: None,
    }
}

fn read_attributes(path: &Path) -> Result<OriginalAttributes, BackupError> {
    let metadata = fs::metadata(path).map_err(|error| BackupError::io("metadata", path, error))?;
    Ok(attributes_from_metadata(&metadata))
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Barrier,
        },
        thread,
        time::Duration,
    };

    use super::{
        aggregate_cleanup_pending, source_lock_key_for, validate_windows_case_sensitive_flag,
        with_source_lock,
    };

    #[test]
    fn same_source_operations_are_serialized_in_process() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(3));
        let source = std::env::temp_dir().join("locked-Engine.ini");
        let lock_key = super::source_lock_key(&source);
        let mut threads = Vec::new();
        for _ in 0..2 {
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let start = Arc::clone(&start);
            let lock_key = lock_key.clone();
            threads.push(thread::spawn(move || {
                start.wait();
                with_source_lock(&lock_key, || {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(40));
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }));
        }
        start.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn windows_lock_keys_are_case_insensitive() {
        use std::path::Path;

        let first = source_lock_key_for(Path::new(r"C:\Game\Engine.ini"), true);
        let second = source_lock_key_for(Path::new(r"c:\game\ENGINE.INI"), true);
        assert_eq!(first, second);
    }

    #[test]
    fn source_ids_do_not_collapse_distinct_non_utf8_paths() {
        let first = super::source_id_from_raw_bytes_for_test(b"/game/\x80.ini");
        let second = super::source_id_from_raw_bytes_for_test(b"/game/\x81.ini");

        assert_ne!(first, second);
    }

    #[test]
    fn windows_case_sensitive_directories_are_rejected() {
        let result =
            validate_windows_case_sensitive_flag(std::path::Path::new(r"C:\Game"), 0x0000_0001);

        assert!(matches!(
            result,
            Err(super::BackupError::InvalidPath {
                reason: "case_sensitive_source_directories_are_not_supported",
                ..
            })
        ));
    }

    #[test]
    fn metadata_and_source_cleanup_warnings_are_aggregated() {
        let metadata = std::path::PathBuf::from("metadata-cleanup.json");
        let source = std::path::PathBuf::from("source-cleanup.json");

        let pending = aggregate_cleanup_pending(vec![metadata.clone()], Some(source.clone()));

        assert_eq!(pending, vec![metadata, source]);
    }
}
