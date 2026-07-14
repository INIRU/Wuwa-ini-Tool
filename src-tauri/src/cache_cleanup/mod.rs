mod cleanup;
mod error;
mod model;
mod receipt;
mod validation;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

pub use error::CacheCleanupError;
pub use model::{
    CacheCleanupWarning, CacheRootKind, CleanupPreview, CleanupReceipt, CleanupRootOutcome,
    CleanupRootPreview, CleanupRootReceipt, CleanupSelection, CleanupStopReason,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::maintenance::{MaintenanceGate, MaintenanceOperation};

use self::{
    cleanup::PreparedRoot,
    receipt::{validate_receipt_directory, ReceiptJournal},
    validation::{derive_roots, scan_root, RootSpec},
};

const PREVIEW_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING_PREVIEWS: usize = 32;

pub trait GameProcessProbe: Send + Sync {
    fn is_running(&self, executable: &Path) -> Result<bool, CacheCleanupError>;
}

pub struct CacheCleanupService<P> {
    game_executable: PathBuf,
    local_app_data: PathBuf,
    receipt_directory: PathBuf,
    probe: P,
    maintenance_gate: MaintenanceGate,
    previews: Mutex<HashMap<Uuid, StoredPreview>>,
    cleanup_lock: Mutex<()>,
}

#[derive(Clone)]
struct StoredPreview {
    selection: CleanupSelection,
    roots: Vec<RootSpec>,
    fingerprints: Vec<String>,
    created_at: Instant,
}

impl<P: GameProcessProbe> CacheCleanupService<P> {
    pub fn new(
        game_executable: PathBuf,
        local_app_data: PathBuf,
        receipt_directory: PathBuf,
        probe: P,
    ) -> Result<Self, CacheCleanupError> {
        let receipt_directory = validate_receipt_directory(&receipt_directory)?;
        let (game_executable, _) = derive_roots(
            &game_executable,
            &local_app_data,
            CleanupSelection {
                wuwa: true,
                nvidia: true,
            },
        )?;
        Ok(Self {
            game_executable,
            local_app_data,
            receipt_directory,
            probe,
            maintenance_gate: MaintenanceGate::new(),
            previews: Mutex::new(HashMap::new()),
            cleanup_lock: Mutex::new(()),
        })
    }

    pub fn new_with_gate(
        game_executable: PathBuf,
        local_app_data: PathBuf,
        receipt_directory: PathBuf,
        probe: P,
        gate: MaintenanceGate,
    ) -> Result<Self, CacheCleanupError> {
        let mut service = Self::new(game_executable, local_app_data, receipt_directory, probe)?;
        service.maintenance_gate = gate;
        Ok(service)
    }

    pub fn preview(
        &self,
        selection: CleanupSelection,
    ) -> Result<CleanupPreview, CacheCleanupError> {
        let (_, roots) = derive_roots(&self.game_executable, &self.local_app_data, selection)?;
        let scans = roots.iter().map(scan_root).collect::<Result<Vec<_>, _>>()?;
        let root_previews = scans.iter().map(|scan| scan.preview.clone()).collect();
        let fingerprints = scans.into_iter().map(|scan| scan.fingerprint).collect();
        let token = Uuid::new_v4();
        let created_at = Instant::now();
        let mut previews = self
            .previews
            .lock()
            .map_err(|_| CacheCleanupError::StateUnavailable)?;
        previews
            .retain(|_, preview| created_at.duration_since(preview.created_at) <= PREVIEW_LIFETIME);
        while previews.len() >= MAX_PENDING_PREVIEWS {
            let Some(oldest) = previews
                .iter()
                .min_by_key(|(_, preview)| preview.created_at)
                .map(|(token, _)| *token)
            else {
                break;
            };
            previews.remove(&oldest);
        }
        previews.insert(
            token,
            StoredPreview {
                selection,
                roots,
                fingerprints,
                created_at,
            },
        );
        Ok(CleanupPreview {
            token,
            selection,
            roots: root_previews,
            warnings: preview_warnings(selection),
        })
    }

    pub fn execute(
        &self,
        token: Uuid,
        confirmed: bool,
    ) -> Result<CleanupReceipt, CacheCleanupError> {
        if !confirmed {
            return Err(CacheCleanupError::ConfirmationRequired);
        }
        let _maintenance_guard = self
            .maintenance_gate
            .try_acquire(MaintenanceOperation::CacheCleanup)
            .map_err(|_| CacheCleanupError::MaintenanceBusy)?;
        let _cleanup_guard = self
            .cleanup_lock
            .lock()
            .map_err(|_| CacheCleanupError::StateUnavailable)?;
        let preview = self
            .previews
            .lock()
            .map_err(|_| CacheCleanupError::StateUnavailable)?
            .remove(&token)
            .ok_or(CacheCleanupError::UnknownPreview)?;
        if preview.created_at.elapsed() > PREVIEW_LIFETIME {
            return Err(CacheCleanupError::UnknownPreview);
        }
        let (executable, current_roots) = derive_roots(
            &self.game_executable,
            &self.local_app_data,
            preview.selection,
        )?;
        if !same_roots(&preview.roots, &current_roots) {
            return Err(CacheCleanupError::UnknownPreview);
        }
        let current_fingerprints = current_roots
            .iter()
            .map(scan_root)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|scan| scan.fingerprint)
            .collect::<Vec<_>>();
        if current_fingerprints != preview.fingerprints {
            return Err(CacheCleanupError::CacheChanged);
        }
        let prepared = current_roots
            .iter()
            .map(PreparedRoot::prepare)
            .collect::<Result<Vec<_>, _>>()?;
        let prepared_fingerprints = prepared
            .iter()
            .map(|root| root.fingerprint().to_owned())
            .collect::<Vec<_>>();
        if prepared_fingerprints != preview.fingerprints {
            return Err(CacheCleanupError::CacheChanged);
        }
        if preview.selection.wuwa && self.probe.is_running(&executable)? {
            return Err(CacheCleanupError::GameRunning);
        }
        let mut receipt = CleanupReceipt {
            completed_at_unix: 0,
            roots: Vec::with_capacity(prepared.len()),
            receipt_persisted: true,
            stop_reason: None,
        };
        let mut journal = ReceiptJournal::start(&self.receipt_directory, &receipt)?;
        let mut stop_for_running_game = false;
        let mut roots = Vec::with_capacity(prepared.len());
        for root in prepared {
            if !stop_for_running_game && preview.selection.wuwa {
                match self.probe.is_running(&executable) {
                    Ok(true) => {
                        stop_for_running_game = true;
                        receipt.stop_reason = Some(CleanupStopReason::GameStarted);
                    }
                    Ok(false) => {}
                    Err(_) => {
                        stop_for_running_game = true;
                        receipt.stop_reason = Some(CleanupStopReason::ProcessStateUnavailable);
                    }
                }
            }
            roots.push(if stop_for_running_game {
                root.skip()
            } else {
                root.delete()
            });
            receipt.roots = roots.clone();
            if journal.progress(&receipt).is_err() {
                receipt.receipt_persisted = false;
            }
        }
        receipt.completed_at_unix = OffsetDateTime::now_utc().unix_timestamp();
        receipt.roots = roots;
        if journal.complete(&receipt).is_err() {
            receipt.receipt_persisted = false;
        }
        Ok(receipt)
    }
}

fn preview_warnings(selection: CleanupSelection) -> Vec<CacheCleanupWarning> {
    let mut warnings = vec![
        CacheCleanupWarning::TroubleshootingOnly,
        CacheCleanupWarning::ShaderRebuildMayStutter,
        CacheCleanupWarning::NoBackupOrRestore,
    ];
    if selection.nvidia {
        warnings.push(CacheCleanupWarning::NvidiaCacheIsDriverWide);
    }
    warnings
}

fn same_roots(expected: &[RootSpec], actual: &[RootSpec]) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(left, right)| {
            left.kind == right.kind && left.boundary == right.boundary && left.path == right.path
        })
}
