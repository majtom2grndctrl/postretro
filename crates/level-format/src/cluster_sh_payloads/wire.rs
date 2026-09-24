//! Little-endian wire primitives and named error constructors for id 50.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use super::*;

pub(super) fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

pub(super) fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated slice"),
    )
}

pub(super) fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("validated slice"),
    )
}

pub(super) fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub(super) fn try_vec<T>(
    capacity: usize,
    what: &'static str,
) -> Result<Vec<T>, ClusterShPayloadsError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(capacity)
        .map_err(|_| ClusterShPayloadsError::AllocationFailed(what))?;
    Ok(result)
}

pub(super) fn invalid<T>(message: String) -> Result<T, ClusterShPayloadsError> {
    Err(ClusterShPayloadsError::InvalidData(message))
}

pub(super) fn source_mismatch<T>(message: String) -> Result<T, ClusterShPayloadsError> {
    Err(ClusterShPayloadsError::SourceMismatch(message))
}
