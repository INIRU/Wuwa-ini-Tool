use std::io;

use super::{CleanupRootOutcome, CleanupRootReceipt};

#[cfg(target_os = "windows")]
#[path = "windows_cleanup.rs"]
mod platform;

#[cfg(not(target_os = "windows"))]
mod platform {
    use std::{
        collections::HashSet,
        fs, io,
        path::{Path, PathBuf},
    };

    use super::{classify_error, empty_receipt, finalize_outcome};
    use crate::cache_cleanup::{
        validation::{fingerprint, is_alias, metadata_record, scan_root, validate_root, RootSpec},
        CacheCleanupError, CleanupRootOutcome, CleanupRootReceipt,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct FileIdentity {
        first: u64,
        second: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum PlannedKind {
        File,
        Directory,
        Alias,
        Other,
    }

    #[derive(Clone, Debug)]
    struct PlannedEntry {
        relative: PathBuf,
        identity: FileIdentity,
        kind: PlannedKind,
        bytes: u64,
    }

    pub(crate) struct PreparedRoot {
        root: RootSpec,
        root_identity: Option<FileIdentity>,
        entries: Vec<PlannedEntry>,
        fingerprint: String,
    }

    impl PreparedRoot {
        pub(crate) fn prepare(root: &RootSpec) -> Result<Self, CacheCleanupError> {
            validate_root(root)?;
            let initial = scan_root(root)?;
            if !root.path.exists() {
                return Ok(Self {
                    root: root.clone(),
                    root_identity: None,
                    entries: Vec::new(),
                    fingerprint: initial.fingerprint,
                });
            }
            let root_metadata = fs::symlink_metadata(&root.path)
                .map_err(|error| CacheCleanupError::io("root_identity", &root.path, error))?;
            if is_alias(&root_metadata) || !root_metadata.is_dir() {
                return Err(CacheCleanupError::UnsafePath(root.path.clone()));
            }
            let root_identity = file_identity(&root_metadata);
            let mut entries = Vec::new();
            let mut records = Vec::new();
            collect_plan(root, &root.path, &mut entries, &mut records)?;
            let secure_fingerprint = fingerprint(&records);
            if secure_fingerprint != initial.fingerprint {
                return Err(CacheCleanupError::CacheChanged);
            }
            Ok(Self {
                root: root.clone(),
                root_identity: Some(root_identity),
                entries,
                fingerprint: secure_fingerprint,
            })
        }

        pub(crate) fn fingerprint(&self) -> &str {
            &self.fingerprint
        }

        pub(crate) fn skip(self) -> CleanupRootReceipt {
            let mut receipt = empty_receipt(self.root.kind);
            receipt.outcome = CleanupRootOutcome::Skipped;
            receipt
        }

        pub(crate) fn delete(mut self) -> CleanupRootReceipt {
            let mut receipt = empty_receipt(self.root.kind);
            let Some(root_identity) = self.root_identity else {
                receipt.outcome = CleanupRootOutcome::Skipped;
                return receipt;
            };
            let current_root = match fs::symlink_metadata(&self.root.path) {
                Ok(metadata)
                    if !is_alias(&metadata)
                        && metadata.is_dir()
                        && file_identity(&metadata) == root_identity =>
                {
                    metadata
                }
                Ok(_) | Err(_) => {
                    receipt.changed_entries = 1;
                    receipt.outcome = CleanupRootOutcome::Changed;
                    return receipt;
                }
            };
            let _ = current_root;
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
                        .then_some((entry.relative.clone(), entry.identity))
                })
                .collect::<std::collections::HashMap<_, _>>();
            for entry in &self.entries {
                let path = self.root.path.join(&entry.relative);
                match entry.kind {
                    PlannedKind::Alias | PlannedKind::Other => {
                        receipt.skipped_entries = receipt.skipped_entries.saturating_add(1);
                    }
                    PlannedKind::File => delete_file(
                        entry,
                        &self.root.path,
                        &path,
                        &directory_identities,
                        &mut receipt,
                    ),
                    PlannedKind::Directory => delete_directory(
                        entry,
                        &self.root.path,
                        &path,
                        &directory_identities,
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

    fn collect_plan(
        root: &RootSpec,
        directory: &Path,
        entries: &mut Vec<PlannedEntry>,
        records: &mut Vec<Vec<u8>>,
    ) -> Result<(), CacheCleanupError> {
        for entry in fs::read_dir(directory)
            .map_err(|error| CacheCleanupError::io("prepare_directory", directory, error))?
        {
            let entry = entry.map_err(|error| {
                CacheCleanupError::io("prepare_directory_entry", directory, error)
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| CacheCleanupError::io("prepare_entry", &path, error))?;
            let relative = path
                .strip_prefix(&root.path)
                .map_err(|_| CacheCleanupError::UnsafePath(path.clone()))?
                .to_path_buf();
            let kind = if is_alias(&metadata) {
                PlannedKind::Alias
            } else if metadata.is_file() {
                PlannedKind::File
            } else if metadata.is_dir() {
                PlannedKind::Directory
            } else {
                PlannedKind::Other
            };
            records.push(metadata_record(&relative, &metadata));
            entries.push(PlannedEntry {
                relative: relative.clone(),
                identity: file_identity(&metadata),
                kind,
                bytes: metadata.len(),
            });
            if kind == PlannedKind::Directory {
                collect_plan(root, &path, entries, records)?;
            }
        }
        Ok(())
    }

    fn delete_file(
        entry: &PlannedEntry,
        root: &Path,
        path: &Path,
        directory_identities: &std::collections::HashMap<PathBuf, FileIdentity>,
        receipt: &mut CleanupRootReceipt,
    ) {
        if !ancestors_match(root, &entry.relative, directory_identities) {
            receipt.changed_entries = receipt.changed_entries.saturating_add(1);
            return;
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                receipt.changed_entries = receipt.changed_entries.saturating_add(1);
                return;
            }
            Err(error) => {
                classify_error(receipt, &error);
                return;
            }
        };
        if is_alias(&metadata) || !metadata.is_file() || file_identity(&metadata) != entry.identity
        {
            receipt.changed_entries = receipt.changed_entries.saturating_add(1);
            return;
        }
        match fs::remove_file(path) {
            Ok(()) => {
                receipt.deleted_files = receipt.deleted_files.saturating_add(1);
                receipt.deleted_bytes = receipt.deleted_bytes.saturating_add(entry.bytes);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                receipt.changed_entries = receipt.changed_entries.saturating_add(1);
            }
            Err(error) => classify_error(receipt, &error),
        }
    }

    fn delete_directory(
        entry: &PlannedEntry,
        root: &Path,
        path: &Path,
        directory_identities: &std::collections::HashMap<PathBuf, FileIdentity>,
        receipt: &mut CleanupRootReceipt,
    ) {
        if !ancestors_match(root, &entry.relative, directory_identities) {
            receipt.changed_entries = receipt.changed_entries.saturating_add(1);
            return;
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => {
                classify_error(receipt, &error);
                return;
            }
        };
        if is_alias(&metadata) || !metadata.is_dir() || file_identity(&metadata) != entry.identity {
            receipt.changed_entries = receipt.changed_entries.saturating_add(1);
            return;
        }
        if let Err(error) = fs::remove_dir(path) {
            if error.kind() == io::ErrorKind::DirectoryNotEmpty {
                receipt.changed_entries = receipt.changed_entries.saturating_add(1);
            } else {
                classify_error(receipt, &error);
            }
        }
    }

    fn ancestors_match(
        root: &Path,
        relative: &Path,
        directory_identities: &std::collections::HashMap<PathBuf, FileIdentity>,
    ) -> bool {
        let Some(parent) = relative.parent() else {
            return true;
        };
        let mut current = PathBuf::new();
        for component in parent.components() {
            current.push(component);
            let Some(expected) = directory_identities.get(&current) else {
                return false;
            };
            let Ok(metadata) = fs::symlink_metadata(root.join(&current)) else {
                return false;
            };
            if is_alias(&metadata) || !metadata.is_dir() || file_identity(&metadata) != *expected {
                return false;
            }
        }
        true
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
                count_unplanned(root, &path, planned, receipt);
            }
        }
    }

    fn entry_order(entry: &PlannedEntry) -> (usize, bool) {
        (
            entry.relative.components().count(),
            entry.kind == PlannedKind::Directory,
        )
    }

    #[cfg(unix)]
    fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
        use std::os::unix::fs::MetadataExt;

        FileIdentity {
            first: metadata.dev(),
            second: metadata.ino(),
        }
    }

    #[cfg(not(unix))]
    fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
        FileIdentity {
            first: metadata.len(),
            second: metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |value| value.as_nanos() as u64),
        }
    }
}

pub(crate) use platform::PreparedRoot;

pub(crate) fn empty_receipt(kind: crate::cache_cleanup::CacheRootKind) -> CleanupRootReceipt {
    CleanupRootReceipt {
        kind,
        outcome: CleanupRootOutcome::Complete,
        deleted_files: 0,
        deleted_bytes: 0,
        skipped_entries: 0,
        locked_entries: 0,
        denied_entries: 0,
        changed_entries: 0,
        failed_entries: 0,
    }
}

pub(crate) fn finalize_outcome(receipt: &CleanupRootReceipt) -> CleanupRootOutcome {
    if receipt.changed_entries > 0 {
        CleanupRootOutcome::Changed
    } else if receipt.failed_entries > 0 || receipt.denied_entries > 0 || receipt.locked_entries > 0
    {
        if receipt.deleted_files > 0 {
            CleanupRootOutcome::Partial
        } else {
            CleanupRootOutcome::Failed
        }
    } else if receipt.skipped_entries > 0 {
        CleanupRootOutcome::Partial
    } else {
        CleanupRootOutcome::Complete
    }
}

pub(crate) fn classify_error(receipt: &mut CleanupRootReceipt, error: &io::Error) {
    match error.raw_os_error() {
        Some(32 | 33) => receipt.locked_entries = receipt.locked_entries.saturating_add(1),
        Some(5) => receipt.denied_entries = receipt.denied_entries.saturating_add(1),
        _ if error.kind() == io::ErrorKind::PermissionDenied => {
            receipt.denied_entries = receipt.denied_entries.saturating_add(1);
        }
        _ => receipt.failed_entries = receipt.failed_entries.saturating_add(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_sharing_violations_are_reported_as_locked() {
        let mut receipt = empty_receipt(crate::cache_cleanup::CacheRootKind::WuwaPso);
        classify_error(&mut receipt, &io::Error::from_raw_os_error(32));
        classify_error(&mut receipt, &io::Error::from_raw_os_error(33));

        assert_eq!(receipt.locked_entries, 2);
        assert_eq!(receipt.failed_entries, 0);
    }

    #[test]
    fn access_denied_is_reported_separately() {
        let mut receipt = empty_receipt(crate::cache_cleanup::CacheRootKind::WuwaPso);
        classify_error(&mut receipt, &io::Error::from_raw_os_error(5));

        assert_eq!(receipt.denied_entries, 1);
        assert_eq!(receipt.failed_entries, 0);
    }
}
