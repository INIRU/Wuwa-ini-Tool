use std::{
    collections::{HashMap, HashSet},
    ffi::{c_void, OsStr},
    fs, io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr,
};

use super::{classify_error, empty_receipt, finalize_outcome};
use crate::cache_cleanup::{
    validation::{fingerprint, is_alias, metadata_record, scan_root, validate_root, RootSpec},
    CacheCleanupError, CleanupRootOutcome, CleanupRootReceipt,
};

type RawHandle = *mut c_void;

const INVALID_HANDLE_VALUE: RawHandle = -1_isize as RawHandle;
const FILE_READ_ATTRIBUTES: u32 = 0x0080;
const DELETE: u32 = 0x0001_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_DISPOSITION_INFO_EX_CLASS: u32 = 21;
const FILE_DISPOSITION_FLAG_DELETE: u32 = 0x0000_0001;
const FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE: u32 = 0x0000_0010;

#[repr(C)]
#[derive(Clone, Copy)]
struct FileTime {
    low_date_time: u32,
    high_date_time: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
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

#[repr(C)]
struct FileDispositionInfoEx {
    flags: u32,
}

#[link(name = "Kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: RawHandle,
    ) -> RawHandle;
    fn GetFileInformationByHandle(
        file: RawHandle,
        information: *mut ByHandleFileInformation,
    ) -> i32;
    fn SetFileInformationByHandle(
        file: RawHandle,
        information_class: u32,
        information: *const c_void,
        buffer_size: u32,
    ) -> i32;
    fn CloseHandle(handle: RawHandle) -> i32;
}

struct OwnedHandle(RawHandle);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateFileW and is owned here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StableIdentity {
    volume: u32,
    index: u64,
    creation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileSnapshot {
    stable: StableIdentity,
    modified: u64,
    size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlannedKind {
    File,
    Directory,
    Alias,
    Other,
    Unavailable(UnavailableKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnavailableKind {
    Locked,
    Denied,
    Failed,
}

#[derive(Clone, Debug)]
struct PlannedEntry {
    relative: PathBuf,
    snapshot: Option<FileSnapshot>,
    kind: PlannedKind,
    bytes: u64,
}

enum RootPinResult {
    Missing(Vec<OwnedHandle>),
    Unavailable(Vec<OwnedHandle>, UnavailableKind),
    Ready(Vec<OwnedHandle>, ByHandleFileInformation),
}

pub(crate) struct PreparedRoot {
    root: RootSpec,
    path_handles: Vec<OwnedHandle>,
    root_missing: bool,
    root_unavailable: Option<UnavailableKind>,
    entries: Vec<PlannedEntry>,
    fingerprint: String,
}

impl PreparedRoot {
    pub(crate) fn prepare(root: &RootSpec) -> Result<Self, CacheCleanupError> {
        validate_root(root)?;
        let initial = scan_root(root)?;
        let (path_handles, root_information) = match pin_root_chain(root)? {
            RootPinResult::Missing(handles) => {
                return Ok(Self {
                    root: root.clone(),
                    path_handles: handles,
                    root_missing: true,
                    root_unavailable: None,
                    entries: Vec::new(),
                    fingerprint: initial.fingerprint,
                });
            }
            RootPinResult::Unavailable(handles, kind) => {
                return Ok(Self {
                    root: root.clone(),
                    path_handles: handles,
                    root_missing: false,
                    root_unavailable: Some(kind),
                    entries: Vec::new(),
                    fingerprint: initial.fingerprint,
                });
            }
            RootPinResult::Ready(handles, information) => (handles, information),
        };
        if root_information.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || root_information.file_attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(CacheCleanupError::UnsafePath(root.path.clone()));
        }
        let mut entries = Vec::new();
        let mut records = Vec::new();
        let mut incomplete = false;
        collect_plan(
            root,
            &root.path,
            path_handles
                .last()
                .ok_or(CacheCleanupError::UnsafePath(root.path.clone()))?,
            &mut entries,
            &mut records,
            &mut incomplete,
        )?;
        let secure_fingerprint = fingerprint(&records);
        if !incomplete && secure_fingerprint != initial.fingerprint {
            return Err(CacheCleanupError::CacheChanged);
        }
        Ok(Self {
            root: root.clone(),
            path_handles,
            root_missing: false,
            root_unavailable: None,
            entries,
            fingerprint: initial.fingerprint,
        })
    }

    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn skip(self) -> CleanupRootReceipt {
        let mut receipt = empty_receipt(self.root.kind);
        if self.root_missing {
            receipt.outcome = CleanupRootOutcome::Skipped;
            return receipt;
        }
        receipt.outcome = CleanupRootOutcome::Skipped;
        receipt
    }

    pub(crate) fn delete(mut self) -> CleanupRootReceipt {
        let mut receipt = empty_receipt(self.root.kind);
        if let Some(kind) = self.root_unavailable {
            record_unavailable(&mut receipt, kind);
            receipt.outcome = finalize_outcome(&receipt);
            return receipt;
        }
        if self.path_handles.is_empty() {
            receipt.outcome = CleanupRootOutcome::Skipped;
            return receipt;
        }
        let _path_handles = std::mem::take(&mut self.path_handles);
        self.entries.sort_by(|left, right| {
            entry_order(right)
                .cmp(&entry_order(left))
                .then_with(|| right.relative.cmp(&left.relative))
        });
        let planned_paths = self
            .entries
            .iter()
            .map(|entry| entry.relative.clone())
            .collect::<HashSet<_>>();
        let directory_identities = self
            .entries
            .iter()
            .filter_map(|entry| {
                (entry.kind == PlannedKind::Directory)
                    .then_some(entry.snapshot.map(|snapshot| snapshot.stable))
                    .flatten()
                    .map(|identity| (entry.relative.clone(), identity))
            })
            .collect::<HashMap<_, _>>();
        for entry in &self.entries {
            let path = self.root.path.join(&entry.relative);
            match entry.kind {
                PlannedKind::Alias | PlannedKind::Other => {
                    receipt.skipped_entries = receipt.skipped_entries.saturating_add(1);
                }
                PlannedKind::Unavailable(kind) => record_unavailable(&mut receipt, kind),
                PlannedKind::File => delete_entry(
                    entry,
                    &self.root.path,
                    &path,
                    &directory_identities,
                    false,
                    &mut receipt,
                ),
                PlannedKind::Directory => delete_entry(
                    entry,
                    &self.root.path,
                    &path,
                    &directory_identities,
                    true,
                    &mut receipt,
                ),
            }
        }
        count_unplanned(
            &self.root.path,
            &self.root.path,
            &planned_paths,
            &mut receipt,
        );
        receipt.outcome = finalize_outcome(&receipt);
        receipt
    }
}

fn pin_root_chain(root: &RootSpec) -> Result<RootPinResult, CacheCleanupError> {
    use std::path::Component;

    let mut handles = Vec::new();
    let mut current = PathBuf::new();
    let mut boundary_reached = false;
    for component in root.path.components() {
        current.push(component.as_os_str());
        match component {
            Component::Prefix(_) => continue,
            Component::RootDir | Component::Normal(_) => {}
            Component::CurDir | Component::ParentDir => {
                return Err(CacheCleanupError::UnsafePath(root.path.clone()));
            }
        }
        let handle = match open_path(&current, false) {
            Ok(handle) => handle,
            Err(error) if boundary_reached && error.kind() == io::ErrorKind::NotFound => {
                return Ok(RootPinResult::Missing(handles));
            }
            Err(error) => {
                return Ok(RootPinResult::Unavailable(
                    handles,
                    unavailable_kind(&error),
                ));
            }
        };
        let info = information(&handle)?;
        if info.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || info.file_attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(CacheCleanupError::UnsafePath(current));
        }
        if windows_paths_equal(&current, &root.boundary) {
            boundary_reached = true;
        }
        handles.push(handle);
    }
    if !boundary_reached {
        return Err(CacheCleanupError::UnsafePath(root.path.clone()));
    }
    let root_information = handles
        .last()
        .ok_or(CacheCleanupError::UnsafePath(root.path.clone()))
        .and_then(information)?;
    Ok(RootPinResult::Ready(handles, root_information))
}

fn windows_paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(
            right
                .to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\'),
        )
}

fn collect_plan(
    root: &RootSpec,
    directory: &Path,
    _directory_handle: &OwnedHandle,
    entries: &mut Vec<PlannedEntry>,
    records: &mut Vec<Vec<u8>>,
    incomplete: &mut bool,
) -> Result<(), CacheCleanupError> {
    for entry in fs::read_dir(directory)
        .map_err(|error| CacheCleanupError::io("prepare_directory", directory, error))?
    {
        let entry = entry
            .map_err(|error| CacheCleanupError::io("prepare_directory_entry", directory, error))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(&root.path)
            .map_err(|_| CacheCleanupError::UnsafePath(path.clone()))?
            .to_path_buf();
        let handle = match open_path(&path, false) {
            Ok(handle) => handle,
            Err(error) => {
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|source| CacheCleanupError::io("prepare_entry", &path, source))?;
                records.push(metadata_record(&relative, &metadata));
                entries.push(PlannedEntry {
                    relative,
                    snapshot: None,
                    kind: PlannedKind::Unavailable(unavailable_kind(&error)),
                    bytes: 0,
                });
                *incomplete = true;
                continue;
            }
        };
        let info = information(&handle)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| CacheCleanupError::io("prepare_entry", &path, error))?;
        records.push(metadata_record(&relative, &metadata));
        let kind = if info.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            PlannedKind::Alias
        } else if info.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            PlannedKind::Directory
        } else if metadata.is_file() {
            PlannedKind::File
        } else {
            PlannedKind::Other
        };
        entries.push(PlannedEntry {
            relative: relative.clone(),
            snapshot: Some(snapshot(&info)),
            kind,
            bytes: file_size(&info),
        });
        if kind == PlannedKind::Directory {
            collect_plan(root, &path, &handle, entries, records, incomplete)?;
        }
    }
    Ok(())
}

fn delete_entry(
    entry: &PlannedEntry,
    root: &Path,
    path: &Path,
    directory_identities: &HashMap<PathBuf, StableIdentity>,
    directory: bool,
    receipt: &mut CleanupRootReceipt,
) {
    let _ancestor_handles = match lock_ancestors(root, &entry.relative, directory_identities) {
        Ok(Some(handles)) => handles,
        Ok(None) => {
            receipt.changed_entries = receipt.changed_entries.saturating_add(1);
            return;
        }
        Err(error) => {
            classify_error(receipt, &error);
            return;
        }
    };
    let handle = match open_path(path, true) {
        Ok(handle) => handle,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            receipt.changed_entries = receipt.changed_entries.saturating_add(1);
            return;
        }
        Err(error) => {
            classify_error(receipt, &error);
            return;
        }
    };
    let info = match information(&handle) {
        Ok(info) => info,
        Err(error) => {
            classify_error(receipt, &io_from_cleanup_error(error));
            return;
        }
    };
    let is_directory = info.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if info.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || is_directory != directory
        || if directory {
            entry.snapshot.map(|value| value.stable) != Some(stable_identity(&info))
        } else {
            entry.snapshot != Some(snapshot(&info))
        }
    {
        receipt.changed_entries = receipt.changed_entries.saturating_add(1);
        return;
    }
    if let Err(error) = mark_delete(&handle) {
        if directory && matches!(error.raw_os_error(), Some(145 | 183)) {
            receipt.changed_entries = receipt.changed_entries.saturating_add(1);
        } else {
            classify_error(receipt, &error);
        }
        return;
    }
    drop(handle);
    if !directory {
        receipt.deleted_files = receipt.deleted_files.saturating_add(1);
        receipt.deleted_bytes = receipt.deleted_bytes.saturating_add(entry.bytes);
    }
}

fn lock_ancestors(
    root: &Path,
    relative: &Path,
    directory_identities: &HashMap<PathBuf, StableIdentity>,
) -> io::Result<Option<Vec<OwnedHandle>>> {
    let mut handles = Vec::new();
    let mut current = PathBuf::new();
    let Some(parent) = relative.parent() else {
        return Ok(Some(handles));
    };
    for component in parent.components() {
        current.push(component);
        let Some(expected) = directory_identities.get(&current) else {
            return Ok(None);
        };
        let handle = open_path(&root.join(&current), false)?;
        let info = information_io(&handle)?;
        if info.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || info.file_attributes & FILE_ATTRIBUTE_DIRECTORY == 0
            || stable_identity(&info) != *expected
        {
            return Ok(None);
        }
        handles.push(handle);
    }
    Ok(Some(handles))
}

fn count_unplanned(
    root: &Path,
    directory: &Path,
    planned: &HashSet<PathBuf>,
    receipt: &mut CleanupRootReceipt,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            receipt.changed_entries = receipt.changed_entries.saturating_add(1);
            continue;
        };
        let relative = relative.to_path_buf();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !planned.contains(&relative) {
            receipt.changed_entries = receipt.changed_entries.saturating_add(1);
        }
        if metadata.is_dir() && !is_alias(&metadata) {
            let Ok(handle) = open_path(&path, false) else {
                continue;
            };
            let Ok(info) = information(&handle) else {
                continue;
            };
            if info.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                count_unplanned(root, &path, planned, receipt);
            }
        }
    }
}

fn open_path(path: &Path, delete: bool) -> io::Result<OwnedHandle> {
    let wide = wide_null(path.as_os_str());
    let desired_access = FILE_READ_ATTRIBUTES | if delete { DELETE } else { 0 };
    // Omitting FILE_SHARE_DELETE pins the opened object identity while the handle is alive.
    // FILE_FLAG_OPEN_REPARSE_POINT prevents CreateFileW from following a junction or symlink.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            desired_access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(OwnedHandle(handle))
    }
}

fn information(handle: &OwnedHandle) -> Result<ByHandleFileInformation, CacheCleanupError> {
    information_io(handle)
        .map_err(|source| CacheCleanupError::io("handle_identity", PathBuf::new(), source))
}

fn information_io(handle: &OwnedHandle) -> io::Result<ByHandleFileInformation> {
    let mut information = ByHandleFileInformation {
        file_attributes: 0,
        creation_time: FileTime {
            low_date_time: 0,
            high_date_time: 0,
        },
        last_access_time: FileTime {
            low_date_time: 0,
            high_date_time: 0,
        },
        last_write_time: FileTime {
            low_date_time: 0,
            high_date_time: 0,
        },
        volume_serial_number: 0,
        file_size_high: 0,
        file_size_low: 0,
        number_of_links: 0,
        file_index_high: 0,
        file_index_low: 0,
    };
    let succeeded = unsafe { GetFileInformationByHandle(handle.0, &mut information) };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(information)
    }
}

fn mark_delete(handle: &OwnedHandle) -> io::Result<()> {
    let information = FileDispositionInfoEx {
        flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    let succeeded = unsafe {
        SetFileInformationByHandle(
            handle.0,
            FILE_DISPOSITION_INFO_EX_CLASS,
            (&information as *const FileDispositionInfoEx).cast(),
            std::mem::size_of::<FileDispositionInfoEx>() as u32,
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn stable_identity(info: &ByHandleFileInformation) -> StableIdentity {
    StableIdentity {
        volume: info.volume_serial_number,
        index: join_u32(info.file_index_high, info.file_index_low),
        creation: join_u32(
            info.creation_time.high_date_time,
            info.creation_time.low_date_time,
        ),
    }
}

fn snapshot(info: &ByHandleFileInformation) -> FileSnapshot {
    FileSnapshot {
        stable: stable_identity(info),
        modified: join_u32(
            info.last_write_time.high_date_time,
            info.last_write_time.low_date_time,
        ),
        size: file_size(info),
    }
}

fn file_size(info: &ByHandleFileInformation) -> u64 {
    join_u32(info.file_size_high, info.file_size_low)
}

fn join_u32(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn unavailable_kind(error: &io::Error) -> UnavailableKind {
    match error.raw_os_error() {
        Some(32 | 33) => UnavailableKind::Locked,
        Some(5) => UnavailableKind::Denied,
        _ if error.kind() == io::ErrorKind::PermissionDenied => UnavailableKind::Denied,
        _ => UnavailableKind::Failed,
    }
}

fn record_unavailable(receipt: &mut CleanupRootReceipt, kind: UnavailableKind) {
    match kind {
        UnavailableKind::Locked => {
            receipt.locked_entries = receipt.locked_entries.saturating_add(1)
        }
        UnavailableKind::Denied => {
            receipt.denied_entries = receipt.denied_entries.saturating_add(1)
        }
        UnavailableKind::Failed => {
            receipt.failed_entries = receipt.failed_entries.saturating_add(1)
        }
    }
}

fn entry_order(entry: &PlannedEntry) -> (usize, bool) {
    (
        entry.relative.components().count(),
        entry.kind == PlannedKind::Directory,
    )
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn io_from_cleanup_error(error: CacheCleanupError) -> io::Error {
    match error {
        CacheCleanupError::Io { source, .. } => source,
        _ => io::Error::other(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory_information(modified: u64, size: u64) -> ByHandleFileInformation {
        ByHandleFileInformation {
            file_attributes: FILE_ATTRIBUTE_DIRECTORY,
            creation_time: FileTime {
                low_date_time: 7,
                high_date_time: 0,
            },
            last_access_time: FileTime {
                low_date_time: 0,
                high_date_time: 0,
            },
            last_write_time: FileTime {
                low_date_time: modified as u32,
                high_date_time: (modified >> 32) as u32,
            },
            volume_serial_number: 3,
            file_size_high: (size >> 32) as u32,
            file_size_low: size as u32,
            number_of_links: 1,
            file_index_high: 0,
            file_index_low: 11,
        }
    }

    #[test]
    fn directory_identity_ignores_mutable_metadata_after_child_deletion() {
        let before = directory_information(100, 4096);
        let after = directory_information(200, 0);

        assert_eq!(stable_identity(&before), stable_identity(&after));
        assert_ne!(snapshot(&before), snapshot(&after));
    }

    #[test]
    fn windows_path_comparison_is_case_and_separator_insensitive() {
        assert!(windows_paths_equal(
            Path::new(r"C:\Games\Wuthering Waves"),
            Path::new(r"c:/games/wuthering waves/")
        ));
    }
}
