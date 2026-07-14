use crate::profile_store::{CpuSelection, PriorityClass};
use std::collections::HashSet;

use super::{ApplyStatus, CpuReadback, CpuSetPlan, CpuTopology, ProcessError};

pub fn validate_selection(
    topology: &CpuTopology,
    selection: &CpuSelection,
) -> Result<CpuSetPlan, ProcessError> {
    validate_selection_for_mask_bits(topology, selection, usize::BITS)
}

pub fn validate_selection_for_mask_bits(
    topology: &CpuTopology,
    selection: &CpuSelection,
    mask_bits: u32,
) -> Result<CpuSetPlan, ProcessError> {
    match selection {
        CpuSelection::All => Ok(CpuSetPlan::ResetAll),
        CpuSelection::PreferPerformance => performance_plan(topology),
        CpuSelection::ManualCpuSets { ids } => manual_plan(topology, ids),
        CpuSelection::HardAffinity { group, mask } => {
            hard_affinity_plan(topology, *group, *mask, mask_bits)
        }
    }
}

pub fn validate_priority(
    priority: PriorityClass,
    dangerous_priority_acknowledged: bool,
) -> Result<(), ProcessError> {
    if priority.requires_dangerous_ack() && !dangerous_priority_acknowledged {
        Err(ProcessError::DangerousPriorityNotAcknowledged)
    } else {
        Ok(())
    }
}

pub fn classify_apply_status(
    cpu_error: Option<ProcessError>,
    priority_error: Option<ProcessError>,
) -> ApplyStatus {
    match (cpu_error, priority_error) {
        (None, None) => ApplyStatus::Success,
        (Some(left), Some(right)) if is_access_denied(left) && is_access_denied(right) => {
            ApplyStatus::Denied
        }
        (Some(left), Some(right)) if is_unsupported(left) && is_unsupported(right) => {
            ApplyStatus::Unsupported
        }
        (Some(ProcessError::ProcessExited), Some(ProcessError::ProcessExited)) => {
            ApplyStatus::Exited
        }
        _ => ApplyStatus::Partial,
    }
}

pub fn verify_cpu_plan(plan: &CpuSetPlan, readback: &CpuReadback) -> Result<(), ProcessError> {
    let unrestricted_affinity = readback
        .affinity
        .as_ref()
        .is_none_or(|affinity| affinity.process_mask == affinity.system_mask);
    let verified = match plan {
        CpuSetPlan::ResetAll => readback.default_cpu_sets.is_empty() && unrestricted_affinity,
        CpuSetPlan::CpuSets(expected) => {
            let mut expected = expected.clone();
            expected.sort_unstable();
            let mut actual = readback.default_cpu_sets.clone();
            actual.sort_unstable();
            expected == actual && unrestricted_affinity
        }
        CpuSetPlan::HardAffinity { group, mask } => {
            readback.default_cpu_sets.is_empty()
                && readback.affinity.as_ref().is_some_and(|affinity| {
                    affinity.groups.as_slice() == [*group] && affinity.process_mask == *mask
                })
        }
    };
    verified.then_some(()).ok_or(ProcessError::OperationFailed)
}

const fn is_access_denied(error: ProcessError) -> bool {
    matches!(error, ProcessError::AccessDenied)
}

const fn is_unsupported(error: ProcessError) -> bool {
    matches!(
        error,
        ProcessError::UnsupportedPlatform
            | ProcessError::UnsupportedTopology
            | ProcessError::MultipleProcessorGroups
    )
}

fn performance_plan(topology: &CpuTopology) -> Result<CpuSetPlan, ProcessError> {
    let available = available_cpu_sets(topology)?;
    let minimum = available
        .iter()
        .map(|cpu| cpu.efficiency_class)
        .min()
        .ok_or(ProcessError::EmptyCpuSets)?;
    let maximum = available
        .iter()
        .map(|cpu| cpu.efficiency_class)
        .max()
        .ok_or(ProcessError::EmptyCpuSets)?;
    if minimum == maximum {
        return Ok(CpuSetPlan::ResetAll);
    }
    Ok(CpuSetPlan::CpuSets(
        available
            .into_iter()
            .filter(|cpu| cpu.efficiency_class == maximum)
            .map(|cpu| cpu.id)
            .collect(),
    ))
}

fn manual_plan(topology: &CpuTopology, ids: &[u32]) -> Result<CpuSetPlan, ProcessError> {
    if topology.cpu_sets.is_empty() || ids.is_empty() {
        return Err(ProcessError::EmptyCpuSets);
    }
    let mut seen = HashSet::with_capacity(ids.len());
    for id in ids {
        if !seen.insert(*id) {
            return Err(ProcessError::DuplicateCpuSet);
        }
        let cpu_set = topology
            .cpu_sets
            .iter()
            .find(|cpu_set| cpu_set.id == *id)
            .ok_or(ProcessError::CpuSetNotFound)?;
        if cpu_set.allocated && !cpu_set.allocated_to_target {
            return Err(ProcessError::CpuSetUnavailable);
        }
    }
    Ok(CpuSetPlan::CpuSets(ids.to_vec()))
}

fn hard_affinity_plan(
    topology: &CpuTopology,
    group: u16,
    mask: u64,
    mask_bits: u32,
) -> Result<CpuSetPlan, ProcessError> {
    if topology.groups.len() != 1 {
        return Err(if topology.groups.len() > 1 {
            ProcessError::MultipleProcessorGroups
        } else {
            ProcessError::UnsupportedTopology
        });
    }
    let topology_group = &topology.groups[0];
    if topology_group.group != group {
        return Err(ProcessError::UnsupportedTopology);
    }
    let exceeds_word = mask_bits < u64::BITS && mask >> mask_bits != 0;
    if mask == 0 || exceeds_word || mask & !topology_group.active_mask != 0 {
        return Err(ProcessError::InvalidAffinityMask);
    }
    Ok(CpuSetPlan::HardAffinity { group, mask })
}

fn available_cpu_sets(topology: &CpuTopology) -> Result<Vec<&super::CpuSetInfo>, ProcessError> {
    if topology.cpu_sets.is_empty() {
        return Err(ProcessError::EmptyCpuSets);
    }
    let available = topology
        .cpu_sets
        .iter()
        .filter(|cpu_set| !cpu_set.allocated || cpu_set.allocated_to_target)
        .collect::<Vec<_>>();
    if available.is_empty() {
        Err(ProcessError::CpuSetUnavailable)
    } else {
        Ok(available)
    }
}
