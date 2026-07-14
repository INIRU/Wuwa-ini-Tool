use std::collections::{BTreeMap, HashSet};

use super::{CpuSetInfo, ProcessError, ProcessorGroup};

const CPU_SET_RECORD_TYPE: i32 = 0;
const CPU_SET_RECORD_MINIMUM_SIZE: usize = 32;

pub(crate) fn parse_cpu_set_buffer(bytes: &[u8]) -> Result<Vec<CpuSetInfo>, ProcessError> {
    let mut cpu_sets = Vec::new();
    let mut ids = HashSet::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let header_end = offset
            .checked_add(8)
            .ok_or(ProcessError::MalformedTopology)?;
        if header_end > bytes.len() {
            return Err(ProcessError::MalformedTopology);
        }
        let size = read_u32(bytes, offset)? as usize;
        let kind = read_i32(bytes, offset + 4)?;
        if size < 8 {
            return Err(ProcessError::MalformedTopology);
        }
        let record_end = offset
            .checked_add(size)
            .ok_or(ProcessError::MalformedTopology)?;
        if record_end > bytes.len() {
            return Err(ProcessError::MalformedTopology);
        }
        if kind == CPU_SET_RECORD_TYPE {
            if size < CPU_SET_RECORD_MINIMUM_SIZE {
                return Err(ProcessError::MalformedTopology);
            }
            let id = read_u32(bytes, offset + 8)?;
            if !ids.insert(id) {
                return Err(ProcessError::MalformedTopology);
            }
            let flags = bytes[offset + 19];
            cpu_sets.push(CpuSetInfo {
                id,
                group: read_u16(bytes, offset + 12)?,
                logical_processor_index: bytes[offset + 14],
                core_index: bytes[offset + 15],
                last_level_cache_index: bytes[offset + 16],
                numa_node_index: bytes[offset + 17],
                efficiency_class: bytes[offset + 18],
                parked: flags & 0b0001 != 0,
                allocated: flags & 0b0010 != 0,
                allocated_to_target: flags & 0b0100 != 0,
                realtime: flags & 0b1000 != 0,
            });
        }
        offset = record_end;
    }
    Ok(cpu_sets)
}

pub(crate) fn groups_from_cpu_sets(
    cpu_sets: &[CpuSetInfo],
) -> Result<Vec<ProcessorGroup>, ProcessError> {
    let mut masks = BTreeMap::<u16, u64>::new();
    for cpu_set in cpu_sets {
        let bit = 1_u64
            .checked_shl(u32::from(cpu_set.logical_processor_index))
            .ok_or(ProcessError::MalformedTopology)?;
        *masks.entry(cpu_set.group).or_default() |= bit;
    }
    Ok(masks
        .into_iter()
        .map(|(group, active_mask)| ProcessorGroup { group, active_mask })
        .collect())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ProcessError> {
    let end = offset
        .checked_add(2)
        .ok_or(ProcessError::MalformedTopology)?;
    let value = bytes
        .get(offset..end)
        .ok_or(ProcessError::MalformedTopology)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ProcessError> {
    let end = offset
        .checked_add(4)
        .ok_or(ProcessError::MalformedTopology)?;
    let value = bytes
        .get(offset..end)
        .ok_or(ProcessError::MalformedTopology)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, ProcessError> {
    Ok(i32::from_le_bytes(read_u32(bytes, offset)?.to_le_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        size: u32,
        kind: i32,
        id: u32,
        group: u16,
        logical: u8,
        efficiency: u8,
        flags: u8,
    ) -> Vec<u8> {
        let mut bytes = vec![0; size as usize];
        bytes[0..4].copy_from_slice(&size.to_le_bytes());
        bytes[4..8].copy_from_slice(&kind.to_le_bytes());
        if size >= 32 {
            bytes[8..12].copy_from_slice(&id.to_le_bytes());
            bytes[12..14].copy_from_slice(&group.to_le_bytes());
            bytes[14] = logical;
            bytes[15] = logical / 2;
            bytes[16] = 3;
            bytes[17] = 2;
            bytes[18] = efficiency;
            bytes[19] = flags;
        }
        bytes
    }

    #[test]
    fn parses_known_records_and_skips_bounded_unknown_types() {
        let mut bytes = record(32, 0, 7, 0, 1, 4, 0b1111);
        bytes.extend(record(8, 99, 0, 0, 0, 0, 0));
        bytes.extend(record(40, 0, 9, 1, 3, 8, 0));

        let parsed = parse_cpu_set_buffer(&bytes).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, 7);
        assert_eq!(parsed[0].group, 0);
        assert_eq!(parsed[0].logical_processor_index, 1);
        assert_eq!(parsed[0].core_index, 0);
        assert_eq!(parsed[0].last_level_cache_index, 3);
        assert_eq!(parsed[0].numa_node_index, 2);
        assert_eq!(parsed[0].efficiency_class, 4);
        assert!(parsed[0].parked);
        assert!(parsed[0].allocated);
        assert!(parsed[0].allocated_to_target);
        assert!(parsed[0].realtime);
        assert_eq!(parsed[1].id, 9);
    }

    #[test]
    fn rejects_zero_small_and_truncated_record_sizes_without_reading_past_the_buffer() {
        let mut zero = vec![0; 8];
        zero[4..8].copy_from_slice(&0_i32.to_le_bytes());
        assert_eq!(
            parse_cpu_set_buffer(&zero),
            Err(ProcessError::MalformedTopology)
        );

        assert_eq!(
            parse_cpu_set_buffer(&record(16, 0, 0, 0, 0, 0, 0)),
            Err(ProcessError::MalformedTopology)
        );

        let mut truncated = record(32, 0, 0, 0, 0, 0, 0);
        truncated[0..4].copy_from_slice(&40_u32.to_le_bytes());
        assert_eq!(
            parse_cpu_set_buffer(&truncated),
            Err(ProcessError::MalformedTopology)
        );
    }

    #[test]
    fn derives_group_masks_and_rejects_unrepresentable_logical_indices() {
        let cpu_sets = vec![
            CpuSetInfo {
                id: 1,
                group: 2,
                logical_processor_index: 0,
                core_index: 0,
                last_level_cache_index: 0,
                numa_node_index: 0,
                efficiency_class: 0,
                parked: false,
                allocated: false,
                allocated_to_target: false,
                realtime: false,
            },
            CpuSetInfo {
                id: 2,
                group: 2,
                logical_processor_index: 63,
                core_index: 1,
                last_level_cache_index: 0,
                numa_node_index: 0,
                efficiency_class: 0,
                parked: false,
                allocated: false,
                allocated_to_target: false,
                realtime: false,
            },
        ];
        assert_eq!(
            groups_from_cpu_sets(&cpu_sets),
            Ok(vec![ProcessorGroup {
                group: 2,
                active_mask: 1 | (1_u64 << 63),
            }])
        );

        let mut invalid = cpu_sets[0].clone();
        invalid.logical_processor_index = 64;
        assert_eq!(
            groups_from_cpu_sets(&[invalid]),
            Err(ProcessError::MalformedTopology)
        );
    }
}
