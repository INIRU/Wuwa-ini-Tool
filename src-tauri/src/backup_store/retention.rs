use super::{ApplyReason, StoredBackup};

pub(crate) const AUTOMATIC_BACKUP_LIMIT: usize = 30;

pub(crate) fn prune(records: &mut Vec<StoredBackup>) -> Vec<String> {
    let excess = records
        .iter()
        .filter(|stored| {
            stored.record.reason != ApplyReason::FirstOriginal && !stored.record.pinned
        })
        .count()
        .saturating_sub(AUTOMATIC_BACKUP_LIMIT);
    if excess == 0 {
        return Vec::new();
    }

    let mut remaining = excess;
    let mut removed = Vec::with_capacity(excess);
    records.retain(|stored| {
        let should_remove = remaining > 0
            && stored.record.reason != ApplyReason::FirstOriginal
            && !stored.record.pinned;
        if should_remove {
            remaining -= 1;
            removed.push(stored.file_name.clone());
            false
        } else {
            true
        }
    });
    removed
}
