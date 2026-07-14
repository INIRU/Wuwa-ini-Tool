use std::collections::HashMap;

use crate::{
    game_discovery::GameInstallation,
    process_control::{
        ApplyRequest, ApplyStatus, CpuSelection, ProcessController, ProcessReadback, ProcessTarget,
    },
    profile_store::ProcessProfile,
};

use super::{
    ObservedGame, SupervisorApplyOutcome, SupervisorBackend, SupervisorError,
    SupervisorRestoreOutcome,
};

#[derive(Default)]
pub struct SystemSupervisorBackend {
    installation: Option<GameInstallation>,
    prior: HashMap<(u32, u64), ProcessReadback>,
}

impl SystemSupervisorBackend {
    pub fn new(installation: GameInstallation) -> Self {
        Self {
            installation: Some(installation),
            prior: HashMap::new(),
        }
    }

    pub fn is_running(installation: &GameInstallation) -> Result<bool, SupervisorError> {
        observe(installation).map(|process| process.is_some())
    }

    fn target(&self, process: &ObservedGame) -> Result<ProcessTarget, SupervisorError> {
        let installation = self
            .installation
            .as_ref()
            .ok_or(SupervisorError::BackendFailure)?;
        ProcessTarget::from_installation_with_creation(
            process.pid,
            process.creation_time_100ns,
            installation,
        )
        .map_err(|_| SupervisorError::InvalidGameIdentity)
    }
}

impl SupervisorBackend for SystemSupervisorBackend {
    fn launch(&mut self, installation: &GameInstallation) -> Result<(), SupervisorError> {
        self.installation = Some(installation.clone());
        launch(installation)
    }

    fn observe(&mut self) -> Result<Option<ObservedGame>, SupervisorError> {
        let installation = self
            .installation
            .as_ref()
            .ok_or(SupervisorError::BackendFailure)?;
        observe(installation)
    }

    fn apply(
        &mut self,
        process: &ObservedGame,
        profile: &ProcessProfile,
        dangerous_priority_acknowledged: bool,
    ) -> Result<SupervisorApplyOutcome, SupervisorError> {
        let target = self.target(process)?;
        let prior = ProcessController::readback(&target).map_err(map_process_error)?;
        self.prior
            .entry((process.pid, process.creation_time_100ns))
            .or_insert(prior);
        let report = ProcessController::apply(
            &target,
            &ApplyRequest {
                cpu_selection: profile.cpu_selection.clone(),
                priority: profile.priority,
                dangerous_priority_acknowledged,
            },
        )
        .map_err(map_process_error)?;
        Ok(match report.status {
            ApplyStatus::Success => SupervisorApplyOutcome::Success,
            ApplyStatus::Partial | ApplyStatus::Unsupported => SupervisorApplyOutcome::Partial,
            ApplyStatus::Denied => SupervisorApplyOutcome::Denied,
            ApplyStatus::Exited => SupervisorApplyOutcome::Retryable,
        })
    }

    fn restore(
        &mut self,
        process: &ObservedGame,
    ) -> Result<SupervisorRestoreOutcome, SupervisorError> {
        let Some(prior) = self
            .prior
            .get(&(process.pid, process.creation_time_100ns))
            .cloned()
        else {
            return Ok(SupervisorRestoreOutcome::Restored);
        };
        let target = self.target(process)?;
        let cpu_selection = if !prior.cpu.default_cpu_sets.is_empty() {
            CpuSelection::ManualCpuSets {
                ids: prior.cpu.default_cpu_sets,
            }
        } else if let Some(affinity) = prior.cpu.affinity.filter(|affinity| {
            affinity.groups.len() == 1 && affinity.process_mask != affinity.system_mask
        }) {
            let group = affinity.groups.first().copied().unwrap_or(0);
            CpuSelection::HardAffinity {
                group,
                mask: affinity.process_mask,
            }
        } else {
            CpuSelection::All
        };
        let result = ProcessController::apply(
            &target,
            &ApplyRequest {
                cpu_selection,
                priority: prior.priority,
                dangerous_priority_acknowledged: true,
            },
        );
        match result {
            Ok(report) if report.status == ApplyStatus::Success => {
                self.prior
                    .remove(&(process.pid, process.creation_time_100ns));
                Ok(SupervisorRestoreOutcome::Restored)
            }
            Err(crate::process_control::ProcessError::ProcessExited)
            | Err(crate::process_control::ProcessError::InvalidExecutableIdentity) => {
                self.prior
                    .remove(&(process.pid, process.creation_time_100ns));
                Ok(SupervisorRestoreOutcome::ProcessGone)
            }
            Ok(report) if restore_report_process_is_gone(&report) => {
                self.prior
                    .remove(&(process.pid, process.creation_time_100ns));
                Ok(SupervisorRestoreOutcome::ProcessGone)
            }
            Ok(_) | Err(_) => Err(SupervisorError::BackendFailure),
        }
    }
}

fn restore_report_process_is_gone(report: &crate::process_control::ApplyReport) -> bool {
    report.status == ApplyStatus::Exited
}

#[cfg(any(target_os = "windows", test))]
const HRESULT_NO_MORE_FILES: i32 = 0x8007_0012_u32 as i32;

#[cfg(any(target_os = "windows", test))]
fn enumeration_reached_end(hresult: i32) -> bool {
    hresult == HRESULT_NO_MORE_FILES
}

#[cfg(any(target_os = "windows", test))]
fn require_expected_candidate<T>(
    expected_executable: &std::path::Path,
    observed_name: &str,
    inspected: Option<T>,
) -> Result<Option<T>, SupervisorError> {
    let expected_name = expected_executable
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(SupervisorError::InvalidGameIdentity)?;
    if !expected_name.eq_ignore_ascii_case(observed_name) {
        return Ok(None);
    }
    inspected.map(Some).ok_or(SupervisorError::BackendFailure)
}

#[cfg(any(target_os = "windows", test))]
fn canonical_paths_match(
    left: &std::path::Path,
    right: &std::path::Path,
) -> Result<bool, SupervisorError> {
    let left = left
        .canonicalize()
        .map_err(|_| SupervisorError::BackendFailure)?;
    let right = right
        .canonicalize()
        .map_err(|_| SupervisorError::BackendFailure)?;
    Ok(left
        .as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy()))
}

fn map_process_error(error: crate::process_control::ProcessError) -> SupervisorError {
    use crate::process_control::ProcessError;
    match error {
        ProcessError::AccessDenied => SupervisorError::BackendFailure,
        ProcessError::ProcessExited => SupervisorError::BackendFailure,
        ProcessError::InvalidExecutableIdentity | ProcessError::InvalidProcessId => {
            SupervisorError::InvalidGameIdentity
        }
        _ => SupervisorError::BackendFailure,
    }
}

#[cfg(target_os = "windows")]
fn launch(installation: &GameInstallation) -> Result<(), SupervisorError> {
    std::process::Command::new(&installation.executable)
        .current_dir(&installation.game_root)
        .spawn()
        .map(|_| ())
        .map_err(|_| SupervisorError::BackendFailure)
}

#[cfg(not(target_os = "windows"))]
fn launch(_installation: &GameInstallation) -> Result<(), SupervisorError> {
    Err(SupervisorError::BackendFailure)
}

#[cfg(not(target_os = "windows"))]
fn observe(_installation: &GameInstallation) -> Result<Option<ObservedGame>, SupervisorError> {
    Ok(None)
}

#[cfg(target_os = "windows")]
fn observe(installation: &GameInstallation) -> Result<Option<ObservedGame>, SupervisorError> {
    windows_backend::observe(installation)
}

#[cfg(target_os = "windows")]
mod windows_backend {
    use std::{ffi::OsString, mem::size_of, path::PathBuf};

    use windows::{
        core::PWSTR,
        Win32::{
            Foundation::{CloseHandle, FILETIME, HANDLE},
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                    TH32CS_SNAPPROCESS,
                },
                Threading::{
                    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                    PROCESS_QUERY_LIMITED_INFORMATION,
                },
            },
        },
    };

    use crate::game_discovery::GameInstallation;

    use super::{
        canonical_paths_match, enumeration_reached_end, require_expected_candidate, ObservedGame,
        SupervisorError,
    };

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub(super) fn observe(
        installation: &GameInstallation,
    ) -> Result<Option<ObservedGame>, SupervisorError> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map(OwnedHandle)
            .map_err(|_| SupervisorError::BackendFailure)?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..PROCESSENTRY32W::default()
        };
        if let Err(error) = unsafe { Process32FirstW(snapshot.0, &mut entry) } {
            return if enumeration_reached_end(error.code().0) {
                Ok(None)
            } else {
                Err(SupervisorError::BackendFailure)
            };
        }
        loop {
            let observed_name = process_entry_name(&entry)?;
            if let Some(process) = require_expected_candidate(
                &installation.executable,
                &observed_name,
                inspect(entry.th32ProcessID),
            )? {
                if canonical_paths_match(&process.canonical_image, &installation.executable)? {
                    return Ok(Some(process));
                }
            }
            if let Err(error) = unsafe { Process32NextW(snapshot.0, &mut entry) } {
                return if enumeration_reached_end(error.code().0) {
                    Ok(None)
                } else {
                    Err(SupervisorError::BackendFailure)
                };
            }
        }
    }

    fn process_entry_name(entry: &PROCESSENTRY32W) -> Result<String, SupervisorError> {
        let end = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        String::from_utf16(&entry.szExeFile[..end]).map_err(|_| SupervisorError::BackendFailure)
    }

    fn inspect(pid: u32) -> Option<ObservedGame> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let process = OwnedHandle(handle);
        let mut path = vec![0_u16; 32_768];
        let mut length = path.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                process.0,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut length,
            )
        }
        .ok()?;
        path.truncate(length as usize);
        let canonical_image = PathBuf::from(OsString::from(String::from_utf16(&path).ok()?));
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe { GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user) }
            .ok()?;
        Some(ObservedGame {
            pid,
            creation_time_100ns: (u64::from(creation.dwHighDateTime) << 32)
                | u64::from(creation.dwLowDateTime),
            canonical_image,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        process_control::{ApplyReport, FieldReport},
        profile_store::{CpuSelection, PriorityClass},
    };

    #[test]
    fn exited_restore_report_is_terminal_process_gone() {
        let report = ApplyReport {
            status: ApplyStatus::Exited,
            cpu: FieldReport {
                requested: CpuSelection::All,
                applied: None,
                error_code: Some("process_exited".to_owned()),
            },
            priority: FieldReport {
                requested: PriorityClass::Normal,
                applied: None,
                error_code: Some("process_exited".to_owned()),
            },
        };

        assert!(restore_report_process_is_gone(&report));
    }

    #[test]
    fn process_enumeration_only_accepts_the_documented_end_marker() {
        assert!(enumeration_reached_end(HRESULT_NO_MORE_FILES));
        assert!(!enumeration_reached_end(0x8007_0005_u32 as i32));
    }

    #[test]
    fn expected_executable_name_inspection_failure_is_fail_closed() {
        let expected = std::path::Path::new("C:/Game/Client-Win64-Shipping.exe");

        assert_eq!(
            require_expected_candidate(expected, "other.exe", None::<u8>).unwrap(),
            None
        );
        assert_eq!(
            require_expected_candidate(expected, "CLIENT-WIN64-SHIPPING.EXE", Some(7)).unwrap(),
            Some(7)
        );
        assert_eq!(
            require_expected_candidate(expected, "Client-Win64-Shipping.exe", None::<u8>)
                .unwrap_err(),
            SupervisorError::BackendFailure
        );
    }

    #[test]
    fn expected_candidate_path_resolution_failure_is_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let expected = temp.path().join("Client-Win64-Shipping.exe");
        std::fs::write(&expected, b"fixture").unwrap();

        assert!(canonical_paths_match(&expected, &expected).unwrap());
        assert_eq!(
            canonical_paths_match(&expected, &temp.path().join("missing.exe")).unwrap_err(),
            SupervisorError::BackendFailure
        );
    }
}
