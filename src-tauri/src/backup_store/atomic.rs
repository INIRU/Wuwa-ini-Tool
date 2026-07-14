use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{BackupError, OriginalAttributes};

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
    write_verified_with_post_replace(destination, bytes, attributes, |_| Ok(()))
}

fn write_verified_with_post_replace<F>(
    destination: &Path,
    bytes: &[u8],
    attributes: &OriginalAttributes,
    post_replace: F,
) -> Result<String, BackupError>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
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
    let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));

    let result = (|| {
        let written_hash = write_new_verified(&temporary, bytes)?;
        apply_attributes(&temporary, attributes)?;
        replace_file(destination, &temporary)?;
        sync_parent(destination)?;
        apply_attributes(destination, attributes)?;
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

    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn hash_file(path: &Path) -> Result<String, BackupError> {
    let bytes = fs::read(path).map_err(|error| BackupError::io("read", path, error))?;
    Ok(sha256_hex(&bytes))
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

    const REPLACEFILE_IGNORE_MERGE_ERRORS: u32 = 0x0000_0002;
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
        // SAFETY: both strings are NUL-terminated and remain alive for the call.
        unsafe {
            ReplaceFileW(
                destination_wide.as_ptr(),
                replacement_wide.as_ptr(),
                ptr::null(),
                REPLACEFILE_IGNORE_MERGE_ERRORS,
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

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<(), BackupError> {
    Ok(())
}

#[cfg(windows)]
fn apply_attributes(path: &Path, attributes: &OriginalAttributes) -> Result<(), BackupError> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "Kernel32")]
    extern "system" {
        fn SetFileAttributesW(file_name: *const u16, file_attributes: u32) -> i32;
    }

    if let Some(file_attributes) = attributes.windows_file_attributes {
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
        std::fs::write(&destination, b"before").unwrap();
        let attributes = OriginalAttributes {
            readonly: false,
            windows_file_attributes: None,
        };

        let result =
            write_verified_with_post_replace(&destination, b"after", &attributes, |path| {
                std::fs::write(path, b"interfered")
            });

        assert!(matches!(result, Err(BackupError::ReadbackMismatch { .. })));
    }
}
