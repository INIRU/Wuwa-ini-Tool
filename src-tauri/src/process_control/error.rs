#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessError {
    UnsupportedPlatform,
    InvalidProcessId,
    InvalidExecutableIdentity,
    EmptyCpuSets,
    DuplicateCpuSet,
    CpuSetNotFound,
    CpuSetUnavailable,
    UnsupportedTopology,
    InvalidAffinityMask,
    MultipleProcessorGroups,
    DangerousPriorityNotAcknowledged,
    AccessDenied,
    ProcessExited,
    BufferTooLarge,
    MalformedTopology,
    OperationFailed,
    RecoveryRequired,
    JournalFailure,
}

impl ProcessError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::InvalidProcessId => "invalid_process_id",
            Self::InvalidExecutableIdentity => "invalid_executable_identity",
            Self::EmptyCpuSets => "empty_cpu_sets",
            Self::DuplicateCpuSet => "duplicate_cpu_set",
            Self::CpuSetNotFound => "cpu_set_not_found",
            Self::CpuSetUnavailable => "cpu_set_unavailable",
            Self::UnsupportedTopology => "unsupported_topology",
            Self::InvalidAffinityMask => "invalid_affinity_mask",
            Self::MultipleProcessorGroups => "multiple_processor_groups",
            Self::DangerousPriorityNotAcknowledged => "dangerous_priority_not_acknowledged",
            Self::AccessDenied => "access_denied",
            Self::ProcessExited => "process_exited",
            Self::BufferTooLarge => "buffer_too_large",
            Self::MalformedTopology => "malformed_topology",
            Self::OperationFailed => "operation_failed",
            Self::RecoveryRequired => "recovery_required",
            Self::JournalFailure => "journal_failure",
        }
    }

    pub const fn from_win32(code: u32) -> Self {
        match code {
            5 => Self::AccessDenied,
            6 | 1168 => Self::ProcessExited,
            _ => Self::OperationFailed,
        }
    }

    pub const fn from_open_process_win32(code: u32) -> Self {
        match code {
            5 => Self::AccessDenied,
            6 | 87 | 1168 => Self::ProcessExited,
            _ => Self::OperationFailed,
        }
    }
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ProcessError {}
