use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;
use wuwa_ini_tool_lib::{
    backup_store::{ApplyReason, BackupError, BackupStore, SourceExpectation},
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
        .restore(
            fixture.source(),
            &original.backup.id,
            SourceExpectation::Present(sha256_hex(b"changed")),
        )
        .expect("restore should succeed");

    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"original");
    assert_eq!(
        std::fs::read(result.backup_path.as_ref().unwrap()).unwrap(),
        b"changed"
    );
    assert_eq!(result.backup.unwrap().reason, ApplyReason::Restore);
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

#[test]
fn concurrent_applies_serialize_and_reject_the_stale_preview() {
    use std::sync::{Arc, Barrier};

    let fixture = TestStore::new(b"observed");
    let start = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for after in [b"first".to_vec(), b"second".to_vec()] {
        let store = fixture.store.clone();
        let source = fixture.source.clone();
        let start = Arc::clone(&start);
        threads.push(std::thread::spawn(move || {
            start.wait();
            store.apply(
                &source,
                &MergePreview {
                    before: b"observed".to_vec(),
                    after,
                    semantic_changes: Vec::new(),
                },
                ApplyReason::Preset,
            )
        }));
    }
    start.wait();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(BackupError::SourceConflict { .. })))
            .count(),
        1
    );
    let current = std::fs::read(fixture.source()).unwrap();
    assert!(current == b"first" || current == b"second");
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

    let result = fixture.store.restore(
        fixture.source(),
        "../escape.ini",
        SourceExpectation::Present(sha256_hex(b"before")),
    );

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

#[test]
fn missing_source_can_be_listed_and_restored_with_an_explicit_expectation() {
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
    std::fs::remove_file(fixture.source()).unwrap();

    assert!(!fixture.store.list(fixture.source()).unwrap().is_empty());
    let restored = fixture
        .store
        .restore(
            fixture.source(),
            &original.backup.id,
            SourceExpectation::Missing,
        )
        .expect("an explicitly missing source should be restorable");

    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"original");
    assert!(restored.backup.is_none());
    assert!(restored.backup_path.is_none());
}

#[test]
fn restore_rejects_a_stale_present_expectation() {
    let fixture = TestStore::new(b"original");
    fixture
        .store
        .apply(
            fixture.source(),
            &fixture.preview(b"current"),
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

    let result = fixture.store.restore(
        fixture.source(),
        &original.backup.id,
        SourceExpectation::Present(sha256_hex(b"stale-preview")),
    );

    assert!(matches!(result, Err(BackupError::SourceConflict { .. })));
    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"current");
}

#[test]
fn public_apply_cannot_use_the_first_original_reason() {
    let fixture = TestStore::new(b"before");

    let result = fixture.store.apply(
        fixture.source(),
        &fixture.preview(b"after"),
        ApplyReason::FirstOriginal,
    );

    assert!(matches!(result, Err(BackupError::InvalidReason(_))));
    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"before");
    assert!(fixture.store.list(fixture.source()).unwrap().is_empty());
}

#[test]
fn metadata_rejects_duplicate_ids_and_malformed_record_fields() {
    fn assert_invalid(mutator: impl FnOnce(&mut serde_json::Value)) {
        let fixture = TestStore::new(b"before");
        fixture
            .store
            .apply(
                fixture.source(),
                &fixture.preview(b"after"),
                ApplyReason::Preset,
            )
            .unwrap();
        let entry = fixture.store.list(fixture.source()).unwrap().remove(0);
        let metadata_path = entry.backup_path.parent().unwrap().join("metadata.json");
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&metadata_path).unwrap()).unwrap();
        mutator(&mut json);
        std::fs::write(&metadata_path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
        assert!(matches!(
            fixture.store.list(fixture.source()),
            Err(BackupError::InvalidMetadata(_)) | Err(BackupError::InvalidPath { .. })
        ));
    }

    assert_invalid(|json| {
        let duplicate = json["records"][0].clone();
        json["records"].as_array_mut().unwrap().push(duplicate);
    });
    assert_invalid(|json| json["records"][0]["record"]["sha256"] = "ABC".into());
    assert_invalid(|json| json["records"][0]["record"]["created_at"] = "yesterday".into());
    assert_invalid(|json| json["records"][0]["file_name"] = "other.ini".into());
    assert_invalid(|json| json["records"][0]["record"]["id"] = Uuid::new_v4().to_string().into());
    assert_invalid(|json| json["pending_deletions"] = serde_json::json!(["metadata.json"]));
    assert_invalid(|json| {
        let retired = format!("1-{}.ini", Uuid::new_v4());
        json["pending_deletions"] = serde_json::json!([retired, retired]);
    });
}

#[test]
fn retention_cleanup_failure_is_nonfatal_and_does_not_overwrite_the_backup_path() {
    let fixture = TestStore::new(b"value-0");
    for index in 1..=30 {
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
    let oldest = fixture
        .store
        .list(fixture.source())
        .unwrap()
        .into_iter()
        .rfind(|entry| entry.backup.reason != ApplyReason::FirstOriginal)
        .unwrap();
    let metadata_path = oldest.backup_path.parent().unwrap().join("metadata.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&metadata_path).unwrap()).unwrap();
    metadata["records"]
        .as_array_mut()
        .unwrap()
        .retain(|stored| stored["record"]["id"] != oldest.backup.id);
    metadata["pending_deletions"] =
        serde_json::json!([oldest.backup_path.file_name().unwrap().to_str().unwrap()]);
    std::fs::write(
        &metadata_path,
        serde_json::to_vec_pretty(&metadata).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(&oldest.backup_path).unwrap();
    std::fs::create_dir(&oldest.backup_path).unwrap();

    let result = fixture.store.apply(
        fixture.source(),
        &fixture.preview(b"value-31"),
        ApplyReason::Preset,
    );

    assert!(
        result.is_ok(),
        "cleanup must be retried without failing apply: {result:?}"
    );
    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"value-31");
    assert!(oldest.backup_path.is_dir());
}

#[test]
fn apply_scavenges_strictly_owned_sibling_temp_files() {
    let fixture = TestStore::new(b"before");
    let temporary = fixture
        .source()
        .parent()
        .unwrap()
        .join(format!(".Engine.ini.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, b"stale").unwrap();

    fixture
        .store
        .apply(
            fixture.source(),
            &fixture.preview(b"after"),
            ApplyReason::Preset,
        )
        .unwrap();

    assert!(!temporary.exists());
}

#[test]
fn list_reports_a_corrupted_stored_backup() {
    let fixture = TestStore::new(b"before");
    let applied = fixture
        .store
        .apply(
            fixture.source(),
            &fixture.preview(b"after"),
            ApplyReason::Preset,
        )
        .unwrap();
    std::fs::write(&applied.backup_path, b"corrupt").unwrap();

    let result = fixture.store.list(fixture.source());

    assert!(matches!(result, Err(BackupError::HashMismatch { .. })));
}
