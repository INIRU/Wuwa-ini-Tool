use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::FocusError;

const FOCUS_EXCLUSION_SCHEMA_VERSION: u32 = 1;
const MAX_FOCUS_EXCLUSION_BYTES: u64 = 64 * 1024;
const MAX_PINNED_EXECUTABLES: usize = 256;

pub trait FocusExclusionStore {
    fn load(&self) -> Result<BTreeSet<PathBuf>, FocusError>;
    fn save(&mut self, executables: &BTreeSet<PathBuf>) -> Result<(), FocusError>;
}

#[derive(Clone, Debug)]
pub struct FileFocusExclusionStore {
    root: PathBuf,
    path: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredFocusExclusions {
    schema_version: u32,
    pinned_executables: Vec<PathBuf>,
}

impl FileFocusExclusionStore {
    pub fn new(app_data_dir: impl Into<PathBuf>) -> Self {
        let root = app_data_dir.into();
        let path = root.join("focus-exclusions.json");
        Self { root, path }
    }
}

impl FocusExclusionStore for FileFocusExclusionStore {
    fn load(&self) -> Result<BTreeSet<PathBuf>, FocusError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeSet::new())
            }
            Err(_) => return Err(FocusError::ConfigFailure),
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_FOCUS_EXCLUSION_BYTES
        {
            return Err(FocusError::ConfigFailure);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(&self.path)
            .map_err(|_| FocusError::ConfigFailure)?
            .take(MAX_FOCUS_EXCLUSION_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| FocusError::ConfigFailure)?;
        if bytes.len() as u64 > MAX_FOCUS_EXCLUSION_BYTES {
            return Err(FocusError::ConfigFailure);
        }
        let stored = serde_json::from_slice::<StoredFocusExclusions>(&bytes)
            .map_err(|_| FocusError::ConfigFailure)?;
        validate_stored(&stored)
    }

    fn save(&mut self, executables: &BTreeSet<PathBuf>) -> Result<(), FocusError> {
        let stored = StoredFocusExclusions {
            schema_version: FOCUS_EXCLUSION_SCHEMA_VERSION,
            pinned_executables: executables.iter().cloned().collect(),
        };
        validate_stored(&stored)?;
        let bytes = serde_json::to_vec_pretty(&stored).map_err(|_| FocusError::ConfigFailure)?;
        if bytes.len() as u64 > MAX_FOCUS_EXCLUSION_BYTES {
            return Err(FocusError::ConfigFailure);
        }
        fs::create_dir_all(&self.root).map_err(|_| FocusError::ConfigFailure)?;
        let root_metadata =
            fs::symlink_metadata(&self.root).map_err(|_| FocusError::ConfigFailure)?;
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Err(FocusError::ConfigFailure);
        }
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err(FocusError::ConfigFailure);
        }
        let temporary = self
            .root
            .join(format!(".focus-exclusions.{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| FocusError::ConfigFailure)?;
        let result = (|| {
            file.write_all(&bytes)
                .map_err(|_| FocusError::ConfigFailure)?;
            file.sync_all().map_err(|_| FocusError::ConfigFailure)?;
            replace_config(&temporary, &self.path)?;
            sync_parent(&self.root)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn validate_stored(stored: &StoredFocusExclusions) -> Result<BTreeSet<PathBuf>, FocusError> {
    if stored.schema_version != FOCUS_EXCLUSION_SCHEMA_VERSION
        || stored.pinned_executables.len() > MAX_PINNED_EXECUTABLES
    {
        return Err(FocusError::ConfigFailure);
    }
    let mut unique = BTreeSet::new();
    for executable in &stored.pinned_executables {
        if !valid_executable_path(executable) || !unique.insert(executable.clone()) {
            return Err(FocusError::ConfigFailure);
        }
    }
    Ok(unique)
}

pub(crate) fn valid_executable_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                !name.is_empty()
                    && name.len() <= 32_768
                    && !name.chars().any(char::is_control)
                    && name.to_ascii_lowercase().ends_with(".exe")
            })
}

#[cfg(not(target_os = "windows"))]
fn replace_config(temporary: &Path, destination: &Path) -> Result<(), FocusError> {
    fs::rename(temporary, destination).map_err(|_| FocusError::ConfigFailure)
}

#[cfg(target_os = "windows")]
fn replace_config(temporary: &Path, destination: &Path) -> Result<(), FocusError> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let existing = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replacement = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    (moved != 0).then_some(()).ok_or(FocusError::ConfigFailure)
}

fn sync_parent(parent: &Path) -> Result<(), FocusError> {
    #[cfg(target_os = "windows")]
    {
        let _ = parent;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| FocusError::ConfigFailure)
    }
}
