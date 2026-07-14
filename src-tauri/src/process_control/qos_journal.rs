use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{GameQosRestoreRecord, ProcessError};

const QOS_JOURNAL_SCHEMA_VERSION: u32 = 1;
const MAX_QOS_JOURNAL_BYTES: u64 = 64 * 1024;

pub trait GameQosJournalStore {
    fn load(&self) -> Result<Option<GameQosRestoreRecord>, ProcessError>;
    fn save(&mut self, record: &GameQosRestoreRecord) -> Result<(), ProcessError>;
    fn clear(&mut self) -> Result<(), ProcessError>;
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GameQosJournal {
    schema_version: u32,
    record: GameQosRestoreRecord,
}

#[derive(Clone, Debug)]
pub struct FileGameQosJournalStore {
    path: PathBuf,
}

impl FileGameQosJournalStore {
    pub fn new(app_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: app_data_dir.into().join("game-qos-journal.json"),
        }
    }
}

impl GameQosJournalStore for FileGameQosJournalStore {
    fn load(&self) -> Result<Option<GameQosRestoreRecord>, ProcessError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ProcessError::JournalFailure),
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_QOS_JOURNAL_BYTES
        {
            return Err(ProcessError::JournalFailure);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(&self.path)
            .map_err(|_| ProcessError::JournalFailure)?
            .take(MAX_QOS_JOURNAL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ProcessError::JournalFailure)?;
        if bytes.len() as u64 > MAX_QOS_JOURNAL_BYTES {
            return Err(ProcessError::JournalFailure);
        }
        let journal = serde_json::from_slice::<GameQosJournal>(&bytes)
            .map_err(|_| ProcessError::JournalFailure)?;
        validate_journal(&journal)?;
        Ok(Some(journal.record))
    }

    fn save(&mut self, record: &GameQosRestoreRecord) -> Result<(), ProcessError> {
        let journal = GameQosJournal {
            schema_version: QOS_JOURNAL_SCHEMA_VERSION,
            record: record.clone(),
        };
        validate_journal(&journal)?;
        let bytes = serde_json::to_vec(&journal).map_err(|_| ProcessError::JournalFailure)?;
        if bytes.len() as u64 > MAX_QOS_JOURNAL_BYTES {
            return Err(ProcessError::JournalFailure);
        }
        let parent = self.path.parent().ok_or(ProcessError::JournalFailure)?;
        fs::create_dir_all(parent).map_err(|_| ProcessError::JournalFailure)?;
        let temporary = parent.join(format!(".game-qos-journal.{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| ProcessError::JournalFailure)?;
        let result = (|| {
            file.write_all(&bytes)
                .map_err(|_| ProcessError::JournalFailure)?;
            file.sync_all().map_err(|_| ProcessError::JournalFailure)?;
            replace_journal(&temporary, &self.path)?;
            sync_parent(parent)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn clear(&mut self) -> Result<(), ProcessError> {
        match fs::remove_file(&self.path) {
            Ok(()) => self
                .path
                .parent()
                .ok_or(ProcessError::JournalFailure)
                .and_then(sync_parent),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ProcessError::JournalFailure),
        }
    }
}

fn validate_journal(journal: &GameQosJournal) -> Result<(), ProcessError> {
    let record = &journal.record;
    if journal.schema_version != QOS_JOURNAL_SCHEMA_VERSION
        || record.pid == 0
        || record.creation_time_100ns == 0
        || record.canonical_image.as_os_str().is_empty()
        || !record.prior.execution_speed_throttled
        || record.applied.execution_speed_throttled
    {
        return Err(ProcessError::JournalFailure);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_journal(temporary: &Path, destination: &Path) -> Result<(), ProcessError> {
    fs::rename(temporary, destination).map_err(|_| ProcessError::JournalFailure)
}

#[cfg(target_os = "windows")]
fn replace_journal(temporary: &Path, destination: &Path) -> Result<(), ProcessError> {
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
    (moved != 0)
        .then_some(())
        .ok_or(ProcessError::JournalFailure)
}

fn sync_parent(parent: &Path) -> Result<(), ProcessError> {
    #[cfg(target_os = "windows")]
    {
        let _ = parent;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ProcessError::JournalFailure)
    }
}
