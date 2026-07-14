use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::TempDir;
use wuwa_ini_tool_lib::{
    backup_store::{ApplyReason, BackupError, BackupStore},
    ini_document::MergePreview,
};

struct TestStore {
    _root: TempDir,
    source: PathBuf,
    store: BackupStore,
}

impl TestStore {
    fn new(bytes: &[u8]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("game");
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("Engine.ini");
        std::fs::write(&source, bytes).unwrap();
        let store = BackupStore::new(root.path().join("app-data"));
        Self {
            _root: root,
            source,
            store,
        }
    }

    fn source(&self) -> &Path {
        &self.source
    }

    fn preview(&self, after: &[u8]) -> MergePreview {
        MergePreview {
            before: std::fs::read(self.source()).unwrap(),
            after: after.to_vec(),
            semantic_changes: Vec::new(),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn apply_creates_verified_backup_before_replace() {
    let fixture = TestStore::new(b"before");
    let result = fixture
        .store
        .apply(
            fixture.source(),
            &fixture.preview(b"after"),
            ApplyReason::Preset,
        )
        .expect("apply should succeed");

    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"after");
    assert_eq!(std::fs::read(&result.backup_path).unwrap(), b"before");
    assert_eq!(result.backup.sha256, sha256_hex(b"before"));
    assert_eq!(result.applied_sha256, sha256_hex(b"after"));
}

#[test]
fn first_original_is_immutable_across_later_applies() {
    let fixture = TestStore::new(b"original");
    fixture
        .store
        .apply(
            fixture.source(),
            &fixture.preview(b"second"),
            ApplyReason::Preset,
        )
        .unwrap();
    fixture
        .store
        .apply(
            fixture.source(),
            &fixture.preview(b"third"),
            ApplyReason::RawEditor,
        )
        .unwrap();

    let originals: Vec<_> = fixture
        .store
        .list(fixture.source())
        .unwrap()
        .into_iter()
        .filter(|entry| entry.backup.reason == ApplyReason::FirstOriginal)
        .collect();
    assert_eq!(originals.len(), 1);
    assert_eq!(
        std::fs::read(&originals[0].backup_path).unwrap(),
        b"original"
    );
}

#[test]
fn restore_backs_up_current_bytes_before_replacement() {
    let fixture = TestStore::new(b"original");
    fixture
        .store
        .apply(
            fixture.source(),
            &fixture.preview(b"changed"),
            ApplyReason::Preset,
        )
        .unwrap();
    let original = fixture
        .store
        .list(fixture.source())
        .unwrap()
        .into_iter()
        .find(|entry| entry.backup.reason == ApplyReason::FirstOriginal)
        .unwrap();

    let result = fixture
        .store
        .restore(fixture.source(), &original.backup.id)
        .expect("restore should succeed");

    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"original");
    assert_eq!(std::fs::read(&result.backup_path).unwrap(), b"changed");
    assert_eq!(result.backup.reason, ApplyReason::Restore);
    assert_eq!(result.restored_from.id, original.backup.id);
}

#[test]
fn apply_rejects_an_external_source_change() {
    let fixture = TestStore::new(b"observed");
    let preview = fixture.preview(b"planned");
    std::fs::write(fixture.source(), b"external").unwrap();

    let result = fixture
        .store
        .apply(fixture.source(), &preview, ApplyReason::Preset);

    assert!(matches!(result, Err(BackupError::SourceConflict { .. })));
    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"external");
}

#[cfg(unix)]
#[test]
fn failed_sibling_temp_write_leaves_source_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = TestStore::new(b"before");
    let parent = fixture.source().parent().unwrap();
    let original_mode = std::fs::metadata(parent).unwrap().permissions().mode();
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = fixture.store.apply(
        fixture.source(),
        &fixture.preview(b"after"),
        ApplyReason::Preset,
    );

    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(original_mode)).unwrap();
    assert!(matches!(result, Err(BackupError::Io { .. })));
    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"before");
}

#[test]
fn retention_keeps_only_thirty_recent_unpinned_automatic_backups() {
    let fixture = TestStore::new(b"value-0");
    for index in 1..=35 {
        let next = format!("value-{index}");
        fixture
            .store
            .apply(
                fixture.source(),
                &fixture.preview(next.as_bytes()),
                ApplyReason::Preset,
            )
            .unwrap();
    }

    let records = fixture.store.list(fixture.source()).unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|entry| {
                entry.backup.reason != ApplyReason::FirstOriginal && !entry.backup.pinned
            })
            .count(),
        30
    );
    assert_eq!(
        records
            .iter()
            .filter(|entry| entry.backup.reason == ApplyReason::FirstOriginal)
            .count(),
        1
    );
}

#[test]
fn retention_never_prunes_a_pinned_backup() {
    let fixture = TestStore::new(b"value-0");
    let pinned = fixture
        .store
        .apply(
            fixture.source(),
            &fixture.preview(b"value-1"),
            ApplyReason::Preset,
        )
        .unwrap();
    fixture
        .store
        .pin(fixture.source(), &pinned.backup.id, true)
        .unwrap();

    for index in 2..=36 {
        let next = format!("value-{index}");
        fixture
            .store
            .apply(
                fixture.source(),
                &fixture.preview(next.as_bytes()),
                ApplyReason::Preset,
            )
            .unwrap();
    }

    let records = fixture.store.list(fixture.source()).unwrap();
    assert!(records
        .iter()
        .any(|entry| entry.backup.id == pinned.backup.id && entry.backup.pinned));
}

#[test]
fn restore_rejects_path_traversal_backup_ids() {
    let fixture = TestStore::new(b"before");

    let result = fixture.store.restore(fixture.source(), "../escape.ini");

    assert!(matches!(result, Err(BackupError::InvalidPath { .. })));
    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"before");
}

#[test]
fn create_rejects_relative_source_paths() {
    let fixture = TestStore::new(b"before");

    let result = fixture
        .store
        .create(Path::new("../Engine.ini"), ApplyReason::Manual);

    assert!(matches!(result, Err(BackupError::InvalidPath { .. })));
}
