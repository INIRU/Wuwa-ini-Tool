use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupSelection {
    pub wuwa: bool,
    pub nvidia: bool,
}

impl CleanupSelection {
    pub const fn wuwa_only() -> Self {
        Self {
            wuwa: true,
            nvidia: false,
        }
    }

    pub const fn nvidia_only() -> Self {
        Self {
            wuwa: false,
            nvidia: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheRootKind {
    WuwaPso,
    WuwaPsoReport,
    NvidiaDxCache,
    NvidiaGlCache,
    NvidiaNvCache,
}

impl CacheRootKind {
    pub const fn is_wuwa(self) -> bool {
        matches!(self, Self::WuwaPso | Self::WuwaPsoReport)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCleanupWarning {
    TroubleshootingOnly,
    ShaderRebuildMayStutter,
    NoBackupOrRestore,
    NvidiaCacheIsDriverWide,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupRootOutcome {
    Complete,
    Partial,
    Skipped,
    Changed,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStopReason {
    GameStarted,
    ProcessStateUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupRootPreview {
    pub(crate) kind: CacheRootKind,
    pub(crate) path: PathBuf,
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) skipped_entries: u64,
}

impl CleanupRootPreview {
    pub fn kind(&self) -> CacheRootKind {
        self.kind
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn files(&self) -> u64 {
        self.files
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn skipped_entries(&self) -> u64 {
        self.skipped_entries
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupPreview {
    pub(crate) token: Uuid,
    pub(crate) selection: CleanupSelection,
    pub(crate) roots: Vec<CleanupRootPreview>,
    pub(crate) warnings: Vec<CacheCleanupWarning>,
}

impl CleanupPreview {
    pub fn token(&self) -> Uuid {
        self.token
    }

    pub fn roots(&self) -> &[CleanupRootPreview] {
        &self.roots
    }

    pub fn selection(&self) -> CleanupSelection {
        self.selection
    }

    pub fn warnings(&self) -> &[CacheCleanupWarning] {
        &self.warnings
    }

    pub fn total_files(&self) -> u64 {
        self.roots.iter().map(|root| root.files).sum()
    }

    pub fn total_bytes(&self) -> u64 {
        self.roots.iter().map(|root| root.bytes).sum()
    }
}

impl CleanupRootReceipt {
    pub fn kind(&self) -> CacheRootKind {
        self.kind
    }

    pub fn outcome(&self) -> CleanupRootOutcome {
        self.outcome
    }

    pub fn deleted_files(&self) -> u64 {
        self.deleted_files
    }

    pub fn deleted_bytes(&self) -> u64 {
        self.deleted_bytes
    }

    pub fn skipped_entries(&self) -> u64 {
        self.skipped_entries
    }

    pub fn locked_entries(&self) -> u64 {
        self.locked_entries
    }

    pub fn denied_entries(&self) -> u64 {
        self.denied_entries
    }

    pub fn changed_entries(&self) -> u64 {
        self.changed_entries
    }

    pub fn failed_entries(&self) -> u64 {
        self.failed_entries
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupRootReceipt {
    pub(crate) kind: CacheRootKind,
    pub(crate) outcome: CleanupRootOutcome,
    pub(crate) deleted_files: u64,
    pub(crate) deleted_bytes: u64,
    pub(crate) skipped_entries: u64,
    pub(crate) locked_entries: u64,
    pub(crate) denied_entries: u64,
    pub(crate) changed_entries: u64,
    pub(crate) failed_entries: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupReceipt {
    pub(crate) completed_at_unix: i64,
    pub(crate) roots: Vec<CleanupRootReceipt>,
    pub(crate) receipt_persisted: bool,
    pub(crate) stop_reason: Option<CleanupStopReason>,
}

impl CleanupReceipt {
    pub fn completed_at_unix(&self) -> i64 {
        self.completed_at_unix
    }

    pub fn roots(&self) -> &[CleanupRootReceipt] {
        &self.roots
    }

    pub fn receipt_persisted(&self) -> bool {
        self.receipt_persisted
    }

    pub fn stop_reason(&self) -> Option<CleanupStopReason> {
        self.stop_reason
    }

    pub fn deleted_files(&self) -> u64 {
        self.roots.iter().map(|root| root.deleted_files).sum()
    }

    pub fn deleted_bytes(&self) -> u64 {
        self.roots.iter().map(|root| root.deleted_bytes).sum()
    }

    pub fn skipped_entries(&self) -> u64 {
        self.roots.iter().map(|root| root.skipped_entries).sum()
    }
}
