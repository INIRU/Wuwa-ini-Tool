#[cfg(target_os = "windows")]
mod implementation {
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        mem::{size_of, MaybeUninit},
        os::windows::ffi::OsStringExt,
        path::PathBuf,
        time::Instant,
    };

    use windows::{
        core::{PCWSTR, PWSTR},
        Win32::{
            Foundation::{CloseHandle, FILETIME, HANDLE},
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                    THREADENTRY32,
                },
                Performance::{
                    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
                    PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_CSTATUS_NEW_DATA,
                    PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
                    PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
                },
                Threading::{
                    GetProcessTimes, GetThreadTimes, OpenProcess, OpenThread,
                    QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                    PROCESS_QUERY_LIMITED_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION,
                },
            },
        },
    };

    use super::super::{
        FocusError, FocusProcessIdentity, FocusProcessLoad, FocusTelemetrySample, FocusThresholds,
    };

    const MAX_COUNTER_ARRAY_BYTES: usize = 4 * 1024 * 1024;
    const MAX_COUNTER_ITEMS: usize = 4096;
    const MAX_PROCESS_PATH_UNITS: usize = 32_768;
    const ERROR_NO_MORE_FILES: u32 = 18;

    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        const fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: this wrapper owns the handle returned by a Win32 open/snapshot call.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub struct SystemFocusTelemetrySampler {
        query: PDH_HQUERY,
        logical_counter: PDH_HCOUNTER,
        thresholds: FocusThresholds,
        last_collection: Instant,
        process_times: BTreeMap<FocusProcessIdentity, u64>,
        game_thread_times: BTreeMap<(u32, u64), u64>,
    }

    // SAFETY: PDH query and counter handles are process-local opaque handles and
    // the PDH API does not require calls to stay on the creating thread. Every
    // operation on the sampler requires `&mut self`, and the runtime owns it
    // behind the supervisor mutex, so the query is never accessed concurrently.
    unsafe impl Send for SystemFocusTelemetrySampler {}

    impl SystemFocusTelemetrySampler {
        pub fn new(thresholds: FocusThresholds) -> Result<Self, FocusError> {
            let mut query = PDH_HQUERY::default();
            if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
                return Err(FocusError::TelemetryUnavailable);
            }
            let mut counter = PDH_HCOUNTER::default();
            let path = "\\Processor Information(*)\\% Processor Time\0"
                .encode_utf16()
                .collect::<Vec<_>>();
            if unsafe { PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter) } != 0
                || unsafe { PdhCollectQueryData(query) } != 0
            {
                unsafe { PdhCloseQuery(query) };
                return Err(FocusError::TelemetryUnavailable);
            }
            Ok(Self {
                query,
                logical_counter: counter,
                thresholds: thresholds.bounded(),
                last_collection: Instant::now(),
                process_times: BTreeMap::new(),
                game_thread_times: BTreeMap::new(),
            })
        }

        pub fn sample(
            &mut self,
            game: &FocusProcessIdentity,
            selected: &[FocusProcessIdentity],
            game_foreground: bool,
            protection_triggered: bool,
        ) -> Result<Option<FocusTelemetrySample>, FocusError> {
            let elapsed = self.last_collection.elapsed();
            if elapsed.as_millis() < u128::from(self.thresholds.sample_interval_ms) {
                return Err(FocusError::SampleTooSoon);
            }
            if unsafe { PdhCollectQueryData(self.query) } != 0 {
                return Err(FocusError::TelemetryUnavailable);
            }
            let (total_cpu_basis_points, per_logical_cpu_basis_points) =
                read_processor_utility(self.logical_counter)?;
            let logical_count = per_logical_cpu_basis_points.len().max(1) as u64;
            let elapsed_100ns = (elapsed.as_nanos() / 100).max(1) as u64;

            let game_time = read_exact_process_time(game)?;
            let mut current_process_times = BTreeMap::new();
            current_process_times.insert(game.clone(), game_time);
            let mut selected_process_loads = Vec::new();
            let had_process_baseline = self.process_times.contains_key(game);
            for identity in selected {
                let Ok(total_time) = read_exact_process_time(identity) else {
                    continue;
                };
                if let Some(previous) = self.process_times.get(identity) {
                    selected_process_loads.push(FocusProcessLoad {
                        identity: identity.clone(),
                        cpu_basis_points: utilization_basis_points(
                            total_time.saturating_sub(*previous),
                            elapsed_100ns.saturating_mul(logical_count),
                        ),
                    });
                }
                current_process_times.insert(identity.clone(), total_time);
            }

            let current_threads = read_game_thread_times(game.pid)?;
            let had_thread_baseline = !self.game_thread_times.is_empty();
            let game_hot_thread_basis_points = current_threads
                .iter()
                .filter_map(|(identity, total)| {
                    self.game_thread_times.get(identity).map(|previous| {
                        utilization_basis_points(total.saturating_sub(*previous), elapsed_100ns)
                    })
                })
                .max()
                .unwrap_or(0);

            self.last_collection = Instant::now();
            self.process_times = current_process_times;
            self.game_thread_times = current_threads;
            if !had_process_baseline || !had_thread_baseline {
                return Ok(None);
            }
            Ok(Some(FocusTelemetrySample {
                game_foreground,
                protection_triggered,
                total_cpu_basis_points,
                per_logical_cpu_basis_points,
                game_hot_thread_basis_points,
                selected_process_loads,
            }))
        }
    }

    impl Drop for SystemFocusTelemetrySampler {
        fn drop(&mut self) {
            unsafe { PdhCloseQuery(self.query) };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::SystemFocusTelemetrySampler;

        #[test]
        fn telemetry_sampler_can_move_to_the_serialized_supervisor_thread() {
            fn assert_send<T: Send>() {}

            assert_send::<SystemFocusTelemetrySampler>();
        }
    }

    fn read_processor_utility(counter: PDH_HCOUNTER) -> Result<(u16, Vec<u16>), FocusError> {
        let mut bytes = 0_u32;
        let mut count = 0_u32;
        let first = unsafe {
            PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None)
        };
        if first != PDH_MORE_DATA {
            return Err(FocusError::TelemetryUnavailable);
        }
        let byte_count = bytes as usize;
        if byte_count == 0 || byte_count > MAX_COUNTER_ARRAY_BYTES {
            return Err(FocusError::TelemetryUnavailable);
        }
        let word_count = byte_count
            .checked_add(size_of::<usize>() - 1)
            .ok_or(FocusError::TelemetryUnavailable)?
            / size_of::<usize>();
        let mut storage = vec![MaybeUninit::<usize>::uninit(); word_count];
        let second = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                Some(storage.as_mut_ptr().cast()),
            )
        };
        let item_count = count as usize;
        if second != 0
            || item_count == 0
            || item_count > MAX_COUNTER_ITEMS
            || item_count
                .checked_mul(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>())
                .is_none_or(|required| required > byte_count)
        {
            return Err(FocusError::TelemetryUnavailable);
        }
        let items = unsafe {
            std::slice::from_raw_parts(
                storage.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
                item_count,
            )
        };
        let mut logical = BTreeMap::new();
        let mut total = None;
        for item in items {
            if !matches!(
                item.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            ) {
                continue;
            }
            let value = unsafe { item.FmtValue.Anonymous.doubleValue };
            if !value.is_finite() {
                continue;
            }
            let basis_points = percent_to_basis_points(value);
            let name =
                unsafe { item.szName.to_string() }.map_err(|_| FocusError::TelemetryUnavailable)?;
            if name == "_Total" {
                total = Some(basis_points);
            } else if name.ends_with(",_Total") {
                continue;
            } else {
                logical.insert(name, basis_points);
            }
        }
        let logical = logical.into_values().collect::<Vec<_>>();
        if logical.is_empty() {
            return Err(FocusError::TelemetryUnavailable);
        }
        let total = total.unwrap_or_else(|| {
            (logical.iter().map(|value| u32::from(*value)).sum::<u32>() / logical.len() as u32)
                as u16
        });
        Ok((total, logical))
    }

    fn percent_to_basis_points(percent: f64) -> u16 {
        (percent.clamp(0.0, 100.0) * 100.0).round() as u16
    }

    fn utilization_basis_points(delta: u64, capacity: u64) -> u16 {
        if capacity == 0 {
            return 0;
        }
        ((u128::from(delta) * 10_000 / u128::from(capacity)).min(10_000)) as u16
    }

    fn read_exact_process_time(identity: &FocusProcessIdentity) -> Result<u64, FocusError> {
        let process = unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, identity.pid)
                .map(OwnedHandle)
                .map_err(|_| FocusError::TelemetryUnavailable)?
        };
        let (creation, total) = read_handle_times(process.raw(), false)?;
        let image = query_process_path(process.raw())?;
        if creation != identity.creation_time_100ns
            || !image
                .to_string_lossy()
                .eq_ignore_ascii_case(&identity.canonical_image.to_string_lossy())
        {
            return Err(FocusError::TelemetryUnavailable);
        }
        Ok(total)
    }

    fn read_game_thread_times(pid: u32) -> Result<BTreeMap<(u32, u64), u64>, FocusError> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }
            .map(OwnedHandle)
            .map_err(|_| FocusError::TelemetryUnavailable)?;
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..THREADENTRY32::default()
        };
        if unsafe { Thread32First(snapshot.raw(), &mut entry) }.is_err() {
            return Err(FocusError::TelemetryUnavailable);
        }
        let mut times = BTreeMap::new();
        loop {
            if entry.th32OwnerProcessID == pid {
                if let Ok(thread) = unsafe {
                    OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, entry.th32ThreadID)
                        .map(OwnedHandle)
                } {
                    if let Ok((creation, total)) = read_handle_times(thread.raw(), true) {
                        times.insert((entry.th32ThreadID, creation), total);
                    }
                }
            }
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            match unsafe { Thread32Next(snapshot.raw(), &mut entry) } {
                Ok(()) => {}
                Err(error) if raw_windows_code(&error) == ERROR_NO_MORE_FILES => break,
                Err(_) => return Err(FocusError::TelemetryUnavailable),
            }
        }
        if times.is_empty() {
            Err(FocusError::TelemetryUnavailable)
        } else {
            Ok(times)
        }
    }

    fn read_handle_times(handle: HANDLE, thread: bool) -> Result<(u64, u64), FocusError> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let result = if thread {
            unsafe { GetThreadTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }
        } else {
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }
        };
        result.map_err(|_| FocusError::TelemetryUnavailable)?;
        Ok((
            filetime(creation),
            filetime(kernel).saturating_add(filetime(user)),
        ))
    }

    fn filetime(value: FILETIME) -> u64 {
        (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
    }

    fn query_process_path(process: HANDLE) -> Result<PathBuf, FocusError> {
        let mut buffer = vec![0_u16; MAX_PROCESS_PATH_UNITS];
        let mut length = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        }
        .map_err(|_| FocusError::TelemetryUnavailable)?;
        if length == 0 || length as usize > buffer.len() {
            return Err(FocusError::TelemetryUnavailable);
        }
        buffer.truncate(length as usize);
        PathBuf::from(OsString::from_wide(&buffer))
            .canonicalize()
            .map_err(|_| FocusError::TelemetryUnavailable)
    }

    fn raw_windows_code(error: &windows::core::Error) -> u32 {
        let code = error.code().0 as u32;
        if code & 0xFFFF_0000 == 0x8007_0000 {
            code & 0xFFFF
        } else {
            code
        }
    }
}

#[cfg(target_os = "windows")]
pub use implementation::SystemFocusTelemetrySampler;

#[cfg(not(target_os = "windows"))]
pub struct SystemFocusTelemetrySampler;

#[cfg(not(target_os = "windows"))]
impl SystemFocusTelemetrySampler {
    pub fn new(_thresholds: super::FocusThresholds) -> Result<Self, super::FocusError> {
        Err(super::FocusError::TelemetryUnavailable)
    }

    pub fn sample(
        &mut self,
        _game: &super::FocusProcessIdentity,
        _selected: &[super::FocusProcessIdentity],
        _game_foreground: bool,
        _protection_triggered: bool,
    ) -> Result<Option<super::FocusTelemetrySample>, super::FocusError> {
        Err(super::FocusError::TelemetryUnavailable)
    }
}
