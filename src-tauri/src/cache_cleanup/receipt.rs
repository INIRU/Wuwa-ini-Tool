use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{validation::is_alias, CacheCleanupError, CleanupReceipt};

const RECEIPT_SCHEMA_VERSION: u32 = 1;
const MAX_RECEIPT_FILES: usize = 50;
const MAX_RECEIPT_BYTES: u64 = 1_048_576;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReceiptPhase {
    Started,
    Progress,
    Completed,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ReceiptCheckpoint<'a> {
    schema_version: u32,
    operation_id: Uuid,
    phase: ReceiptPhase,
    receipt: &'a CleanupReceipt,
}

pub(crate) struct ReceiptJournal {
    directory: PathBuf,
    operation_id: Uuid,
    started_at_unix: i64,
    sequence: u8,
}

impl ReceiptJournal {
    pub(crate) fn start(
        directory: &Path,
        receipt: &CleanupReceipt,
    ) -> Result<Self, CacheCleanupError> {
        let directory = validate_receipt_directory(directory)?;
        let journal = Self {
            directory,
            operation_id: Uuid::new_v4(),
            started_at_unix: OffsetDateTime::now_utc().unix_timestamp(),
            sequence: 0,
        };
        journal.write(ReceiptPhase::Started, receipt)?;
        Ok(journal)
    }

    pub(crate) fn progress(&mut self, receipt: &CleanupReceipt) -> Result<(), CacheCleanupError> {
        self.sequence = self.sequence.saturating_add(1).min(98);
        self.write(ReceiptPhase::Progress, receipt)
    }

    pub(crate) fn complete(mut self, receipt: &CleanupReceipt) -> Result<(), CacheCleanupError> {
        self.sequence = 99;
        self.write(ReceiptPhase::Completed, receipt)?;
        prune_receipts(&self.directory)
    }

    fn write(
        &self,
        phase: ReceiptPhase,
        receipt: &CleanupReceipt,
    ) -> Result<(), CacheCleanupError> {
        let checkpoint = ReceiptCheckpoint {
            schema_version: RECEIPT_SCHEMA_VERSION,
            operation_id: self.operation_id,
            phase,
            receipt,
        };
        let bytes = serde_json::to_vec_pretty(&checkpoint)
            .map_err(|_| CacheCleanupError::InvalidReceiptStore)?;
        if bytes.len() as u64 > MAX_RECEIPT_BYTES {
            return Err(CacheCleanupError::InvalidReceiptStore);
        }
        let file_name = checkpoint_name(
            self.started_at_unix,
            self.operation_id,
            self.sequence,
            phase,
        );
        atomic_write_new(&self.directory, &file_name, &bytes)
    }
}

pub(crate) fn validate_receipt_directory(path: &Path) -> Result<PathBuf, CacheCleanupError> {
    fs::create_dir_all(path)
        .map_err(|error| CacheCleanupError::io("create_receipt_directory", path, error))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| CacheCleanupError::io("receipt_directory_metadata", path, error))?;
    if is_alias(&metadata) || !metadata.is_dir() {
        return Err(CacheCleanupError::InvalidReceiptStore);
    }
    path.canonicalize()
        .map_err(|error| CacheCleanupError::io("canonicalize_receipt_directory", path, error))
}

fn atomic_write_new(
    directory: &Path,
    file_name: &str,
    bytes: &[u8],
) -> Result<(), CacheCleanupError> {
    let directory = validate_receipt_directory(directory)?;
    let _pin = DirectoryPin::open(&directory)?;
    scavenge_owned_temporaries(&directory)?;
    prune_receipts_to(&directory, MAX_RECEIPT_FILES.saturating_sub(1))?;
    let temporary_path = directory.join(format!(".maintenance-{}.tmp", Uuid::new_v4()));
    let final_path = directory.join(file_name);
    let result = (|| {
        let mut temporary = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|error| {
                CacheCleanupError::io("create_receipt_temporary", &temporary_path, error)
            })?;
        temporary
            .write_all(bytes)
            .and_then(|()| temporary.sync_all())
            .map_err(|error| {
                CacheCleanupError::io("write_receipt_temporary", &temporary_path, error)
            })?;
        drop(temporary);
        publish_no_replace(&temporary_path, &final_path)
            .map_err(|error| CacheCleanupError::io("publish_receipt", &final_path, error))?;
        sync_directory(&directory)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn prune_receipts(directory: &Path) -> Result<(), CacheCleanupError> {
    let _pin = DirectoryPin::open(directory)?;
    prune_receipts_to(directory, MAX_RECEIPT_FILES)
}

fn prune_receipts_to(directory: &Path, limit: usize) -> Result<(), CacheCleanupError> {
    let mut receipts = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| CacheCleanupError::io("list_receipt_directory", directory, error))?
    {
        let entry =
            entry.map_err(|error| CacheCleanupError::io("list_receipt_entry", directory, error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !is_checkpoint_name(&name) {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| CacheCleanupError::io("receipt_metadata", &path, error))?;
        if metadata.is_file() && !is_alias(&metadata) && metadata.len() <= MAX_RECEIPT_BYTES {
            receipts.push((name.into_owned(), path));
        }
    }
    receipts.sort_by(|left, right| left.0.cmp(&right.0));
    let excess = receipts.len().saturating_sub(limit);
    for (_, path) in receipts.into_iter().take(excess) {
        fs::remove_file(&path)
            .map_err(|error| CacheCleanupError::io("prune_receipt", &path, error))?;
    }
    sync_directory(directory)
}

fn scavenge_owned_temporaries(directory: &Path) -> Result<(), CacheCleanupError> {
    for entry in fs::read_dir(directory)
        .map_err(|error| CacheCleanupError::io("list_receipt_directory", directory, error))?
    {
        let entry =
            entry.map_err(|error| CacheCleanupError::io("list_receipt_entry", directory, error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !is_owned_temporary_name(&name) {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| CacheCleanupError::io("receipt_temp_metadata", &path, error))?;
        if metadata.is_file() && !is_alias(&metadata) {
            fs::remove_file(&path)
                .map_err(|error| CacheCleanupError::io("remove_receipt_temp", &path, error))?;
        }
    }
    Ok(())
}

fn checkpoint_name(
    started_at_unix: i64,
    operation_id: Uuid,
    sequence: u8,
    phase: ReceiptPhase,
) -> String {
    let phase = match phase {
        ReceiptPhase::Started => "started",
        ReceiptPhase::Progress => "progress",
        ReceiptPhase::Completed => "completed",
    };
    format!("maintenance-{started_at_unix:020}-{operation_id}-{sequence:02}-{phase}.json")
}

fn is_checkpoint_name(name: &str) -> bool {
    if !name.starts_with("maintenance-") || !name.ends_with(".json") || name.len() > 128 {
        return false;
    }
    let stem = &name[12..name.len() - 5];
    let Some((timestamp, remainder)) = stem.split_once('-') else {
        return false;
    };
    if timestamp.len() != 20 || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Some((uuid_text, suffix)) = remainder.rsplit_once('-') else {
        return false;
    };
    let Some((uuid_text, sequence)) = uuid_text.rsplit_once('-') else {
        return false;
    };
    let Ok(uuid) = Uuid::parse_str(uuid_text) else {
        return false;
    };
    if uuid.to_string() != uuid_text
        || sequence.len() != 2
        || !sequence.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    match (sequence, suffix) {
        ("00", "started") | ("99", "completed") => true,
        (_, "progress") => sequence
            .parse::<u8>()
            .is_ok_and(|value| (1..=98).contains(&value)),
        _ => false,
    }
}

fn is_owned_temporary_name(name: &str) -> bool {
    let Some(uuid_text) = name
        .strip_prefix(".maintenance-")
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    Uuid::parse_str(uuid_text).is_ok_and(|uuid| uuid.to_string() == uuid_text)
}

#[cfg(target_os = "windows")]
fn publish_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, destination: *const u16, flags: u32) -> i32;
    }

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x0000_0008) };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn publish_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    unsafe extern "C" {
        fn renamex_np(source: *const i8, destination: *const i8, flags: u32) -> i32;
    }

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let moved = unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), 0x0000_0004) };
    if moved == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn publish_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    unsafe extern "C" {
        fn renameat2(
            old_directory: i32,
            old_path: *const i8,
            new_directory: i32,
            new_path: *const i8,
            flags: u32,
        ) -> i32;
    }

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let moved = unsafe { renameat2(-100, source.as_ptr(), -100, destination.as_ptr(), 1) };
    if moved == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn publish_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(source, destination)?;
    fs::remove_file(source)
}

#[cfg(not(target_os = "windows"))]
struct DirectoryPin {
    _file: fs::File,
}

#[cfg(not(target_os = "windows"))]
impl DirectoryPin {
    fn open(path: &Path) -> Result<Self, CacheCleanupError> {
        fs::File::open(path)
            .map(|file| Self { _file: file })
            .map_err(|error| CacheCleanupError::io("pin_receipt_directory", path, error))
    }
}

#[cfg(target_os = "windows")]
struct DirectoryPin(Vec<*mut std::ffi::c_void>);

#[cfg(target_os = "windows")]
impl DirectoryPin {
    fn open(path: &Path) -> Result<Self, CacheCleanupError> {
        use std::{os::windows::ffi::OsStrExt, ptr};

        #[repr(C)]
        struct FileTime {
            low_date_time: u32,
            high_date_time: u32,
        }

        #[repr(C)]
        struct ByHandleFileInformation {
            file_attributes: u32,
            creation_time: FileTime,
            last_access_time: FileTime,
            last_write_time: FileTime,
            volume_serial_number: u32,
            file_size_high: u32,
            file_size_low: u32,
            number_of_links: u32,
            file_index_high: u32,
            file_index_low: u32,
        }

        #[link(name = "Kernel32")]
        unsafe extern "system" {
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
                information: *mut ByHandleFileInformation,
            ) -> i32;
        }

        let mut pin = Self(Vec::new());
        let mut current = PathBuf::new();
        for component in path.components() {
            current.push(component.as_os_str());
            match component {
                std::path::Component::Prefix(_) => continue,
                std::path::Component::RootDir | std::path::Component::Normal(_) => {}
                std::path::Component::CurDir | std::path::Component::ParentDir => {
                    return Err(CacheCleanupError::InvalidReceiptStore);
                }
            }
            let wide = current
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    0x0080,
                    0x0000_0001 | 0x0000_0002,
                    ptr::null_mut(),
                    3,
                    0x0200_0000 | 0x0020_0000,
                    ptr::null_mut(),
                )
            };
            if handle == -1_isize as *mut std::ffi::c_void {
                return Err(CacheCleanupError::io(
                    "pin_receipt_directory",
                    &current,
                    std::io::Error::last_os_error(),
                ));
            }
            pin.0.push(handle);
            let mut information = std::mem::MaybeUninit::<ByHandleFileInformation>::uninit();
            let inspected = unsafe { GetFileInformationByHandle(handle, information.as_mut_ptr()) };
            if inspected == 0 {
                return Err(CacheCleanupError::io(
                    "inspect_receipt_directory",
                    &current,
                    std::io::Error::last_os_error(),
                ));
            }
            let information = unsafe { information.assume_init() };
            if information.file_attributes & 0x0000_0400 != 0
                || information.file_attributes & 0x0000_0010 == 0
            {
                return Err(CacheCleanupError::InvalidReceiptStore);
            }
        }
        if pin.0.is_empty() {
            Err(CacheCleanupError::InvalidReceiptStore)
        } else {
            Ok(pin)
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for DirectoryPin {
    fn drop(&mut self) {
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }

        for handle in self.0.drain(..).rev() {
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn sync_directory(directory: &Path) -> Result<(), CacheCleanupError> {
    fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| CacheCleanupError::io("sync_receipt_directory", directory, error))
}

#[cfg(target_os = "windows")]
fn sync_directory(_directory: &Path) -> Result<(), CacheCleanupError> {
    // Windows does not document FlushFileBuffers for directory handles. Receipt
    // publication uses MoveFileExW with MOVEFILE_WRITE_THROUGH after syncing the
    // temporary file, which is the supported durability boundary here.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_started_checkpoint_survives_without_a_completed_cleanup() {
        let temporary = tempfile::tempdir().unwrap();
        let receipt = CleanupReceipt {
            completed_at_unix: 0,
            roots: Vec::new(),
            receipt_persisted: true,
            stop_reason: None,
        };

        let journal = ReceiptJournal::start(temporary.path(), &receipt).unwrap();
        drop(journal);

        let paths = fs::read_dir(temporary.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1);
        let text = fs::read_to_string(&paths[0]).unwrap();
        assert!(text.contains("\"phase\": \"started\""));
        assert!(!text.contains(temporary.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn checkpoint_names_are_strictly_recognized() {
        let name = checkpoint_name(
            1_752_422_400,
            Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap(),
            99,
            ReceiptPhase::Completed,
        );

        assert!(is_checkpoint_name(&name));
        assert!(!is_checkpoint_name("maintenance-../../outside.json"));
        assert!(!is_checkpoint_name("maintenance-receipts.json"));
        assert!(!is_checkpoint_name(&format!("{name}.tmp")));
        assert!(!is_checkpoint_name(
            "maintenance-00000000001752422400-12345678-1234-4234-8234-123456789ABC-99-completed.json"
        ));
        assert!(!is_checkpoint_name(
            "maintenance-00000000001752422400-12345678-1234-4234-8234-123456789abc-01-started.json"
        ));
        assert!(!is_checkpoint_name(
            "maintenance-00000000001752422400-12345678-1234-4234-8234-123456789abc-00-progress.json"
        ));
        assert!(!is_checkpoint_name(
            "maintenance-00000000001752422400-12345678-1234-4234-8234-123456789abc-98-completed.json"
        ));
    }

    #[test]
    fn repeated_interrupted_operations_never_exceed_the_owned_file_bound() {
        let temporary = tempfile::tempdir().unwrap();
        let foreign = temporary.path().join(
            "maintenance-00000000001752422400-12345678-1234-4234-8234-123456789ABC-99-completed.json",
        );
        let owned_temporary = temporary
            .path()
            .join(".maintenance-12345678-1234-4234-8234-123456789abc.tmp");
        let foreign_temporary = temporary.path().join(".maintenance-not-an-id.tmp");
        fs::write(&foreign, b"foreign").unwrap();
        fs::write(&owned_temporary, b"owned crash residue").unwrap();
        fs::write(&foreign_temporary, b"foreign temp").unwrap();
        let receipt = CleanupReceipt {
            completed_at_unix: 0,
            roots: Vec::new(),
            receipt_persisted: true,
            stop_reason: None,
        };

        for _ in 0..75 {
            drop(ReceiptJournal::start(temporary.path(), &receipt).unwrap());
        }

        let owned = fs::read_dir(temporary.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| is_checkpoint_name(name) || is_owned_temporary_name(name))
            .collect::<Vec<_>>();
        assert_eq!(owned.len(), MAX_RECEIPT_FILES);
        assert!(owned.iter().all(|name| is_checkpoint_name(name)));
        assert_eq!(fs::read(foreign).unwrap(), b"foreign");
        assert!(!owned_temporary.exists());
        assert_eq!(fs::read(foreign_temporary).unwrap(), b"foreign temp");
    }

    #[test]
    fn no_replace_publish_preserves_an_existing_destination() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let destination = temporary.path().join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"existing").unwrap();

        assert!(publish_no_replace(&source, &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"existing");
        assert_eq!(fs::read(&source).unwrap(), b"new");
    }
}
