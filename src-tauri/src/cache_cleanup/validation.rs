use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::game_discovery::validate_game_executable;
use sha2::{Digest, Sha256};

use super::{CacheCleanupError, CacheRootKind, CleanupRootPreview, CleanupSelection};

const MAX_SCANNED_ENTRIES: u64 = 1_000_000;

#[derive(Clone, Debug)]
pub(crate) struct RootSpec {
    pub(crate) kind: CacheRootKind,
    pub(crate) boundary: PathBuf,
    pub(crate) path: PathBuf,
}

pub(crate) struct RootScan {
    pub(crate) preview: CleanupRootPreview,
    pub(crate) fingerprint: String,
}

pub(crate) fn derive_roots(
    executable: &Path,
    local_app_data: &Path,
    selection: CleanupSelection,
) -> Result<(PathBuf, Vec<RootSpec>), CacheCleanupError> {
    if !selection.wuwa && !selection.nvidia {
        return Err(CacheCleanupError::EmptySelection);
    }
    let installation = validate_game_executable(executable)
        .map_err(|_| CacheCleanupError::InvalidGameInstallation)?;
    let canonical_local = validate_boundary(local_app_data)?;
    let mut roots = Vec::with_capacity(5);
    if selection.wuwa {
        let saved = installation.game_root.join("Client/Saved");
        roots.push(RootSpec {
            kind: CacheRootKind::WuwaPso,
            boundary: installation.game_root.clone(),
            path: saved.join("PSO"),
        });
        roots.push(RootSpec {
            kind: CacheRootKind::WuwaPsoReport,
            boundary: installation.game_root,
            path: saved.join("PSOReport"),
        });
    }
    if selection.nvidia {
        roots.push(RootSpec {
            kind: CacheRootKind::NvidiaDxCache,
            boundary: canonical_local.clone(),
            path: canonical_local.join("NVIDIA/DXCache"),
        });
        roots.push(RootSpec {
            kind: CacheRootKind::NvidiaGlCache,
            boundary: canonical_local.clone(),
            path: canonical_local.join("NVIDIA/GLCache"),
        });
        roots.push(RootSpec {
            kind: CacheRootKind::NvidiaNvCache,
            boundary: canonical_local.clone(),
            path: canonical_local.join("NVIDIA Corporation/NV_Cache"),
        });
    }
    for root in &roots {
        validate_root(root)?;
    }
    Ok((installation.executable, roots))
}

pub(crate) fn scan_root(root: &RootSpec) -> Result<RootScan, CacheCleanupError> {
    validate_root(root)?;
    let mut preview = CleanupRootPreview {
        kind: root.kind,
        path: root.path.clone(),
        files: 0,
        bytes: 0,
        skipped_entries: 0,
    };
    let mut records = Vec::new();
    if !root.path.exists() {
        return Ok(RootScan {
            preview,
            fingerprint: fingerprint(&[b"missing".to_vec()]),
        });
    }
    let mut stack = vec![root.path.clone()];
    let mut scanned = 0_u64;
    while let Some(directory) = stack.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| CacheCleanupError::io("read_directory", &directory, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                CacheCleanupError::io("read_directory_entry", &directory, error)
            })?;
            scanned = scanned.saturating_add(1);
            if scanned > MAX_SCANNED_ENTRIES {
                return Err(CacheCleanupError::ScanLimitExceeded);
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| CacheCleanupError::io("entry_metadata", &path, error))?;
            let relative = path
                .strip_prefix(&root.path)
                .map_err(|_| CacheCleanupError::UnsafePath(path.clone()))?;
            if is_alias(&metadata) {
                preview.skipped_entries = preview.skipped_entries.saturating_add(1);
                records.push(metadata_record(relative, &metadata));
            } else if metadata.is_dir() {
                records.push(metadata_record(relative, &metadata));
                stack.push(path);
            } else if metadata.is_file() {
                preview.files = preview.files.saturating_add(1);
                preview.bytes = preview.bytes.saturating_add(metadata.len());
                records.push(metadata_record(relative, &metadata));
            } else {
                preview.skipped_entries = preview.skipped_entries.saturating_add(1);
                records.push(metadata_record(relative, &metadata));
            }
        }
    }
    Ok(RootScan {
        preview,
        fingerprint: fingerprint(&records),
    })
}

pub(crate) fn fingerprint(records: &[Vec<u8>]) -> String {
    let mut records = records.to_vec();
    records.sort();
    let mut digest = Sha256::new();
    for record in records {
        digest.update((record.len() as u64).to_le_bytes());
        digest.update(record);
    }
    format!("{:x}", digest.finalize())
}

pub(crate) fn metadata_record(relative: &Path, metadata: &fs::Metadata) -> Vec<u8> {
    let relative = relative.to_string_lossy();
    if is_alias(metadata) {
        format!("a|{relative}").into_bytes()
    } else if metadata.is_dir() {
        format!("d|{relative}|{}", modified_nanos(metadata)).into_bytes()
    } else if metadata.is_file() {
        format!(
            "f|{relative}|{}|{}",
            metadata.len(),
            modified_nanos(metadata)
        )
        .into_bytes()
    } else {
        format!("o|{relative}").into_bytes()
    }
}

fn modified_nanos(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0_u128, |value| value.as_nanos())
}

pub(crate) fn validate_root(root: &RootSpec) -> Result<(), CacheCleanupError> {
    if !root.path.starts_with(&root.boundary) || root.path == root.boundary {
        return Err(CacheCleanupError::UnsafePath(root.path.clone()));
    }
    let canonical_boundary = validate_boundary(&root.boundary)?;
    let relative = root
        .path
        .strip_prefix(&root.boundary)
        .map_err(|_| CacheCleanupError::UnsafePath(root.path.clone()))?;
    let rebuilt = canonical_boundary.join(relative);
    if rebuilt != root.path && root.boundary != canonical_boundary {
        return Err(CacheCleanupError::UnsafePath(root.path.clone()));
    }
    let mut current = canonical_boundary;
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_alias(&metadata) => {
                return Err(CacheCleanupError::UnsafePath(current));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(CacheCleanupError::UnsafePath(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(CacheCleanupError::io("root_metadata", &current, error)),
        }
    }
    Ok(())
}

fn validate_boundary(path: &Path) -> Result<PathBuf, CacheCleanupError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| CacheCleanupError::io("boundary_metadata", path, error))?;
    if is_alias(&metadata) || !metadata.is_dir() {
        return Err(CacheCleanupError::UnsafePath(path.to_path_buf()));
    }
    path.canonicalize()
        .map_err(|error| CacheCleanupError::io("canonicalize_boundary", path, error))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn is_alias(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(target_os = "windows")]
pub(crate) fn is_alias(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}
