use std::{
    ffi::OsString,
    mem::{size_of, MaybeUninit},
    path::{Path, PathBuf},
    ptr,
};

use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{CloseHandle, GetLastError, FILETIME, HANDLE},
        System::{
            SystemInformation::{GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION},
            Threading::{
                GetPriorityClass, GetProcessAffinityMask, GetProcessDefaultCpuSets,
                GetProcessGroupAffinity, GetProcessInformation, GetProcessTimes, OpenProcess,
                ProcessPowerThrottling, QueryFullProcessImageNameW, SetPriorityClass,
                SetProcessAffinityMask, SetProcessDefaultCpuSets, SetProcessInformation,
                PROCESS_ACCESS_RIGHTS, PROCESS_CREATION_FLAGS, PROCESS_NAME_WIN32,
                PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SET_INFORMATION,
            },
        },
    },
};

use crate::profile_store::PriorityClass;

use super::{
    classify_game_qos_restore,
    cpu_set_buffer::{groups_from_cpu_sets, parse_cpu_set_buffer},
    validation::{classify_apply_status, validate_selection, verify_cpu_plan},
    AffinityReadback, ApplyReport, ApplyRequest, CpuReadback, CpuSetPlan, CpuTopology, FieldReport,
    GameQosApplyReport, GameQosRequest, GameQosRestoreGuard, GameQosRestoreOutcome,
    GameQosRestoreRecord, GameQosState, ProcessError, ProcessReadback, ProcessTarget,
};

const MAX_CPU_SET_BUFFER_BYTES: usize = 16 * 1024 * 1024;
const MAX_CPU_SET_IDS: usize = 4096;
const MAX_PROCESS_GROUPS: usize = 64;
const MAX_PROCESS_PATH_UNITS: usize = 32_768;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    const fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns the valid handle returned by OpenProcess.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub(crate) fn topology() -> Result<CpuTopology, ProcessError> {
    query_topology(None)
}

pub(crate) fn apply(
    target: &ProcessTarget,
    request: &ApplyRequest,
) -> Result<ApplyReport, ProcessError> {
    let process = open_validated_process(target)?;
    let cpu_result = match query_topology(Some(process.raw())).and_then(|topology| {
        validate_selection(&topology, &request.cpu_selection).map(|plan| (topology, plan))
    }) {
        Ok((topology, plan)) => apply_cpu(process.raw(), &topology, &plan)
            .and_then(|readback| verify_cpu_plan(&plan, &readback))
            .map(|()| request.cpu_selection.clone()),
        Err(error) => Err(error),
    };
    let priority_result = apply_priority(process.raw(), request.priority).and_then(|applied| {
        (applied == request.priority)
            .then_some(applied)
            .ok_or(ProcessError::OperationFailed)
    });

    let cpu_error = cpu_result.as_ref().err().copied();
    let priority_error = priority_result.as_ref().err().copied();
    Ok(ApplyReport {
        status: classify_apply_status(cpu_error, priority_error),
        cpu: field_report(request.cpu_selection.clone(), cpu_result),
        priority: field_report(request.priority, priority_result),
    })
}

pub(crate) fn readback(target: &ProcessTarget) -> Result<ProcessReadback, ProcessError> {
    let process = open_validated_process(target)?;
    Ok(ProcessReadback {
        cpu: read_cpu(process.raw())?,
        priority: read_priority(process.raw())?,
    })
}

pub(crate) fn apply_game_qos(
    target: &ProcessTarget,
    request: GameQosRequest,
    before_mutation: &mut dyn FnMut(&GameQosRestoreRecord) -> Result<(), ProcessError>,
) -> Result<GameQosApplyReport, ProcessError> {
    let process = open_validated_process(target)?;
    let prior = read_game_qos(process.raw())?;
    if !request.disable_execution_speed_throttling || !prior.execution_speed_throttled {
        return Ok(GameQosApplyReport {
            prior,
            applied: prior,
            restore_record: None,
        });
    }

    let creation_time_100ns = read_creation_time(process.raw())?;
    let requested = GameQosState {
        execution_speed_throttled: false,
    };
    let restore_record = GameQosRestoreRecord {
        pid: target.pid(),
        creation_time_100ns,
        canonical_image: target.expected_executable().to_path_buf(),
        prior,
        applied: requested,
    };
    before_mutation(&restore_record)?;
    write_game_qos(process.raw(), requested)?;
    let applied = read_game_qos(process.raw())?;
    if applied != requested {
        return Err(ProcessError::OperationFailed);
    }
    Ok(GameQosApplyReport {
        prior,
        applied,
        restore_record: Some(restore_record),
    })
}

pub(crate) fn restore_game_qos(
    target: &ProcessTarget,
    record: &GameQosRestoreRecord,
) -> Result<GameQosRestoreOutcome, ProcessError> {
    if record.pid != target.pid() {
        return Err(ProcessError::InvalidProcessId);
    }
    let process = open_validated_process(target)?;
    let creation_time = read_creation_time(process.raw())?;
    let current = read_game_qos(process.raw())?;
    match classify_game_qos_restore(
        record.creation_time_100ns,
        creation_time,
        current,
        record.applied,
    ) {
        GameQosRestoreGuard::IdentityChanged => Ok(GameQosRestoreOutcome::IdentityChanged),
        GameQosRestoreGuard::ExternallyChanged => Ok(GameQosRestoreOutcome::ExternallyChanged),
        GameQosRestoreGuard::Restore => {
            write_game_qos(process.raw(), record.prior)?;
            (read_game_qos(process.raw())? == record.prior)
                .then_some(GameQosRestoreOutcome::Restored)
                .ok_or(ProcessError::OperationFailed)
        }
    }
}

fn read_game_qos(process: HANDLE) -> Result<GameQosState, ProcessError> {
    let mut state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ..PROCESS_POWER_THROTTLING_STATE::default()
    };
    // SAFETY: state is a writable structure of the exact documented size.
    unsafe {
        GetProcessInformation(
            process,
            ProcessPowerThrottling,
            (&mut state as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
            u32::try_from(size_of::<PROCESS_POWER_THROTTLING_STATE>())
                .map_err(|_| ProcessError::BufferTooLarge)?,
        )
    }
    .map_err(|error| ProcessError::from_win32(raw_error_code(&error)))?;
    Ok(GameQosState {
        execution_speed_throttled: state.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0,
    })
}

fn write_game_qos(process: HANDLE, requested: GameQosState) -> Result<(), ProcessError> {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if requested.execution_speed_throttled {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };
    // SAFETY: state is an initialized structure of the exact documented size.
    unsafe {
        SetProcessInformation(
            process,
            ProcessPowerThrottling,
            (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            u32::try_from(size_of::<PROCESS_POWER_THROTTLING_STATE>())
                .map_err(|_| ProcessError::BufferTooLarge)?,
        )
    }
    .map_err(|error| ProcessError::from_win32(raw_error_code(&error)))
}

fn read_creation_time(process: HANDLE) -> Result<u64, ProcessError> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all pointers reference writable FILETIME values for this valid process handle.
    unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }
        .map_err(|error| ProcessError::from_win32(raw_error_code(&error)))?;
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

fn field_report<T>(requested: T, result: Result<T, ProcessError>) -> FieldReport<T> {
    match result {
        Ok(applied) => FieldReport {
            requested,
            applied: Some(applied),
            error_code: None,
        },
        Err(error) => FieldReport {
            requested,
            applied: None,
            error_code: Some(error.code().to_owned()),
        },
    }
}

fn open_validated_process(target: &ProcessTarget) -> Result<OwnedHandle, ProcessError> {
    let access =
        PROCESS_ACCESS_RIGHTS(PROCESS_QUERY_LIMITED_INFORMATION.0 | PROCESS_SET_INFORMATION.0);
    // SAFETY: the PID and access mask are values; handle ownership is transferred to OwnedHandle.
    let handle = unsafe { OpenProcess(access, false, target.pid()) }
        .map_err(|error| ProcessError::from_open_process_win32(raw_error_code(&error)))?;
    let process = OwnedHandle(handle);
    let actual = query_process_path(process.raw())?;
    if !same_canonical_windows_path(&actual, target.expected_executable())? {
        return Err(ProcessError::InvalidExecutableIdentity);
    }
    if target
        .expected_creation_time_100ns()
        .is_some_and(|expected| read_creation_time(process.raw()) != Ok(expected))
    {
        return Err(ProcessError::InvalidExecutableIdentity);
    }
    Ok(process)
}

fn query_process_path(process: HANDLE) -> Result<PathBuf, ProcessError> {
    let mut buffer = vec![0_u16; MAX_PROCESS_PATH_UNITS];
    let mut length = u32::try_from(buffer.len()).map_err(|_| ProcessError::BufferTooLarge)?;
    // SAFETY: buffer is writable for length UTF-16 units and process has query access.
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .map_err(|error| ProcessError::from_win32(raw_error_code(&error)))?;
    let length = usize::try_from(length).map_err(|_| ProcessError::BufferTooLarge)?;
    if length == 0 || length > buffer.len() {
        return Err(ProcessError::InvalidExecutableIdentity);
    }
    buffer.truncate(length);
    let path = String::from_utf16(&buffer).map_err(|_| ProcessError::InvalidExecutableIdentity)?;
    Ok(PathBuf::from(OsString::from(path)))
}

fn same_canonical_windows_path(left: &Path, right: &Path) -> Result<bool, ProcessError> {
    let left = left
        .canonicalize()
        .map_err(|_| ProcessError::InvalidExecutableIdentity)?;
    let right = right
        .canonicalize()
        .map_err(|_| ProcessError::InvalidExecutableIdentity)?;
    let left = left.as_os_str().to_string_lossy().to_lowercase();
    let right = right.as_os_str().to_string_lossy().to_lowercase();
    Ok(left == right)
}

fn query_topology(process: Option<HANDLE>) -> Result<CpuTopology, ProcessError> {
    let mut required = 0_u32;
    // SAFETY: this is the documented sizing call with no output buffer.
    let first = unsafe { GetSystemCpuSetInformation(None, 0, &mut required, process, None) };
    if !first.as_bool() {
        let code = unsafe { GetLastError() }.0;
        if code != ERROR_INSUFFICIENT_BUFFER {
            return Err(ProcessError::from_win32(code));
        }
    }
    let required = usize::try_from(required).map_err(|_| ProcessError::BufferTooLarge)?;
    if required == 0 {
        return Ok(CpuTopology::default());
    }
    if required > MAX_CPU_SET_BUFFER_BYTES {
        return Err(ProcessError::BufferTooLarge);
    }

    let word_size = size_of::<usize>();
    let word_count = required
        .checked_add(word_size - 1)
        .ok_or(ProcessError::BufferTooLarge)?
        / word_size;
    let mut storage = vec![MaybeUninit::<usize>::uninit(); word_count];
    let capacity = word_count
        .checked_mul(word_size)
        .ok_or(ProcessError::BufferTooLarge)?;
    let capacity_u32 = u32::try_from(capacity).map_err(|_| ProcessError::BufferTooLarge)?;
    let mut returned = 0_u32;
    // SAFETY: storage is aligned, writable for capacity bytes, and parsed only up to returned.
    let second = unsafe {
        GetSystemCpuSetInformation(
            Some(storage.as_mut_ptr().cast::<SYSTEM_CPU_SET_INFORMATION>()),
            capacity_u32,
            &mut returned,
            process,
            None,
        )
    };
    if !second.as_bool() {
        return Err(ProcessError::from_win32(unsafe { GetLastError() }.0));
    }
    let returned = usize::try_from(returned).map_err(|_| ProcessError::BufferTooLarge)?;
    if returned == 0 || returned > capacity {
        return Err(ProcessError::MalformedTopology);
    }
    // SAFETY: Windows initialized exactly returned bytes within storage's allocation.
    let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), returned) };
    let cpu_sets = parse_cpu_set_buffer(bytes)?;
    let groups = groups_from_cpu_sets(&cpu_sets)?;
    Ok(CpuTopology { cpu_sets, groups })
}

fn apply_cpu(
    process: HANDLE,
    topology: &CpuTopology,
    plan: &CpuSetPlan,
) -> Result<CpuReadback, ProcessError> {
    match plan {
        CpuSetPlan::ResetAll => {
            reset_hard_affinity_if_single_group(process)?;
            set_default_cpu_sets(process, None)?;
        }
        CpuSetPlan::CpuSets(ids) => {
            reset_hard_affinity_if_single_group(process)?;
            set_default_cpu_sets(process, Some(ids))?;
        }
        CpuSetPlan::HardAffinity { group, mask } => {
            let groups = read_process_groups(process)?;
            if topology.groups.len() != 1 || groups.as_slice() != [*group] {
                return Err(ProcessError::MultipleProcessorGroups);
            }
            set_default_cpu_sets(process, None)?;
            let mask = usize::try_from(*mask).map_err(|_| ProcessError::InvalidAffinityMask)?;
            // SAFETY: validation proves a nonzero mask within the sole active processor group.
            unsafe { SetProcessAffinityMask(process, mask) }
                .map_err(|error| ProcessError::from_win32(raw_error_code(&error)))?;
        }
    }
    read_cpu(process)
}

fn set_default_cpu_sets(process: HANDLE, ids: Option<&[u32]>) -> Result<(), ProcessError> {
    // SAFETY: ids is either absent for reset or a validated, live slice of CPU Set IDs.
    let applied = unsafe { SetProcessDefaultCpuSets(process, ids) };
    if applied.as_bool() {
        Ok(())
    } else {
        Err(ProcessError::from_win32(unsafe { GetLastError() }.0))
    }
}

fn reset_hard_affinity_if_single_group(process: HANDLE) -> Result<(), ProcessError> {
    if read_process_groups(process)?.len() != 1 {
        return Ok(());
    }
    let mut process_mask = 0_usize;
    let mut system_mask = 0_usize;
    // SAFETY: both mask pointers are valid and process is an open query handle.
    unsafe { GetProcessAffinityMask(process, &mut process_mask, &mut system_mask) }
        .map_err(|error| ProcessError::from_win32(raw_error_code(&error)))?;
    if process_mask != system_mask {
        // SAFETY: system_mask is the scheduler-reported valid mask for this process group.
        unsafe { SetProcessAffinityMask(process, system_mask) }
            .map_err(|error| ProcessError::from_win32(raw_error_code(&error)))?;
    }
    Ok(())
}

fn apply_priority(process: HANDLE, priority: PriorityClass) -> Result<PriorityClass, ProcessError> {
    // SAFETY: win32_value returns exactly one documented priority class constant.
    unsafe { SetPriorityClass(process, PROCESS_CREATION_FLAGS(priority.win32_value())) }
        .map_err(|error| ProcessError::from_win32(raw_error_code(&error)))?;
    read_priority(process)
}

fn read_priority(process: HANDLE) -> Result<PriorityClass, ProcessError> {
    // SAFETY: process is an open query handle.
    let value = unsafe { GetPriorityClass(process) };
    if value == 0 {
        Err(ProcessError::from_win32(unsafe { GetLastError() }.0))
    } else {
        PriorityClass::from_win32(value)
    }
}

fn read_cpu(process: HANDLE) -> Result<CpuReadback, ProcessError> {
    let default_cpu_sets = read_default_cpu_sets(process)?;
    let groups = read_process_groups(process)?;
    let affinity = if groups.len() == 1 {
        let mut process_mask = 0_usize;
        let mut system_mask = 0_usize;
        // SAFETY: both mask pointers are valid and process is an open query handle.
        unsafe { GetProcessAffinityMask(process, &mut process_mask, &mut system_mask) }
            .map_err(|error| ProcessError::from_win32(raw_error_code(&error)))?;
        Some(AffinityReadback {
            process_mask: process_mask as u64,
            system_mask: system_mask as u64,
            groups,
        })
    } else {
        None
    };
    Ok(CpuReadback {
        default_cpu_sets,
        affinity,
    })
}

fn read_default_cpu_sets(process: HANDLE) -> Result<Vec<u32>, ProcessError> {
    let mut required = 0_u32;
    // SAFETY: this is the documented sizing call with no ID buffer.
    let first = unsafe { GetProcessDefaultCpuSets(process, None, &mut required) };
    if !first.as_bool() && unsafe { GetLastError() }.0 != ERROR_INSUFFICIENT_BUFFER {
        return Err(ProcessError::from_win32(unsafe { GetLastError() }.0));
    }
    let required = usize::try_from(required).map_err(|_| ProcessError::BufferTooLarge)?;
    if required == 0 {
        return Ok(Vec::new());
    }
    if required > MAX_CPU_SET_IDS {
        return Err(ProcessError::BufferTooLarge);
    }
    let mut ids = vec![0_u32; required];
    let mut returned = 0_u32;
    // SAFETY: ids is writable and the binding supplies its element count to Windows.
    let second = unsafe { GetProcessDefaultCpuSets(process, Some(&mut ids), &mut returned) };
    if !second.as_bool() {
        return Err(ProcessError::from_win32(unsafe { GetLastError() }.0));
    }
    let returned = usize::try_from(returned).map_err(|_| ProcessError::BufferTooLarge)?;
    if returned > ids.len() {
        return Err(ProcessError::MalformedTopology);
    }
    ids.truncate(returned);
    Ok(ids)
}

fn read_process_groups(process: HANDLE) -> Result<Vec<u16>, ProcessError> {
    let mut required = 0_u16;
    // SAFETY: this is the documented sizing call with a null group array.
    let first = unsafe { GetProcessGroupAffinity(process, &mut required, ptr::null_mut()) };
    if !first.as_bool() && unsafe { GetLastError() }.0 != ERROR_INSUFFICIENT_BUFFER {
        return Err(ProcessError::from_win32(unsafe { GetLastError() }.0));
    }
    let required = usize::from(required);
    if required == 0 || required > MAX_PROCESS_GROUPS {
        return Err(ProcessError::UnsupportedTopology);
    }
    let mut groups = vec![0_u16; required];
    let mut returned = u16::try_from(groups.len()).map_err(|_| ProcessError::BufferTooLarge)?;
    // SAFETY: groups is writable for returned elements and process is a query handle.
    let second = unsafe { GetProcessGroupAffinity(process, &mut returned, groups.as_mut_ptr()) };
    if !second.as_bool() {
        return Err(ProcessError::from_win32(unsafe { GetLastError() }.0));
    }
    let returned = usize::from(returned);
    if returned == 0 || returned > groups.len() {
        return Err(ProcessError::MalformedTopology);
    }
    groups.truncate(returned);
    groups.sort_unstable();
    groups.dedup();
    Ok(groups)
}

fn raw_error_code(error: &windows::core::Error) -> u32 {
    let code = error.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        code & 0x0000_FFFF
    } else {
        code
    }
}
