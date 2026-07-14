use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{BackupError, OriginalAttributes};

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
        let metadata = self
            .file
            .metadata()
            .map_err(|error| BackupError::io("readonly_guard_identity", &self.path, error))?;
        if owned_file_identity(&metadata, &self.path)? != self.expected_identity {
            return Err(BackupError::SourceConflict {
                expected: format!("readonly_identity:{:?}", self.expected_identity),
                actual: "readonly_handle_identity_changed".to_owned(),
            });
        }
        set_windows_basic_info(&self.file, &self.path, &self.original)
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
    Missing,
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
) -> Result<String, BackupError> {
    let rollback = if destination.exists() {
        let fingerprint = fingerprint_file(destination)?;
        let previous = fs::read(destination)
            .map_err(|error| BackupError::io("read_rollback_source", destination, error))?;
        RollbackSource::Bytes {
            sha256: sha256_hex(&previous),
            identity: fingerprint.identity,
            bytes: previous,
            attributes: attributes.clone(),
        }
    } else {
        RollbackSource::Missing
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
) -> Result<String, BackupError> {
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
) -> Result<String, BackupError> {
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
    cleanup_temporary(&temporary)?;

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
        Ok(hash) => Ok(hash),
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
) -> Result<String, BackupError>
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
        expected_old.as_ref(),
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
            cleanup_capture(receipt.capture.as_deref())?;
            Ok(hash)
        }
        Err(original) => match rollback_destination(destination, &rollback, &receipt) {
            Ok(()) => Err(original),
            Err(rollback) => Err(BackupError::Unrecoverable {
                original: Box::new(original),
                rollback: Box::new(rollback),
            }),
        },
    }
}

fn rollback_destination(
    destination: &Path,
    rollback: &RollbackSource,
    receipt: &ReplaceReceipt,
) -> Result<(), BackupError> {
    ensure_installed_destination(destination, &receipt.installed)?;
    if let Some(capture) = &receipt.capture {
        let expected =
            expected_fingerprint(rollback).ok_or_else(|| BackupError::SourceConflict {
                expected: "present_atomic_capture".to_owned(),
                actual: "missing_expected_source".to_owned(),
            })?;
        let captured = fingerprint_file(capture)?;
        if captured != expected {
            return Err(BackupError::SourceConflict {
                expected: format!("captured:{expected:?}"),
                actual: format!("captured:{captured:?}"),
            });
        }
        restore_capture_platform(capture, destination)?;
        persist_attributes_and_flush(destination, rollback_attributes(rollback)?)?;
        let restored = fingerprint_file(destination)?;
        if restored != expected {
            return Err(BackupError::ReadbackMismatch {
                path: destination.to_path_buf(),
                expected: expected.sha256,
                actual: restored.sha256,
            });
        }
        return Ok(());
    }
    match rollback {
        RollbackSource::Missing => {
            if destination.exists() {
                delete_file_no_follow(destination, receipt.installed.identity)?;
                sync_parent(destination)?;
            }
            Ok(())
        }
        RollbackSource::Backup {
            path,
            sha256,
            identity: _,
            attributes,
        } => {
            let bytes = fs::read(path)
                .map_err(|error| BackupError::io("read_rollback_backup", path, error))?;
            restore_present(destination, &bytes, sha256, attributes)
        }
        RollbackSource::Bytes {
            bytes,
            sha256,
            identity: _,
            attributes,
        } => restore_present(destination, bytes, sha256, attributes),
    }
}

fn rollback_attributes(rollback: &RollbackSource) -> Result<&OriginalAttributes, BackupError> {
    match rollback {
        RollbackSource::Backup { attributes, .. } | RollbackSource::Bytes { attributes, .. } => {
            Ok(attributes)
        }
        RollbackSource::Missing => Err(BackupError::SourceConflict {
            expected: "present_rollback_attributes".to_owned(),
            actual: "missing_rollback_source".to_owned(),
        }),
    }
}

fn expected_fingerprint(rollback: &RollbackSource) -> Option<FileFingerprint> {
    match rollback {
        RollbackSource::Backup {
            sha256, identity, ..
        }
        | RollbackSource::Bytes {
            sha256, identity, ..
        } => Some(FileFingerprint {
            sha256: sha256.clone(),
            identity: *identity,
        }),
        RollbackSource::Missing => None,
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
        if !matches!(extension, "tmp" | "rollback" | "capture") || !canonical_id {
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
    if !path.exists() {
        return Ok(());
    }
    remove_owned_temp_with_hook(path, |_| Ok(()))
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
fn owned_file_identity(metadata: &fs::Metadata, path: &Path) -> Result<(u64, u64), BackupError> {
    use std::os::windows::fs::MetadataExt;

    let volume = metadata
        .volume_serial_number()
        .ok_or_else(|| BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "temporary_volume_identity_unavailable",
        })?;
    let index = metadata
        .file_index()
        .ok_or_else(|| BackupError::InvalidPath {
            path: path.to_path_buf(),
            reason: "temporary_file_identity_unavailable",
        })?;
    Ok((u64::from(volume), index))
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
    let bytes = fs::read(path).map_err(|error| BackupError::io("read", path, error))?;
    Ok(sha256_hex(&bytes))
}

fn fingerprint_file(path: &Path) -> Result<FileFingerprint, BackupError> {
    let metadata =
        fs::metadata(path).map_err(|error| BackupError::io("fingerprint_metadata", path, error))?;
    Ok(FileFingerprint {
        sha256: hash_file(path)?,
        identity: owned_file_identity(&metadata, path)?,
    })
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
    let handle_metadata = file
        .metadata()
        .map_err(|error| BackupError::io("handle_identity", path, error))?;
    let path_metadata =
        fs::metadata(path).map_err(|error| BackupError::io("path_identity", path, error))?;
    let handle_identity = owned_file_identity(&handle_metadata, path)?;
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

    let expected_old = expected_old.ok_or_else(|| BackupError::SourceConflict {
        expected: "present_destination".to_owned(),
        actual: "missing_expected_fingerprint".to_owned(),
    })?;
    let capture = owned_temporary_path(destination, "capture")?;
    let readonly_guard = clear_readonly_for_replace(destination, expected_old)?;
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
    let layout = ReplaceLayout {
        destination: observe_artifact(destination, expected_old, installed)?,
        replacement: observe_artifact(replacement, expected_old, installed)?,
        capture: observe_artifact(&capture, expected_old, installed)?,
    };

    let action = classify_replace(call, layout);
    if action == ReconcileAction::Installed {
        return Ok(ReplaceReceipt {
            installed: installed.clone(),
            capture: Some(capture),
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
            install_new_file_platform(&capture, destination)
                .map_err(|error| BackupError::io("restore_partial_capture", destination, error))?;
            if fingerprint_file(destination)? != *expected_old {
                return Err(BackupError::ReconciliationRequired {
                    code,
                    destination: destination.to_path_buf(),
                    replacement: replacement.to_path_buf(),
                    capture,
                });
            }
            Err(BackupError::io(
                "replace_partial_recovered",
                destination,
                std::io::Error::from_raw_os_error(code as i32),
            ))
        }
        ReconcileAction::RestoreCaptureAndConflict => {
            authorize_rollback(layout.destination, layout.capture)?;
            let captured = fingerprint_file(&capture)?;
            restore_capture_platform(&capture, destination)?;
            if fingerprint_file(destination)? != captured {
                return Err(BackupError::ReconciliationRequired {
                    code,
                    destination: destination.to_path_buf(),
                    replacement: replacement.to_path_buf(),
                    capture,
                });
            }
            Err(BackupError::SourceConflict {
                expected: format!("source_at_preview:{expected_old:?}"),
                actual: format!("source_at_replace:{captured:?}"),
            })
        }
        ReconcileAction::PreserveForManualRecovery => Err(BackupError::ReconciliationRequired {
            code,
            destination: destination.to_path_buf(),
            replacement: replacement.to_path_buf(),
            capture,
        }),
    };
    if let Some(guard) = readonly_guard {
        if let Err(rollback) = guard.restore().and_then(|()| sync_parent(destination)) {
            let original = result.expect_err("non-installed reconciliation must return an error");
            return Err(BackupError::Unrecoverable {
                original: Box::new(original),
                rollback: Box::new(rollback),
            });
        }
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
    let metadata = file
        .metadata()
        .map_err(|error| BackupError::io("readonly_guard_metadata", path, error))?;
    let identity = owned_file_identity(&metadata, path)?;
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

#[cfg(windows)]
fn restore_capture_platform(capture: &Path, destination: &Path) -> Result<(), BackupError> {
    move_file_replace_write_through(capture, destination)
        .map_err(|error| BackupError::io("restore_atomic_capture", destination, error))
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

    if let Some(original) = attributes.windows_file_attributes {
        let supported = original & SUPPORTED;
        let file_attributes = if supported == 0 {
            FILE_ATTRIBUTE_NORMAL
        } else {
            supported
        };
        let mut info = get_windows_basic_info(file, path)?;
        info.file_attributes = file_attributes;
        set_windows_basic_info(file, path, &info)?;
    }
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
        };

        rollback_destination(&destination, &rollback, &receipt).unwrap();

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
}
