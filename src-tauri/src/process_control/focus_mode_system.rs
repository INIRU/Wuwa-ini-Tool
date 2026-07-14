#[cfg(target_os = "windows")]
mod implementation {
    use std::{
        collections::{BTreeMap, BTreeSet},
        ffi::OsString,
        mem::size_of,
        os::windows::ffi::OsStringExt,
        path::{Path, PathBuf},
    };

    use windows::{
        core::{Interface, BOOL, PWSTR},
        Win32::{
            Foundation::{CloseHandle, FILETIME, HANDLE, HWND, LPARAM, RPC_E_CHANGED_MODE},
            Media::Audio::{
                eRender, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
                IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
            },
            Security::{GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER},
            System::{
                Com::{
                    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL,
                    COINIT_MULTITHREADED,
                },
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                    TH32CS_SNAPPROCESS,
                },
                RemoteDesktop::ProcessIdToSessionId,
                Threading::{
                    GetCurrentProcessId, GetPriorityClass, GetProcessInformation, GetProcessTimes,
                    IsProcessCritical, OpenProcess, OpenProcessToken, ProcessProtectionLevelInfo,
                    QueryFullProcessImageNameW, SetPriorityClass, PROCESS_ACCESS_RIGHTS,
                    PROCESS_CREATION_FLAGS, PROCESS_NAME_WIN32,
                    PROCESS_PROTECTION_LEVEL_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
                    PROCESS_SET_INFORMATION, PROTECTION_LEVEL_NONE,
                },
            },
            UI::WindowsAndMessaging::{
                EnumWindows, GetForegroundWindow, GetShellWindow, GetWindowThreadProcessId,
                IsWindowVisible,
            },
        },
    };

    use crate::{
        game_discovery::{validate_game_executable, GameInstallation},
        profile_store::PriorityClass,
    };

    use super::super::focus_mode::{
        FocusBackend, FocusError, FocusProcessIdentity, FocusProcessSnapshot,
    };

    const MAX_TOKEN_BYTES: usize = 64 * 1024;
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
            // SAFETY: this wrapper exclusively owns a valid non-pseudo handle.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub struct SystemFocusBackend {
        game_image: PathBuf,
        tool_image: PathBuf,
        tool_pid: u32,
        session_id: u32,
        user_sid: Vec<u8>,
    }

    impl SystemFocusBackend {
        pub fn new(installation: &GameInstallation) -> Result<Self, FocusError> {
            let tool_pid = unsafe { GetCurrentProcessId() };
            let tool = open_process(tool_pid, false)?;
            let tool_image = query_process_path(tool.raw())?;
            let validated = validate_game_executable(&installation.executable)
                .map_err(|_| FocusError::BackendFailure)?;
            if validated.game_root != installation.game_root
                || validated.executable != installation.executable
                || validated.engine_ini != installation.engine_ini
            {
                return Err(FocusError::BackendFailure);
            }
            let game_image = installation.executable.clone();
            let mut session_id = 0_u32;
            // SAFETY: session_id is writable and tool_pid identifies the current process.
            unsafe { ProcessIdToSessionId(tool_pid, &mut session_id) }
                .map_err(|_| FocusError::BackendFailure)?;
            let user_sid = query_user_sid(tool.raw())?;
            Ok(Self {
                game_image,
                tool_image,
                tool_pid,
                session_id,
                user_sid,
            })
        }

        fn inspect_process(
            &self,
            pid: u32,
            visible: &BTreeSet<u32>,
            foreground: &BTreeSet<u32>,
            audio: &BTreeSet<u32>,
        ) -> Result<Option<FocusProcessSnapshot>, FocusError> {
            if pid == 0 {
                return Ok(None);
            }
            let process = match open_process(pid, false) {
                Ok(process) => process,
                Err(FocusError::AccessDenied) => {
                    return Ok(Some(denied_snapshot(pid)));
                }
                Err(error) => return Err(error),
            };
            let image = query_process_path(process.raw())?;
            let creation_time_100ns = query_creation_time(process.raw())?;
            let priority = query_priority(process.raw())?;
            let mut process_session = 0_u32;
            // SAFETY: process_session is valid writable storage.
            unsafe { ProcessIdToSessionId(pid, &mut process_session) }
                .map_err(|_| FocusError::BackendFailure)?;
            let same_user = query_user_sid(process.raw())? == self.user_sid;
            let critical = query_critical(process.raw())?;
            let protected = query_protected(process.raw())?;
            let name = image
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            Ok(Some(FocusProcessSnapshot {
                identity: FocusProcessIdentity {
                    pid,
                    creation_time_100ns,
                    canonical_image: image.clone(),
                },
                display_name: image
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("process")
                    .to_owned(),
                priority,
                same_user,
                same_session: process_session == self.session_id,
                session_zero: process_session == 0,
                system_process: pid <= 4,
                protected_process: protected,
                critical_process: critical,
                access_denied: false,
                game_process: paths_match(&image, &self.game_image),
                tool_process: pid == self.tool_pid || paths_match(&image, &self.tool_image),
                launcher_or_overlay: is_launcher_or_overlay(&name),
                foreground_family: foreground.contains(&pid),
                visible_window_family: visible.contains(&pid),
                active_audio: audio.contains(&pid),
            }))
        }
    }

    impl FocusBackend for SystemFocusBackend {
        fn enumerate(&mut self) -> Result<Vec<FocusProcessSnapshot>, FocusError> {
            let entries = process_entries()?;
            let parents = entries.iter().copied().collect::<BTreeMap<_, _>>();
            let shell = shell_process();
            let visible = expand_process_families(&visible_processes()?, &parents, shell);
            let foreground = expand_process_families(
                &foreground_process().into_iter().collect(),
                &parents,
                shell,
            );
            let audio = active_audio_processes()?;
            let mut processes = Vec::new();
            for (pid, _) in entries {
                if let Some(process) = self.inspect_process(pid, &visible, &foreground, &audio)? {
                    processes.push(process);
                }
            }
            Ok(processes)
        }

        fn inspect(&mut self, pid: u32) -> Result<Option<FocusProcessSnapshot>, FocusError> {
            let parents = process_entries()?.into_iter().collect::<BTreeMap<_, _>>();
            let shell = shell_process();
            let visible = expand_process_families(&visible_processes()?, &parents, shell);
            let foreground = expand_process_families(
                &foreground_process().into_iter().collect(),
                &parents,
                shell,
            );
            self.inspect_process(pid, &visible, &foreground, &active_audio_processes()?)
        }

        fn set_priority(
            &mut self,
            identity: &FocusProcessIdentity,
            priority: PriorityClass,
        ) -> Result<(), FocusError> {
            if !matches!(priority, PriorityClass::Normal | PriorityClass::BelowNormal) {
                return Err(FocusError::BackendFailure);
            }
            let process = open_process(identity.pid, true)?;
            let actual = FocusProcessIdentity {
                pid: identity.pid,
                creation_time_100ns: query_creation_time(process.raw())?,
                canonical_image: query_process_path(process.raw())?,
            };
            if !identities_match(&actual, identity) {
                return Err(FocusError::BackendFailure);
            }
            // SAFETY: priority is restricted above to the two documented Focus Mode values.
            unsafe {
                SetPriorityClass(
                    process.raw(),
                    PROCESS_CREATION_FLAGS(priority.win32_value()),
                )
            }
            .map_err(map_windows_error)
        }
    }

    fn open_process(pid: u32, set_priority: bool) -> Result<OwnedHandle, FocusError> {
        let mut access = PROCESS_QUERY_LIMITED_INFORMATION.0;
        if set_priority {
            access |= PROCESS_SET_INFORMATION.0;
        }
        // SAFETY: access contains only query and optional set-information rights.
        unsafe { OpenProcess(PROCESS_ACCESS_RIGHTS(access), false, pid) }
            .map(OwnedHandle)
            .map_err(map_windows_error)
    }

    fn query_process_path(process: HANDLE) -> Result<PathBuf, FocusError> {
        let mut buffer = vec![0_u16; MAX_PROCESS_PATH_UNITS];
        let mut length = buffer.len() as u32;
        // SAFETY: buffer is writable for length UTF-16 units.
        unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        }
        .map_err(map_windows_error)?;
        let length = length as usize;
        if length == 0 || length > buffer.len() {
            return Err(FocusError::BackendFailure);
        }
        buffer.truncate(length);
        let path = PathBuf::from(OsString::from_wide(&buffer));
        path.canonicalize().map_err(|_| FocusError::BackendFailure)
    }

    fn query_creation_time(process: HANDLE) -> Result<u64, FocusError> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: all FILETIME pointers are valid writable storage.
        unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }
            .map_err(map_windows_error)?;
        Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
    }

    fn query_priority(process: HANDLE) -> Result<PriorityClass, FocusError> {
        let value = unsafe { GetPriorityClass(process) };
        PriorityClass::from_win32(value).map_err(|_| FocusError::BackendFailure)
    }

    fn query_user_sid(process: HANDLE) -> Result<Vec<u8>, FocusError> {
        let mut token = HANDLE::default();
        // SAFETY: token is writable and process has limited query access.
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.map_err(map_windows_error)?;
        let token = OwnedHandle(token);
        let mut required = 0_u32;
        let _ = unsafe { GetTokenInformation(token.raw(), TokenUser, None, 0, &mut required) };
        let required_size = required as usize;
        if required_size < size_of::<TOKEN_USER>() || required_size > MAX_TOKEN_BYTES {
            return Err(FocusError::BackendFailure);
        }
        let mut buffer = vec![0_u8; required_size];
        let mut returned = 0_u32;
        // SAFETY: buffer is writable for required bytes and token is a query handle.
        unsafe {
            GetTokenInformation(
                token.raw(),
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                required,
                &mut returned,
            )
        }
        .map_err(map_windows_error)?;
        // SAFETY: GetTokenInformation initialized a TOKEN_USER at the buffer start.
        let user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
        let sid_length = unsafe { GetLengthSid(user.User.Sid) } as usize;
        if returned as usize > buffer.len() || sid_length == 0 || sid_length > returned as usize {
            return Err(FocusError::BackendFailure);
        }
        // SAFETY: the SID pointer is owned by buffer and GetLengthSid bounded its length.
        Ok(
            unsafe { std::slice::from_raw_parts(user.User.Sid.0.cast::<u8>(), sid_length) }
                .to_vec(),
        )
    }

    fn query_critical(process: HANDLE) -> Result<bool, FocusError> {
        let mut critical = BOOL::default();
        unsafe { IsProcessCritical(process, &mut critical) }.map_err(map_windows_error)?;
        Ok(critical.as_bool())
    }

    fn query_protected(process: HANDLE) -> Result<bool, FocusError> {
        let mut information = PROCESS_PROTECTION_LEVEL_INFORMATION::default();
        unsafe {
            GetProcessInformation(
                process,
                ProcessProtectionLevelInfo,
                (&mut information as *mut PROCESS_PROTECTION_LEVEL_INFORMATION).cast(),
                size_of::<PROCESS_PROTECTION_LEVEL_INFORMATION>() as u32,
            )
        }
        .map_err(map_windows_error)?;
        Ok(information.ProtectionLevel != PROTECTION_LEVEL_NONE)
    }

    unsafe extern "system" fn collect_visible_window(hwnd: HWND, context: LPARAM) -> BOOL {
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            let mut pid = 0_u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid != 0 {
                let processes = unsafe { &mut *(context.0 as *mut BTreeSet<u32>) };
                processes.insert(pid);
            }
        }
        true.into()
    }

    fn visible_processes() -> Result<BTreeSet<u32>, FocusError> {
        let mut processes = BTreeSet::new();
        // SAFETY: the callback uses the live set pointer only during this synchronous call.
        unsafe {
            EnumWindows(
                Some(collect_visible_window),
                LPARAM((&mut processes as *mut BTreeSet<u32>) as isize),
            )
        }
        .map_err(|_| FocusError::BackendFailure)?;
        Ok(processes)
    }

    fn process_entries() -> Result<Vec<(u32, u32)>, FocusError> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map(OwnedHandle)
            .map_err(|_| FocusError::BackendFailure)?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..PROCESSENTRY32W::default()
        };
        if unsafe { Process32FirstW(snapshot.raw(), &mut entry) }.is_err() {
            return Err(FocusError::BackendFailure);
        }
        let mut entries = Vec::new();
        loop {
            entries.push((entry.th32ProcessID, entry.th32ParentProcessID));
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            match unsafe { Process32NextW(snapshot.raw(), &mut entry) } {
                Ok(()) => {}
                Err(error) if raw_windows_code(&error) == ERROR_NO_MORE_FILES => break,
                Err(_) => return Err(FocusError::BackendFailure),
            }
        }
        Ok(entries)
    }

    fn expand_process_families(
        seeds: &BTreeSet<u32>,
        parents: &BTreeMap<u32, u32>,
        shell: Option<u32>,
    ) -> BTreeSet<u32> {
        let mut family = seeds.clone();
        for &seed in seeds {
            let mut current = seed;
            for _ in 0..64 {
                let Some(&parent) = parents.get(&current) else {
                    break;
                };
                if parent <= 4 || !family.insert(parent) {
                    break;
                }
                current = parent;
            }
        }
        for &pid in parents.keys() {
            let mut current = pid;
            for _ in 0..64 {
                let Some(&parent) = parents.get(&current) else {
                    break;
                };
                if seeds.contains(&parent) && Some(parent) != shell {
                    family.insert(pid);
                    break;
                }
                if parent <= 4 || parent == current {
                    break;
                }
                current = parent;
            }
        }
        family
    }

    fn foreground_process() -> Option<u32> {
        let window = unsafe { GetForegroundWindow() };
        if window.0.is_null() {
            return None;
        }
        let mut pid = 0_u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        (pid != 0).then_some(pid)
    }

    fn shell_process() -> Option<u32> {
        let window = unsafe { GetShellWindow() };
        if window.0.is_null() {
            return None;
        }
        let mut pid = 0_u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        (pid != 0).then_some(pid)
    }

    fn active_audio_processes() -> Result<BTreeSet<u32>, FocusError> {
        let initialized = match unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
            Ok(()) => true,
            Err(error) if error.code() == RPC_E_CHANGED_MODE => false,
            Err(_) => return Err(FocusError::BackendFailure),
        };
        let result = (|| {
            let enumerator: IMMDeviceEnumerator =
                unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                    .map_err(|_| FocusError::BackendFailure)?;
            let endpoints = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
                .map_err(|_| FocusError::BackendFailure)?;
            let endpoint_count =
                unsafe { endpoints.GetCount() }.map_err(|_| FocusError::BackendFailure)?;
            let mut active = BTreeSet::new();
            for endpoint_index in 0..endpoint_count {
                let endpoint = unsafe { endpoints.Item(endpoint_index) }
                    .map_err(|_| FocusError::BackendFailure)?;
                collect_active_audio_sessions(&endpoint, &mut active)?;
            }
            Ok(active)
        })();
        if initialized {
            unsafe { CoUninitialize() };
        }
        result
    }

    fn collect_active_audio_sessions(
        endpoint: &IMMDevice,
        active: &mut BTreeSet<u32>,
    ) -> Result<(), FocusError> {
        let manager: IAudioSessionManager2 = unsafe { endpoint.Activate(CLSCTX_ALL, None) }
            .map_err(|_| FocusError::BackendFailure)?;
        let sessions =
            unsafe { manager.GetSessionEnumerator() }.map_err(|_| FocusError::BackendFailure)?;
        let count = unsafe { sessions.GetCount() }.map_err(|_| FocusError::BackendFailure)?;
        for index in 0..count {
            let control =
                unsafe { sessions.GetSession(index) }.map_err(|_| FocusError::BackendFailure)?;
            if unsafe { control.GetState() }.map_err(|_| FocusError::BackendFailure)?
                != AudioSessionStateActive
            {
                continue;
            }
            let control: IAudioSessionControl2 =
                control.cast().map_err(|_| FocusError::BackendFailure)?;
            let pid = unsafe { control.GetProcessId() }.map_err(|_| FocusError::BackendFailure)?;
            if pid != 0 {
                active.insert(pid);
            }
        }
        Ok(())
    }

    fn denied_snapshot(pid: u32) -> FocusProcessSnapshot {
        FocusProcessSnapshot {
            identity: FocusProcessIdentity {
                pid,
                creation_time_100ns: 0,
                canonical_image: PathBuf::new(),
            },
            display_name: format!("process-{pid}"),
            priority: PriorityClass::Normal,
            same_user: false,
            same_session: false,
            session_zero: false,
            system_process: pid <= 4,
            protected_process: false,
            critical_process: false,
            access_denied: true,
            game_process: false,
            tool_process: false,
            launcher_or_overlay: false,
            foreground_family: false,
            visible_window_family: false,
            active_audio: false,
        }
    }

    fn is_launcher_or_overlay(name: &str) -> bool {
        name.contains("launcher")
            || name.contains("overlay")
            || name == "steam.exe"
            || name.contains("gamebar")
    }

    fn identities_match(left: &FocusProcessIdentity, right: &FocusProcessIdentity) -> bool {
        left.pid == right.pid
            && left.creation_time_100ns == right.creation_time_100ns
            && paths_match(&left.canonical_image, &right.canonical_image)
    }

    fn paths_match(left: &Path, right: &Path) -> bool {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }

    fn map_windows_error(error: windows::core::Error) -> FocusError {
        let code = raw_windows_code(&error);
        if code == 5 {
            FocusError::AccessDenied
        } else {
            FocusError::BackendFailure
        }
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
pub use implementation::SystemFocusBackend;

#[cfg(not(target_os = "windows"))]
pub struct SystemFocusBackend;

#[cfg(not(target_os = "windows"))]
impl SystemFocusBackend {
    pub fn new(
        _installation: &crate::game_discovery::GameInstallation,
    ) -> Result<Self, super::FocusError> {
        Err(super::FocusError::BackendFailure)
    }
}

#[cfg(not(target_os = "windows"))]
impl super::FocusBackend for SystemFocusBackend {
    fn enumerate(&mut self) -> Result<Vec<super::FocusProcessSnapshot>, super::FocusError> {
        Err(super::FocusError::BackendFailure)
    }

    fn inspect(
        &mut self,
        _pid: u32,
    ) -> Result<Option<super::FocusProcessSnapshot>, super::FocusError> {
        Err(super::FocusError::BackendFailure)
    }

    fn set_priority(
        &mut self,
        _identity: &super::FocusProcessIdentity,
        _priority: crate::profile_store::PriorityClass,
    ) -> Result<(), super::FocusError> {
        Err(super::FocusError::BackendFailure)
    }
}
