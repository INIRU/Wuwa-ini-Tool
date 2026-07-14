use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{BackupError, OriginalAttributes};

#[derive(Clone, Debug)]
pub(crate) enum RollbackSource {
    Backup {
        path: PathBuf,
        sha256: String,
        attributes: OriginalAttributes,
    },
    Bytes {
        bytes: Vec<u8>,
        sha256: String,
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
        let previous = fs::read(destination)
            .map_err(|error| BackupError::io("read_rollback_source", destination, error))?;
        RollbackSource::Bytes {
            sha256: sha256_hex(&previous),
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
    backup_attributes: &OriginalAttributes,
) -> Result<String, BackupError> {
    write_verified_with_post_replace(
        destination,
        bytes,
        attributes,
        RollbackSource::Backup {
            path: backup_path.to_path_buf(),
            sha256: backup_sha256.to_owned(),
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
        Err(original) => match rollback_installed_file(destination, &written_hash) {
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

    if let Err(error) = replace_file(destination, &temporary) {
        cleanup_temporary(&temporary)?;
        return Err(error);
    }

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
        Ok(hash) => Ok(hash),
        Err(original) => match rollback_destination(destination, &rollback) {
            Ok(()) => Err(original),
            Err(rollback) => Err(BackupError::Unrecoverable {
                original: Box::new(original),
                rollback: Box::new(rollback),
            }),
        },
    }
}

fn rollback_destination(destination: &Path, rollback: &RollbackSource) -> Result<(), BackupError> {
    match rollback {
        RollbackSource::Missing => {
            if destination.exists() {
                prepare_for_delete(destination)?;
                fs::remove_file(destination)
                    .map_err(|error| BackupError::io("rollback_remove", destination, error))?;
                sync_parent(destination)?;
            }
            Ok(())
        }
        RollbackSource::Backup {
            path,
            sha256,
            attributes,
        } => {
            let bytes = fs::read(path)
                .map_err(|error| BackupError::io("read_rollback_backup", path, error))?;
            restore_present(destination, &bytes, sha256, attributes)
        }
        RollbackSource::Bytes {
            bytes,
            sha256,
            attributes,
        } => restore_present(destination, bytes, sha256, attributes),
    }
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
    if let Err(error) = replace_file(destination, &temporary) {
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
        if !matches!(extension, "tmp" | "rollback") || !canonical_id {
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
        prepare_for_delete(&path)?;
        fs::remove_file(&path)
            .map_err(|error| BackupError::io("remove_stale_temporary", &path, error))?;
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
    prepare_for_delete(path)?;
    fs::remove_file(path).map_err(|error| BackupError::io("cleanup_temporary", path, error))
}

fn prepare_for_delete(path: &Path) -> Result<(), BackupError> {
    prepare_for_delete_platform(path)
}

#[cfg(unix)]
fn prepare_for_delete_platform(path: &Path) -> Result<(), BackupError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata =
        fs::metadata(path).map_err(|error| BackupError::io("cleanup_metadata", path, error))?;
    let mode = metadata.mode();
    if mode & 0o200 == 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o200))
            .map_err(|error| BackupError::io("cleanup_permissions", path, error))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_for_delete_platform(path: &Path) -> Result<(), BackupError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| BackupError::io("cleanup_metadata", path, error))?
        .permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)
            .map_err(|error| BackupError::io("cleanup_permissions", path, error))?;
    }
    Ok(())
}

pub(crate) fn hash_file(path: &Path) -> Result<String, BackupError> {
    let bytes = fs::read(path).map_err(|error| BackupError::io("read", path, error))?;
    Ok(sha256_hex(&bytes))
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
    file.sync_all()
        .map_err(|error| BackupError::io("flush_destination", path, error))?;
    apply_attributes(path, attributes)?;
    file.sync_all()
        .map_err(|error| BackupError::io("flush_destination_attributes", path, error))?;
    sync_parent(path)
}

fn rollback_installed_file(destination: &Path, installed_hash: &str) -> Result<(), BackupError> {
    let actual = hash_file(destination)?;
    if actual != installed_hash {
        return Err(BackupError::SourceConflict {
            expected: installed_hash.to_owned(),
            actual,
        });
    }
    prepare_for_delete(destination)?;
    fs::remove_file(destination)
        .map_err(|error| BackupError::io("rollback_remove_installed", destination, error))?;
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

fn replace_file(destination: &Path, replacement: &Path) -> Result<(), BackupError> {
    replace_file_platform(destination, replacement)
        .map_err(|error| BackupError::io("replace", destination, error))
}

#[cfg(not(windows))]
fn replace_file_platform(destination: &Path, replacement: &Path) -> std::io::Result<()> {
    fs::rename(replacement, destination)
}

#[cfg(windows)]
fn replace_file_platform(destination: &Path, replacement: &Path) -> std::io::Result<()> {
    use std::{os::windows::ffi::OsStrExt, ptr};

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

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
        fn MoveFileExW(existing: *const u16, new_name: *const u16, flags: u32) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let destination_wide = wide(destination);
    let replacement_wide = wide(replacement);
    let result = if destination.exists() {
        // Default flags preserve fail-closed ACL and named-stream merge behavior.
        // SAFETY: both strings are NUL-terminated and remain alive for the call.
        unsafe {
            ReplaceFileW(
                destination_wide.as_ptr(),
                replacement_wide.as_ptr(),
                ptr::null(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        }
    } else {
        // SAFETY: both strings are NUL-terminated and remain alive for the call.
        unsafe {
            MoveFileExW(
                replacement_wide.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
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
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    let parent = path.parent().ok_or_else(|| BackupError::InvalidPath {
        path: path.to_path_buf(),
        reason: "path_has_no_parent",
    })?;
    let result = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(parent)
        .and_then(|directory| directory.sync_all());
    match result {
        Ok(()) => Ok(()),
        // Windows does not guarantee that every filesystem flushes directory handles.
        Err(error) if matches!(error.raw_os_error(), Some(5 | 6 | 87)) => Ok(()),
        Err(error) => Err(BackupError::io("sync_parent", parent, error)),
    }
}

#[cfg(windows)]
fn apply_attributes(path: &Path, attributes: &OriginalAttributes) -> Result<(), BackupError> {
    use std::os::windows::ffi::OsStrExt;

    const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;
    const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0000_0020;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
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

    #[link(name = "Kernel32")]
    extern "system" {
        fn SetFileAttributesW(file_name: *const u16, file_attributes: u32) -> i32;
    }

    if let Some(original) = attributes.windows_file_attributes {
        let supported = original & SUPPORTED;
        let file_attributes = if supported == 0 {
            FILE_ATTRIBUTE_NORMAL
        } else {
            supported
        };
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: the path is NUL-terminated and remains alive for the call.
        let result = unsafe { SetFileAttributesW(wide.as_ptr(), file_attributes) };
        if result == 0 {
            return Err(BackupError::io(
                "set_attributes",
                path,
                std::io::Error::last_os_error(),
            ));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn apply_attributes(path: &Path, attributes: &OriginalAttributes) -> Result<(), BackupError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| BackupError::io("metadata", path, error))?
        .permissions();
    permissions.set_readonly(attributes.readonly);
    fs::set_permissions(path, permissions)
        .map_err(|error| BackupError::io("set_permissions", path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_readback_mismatch_after_replacement() {
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
            attributes: attributes.clone(),
        };

        let result = write_verified_with_post_replace(
            &destination,
            b"after",
            &attributes,
            rollback,
            |path| fs::write(path, b"interfered"),
        );

        assert!(matches!(result, Err(BackupError::ReadbackMismatch { .. })));
        assert_eq!(fs::read(&destination).unwrap(), b"before");
    }

    #[test]
    fn preserves_primary_and_rollback_errors_when_recovery_fails() {
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
                assert!(matches!(*rollback, BackupError::Io { .. }));
            }
            other => panic!("expected both errors, got {other:?}"),
        }
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
}
