use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{BackupError, OriginalAttributes, ReconciliationState};

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplaceCallState {
    Success,
    UnableToRemoveReplaced,
    UnableToMoveReplacement,
    UnableToMoveReplacement2,
    Other(u32),
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObservedArtifact {
    Missing,
    ExpectedOld,
    AppInstalled,
    External,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReplaceLayout {
    destination: ObservedArtifact,
    replacement: ObservedArtifact,
    capture: ObservedArtifact,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReconcileAction {
    Installed,
    NoMutation,
    RestoreCapture,
    RestoreCaptureAndConflict,
    PreserveForManualRecovery,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileFingerprint {
    sha256: String,
    identity: (u64, u64),
}

#[derive(Debug)]
struct ReplaceReceipt {
    installed: FileFingerprint,
    capture: Option<PathBuf>,
    #[cfg(windows)]
    journal: Option<TransactionJournalHandle>,
}

#[derive(Debug)]
pub(crate) struct AtomicWriteResult {
    pub(crate) sha256: String,
    pub(crate) cleanup_pending: Option<PathBuf>,
}

const TRANSACTION_JOURNAL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum TransactionPhase {
    Prepared,
    Replaced,
    Ambiguous,
    CleanupPending,
    Resolved,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct TransactionJournal {
    schema_version: u32,
    operation_id: String,
    destination_name: String,
    replacement_name: String,
    capture_name: String,
    expected_destination_sha256: String,
    replacement_sha256: String,
    phase: TransactionPhase,
}

#[cfg(any(windows, test))]
#[derive(Debug)]
struct TransactionJournalHandle {
    path: PathBuf,
    record: TransactionJournal,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JournalUpdatePoint {
    BeforeTempWrite,
    AfterTempWrite,
    AfterTempSync,
    AfterReplace,
    AfterParentSync,
}

#[cfg(windows)]
const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
#[cfg(windows)]
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
#[cfg(windows)]
const FILE_BASIC_INFO_CLASS: u32 = 0;

#[cfg(windows)]
#[derive(Clone, Copy)]
#[repr(C)]
struct WindowsFileBasicInfo {
    creation_time: i64,
    last_access_time: i64,
    last_write_time: i64,
    change_time: i64,
    file_attributes: u32,
}

#[cfg(windows)]
fn get_windows_basic_info(file: &File, path: &Path) -> Result<WindowsFileBasicInfo, BackupError> {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "Kernel32")]
    extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut std::ffi::c_void,
            info_class: u32,
            info: *mut std::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    let mut info = WindowsFileBasicInfo {
        creation_time: 0,
        last_access_time: 0,
        last_write_time: 0,
        change_time: 0,
        file_attributes: 0,
    };
    // SAFETY: info is writable and sized for FILE_BASIC_INFO.
    let read = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_BASIC_INFO_CLASS,
            (&mut info as *mut WindowsFileBasicInfo).cast(),
            std::mem::size_of::<WindowsFileBasicInfo>() as u32,
        )
    };
    if read == 0 {
        Err(BackupError::io(
            "get_basic_info",
            path,
            std::io::Error::last_os_error(),
        ))
    } else {
        Ok(info)
    }
}

#[cfg(windows)]
fn set_windows_basic_info(
    file: &File,
    path: &Path,
    info: &WindowsFileBasicInfo,
) -> Result<(), BackupError> {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "Kernel32")]
    extern "system" {
        fn SetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            info_class: u32,
            info: *const std::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    // SAFETY: info remains valid and sized for FILE_BASIC_INFO for the call.
    let written = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FILE_BASIC_INFO_CLASS,
            (info as *const WindowsFileBasicInfo).cast(),
            std::mem::size_of::<WindowsFileBasicInfo>() as u32,
        )
    };
    if written == 0 {
        Err(BackupError::io(
            "set_basic_info",
            path,
            std::io::Error::last_os_error(),
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
struct WindowsReadonlyGuard {
    file: File,
    path: PathBuf,
    original: WindowsFileBasicInfo,
    expected_identity: (u64, u64),
}

#[cfg(windows)]
impl WindowsReadonlyGuard {
    fn restore(self) -> Result<(), BackupError> {
        if windows_handle_identity(&self.file, &self.path)? != self.expected_identity {
            return Err(BackupError::SourceConflict {
                expected: format!("readonly_identity:{:?}", self.expected_identity),
                actual: "readonly_handle_identity_changed".to_owned(),
            });
        }
        set_windows_basic_info(&self.file, &self.path, &self.original)?;
        self.file
            .sync_all()
            .map_err(|error| BackupError::io("flush_readonly_restore", &self.path, error))
    }
}

#[cfg(any(windows, test))]
fn classify_replace(call: ReplaceCallState, layout: ReplaceLayout) -> ReconcileAction {
    use ObservedArtifact::{AppInstalled, ExpectedOld, External, Missing};
    match (call, layout) {
        (
            ReplaceCallState::Success,
            ReplaceLayout {
                destination: AppInstalled,
                replacement: Missing,
                capture: ExpectedOld,
            },
        ) => ReconcileAction::Installed,
        (
            ReplaceCallState::Success,
            ReplaceLayout {
                destination: AppInstalled,
                replacement: Missing,
                capture: External,
            },
        ) => ReconcileAction::RestoreCaptureAndConflict,
        (
            ReplaceCallState::UnableToRemoveReplaced
            | ReplaceCallState::UnableToMoveReplacement
            | ReplaceCallState::Other(_),
            ReplaceLayout {
                destination: ExpectedOld,
                replacement: AppInstalled,
                capture: Missing,
            },
        ) => ReconcileAction::NoMutation,
        (
            ReplaceCallState::UnableToMoveReplacement2,
            ReplaceLayout {
                destination: Missing,
                replacement: AppInstalled,
                capture: ExpectedOld,
            },
        ) => ReconcileAction::RestoreCapture,
        _ => ReconcileAction::PreserveForManualRecovery,
    }
}

#[cfg(any(windows, test))]
fn authorize_rollback(
    current: ObservedArtifact,
    capture: ObservedArtifact,
) -> Result<(), BackupError> {
    if current == ObservedArtifact::AppInstalled
        && matches!(
            capture,
            ObservedArtifact::ExpectedOld | ObservedArtifact::External
        )
    {
        Ok(())
    } else {
        Err(BackupError::SourceConflict {
            expected: "app_installed_destination".to_owned(),
            actual: format!("{current:?}"),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) enum RollbackSource {
    Backup {
        path: PathBuf,
        sha256: String,
        identity: (u64, u64),
        attributes: OriginalAttributes,
    },
    Bytes {
        bytes: Vec<u8>,
        sha256: String,
        identity: (u64, u64),
        attributes: OriginalAttributes,
    },
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn write_new_verified(path: &Path, bytes: &[u8]) -> Result<String, BackupError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| BackupError::io("create", path, error))?;
    file.write_all(bytes)
        .map_err(|error| BackupError::io("write", path, error))?;
    file.sync_all()
        .map_err(|error| BackupError::io("sync", path, error))?;
    drop(file);

    let expected = sha256_hex(bytes);
    let actual = hash_file(path)?;
    if actual != expected {
        return Err(BackupError::HashMismatch {
            path: path.to_path_buf(),
            expected,
            actual,
        });
    }
    sync_parent(path)?;
    Ok(actual)
}

pub(crate) fn write_verified(
    destination: &Path,
    bytes: &[u8],
    attributes: &OriginalAttributes,
) -> Result<AtomicWriteResult, BackupError> {
    if !destination.exists() {
        return write_verified_if_missing(destination, bytes, attributes);
    }
    let (previous, fingerprint) = read_fingerprinted_file(destination)?;
    let rollback = RollbackSource::Bytes {
        sha256: sha256_hex(&previous),
        identity: fingerprint.identity,
        bytes: previous,
        attributes: attributes.clone(),
    };
    write_verified_with_post_replace(destination, bytes, attributes, rollback, |_| Ok(()))
}

pub(crate) fn write_verified_from_backup(
    destination: &Path,
    bytes: &[u8],
    attributes: &OriginalAttributes,
    backup_path: &Path,
    backup_sha256: &str,
    expected_identity: (u64, u64),
    backup_attributes: &OriginalAttributes,
) -> Result<AtomicWriteResult, BackupError> {
    write_verified_with_post_replace(
        destination,
        bytes,
        attributes,
        RollbackSource::Backup {
            path: backup_path.to_path_buf(),
            sha256: backup_sha256.to_owned(),
            identity: expected_identity,
            attributes: backup_attributes.clone(),
        },
        |_| Ok(()),
    )
}

pub(crate) fn write_verified_if_missing(
    destination: &Path,
    bytes: &[u8],
    attributes: &OriginalAttributes,
) -> Result<AtomicWriteResult, BackupError> {
    scavenge_owned_temps(destination)?;
    let temporary = owned_temporary_path(destination, "tmp")?;
    let written_hash = match write_new_verified(&temporary, bytes) {
        Ok(hash) => hash,
        Err(error) => {
            cleanup_temporary(&temporary)?;
            return Err(error);
        }
    };
    let installed_fingerprint = fingerprint_file(&temporary)?;
    match install_new_file_platform(&temporary, destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            cleanup_temporary(&temporary)?;
            return Err(BackupError::SourceConflict {
                expected: "missing".to_owned(),
                actual: "present".to_owned(),
            });
        }
        Err(error) => {
            cleanup_temporary(&temporary)?;
            return Err(BackupError::io("install_new", destination, error));
        }
    }
    let cleanup_pending = cleanup_temporary(&temporary)
        .err()
        .map(|_| temporary.clone());

    let after_install = persist_attributes_and_flush(destination, attributes).and_then(|()| {
        let actual = hash_file(destination)?;
        if actual != written_hash {
            return Err(BackupError::ReadbackMismatch {
                path: destination.to_path_buf(),
                expected: written_hash.clone(),
                actual,
            });
        }
        Ok(actual)
    });
    match after_install {
        Ok(hash) => Ok(AtomicWriteResult {
            sha256: hash,
            cleanup_pending,
        }),
        Err(original) => match rollback_installed_file(destination, &installed_fingerprint) {
            Ok(()) => Err(original),
            Err(rollback) => Err(BackupError::Unrecoverable {
                original: Box::new(original),
                rollback: Box::new(rollback),
            }),
        },
    }
}

fn write_verified_with_post_replace<F>(
    destination: &Path,
    bytes: &[u8],
    attributes: &OriginalAttributes,
    rollback: RollbackSource,
    post_replace: F,
) -> Result<AtomicWriteResult, BackupError>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    scavenge_owned_temps(destination)?;
    let temporary = owned_temporary_path(destination, "tmp")?;
    let written_hash = match write_new_verified(&temporary, bytes) {
        Ok(hash) => hash,
        Err(error) => {
            cleanup_temporary(&temporary)?;
            return Err(error);
        }
    };
    let installed_fingerprint = fingerprint_file(&temporary)?;
    let expected_old = expected_fingerprint(&rollback);

    let receipt = match replace_file(
        destination,
        &temporary,
        Some(&expected_old),
        &installed_fingerprint,
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            if !matches!(error, BackupError::ReconciliationRequired { .. }) {
                cleanup_temporary(&temporary)?;
            }
            return Err(error);
        }
    };

    let after_replace = (|| {
        persist_attributes_and_flush(destination, attributes)?;
        post_replace(destination)
            .map_err(|error| BackupError::io("post_replace", destination, error))?;
        let actual = hash_file(destination)?;
        if actual != written_hash {
            return Err(BackupError::ReadbackMismatch {
                path: destination.to_path_buf(),
                expected: written_hash,
                actual,
            });
        }
        Ok(actual)
    })();

    match after_replace {
        Ok(hash) => {
            let cleanup_pending = finalize_replace_receipt(receipt);
            Ok(AtomicWriteResult {
                sha256: hash,
                cleanup_pending,
            })
        }
        Err(original) => finish_failed_write(
            original,
            rollback_destination(destination, &rollback, receipt),
        ),
    }
}

fn finish_failed_write(
    original: BackupError,
    rollback: Result<Vec<PathBuf>, BackupError>,
) -> Result<AtomicWriteResult, BackupError> {
    match rollback {
        Ok(paths) if paths.is_empty() => Err(original),
        Ok(paths) => Err(BackupError::Unrecoverable {
            original: Box::new(original),
            rollback: Box::new(BackupError::CleanupPending { paths }),
        }),
        Err(rollback) => Err(BackupError::Unrecoverable {
            original: Box::new(original),
            rollback: Box::new(rollback),
        }),
    }
}

fn finalize_replace_receipt(receipt: ReplaceReceipt) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut receipt = receipt;
        let Some(mut journal) = receipt.journal.take() else {
            return None;
        };
        if update_transaction_phase(&mut journal, TransactionPhase::CleanupPending).is_err() {
            return Some(journal.path);
        }
        if cleanup_capture(receipt.capture.as_deref()).is_err() {
            return Some(journal.path);
        }
        if update_transaction_phase(&mut journal, TransactionPhase::Resolved).is_err() {
            return Some(journal.path);
        }
        if cleanup_temporary(&journal.path).is_err() || sync_parent(&journal.path).is_err() {
            return Some(journal.path);
        }
        None
    }
    #[cfg(not(windows))]
    {
        match cleanup_capture(receipt.capture.as_deref()) {
            Ok(()) => None,
            Err(_) => receipt.capture,
        }
    }
}

fn take_outer_capture(receipt: &mut ReplaceReceipt) -> Option<PathBuf> {
    receipt.capture.take()
}

fn complete_capture_rollback_with_finalize<D, F>(
    mut cleanup_pending: Vec<PathBuf>,
    durable_restore: D,
    finalize_outer: F,
) -> Result<Vec<PathBuf>, BackupError>
where
    D: FnOnce() -> Result<(), BackupError>,
    F: FnOnce() -> Option<PathBuf>,
{
    durable_restore()?;
    if let Some(path) = finalize_outer() {
        cleanup_pending.push(path);
    }
    Ok(cleanup_pending)
}

fn rollback_destination(
    destination: &Path,
    rollback: &RollbackSource,
    mut receipt: ReplaceReceipt,
) -> Result<Vec<PathBuf>, BackupError> {
    ensure_installed_destination(destination, &receipt.installed)?;
    if let Some(capture) = take_outer_capture(&mut receipt) {
        #[cfg(windows)]
        let mut cleanup_pending = Vec::new();
        #[cfg(not(windows))]
        let cleanup_pending = Vec::new();
        let expected = expected_fingerprint(rollback);
        let captured = fingerprint_file(&capture)?;
        if captured != expected {
            return Err(BackupError::SourceConflict {
                expected: format!("captured:{expected:?}"),
                actual: format!("captured:{captured:?}"),
            });
        }
        #[cfg(windows)]
        {
            let rollback_receipt = replace_file_windows_transaction(
                destination,
                &capture,
                &receipt.installed,
                &expected,
                true,
            )?;
            if let Some(path) = finalize_replace_receipt(rollback_receipt) {
                cleanup_pending.push(path);
            }
        }
        #[cfg(not(windows))]
        restore_capture_platform(&capture, destination)?;
        return complete_capture_rollback_with_finalize(
            cleanup_pending,
            || {
                persist_attributes_and_flush(destination, rollback_attributes(rollback))?;
                let restored = fingerprint_file(destination)?;
                if restored != expected {
                    return Err(BackupError::ReadbackMismatch {
                        path: destination.to_path_buf(),
                        expected: expected.sha256,
                        actual: restored.sha256,
                    });
                }
                Ok(())
            },
            || finalize_replace_receipt(receipt),
        );
    }
    match rollback {
        RollbackSource::Backup {
            path,
            sha256,
            identity: _,
            attributes,
        } => {
            let bytes = read_verified_bytes(path, sha256)?;
            restore_present(destination, &bytes, sha256, attributes)?;
            Ok(finalize_replace_receipt(receipt).into_iter().collect())
        }
        RollbackSource::Bytes {
            bytes,
            sha256,
            identity: _,
            attributes,
        } => {
            restore_present(destination, bytes, sha256, attributes)?;
            Ok(finalize_replace_receipt(receipt).into_iter().collect())
        }
    }
}

fn rollback_attributes(rollback: &RollbackSource) -> &OriginalAttributes {
    match rollback {
        RollbackSource::Backup { attributes, .. } | RollbackSource::Bytes { attributes, .. } => {
            attributes
        }
    }
}

fn expected_fingerprint(rollback: &RollbackSource) -> FileFingerprint {
    match rollback {
        RollbackSource::Backup {
            sha256, identity, ..
        }
        | RollbackSource::Bytes {
            sha256, identity, ..
        } => FileFingerprint {
            sha256: sha256.clone(),
            identity: *identity,
        },
    }
}

fn cleanup_capture(capture: Option<&Path>) -> Result<(), BackupError> {
    if let Some(capture) = capture {
        remove_owned_temp_with_hook(capture, |_| Ok(()))?;
        sync_parent(capture)?;
    }
    Ok(())
}

fn restore_present(
    destination: &Path,
    bytes: &[u8],
    expected_hash: &str,
    attributes: &OriginalAttributes,
) -> Result<(), BackupError> {
    let source_hash = sha256_hex(bytes);
    if source_hash != expected_hash {
        return Err(BackupError::HashMismatch {
            path: destination.to_path_buf(),
            expected: expected_hash.to_owned(),
            actual: source_hash,
        });
    }
    let temporary = owned_temporary_path(destination, "rollback")?;
    if let Err(error) = write_new_verified(&temporary, bytes) {
        cleanup_temporary(&temporary)?;
        return Err(error);
    }
    if let Err(error) = replace_file_simple(destination, &temporary) {
        cleanup_temporary(&temporary)?;
        return Err(error);
    }
    persist_attributes_and_flush(destination, attributes)?;
    let actual = hash_file(destination)?;
    if actual != expected_hash {
        return Err(BackupError::ReadbackMismatch {
            path: destination.to_path_buf(),
            expected: expected_hash.to_owned(),
            actual,
        });
    }
    Ok(())
}

pub(crate) fn scavenge_owned_temps(destination: &Path) -> Result<(), BackupError> {
    let parent = destination
        .parent()
        .ok_or_else(|| BackupError::InvalidPath {
            path: destination.to_path_buf(),
            reason: "destination_has_no_parent",
        })?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| BackupError::InvalidPath {
            path: destination.to_path_buf(),
            reason: "destination_has_no_file_name",
        })?;
    let prefix = format!(".{file_name}.");
    detect_unresolved_transaction(destination, parent, &prefix)?;
    for entry in
        fs::read_dir(parent).map_err(|error| BackupError::io("scan_temporary", parent, error))?
    {
        let entry =
            entry.map_err(|error| BackupError::io("read_temporary_entry", parent, error))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(suffix) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some((id, extension)) = suffix.rsplit_once('.') else {
            continue;
        };
        let canonical_id = Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id);
        // Capture and rollback artifacts are recovery evidence. They are removed
        // only by the transaction that owns a resolved durable journal.
        if extension != "tmp" || !canonical_id {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| BackupError::io("temporary_metadata", &path, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(BackupError::InvalidPath {
                path,
                reason: "owned_temporary_must_be_a_regular_file",
            });
        }
        remove_owned_temp_with_hook(&path, |_| Ok(()))?;
    }
    sync_parent(destination)
}

fn detect_unresolved_transaction(
    destination: &Path,
    parent: &Path,
    prefix: &str,
) -> Result<(), BackupError> {
    for entry in
        fs::read_dir(parent).map_err(|error| BackupError::io("scan_journal", parent, error))?
    {
        let entry = entry.map_err(|error| BackupError::io("read_journal_entry", parent, error))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(suffix) = name.strip_prefix(prefix) else {
            continue;
        };
        let Some(operation_id) = suffix.strip_suffix(".journal") else {
            continue;
        };
        if !Uuid::parse_str(operation_id).is_ok_and(|parsed| parsed.to_string() == operation_id) {
            return Err(BackupError::InvalidPath {
                path: entry.path(),
                reason: "recovery_journal_id_must_be_a_canonical_uuid",
            });
        }
        let journal_path = entry.path();
        let metadata = fs::symlink_metadata(&journal_path)
            .map_err(|error| BackupError::io("journal_metadata", &journal_path, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(BackupError::InvalidPath {
                path: journal_path,
                reason: "recovery_journal_must_be_a_regular_file",
            });
        }
        let bytes = fs::read(&journal_path)
            .map_err(|error| BackupError::io("read_recovery_journal", &journal_path, error))?;
        let journal: TransactionJournal = match serde_json::from_slice(&bytes) {
            Ok(journal) => journal,
            Err(error) => {
                let file_name = destination
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Engine.ini");
                return Err(BackupError::ReconciliationRequired {
                    state: Box::new(ReconciliationState {
                        operation_id: operation_id.to_owned(),
                        code: 0,
                        destination: destination.to_path_buf(),
                        replacement: parent.join(format!(".{file_name}.{operation_id}.rollback")),
                        capture: parent.join(format!(".{file_name}.{operation_id}.capture")),
                        journal: journal_path,
                        context: format!("recovery journal is malformed or truncated: {error}"),
                    }),
                });
            }
        };
        validate_transaction_journal(destination, &journal_path, &journal)?;
        if journal.phase == TransactionPhase::Resolved {
            remove_owned_temp_with_hook(&journal_path, |_| Ok(()))?;
            continue;
        }
        return Err(reconciliation_from_journal(
            destination,
            &journal_path,
            &journal,
            0,
            "unresolved durable transaction journal",
        ));
    }
    Ok(())
}

fn validate_transaction_journal(
    destination: &Path,
    journal_path: &Path,
    journal: &TransactionJournal,
) -> Result<(), BackupError> {
    if journal.schema_version != TRANSACTION_JOURNAL_SCHEMA_VERSION {
        return Err(BackupError::InvalidMetadata(
            "unsupported_recovery_journal_version",
        ));
    }
    let parsed = Uuid::parse_str(&journal.operation_id)
        .map_err(|_| BackupError::InvalidMetadata("recovery_journal_operation_id_invalid"))?;
    if parsed.to_string() != journal.operation_id {
        return Err(BackupError::InvalidMetadata(
            "recovery_journal_operation_id_not_canonical",
        ));
    }
    let expected_journal_name = format!(
        ".{}.{}.journal",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| BackupError::InvalidPath {
                path: destination.to_path_buf(),
                reason: "destination_has_no_file_name",
            })?,
        journal.operation_id
    );
    if journal_path.file_name().and_then(|name| name.to_str()) != Some(&expected_journal_name) {
        return Err(BackupError::InvalidPath {
            path: journal_path.to_path_buf(),
            reason: "recovery_journal_name_mismatch",
        });
    }
    for artifact in [
        &journal.destination_name,
        &journal.replacement_name,
        &journal.capture_name,
    ] {
        let path = Path::new(artifact);
        let mut components = path.components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
            || artifact.contains(':')
        {
            return Err(BackupError::InvalidPath {
                path: PathBuf::from(artifact),
                reason: "recovery_journal_artifact_must_be_a_file_name",
            });
        }
    }
    if destination.file_name().and_then(|name| name.to_str())
        != Some(journal.destination_name.as_str())
    {
        return Err(BackupError::InvalidPath {
            path: destination.to_path_buf(),
            reason: "recovery_journal_destination_mismatch",
        });
    }
    for hash in [
        &journal.expected_destination_sha256,
        &journal.replacement_sha256,
    ] {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(BackupError::InvalidMetadata(
                "recovery_journal_hash_invalid",
            ));
        }
    }
    Ok(())
}

fn reconciliation_from_journal(
    destination: &Path,
    journal_path: &Path,
    journal: &TransactionJournal,
    code: u32,
    context: impl Into<String>,
) -> BackupError {
    let parent = destination.parent().unwrap_or_else(|| Path::new(""));
    BackupError::ReconciliationRequired {
        state: Box::new(ReconciliationState {
            operation_id: journal.operation_id.clone(),
            code,
            destination: destination.to_path_buf(),
            replacement: parent.join(&journal.replacement_name),
            capture: parent.join(&journal.capture_name),
            journal: journal_path.to_path_buf(),
            context: context.into(),
        }),
    }
}

#[cfg(windows)]
fn create_transaction_journal(
    destination: &Path,
    replacement: &Path,
    capture: &Path,
    operation_id: Uuid,
    expected: &FileFingerprint,
    installed: &FileFingerprint,
) -> Result<TransactionJournalHandle, BackupError> {
    let parent = destination
        .parent()
        .ok_or_else(|| BackupError::InvalidPath {
            path: destination.to_path_buf(),
            reason: "destination_has_no_parent",
        })?;
    let destination_name = path_file_name(destination)?;
    let record = TransactionJournal {
        schema_version: TRANSACTION_JOURNAL_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        destination_name: destination_name.clone(),
        replacement_name: path_file_name(replacement)?,
        capture_name: path_file_name(capture)?,
        expected_destination_sha256: expected.sha256.clone(),
        replacement_sha256: installed.sha256.clone(),
        phase: TransactionPhase::Prepared,
    };
    let path = parent.join(format!(".{destination_name}.{operation_id}.journal"));
    validate_transaction_journal(destination, &path, &record)?;
    let handle = TransactionJournalHandle { path, record };
    persist_transaction_journal(&handle, true)?;
    Ok(handle)
}

#[cfg(windows)]
fn path_file_name(path: &Path) -> Result<String, BackupError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "transaction_artifact_has_no_utf8_file_name",
        })
}

#[cfg(windows)]
fn persist_transaction_journal(
    journal: &TransactionJournalHandle,
    create: bool,
) -> Result<(), BackupError> {
    persist_transaction_journal_with_hook(journal, create, |_| Ok(()))
}

#[cfg(any(windows, test))]
fn persist_transaction_journal_with_hook<F>(
    journal: &TransactionJournalHandle,
    create: bool,
    mut hook: F,
) -> Result<(), BackupError>
where
    F: FnMut(JournalUpdatePoint) -> std::io::Result<()>,
{
    let bytes = serde_json::to_vec(&journal.record)?;
    if create {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&journal.path)
            .map_err(|error| BackupError::io("open_recovery_journal", &journal.path, error))?;
        file.write_all(&bytes)
            .map_err(|error| BackupError::io("write_recovery_journal", &journal.path, error))?;
        file.sync_all()
            .map_err(|error| BackupError::io("sync_recovery_journal", &journal.path, error))?;
        drop(file);
        return sync_parent(&journal.path);
    }

    let metadata = fs::symlink_metadata(&journal.path)
        .map_err(|error| BackupError::io("journal_metadata", &journal.path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BackupError::InvalidPath {
            path: journal.path.clone(),
            reason: "recovery_journal_must_be_a_regular_file",
        });
    }
    let temporary = journal_update_temporary_path(journal)?;
    hook(JournalUpdatePoint::BeforeTempWrite)
        .map_err(|error| BackupError::io("journal_update_hook", &journal.path, error))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| BackupError::io("create_journal_update", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| {
        hook(JournalUpdatePoint::AfterTempWrite)?;
        file.sync_all()?;
        hook(JournalUpdatePoint::AfterTempSync)
    }) {
        drop(file);
        let _ = cleanup_temporary(&temporary);
        return Err(BackupError::io("write_journal_update", &temporary, error));
    }
    drop(file);
    if let Err(error) = replace_file_platform(&journal.path, &temporary) {
        let _ = cleanup_temporary(&temporary);
        return Err(BackupError::io(
            "replace_recovery_journal",
            &journal.path,
            error,
        ));
    }
    hook(JournalUpdatePoint::AfterReplace)
        .map_err(|error| BackupError::io("journal_update_hook", &journal.path, error))?;
    sync_parent(&journal.path)?;
    hook(JournalUpdatePoint::AfterParentSync)
        .map_err(|error| BackupError::io("journal_update_hook", &journal.path, error))
}

#[cfg(windows)]
fn update_transaction_phase(
    journal: &mut TransactionJournalHandle,
    phase: TransactionPhase,
) -> Result<(), BackupError> {
    journal.record.phase = phase;
    persist_transaction_journal(journal, false)
}

#[cfg(windows)]
fn reconciliation_after_call(
    destination: &Path,
    journal: &mut TransactionJournalHandle,
    code: u32,
    context: impl Into<String>,
) -> BackupError {
    let context = context.into();
    if let Err(error) = update_transaction_phase(journal, TransactionPhase::Ambiguous) {
        return reconciliation_from_journal(
            destination,
            &journal.path,
            &journal.record,
            code,
            format!("{context}; journal_update_error={error}"),
        );
    }
    reconciliation_from_journal(destination, &journal.path, &journal.record, code, context)
}

fn owned_temporary_path(destination: &Path, extension: &str) -> Result<PathBuf, BackupError> {
    let parent = destination
        .parent()
        .ok_or_else(|| BackupError::InvalidPath {
            path: destination.to_path_buf(),
            reason: "destination_has_no_parent",
        })?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| BackupError::InvalidPath {
            path: destination.to_path_buf(),
            reason: "destination_has_no_file_name",
        })?;
    Ok(parent.join(format!(".{file_name}.{}.{extension}", Uuid::new_v4())))
}

fn cleanup_temporary(path: &Path) -> Result<(), BackupError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BackupError::io("temporary_metadata", path, error)),
        Ok(_) => remove_owned_temp_with_hook(path, |_| Ok(())),
    }
}

#[cfg(any(windows, test))]
fn journal_update_temporary_path(
    journal: &TransactionJournalHandle,
) -> Result<PathBuf, BackupError> {
    let parent = journal
        .path
        .parent()
        .ok_or_else(|| BackupError::InvalidPath {
            path: journal.path.clone(),
            reason: "recovery_journal_has_no_parent",
        })?;
    Ok(parent.join(format!(
        ".{}.{}.tmp",
        journal.record.destination_name,
        Uuid::new_v4()
    )))
}

fn remove_owned_temp_with_hook<F>(path: &Path, hook: F) -> Result<(), BackupError>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| BackupError::io("temporary_metadata", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "owned_temporary_must_be_a_regular_file",
        });
    }
    let original_identity = owned_file_identity(&metadata, path)?;
    hook(path).map_err(|error| BackupError::io("temporary_hook", path, error))?;
    let current = fs::symlink_metadata(path)
        .map_err(|error| BackupError::io("temporary_recheck", path, error))?;
    if current.file_type().is_symlink()
        || !current.is_file()
        || owned_file_identity(&current, path)? != original_identity
    {
        return Err(BackupError::SourceConflict {
            expected: "same_owned_temporary".to_owned(),
            actual: "temporary_identity_changed".to_owned(),
        });
    }
    delete_file_no_follow(path, original_identity)
}

#[cfg(unix)]
fn owned_file_identity(metadata: &fs::Metadata, _path: &Path) -> Result<(u64, u64), BackupError> {
    use std::os::unix::fs::MetadataExt;

    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn owned_file_identity(_metadata: &fs::Metadata, path: &Path) -> Result<(u64, u64), BackupError> {
    let file = open_fingerprint_handle(path)?;
    windows_handle_identity(&file, path)
}

#[cfg(windows)]
fn windows_handle_identity(file: &File, path: &Path) -> Result<(u64, u64), BackupError> {
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        attributes: u32,
        creation: FileTime,
        access: FileTime,
        write: FileTime,
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }

    #[link(name = "Kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            info: *mut ByHandleFileInformation,
        ) -> i32;
    }

    let mut info = std::mem::MaybeUninit::<ByHandleFileInformation>::uninit();
    // SAFETY: `file` owns a valid Windows handle and `info` points to writable
    // storage of the exact structure expected by GetFileInformationByHandle.
    let inspected = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
    if inspected == 0 {
        return Err(BackupError::io(
            "inspect_file_identity",
            path,
            std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: the successful call initialized the complete structure.
    let info = unsafe { info.assume_init() };
    if info.links != 1 || info.attributes & 0x0000_0400 != 0 {
        return Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "hardlinks_and_reparse_points_are_not_allowed",
        });
    }
    Ok((
        u64::from(info.volume),
        (u64::from(info.index_high) << 32) | u64::from(info.index_low),
    ))
}

#[cfg(unix)]
fn delete_file_no_follow(path: &Path, expected_identity: (u64, u64)) -> Result<(), BackupError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| BackupError::io("delete_metadata", path, error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || owned_file_identity(&metadata, path)? != expected_identity
    {
        return Err(BackupError::SourceConflict {
            expected: format!("delete_identity:{expected_identity:?}"),
            actual: "delete_target_changed".to_owned(),
        });
    }
    fs::remove_file(path).map_err(|error| BackupError::io("delete_owned_file", path, error))
}

#[cfg(windows)]
fn delete_file_no_follow(path: &Path, expected_identity: (u64, u64)) -> Result<(), BackupError> {
    use std::os::windows::ffi::OsStrExt;

    const DELETE: u32 = 0x0001_0000;
    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0000_0100;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_BASIC_INFO_CLASS: u32 = 0;
    const FILE_DISPOSITION_INFO_CLASS: u32 = 4;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    struct ByHandleFileInformation {
        attributes: u32,
        creation: FileTime,
        access: FileTime,
        write: FileTime,
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[repr(C)]
    struct FileDispositionInfo {
        delete_file: u8,
    }
    #[derive(Clone, Copy)]
    #[repr(C)]
    struct FileBasicInfo {
        creation_time: i64,
        last_access_time: i64,
        last_write_time: i64,
        change_time: i64,
        file_attributes: u32,
    }
    #[link(name = "Kernel32")]
    extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *mut std::ffi::c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
        fn GetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            info: *mut ByHandleFileInformation,
        ) -> i32;
        fn GetFileInformationByHandleEx(
            file: *mut std::ffi::c_void,
            info_class: u32,
            info: *mut std::ffi::c_void,
            size: u32,
        ) -> i32;
        fn SetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            info_class: u32,
            info: *const std::ffi::c_void,
            size: u32,
        ) -> i32;
        fn CloseHandle(object: *mut std::ffi::c_void) -> i32;
    }

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: path is NUL-terminated and pointer arguments follow CreateFileW's contract.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            DELETE | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle as isize == -1 {
        return Err(BackupError::io(
            "open_owned_file_no_follow",
            path,
            std::io::Error::last_os_error(),
        ));
    }
    let mut info = std::mem::MaybeUninit::<ByHandleFileInformation>::uninit();
    // SAFETY: handle is valid and info points to writable storage of the required size.
    let read = unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) };
    if read == 0 {
        let source = std::io::Error::last_os_error();
        // SAFETY: handle is valid and closed once on this error path.
        unsafe { CloseHandle(handle) };
        return Err(BackupError::io("owned_file_identity", path, source));
    }
    // SAFETY: the preceding call initialized info after returning nonzero.
    let info = unsafe { info.assume_init() };
    let actual_identity = (
        u64::from(info.volume),
        (u64::from(info.index_high) << 32) | u64::from(info.index_low),
    );
    if actual_identity != expected_identity {
        // SAFETY: handle is valid and closed once on this conflict path.
        unsafe { CloseHandle(handle) };
        return Err(BackupError::SourceConflict {
            expected: format!("delete_identity:{expected_identity:?}"),
            actual: format!("delete_identity:{actual_identity:?}"),
        });
    }
    let mut original_basic = FileBasicInfo {
        creation_time: 0,
        last_access_time: 0,
        last_write_time: 0,
        change_time: 0,
        file_attributes: 0,
    };
    // SAFETY: original_basic is writable storage for FILE_BASIC_INFO.
    let basic_read = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FILE_BASIC_INFO_CLASS,
            (&mut original_basic as *mut FileBasicInfo).cast(),
            std::mem::size_of::<FileBasicInfo>() as u32,
        )
    };
    if basic_read == 0 {
        let source = std::io::Error::last_os_error();
        // SAFETY: handle is valid and closed once on this error path.
        unsafe { CloseHandle(handle) };
        return Err(BackupError::io("owned_file_basic_info", path, source));
    }
    let readonly_cleared = original_basic.file_attributes & FILE_ATTRIBUTE_READONLY != 0;
    if readonly_cleared {
        let mut writable = original_basic;
        writable.file_attributes &= !FILE_ATTRIBUTE_READONLY;
        if writable.file_attributes == 0 {
            writable.file_attributes = FILE_ATTRIBUTE_NORMAL;
        }
        // SAFETY: writable is a valid FILE_BASIC_INFO and the handle permits attribute writes.
        let cleared = unsafe {
            SetFileInformationByHandle(
                handle,
                FILE_BASIC_INFO_CLASS,
                (&writable as *const FileBasicInfo).cast(),
                std::mem::size_of::<FileBasicInfo>() as u32,
            )
        };
        if cleared == 0 {
            let source = std::io::Error::last_os_error();
            // SAFETY: handle is valid and closed once on this error path.
            unsafe { CloseHandle(handle) };
            return Err(BackupError::io("clear_owned_file_readonly", path, source));
        }
    }
    let disposition = FileDispositionInfo { delete_file: 1 };
    // SAFETY: disposition is valid for FileDispositionInfo and handle has DELETE access.
    let deleted = unsafe {
        SetFileInformationByHandle(
            handle,
            FILE_DISPOSITION_INFO_CLASS,
            (&disposition as *const FileDispositionInfo).cast(),
            std::mem::size_of::<FileDispositionInfo>() as u32,
        )
    };
    let delete_error = (deleted == 0).then(std::io::Error::last_os_error);
    let restore_error = if delete_error.is_some() && readonly_cleared {
        // SAFETY: original_basic is valid and the same identity-checked handle remains open.
        let restored = unsafe {
            SetFileInformationByHandle(
                handle,
                FILE_BASIC_INFO_CLASS,
                (&original_basic as *const FileBasicInfo).cast(),
                std::mem::size_of::<FileBasicInfo>() as u32,
            )
        };
        (restored == 0).then(std::io::Error::last_os_error)
    } else {
        None
    };
    // SAFETY: handle is valid and closed exactly once.
    unsafe { CloseHandle(handle) };
    if let Some(source) = delete_error {
        let original = BackupError::io("delete_owned_file", path, source);
        if let Some(source) = restore_error {
            Err(BackupError::Unrecoverable {
                original: Box::new(original),
                rollback: Box::new(BackupError::io("restore_owned_file_readonly", path, source)),
            })
        } else {
            Err(original)
        }
    } else {
        Ok(())
    }
}

pub(crate) fn hash_file(path: &Path) -> Result<String, BackupError> {
    Ok(fingerprint_file(path)?.sha256)
}

pub(crate) fn read_verified_bytes(
    path: &Path,
    expected_hash: &str,
) -> Result<Vec<u8>, BackupError> {
    let (bytes, fingerprint) = read_fingerprinted_file(path)?;
    if fingerprint.sha256 == expected_hash {
        Ok(bytes)
    } else {
        Err(BackupError::HashMismatch {
            path: path.to_path_buf(),
            expected: expected_hash.to_owned(),
            actual: fingerprint.sha256,
        })
    }
}

fn fingerprint_file(path: &Path) -> Result<FileFingerprint, BackupError> {
    read_fingerprinted_file(path).map(|(_, fingerprint)| fingerprint)
}

fn read_fingerprinted_file(path: &Path) -> Result<(Vec<u8>, FileFingerprint), BackupError> {
    read_fingerprinted_file_with_hook(path, |_| Ok(()))
}

fn read_fingerprinted_file_with_hook<F>(
    path: &Path,
    hook: F,
) -> Result<(Vec<u8>, FileFingerprint), BackupError>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    let before = fs::symlink_metadata(path)
        .map_err(|error| BackupError::io("fingerprint_metadata", path, error))?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err(BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "fingerprint_source_must_be_a_regular_file",
        });
    }
    let before_identity = owned_file_identity(&before, path)?;
    let mut file = open_fingerprint_handle(path)?;
    let _handle_metadata = file
        .metadata()
        .map_err(|error| BackupError::io("fingerprint_handle_metadata", path, error))?;
    let handle_identity = {
        #[cfg(windows)]
        {
            windows_handle_identity(&file, path)?
        }
        #[cfg(not(windows))]
        {
            owned_file_identity(&_handle_metadata, path)?
        }
    };
    if handle_identity != before_identity {
        return Err(BackupError::SourceConflict {
            expected: format!("fingerprint_identity:{before_identity:?}"),
            actual: format!("fingerprint_handle:{handle_identity:?}"),
        });
    }
    hook(path).map_err(|error| BackupError::io("fingerprint_hook", path, error))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| BackupError::io("fingerprint_read", path, error))?;
    let after = fs::symlink_metadata(path)
        .map_err(|error| BackupError::io("fingerprint_path_recheck", path, error))?;
    if after.file_type().is_symlink()
        || !after.is_file()
        || owned_file_identity(&after, path)? != handle_identity
    {
        return Err(BackupError::SourceConflict {
            expected: format!("fingerprint_handle:{handle_identity:?}"),
            actual: "fingerprint_path_identity_changed".to_owned(),
        });
    }
    let sha256 = sha256_hex(&bytes);
    Ok((
        bytes,
        FileFingerprint {
            sha256,
            identity: handle_identity,
        },
    ))
}

#[cfg(not(windows))]
fn open_fingerprint_handle(path: &Path) -> Result<File, BackupError> {
    File::open(path).map_err(|error| BackupError::io("open_fingerprint", path, error))
}

#[cfg(windows)]
fn open_fingerprint_handle(path: &Path) -> Result<File, BackupError> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| BackupError::io("open_fingerprint_no_follow", path, error))
}

fn ensure_installed_destination(
    destination: &Path,
    installed: &FileFingerprint,
) -> Result<(), BackupError> {
    let actual = fingerprint_file(destination)?;
    if &actual == installed {
        Ok(())
    } else {
        Err(BackupError::SourceConflict {
            expected: format!("installed:{}:{:?}", installed.sha256, installed.identity),
            actual: format!("current:{}:{:?}", actual.sha256, actual.identity),
        })
    }
}

fn persist_attributes_and_flush(
    path: &Path,
    attributes: &OriginalAttributes,
) -> Result<(), BackupError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| BackupError::io("open_destination_for_flush", path, error))?;
    verify_handle_path_identity(&file, path)?;
    file.sync_all()
        .map_err(|error| BackupError::io("flush_destination", path, error))?;
    apply_attributes(&file, path, attributes)?;
    file.sync_all()
        .map_err(|error| BackupError::io("flush_destination_attributes", path, error))?;
    verify_handle_path_identity(&file, path)?;
    sync_parent(path)
}

fn verify_handle_path_identity(file: &File, path: &Path) -> Result<(), BackupError> {
    let _handle_metadata = file
        .metadata()
        .map_err(|error| BackupError::io("handle_identity", path, error))?;
    let path_metadata =
        fs::metadata(path).map_err(|error| BackupError::io("path_identity", path, error))?;
    let handle_identity = {
        #[cfg(windows)]
        {
            windows_handle_identity(file, path)?
        }
        #[cfg(not(windows))]
        {
            owned_file_identity(&_handle_metadata, path)?
        }
    };
    let path_identity = owned_file_identity(&path_metadata, path)?;
    if handle_identity == path_identity {
        Ok(())
    } else {
        Err(BackupError::SourceConflict {
            expected: format!("open_handle:{handle_identity:?}"),
            actual: format!("path:{path_identity:?}"),
        })
    }
}

fn rollback_installed_file(
    destination: &Path,
    installed: &FileFingerprint,
) -> Result<(), BackupError> {
    ensure_installed_destination(destination, installed)?;
    delete_file_no_follow(destination, installed.identity)?;
    sync_parent(destination)
}

#[cfg(not(windows))]
fn install_new_file_platform(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(temporary, destination)
}

#[cfg(windows)]
fn install_new_file_platform(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new_name: *const u16, flags: u32) -> i32;
    }
    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }
    let temporary = wide(temporary);
    let destination = wide(destination);
    // SAFETY: both strings are NUL-terminated and remain alive for the call.
    let result = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn sync_parent_directory(path: &Path) -> Result<(), BackupError> {
    sync_parent(path)
}

fn replace_file(
    destination: &Path,
    replacement: &Path,
    expected_old: Option<&FileFingerprint>,
    installed: &FileFingerprint,
) -> Result<ReplaceReceipt, BackupError> {
    replace_file_reconciled(destination, replacement, expected_old, installed)
}

#[cfg(not(windows))]
fn replace_file_reconciled(
    destination: &Path,
    replacement: &Path,
    _expected_old: Option<&FileFingerprint>,
    installed: &FileFingerprint,
) -> Result<ReplaceReceipt, BackupError> {
    replace_file_simple(destination, replacement)?;
    ensure_installed_destination(destination, installed)?;
    Ok(ReplaceReceipt {
        installed: installed.clone(),
        capture: None,
    })
}

#[cfg(windows)]
fn replace_file_reconciled(
    destination: &Path,
    replacement: &Path,
    expected_old: Option<&FileFingerprint>,
    installed: &FileFingerprint,
) -> Result<ReplaceReceipt, BackupError> {
    replace_file_windows(destination, replacement, expected_old, installed)
}

#[cfg(windows)]
fn replace_file_windows(
    destination: &Path,
    replacement: &Path,
    expected_old: Option<&FileFingerprint>,
    installed: &FileFingerprint,
) -> Result<ReplaceReceipt, BackupError> {
    let expected_old = expected_old.ok_or_else(|| BackupError::SourceConflict {
        expected: "present_destination".to_owned(),
        actual: "missing_expected_fingerprint".to_owned(),
    })?;
    replace_file_windows_transaction(destination, replacement, expected_old, installed, true)
}

#[cfg(windows)]
fn replace_file_windows_transaction(
    destination: &Path,
    replacement: &Path,
    expected_old: &FileFingerprint,
    installed: &FileFingerprint,
    allow_conflict_restore: bool,
) -> Result<ReplaceReceipt, BackupError> {
    use std::{os::windows::ffi::OsStrExt, ptr};

    const ERROR_UNABLE_TO_REMOVE_REPLACED: u32 = 1175;
    const ERROR_UNABLE_TO_MOVE_REPLACEMENT: u32 = 1176;
    const ERROR_UNABLE_TO_MOVE_REPLACEMENT_2: u32 = 1177;

    #[link(name = "Kernel32")]
    extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let operation_id = Uuid::new_v4();
    let destination_name = path_file_name(destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| BackupError::InvalidPath {
            path: destination.to_path_buf(),
            reason: "destination_has_no_parent",
        })?;
    let capture = parent.join(format!(".{destination_name}.{operation_id}.capture"));
    let mut journal = create_transaction_journal(
        destination,
        replacement,
        &capture,
        operation_id,
        expected_old,
        installed,
    )?;
    let readonly_guard = match clear_readonly_for_replace(destination, expected_old) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = update_transaction_phase(&mut journal, TransactionPhase::Resolved);
            let _ = cleanup_temporary(&journal.path);
            return Err(error);
        }
    };
    let destination_wide = wide(destination);
    let replacement_wide = wide(replacement);
    let capture_wide = wide(&capture);
    // SAFETY: all paths are NUL-terminated and remain alive for the call.
    let result = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            replacement_wide.as_ptr(),
            capture_wide.as_ptr(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    let code = if result != 0 {
        0
    } else {
        std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or_default() as u32
    };
    let call = match (result != 0, code) {
        (true, _) => ReplaceCallState::Success,
        (false, ERROR_UNABLE_TO_REMOVE_REPLACED) => ReplaceCallState::UnableToRemoveReplaced,
        (false, ERROR_UNABLE_TO_MOVE_REPLACEMENT) => ReplaceCallState::UnableToMoveReplacement,
        (false, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2) => ReplaceCallState::UnableToMoveReplacement2,
        (false, code) => ReplaceCallState::Other(code),
    };
    let invoked_phase = if result != 0 {
        TransactionPhase::Replaced
    } else {
        TransactionPhase::Ambiguous
    };
    if let Err(error) = update_transaction_phase(&mut journal, invoked_phase) {
        return Err(reconciliation_after_call(
            destination,
            &mut journal,
            code,
            format!("phase update after ReplaceFileW failed: {error}"),
        ));
    }
    let observe = |path: &Path| observe_artifact(path, expected_old, installed);
    let layout = match (
        observe(destination),
        observe(replacement),
        observe(&capture),
    ) {
        (Ok(destination), Ok(replacement), Ok(capture)) => ReplaceLayout {
            destination,
            replacement,
            capture,
        },
        observed => {
            return Err(reconciliation_after_call(
                destination,
                &mut journal,
                code,
                format!("artifact observation failed after ReplaceFileW: {observed:?}"),
            ));
        }
    };

    let action = classify_replace(call, layout);
    if action == ReconcileAction::Installed {
        return Ok(ReplaceReceipt {
            installed: installed.clone(),
            capture: Some(capture),
            journal: Some(journal),
        });
    }
    let result = match action {
        ReconcileAction::Installed => unreachable!("installed action returned above"),
        ReconcileAction::NoMutation => Err(BackupError::io(
            "replace",
            destination,
            std::io::Error::from_raw_os_error(code as i32),
        )),
        ReconcileAction::RestoreCapture => {
            if let Err(error) = install_new_file_platform(&capture, destination) {
                return Err(reconciliation_after_call(
                    destination,
                    &mut journal,
                    code,
                    format!("restore partial capture failed: {error}"),
                ));
            }
            match fingerprint_file(destination) {
                Ok(actual) if actual == *expected_old => {}
                Ok(actual) => {
                    return Err(reconciliation_after_call(
                        destination,
                        &mut journal,
                        code,
                        format!("restored capture mismatch: {actual:?}"),
                    ));
                }
                Err(error) => {
                    return Err(reconciliation_after_call(
                        destination,
                        &mut journal,
                        code,
                        format!("restored capture observation failed: {error}"),
                    ));
                }
            }
            Err(BackupError::io(
                "replace_partial_recovered",
                destination,
                std::io::Error::from_raw_os_error(code as i32),
            ))
        }
        ReconcileAction::RestoreCaptureAndConflict => {
            if let Err(error) = authorize_rollback(layout.destination, layout.capture) {
                return Err(reconciliation_after_call(
                    destination,
                    &mut journal,
                    code,
                    format!("conflict recovery authorization failed: {error}"),
                ));
            }
            let captured = match fingerprint_file(&capture) {
                Ok(captured) => captured,
                Err(error) => {
                    return Err(reconciliation_after_call(
                        destination,
                        &mut journal,
                        code,
                        format!("captured external fingerprint failed: {error}"),
                    ));
                }
            };
            if !allow_conflict_restore {
                return Err(reconciliation_after_call(
                    destination,
                    &mut journal,
                    code,
                    "bounded conflict recovery detected a second destination race",
                ));
            }
            let recovery = replace_file_windows_transaction(
                destination,
                &capture,
                installed,
                &captured,
                false,
            );
            match recovery {
                Ok(receipt) => {
                    if let Some(path) = finalize_replace_receipt(receipt) {
                        return Err(reconciliation_after_call(
                            destination,
                            &mut journal,
                            code,
                            format!(
                                "authoritative external restore cleanup remains pending: {}",
                                path.display()
                            ),
                        ));
                    }
                }
                Err(error) => {
                    return Err(reconciliation_after_call(
                        destination,
                        &mut journal,
                        code,
                        format!("authoritative external restore failed: {error}"),
                    ));
                }
            }
            Err(BackupError::SourceConflict {
                expected: format!("source_at_preview:{expected_old:?}"),
                actual: format!("source_at_replace:{captured:?}"),
            })
        }
        ReconcileAction::PreserveForManualRecovery => Err(reconciliation_after_call(
            destination,
            &mut journal,
            code,
            format!("unrecognized ReplaceFileW post-state: {layout:?}"),
        )),
    };
    if let Some(guard) = readonly_guard {
        if let Err(rollback) = guard.restore().and_then(|()| sync_parent(destination)) {
            let original = result.expect_err("non-installed reconciliation must return an error");
            return Err(reconciliation_after_call(
                destination,
                &mut journal,
                code,
                format!(
                    "post-call recovery failed: original={original}; readonly_restore={rollback}"
                ),
            ));
        }
    }
    if let Err(error) = update_transaction_phase(&mut journal, TransactionPhase::Resolved)
        .and_then(|()| cleanup_temporary(&journal.path))
    {
        return Err(reconciliation_after_call(
            destination,
            &mut journal,
            code,
            format!("resolved transaction durability failed: {error}"),
        ));
    }
    result
}

#[cfg(windows)]
fn clear_readonly_for_replace(
    path: &Path,
    expected: &FileFingerprint,
) -> Result<Option<WindowsReadonlyGuard>, BackupError> {
    use std::os::windows::fs::OpenOptionsExt;

    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0000_0100;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let file = OpenOptions::new()
        .access_mode(GENERIC_READ | FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| BackupError::io("open_readonly_guard", path, error))?;
    verify_handle_path_identity(&file, path)?;
    let identity = windows_handle_identity(&file, path)?;
    let actual = FileFingerprint {
        sha256: hash_file(path)?,
        identity,
    };
    verify_handle_path_identity(&file, path)?;
    if &actual != expected {
        return Err(BackupError::SourceConflict {
            expected: format!("readonly_source:{expected:?}"),
            actual: format!("readonly_source:{actual:?}"),
        });
    }
    let original = get_windows_basic_info(&file, path)?;
    if original.file_attributes & FILE_ATTRIBUTE_READONLY == 0 {
        return Ok(None);
    }
    let mut writable = original;
    writable.file_attributes &= !FILE_ATTRIBUTE_READONLY;
    if writable.file_attributes == 0 {
        writable.file_attributes = FILE_ATTRIBUTE_NORMAL;
    }
    set_windows_basic_info(&file, path, &writable)?;
    if let Err(error) = file.sync_all() {
        let original_error = BackupError::io("flush_readonly_clear", path, error);
        return match set_windows_basic_info(&file, path, &original).and_then(|()| {
            file.sync_all().map_err(|error| {
                BackupError::io("flush_readonly_restore_after_clear_failure", path, error)
            })
        }) {
            Ok(()) => Err(original_error),
            Err(rollback) => Err(BackupError::Unrecoverable {
                original: Box::new(original_error),
                rollback: Box::new(rollback),
            }),
        };
    }
    verify_handle_path_identity(&file, path)?;
    if let Err(original_error) = sync_parent(path) {
        return match set_windows_basic_info(&file, path, &original) {
            Ok(()) => Err(original_error),
            Err(rollback) => Err(BackupError::Unrecoverable {
                original: Box::new(original_error),
                rollback: Box::new(rollback),
            }),
        };
    }
    Ok(Some(WindowsReadonlyGuard {
        file,
        path: path.to_path_buf(),
        original,
        expected_identity: identity,
    }))
}

#[cfg(windows)]
fn observe_artifact(
    path: &Path,
    expected_old: &FileFingerprint,
    installed: &FileFingerprint,
) -> Result<ObservedArtifact, BackupError> {
    match fingerprint_file(path) {
        Ok(actual) if actual == *expected_old => Ok(ObservedArtifact::ExpectedOld),
        Ok(actual) if actual == *installed => Ok(ObservedArtifact::AppInstalled),
        Ok(_) => Ok(ObservedArtifact::External),
        Err(BackupError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(ObservedArtifact::Missing)
        }
        Err(error) => Err(error),
    }
}

fn replace_file_simple(destination: &Path, replacement: &Path) -> Result<(), BackupError> {
    replace_file_platform(destination, replacement)
        .map_err(|error| BackupError::io("replace", destination, error))
}

#[cfg(not(windows))]
fn restore_capture_platform(capture: &Path, destination: &Path) -> Result<(), BackupError> {
    replace_file_simple(destination, capture)
}

#[cfg(not(windows))]
fn replace_file_platform(destination: &Path, replacement: &Path) -> std::io::Result<()> {
    fs::rename(replacement, destination)
}

#[cfg(windows)]
fn replace_file_platform(destination: &Path, replacement: &Path) -> std::io::Result<()> {
    move_file_replace_write_through(replacement, destination)
}

#[cfg(windows)]
fn move_file_replace_write_through(existing: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new_name: *const u16, flags: u32) -> i32;
    }
    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }
    let existing = wide(existing);
    let destination = wide(destination);
    // SAFETY: both strings are NUL-terminated and remain alive for the call.
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<(), BackupError> {
    let parent = path.parent().ok_or_else(|| BackupError::InvalidPath {
        path: path.to_path_buf(),
        reason: "path_has_no_parent",
    })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| BackupError::io("sync_parent", parent, error))
}

#[cfg(windows)]
fn sync_parent(path: &Path) -> Result<(), BackupError> {
    use std::os::windows::ffi::OsStrExt;

    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const OPEN_EXISTING: u32 = 3;
    #[link(name = "Kernel32")]
    extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *mut std::ffi::c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
        fn FlushFileBuffers(file: *mut std::ffi::c_void) -> i32;
        fn CloseHandle(object: *mut std::ffi::c_void) -> i32;
    }
    let parent = path.parent().ok_or_else(|| BackupError::InvalidPath {
        path: path.to_path_buf(),
        reason: "path_has_no_parent",
    })?;
    let wide: Vec<u16> = parent.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: the path is NUL-terminated and all pointer arguments follow CreateFileW's contract.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle as isize == -1 {
        return Err(BackupError::DurabilityUnavailable {
            path: parent.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: handle is valid until CloseHandle below.
    let flushed = unsafe { FlushFileBuffers(handle) };
    let flush_error = (flushed == 0).then(std::io::Error::last_os_error);
    // SAFETY: handle was returned by CreateFileW and is closed exactly once.
    unsafe { CloseHandle(handle) };
    if let Some(source) = flush_error {
        Err(BackupError::DurabilityUnavailable {
            path: parent.to_path_buf(),
            source,
        })
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn apply_attributes(
    file: &File,
    path: &Path,
    attributes: &OriginalAttributes,
) -> Result<(), BackupError> {
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;
    const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0000_0020;
    const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x0000_0100;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: u32 = 0x0000_2000;
    const SUPPORTED: u32 = FILE_ATTRIBUTE_READONLY
        | FILE_ATTRIBUTE_HIDDEN
        | FILE_ATTRIBUTE_SYSTEM
        | FILE_ATTRIBUTE_ARCHIVE
        | FILE_ATTRIBUTE_TEMPORARY
        | FILE_ATTRIBUTE_OFFLINE
        | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED;

    let mut info = get_windows_basic_info(file, path)?;
    info.file_attributes = match attributes.windows_file_attributes {
        Some(original) => {
            let supported = original & SUPPORTED;
            if supported == 0 {
                FILE_ATTRIBUTE_NORMAL
            } else {
                supported
            }
        }
        None if attributes.readonly => {
            (info.file_attributes & !FILE_ATTRIBUTE_NORMAL) | FILE_ATTRIBUTE_READONLY
        }
        None => {
            let writable = info.file_attributes & !FILE_ATTRIBUTE_READONLY;
            if writable == 0 {
                FILE_ATTRIBUTE_NORMAL
            } else {
                writable
            }
        }
    };
    set_windows_basic_info(file, path, &info)?;
    Ok(())
}

#[cfg(not(windows))]
fn apply_attributes(
    file: &File,
    path: &Path,
    attributes: &OriginalAttributes,
) -> Result<(), BackupError> {
    let mut permissions = file
        .metadata()
        .map_err(|error| BackupError::io("metadata", path, error))?
        .permissions();
    permissions.set_readonly(attributes.readonly);
    file.set_permissions(permissions)
        .map_err(|error| BackupError::io("set_permissions", path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postwrite_error_rolls_back_to_the_atomic_capture() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        let rollback_path = directory.path().join("operation-backup.ini");
        fs::write(&destination, b"before").unwrap();
        fs::write(&rollback_path, b"before").unwrap();
        let attributes = OriginalAttributes {
            readonly: false,
            windows_file_attributes: None,
        };
        let rollback = RollbackSource::Backup {
            path: rollback_path,
            sha256: sha256_hex(b"before"),
            identity: fingerprint_file(&destination).unwrap().identity,
            attributes: attributes.clone(),
        };

        let result = write_verified_with_post_replace(
            &destination,
            b"after",
            &attributes,
            rollback,
            |_path| Err(std::io::Error::other("injected postwrite failure")),
        );

        assert!(matches!(result, Err(BackupError::Io { .. })));
        assert_eq!(fs::read(&destination).unwrap(), b"before");
        write_verified(&destination, b"next", &attributes)
            .expect("resolved rollback journals must not block the next mutation");
        assert_eq!(fs::read(&destination).unwrap(), b"next");
    }

    #[test]
    fn successful_write_restores_readonly_attribute() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        fs::write(&destination, b"before").unwrap();
        let attributes = OriginalAttributes {
            readonly: true,
            windows_file_attributes: None,
        };

        write_verified(&destination, b"after", &attributes).unwrap();

        assert!(fs::metadata(&destination).unwrap().permissions().readonly());
    }

    #[test]
    fn capture_rollback_restores_readonly_attribute() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        let capture = directory
            .path()
            .join(format!(".Engine.ini.{}.capture", uuid::Uuid::new_v4()));
        let rollback_path = directory.path().join("operation-backup.ini");
        fs::write(&destination, b"after").unwrap();
        fs::write(&capture, b"before").unwrap();
        fs::write(&rollback_path, b"before").unwrap();
        let expected_old = fingerprint_file(&capture).unwrap();
        let rollback = RollbackSource::Backup {
            path: rollback_path,
            sha256: expected_old.sha256.clone(),
            identity: expected_old.identity,
            attributes: OriginalAttributes {
                readonly: true,
                windows_file_attributes: None,
            },
        };
        let receipt = ReplaceReceipt {
            installed: fingerprint_file(&destination).unwrap(),
            capture: Some(capture),
            #[cfg(windows)]
            journal: None,
        };

        rollback_destination(&destination, &rollback, receipt).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"before");
        assert!(fs::metadata(&destination).unwrap().permissions().readonly());
    }

    #[test]
    fn external_postwrite_change_is_never_overwritten_by_rollback() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        let rollback_path = directory.path().join("operation-backup.ini");
        fs::write(&destination, b"before").unwrap();
        fs::write(&rollback_path, b"before").unwrap();
        let attributes = OriginalAttributes {
            readonly: false,
            windows_file_attributes: None,
        };
        let rollback = RollbackSource::Backup {
            path: rollback_path,
            sha256: sha256_hex(b"before"),
            identity: fingerprint_file(&destination).unwrap().identity,
            attributes: attributes.clone(),
        };

        let result = write_verified_with_post_replace(
            &destination,
            b"after",
            &attributes,
            rollback,
            |path| fs::write(path, b"external"),
        );

        assert!(matches!(
            result,
            Err(BackupError::Unrecoverable { rollback, .. })
                if matches!(*rollback, BackupError::SourceConflict { .. })
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"external");
    }

    #[test]
    fn preserves_primary_and_conflict_errors_when_external_change_blocks_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        let rollback_path = directory.path().join("operation-backup.ini");
        fs::write(&destination, b"before").unwrap();
        fs::write(&rollback_path, b"before").unwrap();
        let attributes = OriginalAttributes {
            readonly: false,
            windows_file_attributes: None,
        };
        let rollback = RollbackSource::Backup {
            path: rollback_path.clone(),
            sha256: sha256_hex(b"before"),
            identity: fingerprint_file(&destination).unwrap().identity,
            attributes: attributes.clone(),
        };

        let result = write_verified_with_post_replace(
            &destination,
            b"after",
            &attributes,
            rollback,
            |path| {
                fs::remove_file(&rollback_path)?;
                fs::write(path, b"interfered")
            },
        );

        match result {
            Err(BackupError::Unrecoverable { original, rollback }) => {
                assert!(matches!(*original, BackupError::ReadbackMismatch { .. }));
                assert!(matches!(*rollback, BackupError::SourceConflict { .. }));
            }
            other => panic!("expected both errors, got {other:?}"),
        }
        assert_eq!(fs::read(&destination).unwrap(), b"interfered");
    }

    #[test]
    fn missing_install_never_replaces_a_file_that_appeared() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        fs::write(&destination, b"external").unwrap();
        let attributes = OriginalAttributes {
            readonly: false,
            windows_file_attributes: None,
        };

        let result = write_verified_if_missing(&destination, b"restored", &attributes);

        assert!(matches!(result, Err(BackupError::SourceConflict { .. })));
        assert_eq!(fs::read(destination).unwrap(), b"external");
    }

    #[test]
    fn replace_state_machine_covers_documented_windows_post_states() {
        let installed = ReplaceLayout {
            destination: ObservedArtifact::AppInstalled,
            replacement: ObservedArtifact::Missing,
            capture: ObservedArtifact::ExpectedOld,
        };
        let unchanged = ReplaceLayout {
            destination: ObservedArtifact::ExpectedOld,
            replacement: ObservedArtifact::AppInstalled,
            capture: ObservedArtifact::Missing,
        };
        let moved_old_only = ReplaceLayout {
            destination: ObservedArtifact::Missing,
            replacement: ObservedArtifact::AppInstalled,
            capture: ObservedArtifact::ExpectedOld,
        };
        assert_eq!(
            classify_replace(ReplaceCallState::Success, installed),
            ReconcileAction::Installed
        );
        assert_eq!(
            classify_replace(ReplaceCallState::UnableToRemoveReplaced, unchanged),
            ReconcileAction::NoMutation
        );
        assert_eq!(
            classify_replace(ReplaceCallState::UnableToMoveReplacement, unchanged),
            ReconcileAction::NoMutation
        );
        assert_eq!(
            classify_replace(ReplaceCallState::UnableToMoveReplacement2, moved_old_only),
            ReconcileAction::RestoreCapture
        );
        assert_eq!(
            classify_replace(ReplaceCallState::Other(5), unchanged),
            ReconcileAction::NoMutation
        );
    }

    #[test]
    fn replace_state_machine_restores_atomic_capture_on_source_race() {
        let raced = ReplaceLayout {
            destination: ObservedArtifact::AppInstalled,
            replacement: ObservedArtifact::Missing,
            capture: ObservedArtifact::External,
        };

        assert_eq!(
            classify_replace(ReplaceCallState::Success, raced),
            ReconcileAction::RestoreCaptureAndConflict
        );
    }

    #[test]
    fn rollback_refuses_to_overwrite_external_destination() {
        let result = authorize_rollback(ObservedArtifact::External, ObservedArtifact::ExpectedOld);

        assert!(matches!(result, Err(BackupError::SourceConflict { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn scavenger_symlink_swap_cannot_change_target_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join(".Engine.ini.test.tmp");
        let target = directory.path().join("target.ini");
        fs::write(&temporary, b"temporary").unwrap();
        fs::write(&target, b"target").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).unwrap();

        let result = remove_owned_temp_with_hook(&temporary, |path| {
            fs::remove_file(path)?;
            symlink(&target, path)
        });

        assert!(result.is_err());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }

    #[test]
    fn unresolved_journal_prevents_retry_from_deleting_the_only_capture() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        let operation_id = uuid::Uuid::new_v4();
        let capture_name = format!(".Engine.ini.{operation_id}.capture");
        let capture = directory.path().join(&capture_name);
        let journal = directory
            .path()
            .join(format!(".Engine.ini.{operation_id}.journal"));
        fs::write(&capture, b"external-only-copy").unwrap();
        fs::write(
            &journal,
            format!(
                "{{\"schema_version\":1,\"operation_id\":\"{operation_id}\",\"destination_name\":\"Engine.ini\",\"replacement_name\":\".Engine.ini.{operation_id}.tmp\",\"capture_name\":\"{capture_name}\",\"expected_destination_sha256\":\"{}\",\"replacement_sha256\":\"{}\",\"phase\":\"ambiguous\"}}",
                "a".repeat(64),
                "b".repeat(64)
            ),
        )
        .unwrap();

        let result = scavenge_owned_temps(&destination);

        assert!(matches!(
            result,
            Err(BackupError::ReconciliationRequired { .. })
        ));
        assert_eq!(fs::read(capture).unwrap(), b"external-only-copy");
    }

    #[test]
    fn tampered_recovery_journal_path_traversal_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        let operation_id = uuid::Uuid::new_v4();
        let journal = directory
            .path()
            .join(format!(".Engine.ini.{operation_id}.journal"));
        fs::write(
            &journal,
            format!(
                "{{\"schema_version\":1,\"operation_id\":\"{operation_id}\",\"destination_name\":\"Engine.ini\",\"replacement_name\":\".Engine.ini.{operation_id}.tmp\",\"capture_name\":\"../outside.ini\",\"expected_destination_sha256\":\"{}\",\"replacement_sha256\":\"{}\",\"phase\":\"ambiguous\"}}",
                "a".repeat(64),
                "b".repeat(64)
            ),
        )
        .unwrap();

        let result = scavenge_owned_temps(&destination);

        assert!(matches!(
            result,
            Err(BackupError::InvalidPath {
                reason: "recovery_journal_artifact_must_be_a_file_name",
                ..
            })
        ));
    }

    #[test]
    fn truncated_recovery_journal_fails_closed_and_preserves_capture() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        let operation_id = uuid::Uuid::new_v4();
        let capture = directory
            .path()
            .join(format!(".Engine.ini.{operation_id}.capture"));
        let journal = directory
            .path()
            .join(format!(".Engine.ini.{operation_id}.journal"));
        fs::write(&capture, b"only-captured-copy").unwrap();
        fs::write(&journal, b"{\"schema_version\":1").unwrap();

        let result = scavenge_owned_temps(&destination);

        assert!(matches!(
            result,
            Err(BackupError::ReconciliationRequired { .. })
        ));
        assert_eq!(fs::read(capture).unwrap(), b"only-captured-copy");
    }

    #[test]
    fn interrupted_journal_updates_leave_old_or_new_complete_json() {
        for interruption in [
            JournalUpdatePoint::BeforeTempWrite,
            JournalUpdatePoint::AfterTempWrite,
            JournalUpdatePoint::AfterTempSync,
            JournalUpdatePoint::AfterReplace,
            JournalUpdatePoint::AfterParentSync,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let operation_id = uuid::Uuid::new_v4();
            let path = directory
                .path()
                .join(format!(".Engine.ini.{operation_id}.journal"));
            let mut record = TransactionJournal {
                schema_version: TRANSACTION_JOURNAL_SCHEMA_VERSION,
                operation_id: operation_id.to_string(),
                destination_name: "Engine.ini".to_owned(),
                replacement_name: format!(".Engine.ini.{operation_id}.tmp"),
                capture_name: format!(".Engine.ini.{operation_id}.capture"),
                expected_destination_sha256: "a".repeat(64),
                replacement_sha256: "b".repeat(64),
                phase: TransactionPhase::Prepared,
            };
            fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
            record.phase = TransactionPhase::CleanupPending;
            let journal = TransactionJournalHandle {
                path: path.clone(),
                record,
            };

            let result = persist_transaction_journal_with_hook(&journal, false, |point| {
                if point == interruption {
                    Err(std::io::Error::other("injected journal interruption"))
                } else {
                    Ok(())
                }
            });

            assert!(result.is_err());
            let persisted: TransactionJournal =
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert!(matches!(
                persisted.phase,
                TransactionPhase::Prepared | TransactionPhase::CleanupPending
            ));
        }
    }

    #[test]
    fn fingerprint_rejects_path_swapped_after_handle_open() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Engine.ini");
        let external = directory.path().join("external.ini");
        fs::write(&path, b"expected").unwrap();
        fs::write(&external, b"external").unwrap();

        let result = read_fingerprinted_file_with_hook(&path, |opened_path| {
            fs::remove_file(opened_path)?;
            fs::rename(&external, opened_path)
        });

        assert!(matches!(result, Err(BackupError::SourceConflict { .. })));
        assert_eq!(fs::read(path).unwrap(), b"external");
    }

    #[cfg(not(windows))]
    #[test]
    fn committed_write_reports_pending_cleanup_instead_of_failing() {
        let directory = tempfile::tempdir().unwrap();
        let capture = directory
            .path()
            .join(format!(".Engine.ini.{}.capture", uuid::Uuid::new_v4()));
        fs::create_dir(&capture).unwrap();
        let receipt = ReplaceReceipt {
            installed: FileFingerprint {
                sha256: sha256_hex(b"installed"),
                identity: (1, 1),
            },
            capture: Some(capture.clone()),
        };

        let pending = finalize_replace_receipt(receipt);

        assert_eq!(pending, Some(capture));
    }

    #[test]
    fn rollback_cleanup_pending_is_surfaced_with_the_primary_error() {
        let pending = std::env::temp_dir().join("pending-recovery-journal.json");
        let result = finish_failed_write(
            BackupError::io(
                "post_replace",
                "Engine.ini",
                std::io::Error::other("primary"),
            ),
            Ok(vec![pending.clone()]),
        );

        assert!(matches!(
            result,
            Err(BackupError::Unrecoverable { rollback, .. })
                if matches!(rollback.as_ref(), BackupError::CleanupPending { paths } if paths == &vec![pending.clone()])
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn rollback_transfers_capture_ownership_before_nested_replace() {
        let capture = std::env::temp_dir().join("outer-capture.ini");
        let mut receipt = ReplaceReceipt {
            installed: FileFingerprint {
                sha256: sha256_hex(b"installed"),
                identity: (1, 1),
            },
            capture: Some(capture.clone()),
        };

        let transferred = take_outer_capture(&mut receipt);

        assert_eq!(transferred, Some(capture));
        assert!(receipt.capture.is_none());
    }

    #[test]
    fn nested_cleanup_warning_still_finishes_durability_and_outer_resolution() {
        let durable = std::cell::Cell::new(false);
        let outer_resolved = std::cell::Cell::new(false);
        let nested_pending = std::path::PathBuf::from("nested-cleanup.json");

        let warnings = complete_capture_rollback_with_finalize(
            vec![nested_pending.clone()],
            || {
                durable.set(true);
                Ok(())
            },
            || {
                outer_resolved.set(true);
                None
            },
        )
        .unwrap();

        assert!(durable.get());
        assert!(outer_resolved.get());
        assert_eq!(warnings, vec![nested_pending]);
    }

    #[test]
    fn journal_update_temp_uses_owned_scavenger_grammar() {
        let directory = tempfile::tempdir().unwrap();
        let operation_id = uuid::Uuid::new_v4();
        let journal = TransactionJournalHandle {
            path: directory
                .path()
                .join(format!(".Engine.ini.{operation_id}.journal")),
            record: TransactionJournal {
                schema_version: TRANSACTION_JOURNAL_SCHEMA_VERSION,
                operation_id: operation_id.to_string(),
                destination_name: "Engine.ini".to_owned(),
                replacement_name: format!(".Engine.ini.{operation_id}.tmp"),
                capture_name: format!(".Engine.ini.{operation_id}.capture"),
                expected_destination_sha256: "a".repeat(64),
                replacement_sha256: "b".repeat(64),
                phase: TransactionPhase::Prepared,
            },
        };

        let temporary = journal_update_temporary_path(&journal).unwrap();
        let name = temporary.file_name().unwrap().to_str().unwrap();
        let suffix = name
            .strip_prefix(".Engine.ini.")
            .and_then(|name| name.strip_suffix(".tmp"))
            .expect("journal temp must match the owned scavenger prefix");

        assert!(uuid::Uuid::parse_str(suffix).is_ok_and(|parsed| parsed.to_string() == suffix));
    }

    #[test]
    fn crash_residue_from_synced_journal_temp_is_scavenged_without_foreign_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Engine.ini");
        let operation_id = uuid::Uuid::new_v4();
        let journal_path = directory
            .path()
            .join(format!(".Engine.ini.{operation_id}.journal"));
        let journal = TransactionJournalHandle {
            path: journal_path.clone(),
            record: TransactionJournal {
                schema_version: TRANSACTION_JOURNAL_SCHEMA_VERSION,
                operation_id: operation_id.to_string(),
                destination_name: "Engine.ini".to_owned(),
                replacement_name: format!(".Engine.ini.{operation_id}.tmp"),
                capture_name: format!(".Engine.ini.{operation_id}.capture"),
                expected_destination_sha256: "a".repeat(64),
                replacement_sha256: "b".repeat(64),
                phase: TransactionPhase::Resolved,
            },
        };
        fs::write(&journal_path, serde_json::to_vec(&journal.record).unwrap()).unwrap();
        let generation_temp = journal_update_temporary_path(&journal).unwrap();
        fs::write(&generation_temp, b"complete-new-generation").unwrap();
        let foreign = directory.path().join(".Engine.ini.not-a-uuid.tmp");
        fs::write(&foreign, b"foreign").unwrap();

        scavenge_owned_temps(&destination).unwrap();

        assert!(!generation_temp.exists());
        assert_eq!(fs::read(foreign).unwrap(), b"foreign");
    }

    #[test]
    fn verified_write_installs_a_previously_missing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("metadata.json");
        let attributes = OriginalAttributes {
            readonly: false,
            windows_file_attributes: None,
        };

        let result = write_verified(&destination, b"{}", &attributes).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"{}");
        assert_eq!(result.sha256, sha256_hex(b"{}"));
    }
}
