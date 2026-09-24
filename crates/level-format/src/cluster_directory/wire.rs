//! Binary codec for the cluster-directory section.
//! See: context/lib/build_pipeline.md §PRL section IDs

use super::{
    CLUSTER_DIRECTORY_VERSION, CLUSTER_HINT_RECORD_SIZE, CLUSTER_RECORD_SIZE,
    ClusterDirectoryError, ClusterDirectorySection, ClusterHintRecord, ClusterRangeRecord,
    ClusterRangeRole, ClusterResourceDomain, ClusterResourceRecord, HEADER_SIZE,
    MEMBER_RECORD_SIZE, RANGE_RECORD_SIZE, RESOURCE_RECORD_SIZE, try_vec, u32_len, usize_count,
};

impl ClusterDirectorySection {
    pub fn byte_len(&self) -> Result<usize, ClusterDirectoryError> {
        checked_wire_len(
            self.clusters.len(),
            self.resources.len(),
            self.members.len(),
            self.ranges.len(),
            self.seam_portal_ids.len(),
            self.cluster_hints.len(),
        )
    }

    pub fn try_to_bytes(&self) -> Result<Vec<u8>, ClusterDirectoryError> {
        self.validate_structure()?;
        let len = self.byte_len()?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| ClusterDirectoryError::AllocationFailed("encoded section"))?;
        push_u32(&mut bytes, CLUSTER_DIRECTORY_VERSION);
        push_u32(&mut bytes, self.runtime_cell_count);
        push_u32(&mut bytes, u32_len(self.clusters.len(), "cluster count")?);
        push_u32(&mut bytes, u32_len(self.resources.len(), "resource count")?);
        push_u32(&mut bytes, u32_len(self.members.len(), "member count")?);
        push_u32(&mut bytes, u32_len(self.ranges.len(), "range count")?);
        push_u32(&mut bytes, self.primitive_limit);
        push_u32(&mut bytes, self.cell_limit);
        push_u32(
            &mut bytes,
            u32_len(self.seam_portal_ids.len(), "seam portal count")?,
        );
        push_u32(
            &mut bytes,
            u32_len(self.cluster_hints.len(), "cluster hint count")?,
        );
        for cluster in &self.clusters {
            for value in cluster.bounds_min.into_iter().chain(cluster.bounds_max) {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            push_u32(&mut bytes, cluster.member_start);
            push_u32(&mut bytes, cluster.member_count);
            push_u32(&mut bytes, cluster.range_start);
            push_u32(&mut bytes, cluster.range_count);
            push_u32(&mut bytes, cluster.primitive_count);
            push_u32(&mut bytes, cluster.flags);
        }
        for resource in &self.resources {
            push_u32(&mut bytes, resource.section_id);
            push_u32(&mut bytes, resource.domain as u32);
            for dimension in resource.dimensions {
                push_u32(&mut bytes, dimension);
            }
            push_u32(&mut bytes, 0);
        }
        for &member in &self.members {
            push_u32(&mut bytes, member);
        }
        for range in &self.ranges {
            push_u32(&mut bytes, range.resource_index);
            push_u32(&mut bytes, range.start);
            push_u32(&mut bytes, range.count);
            push_u32(&mut bytes, range.owner_cluster_id);
            push_u32(&mut bytes, range.role as u32);
            push_u32(&mut bytes, 0);
        }
        for &portal_id in &self.seam_portal_ids {
            push_u32(&mut bytes, portal_id);
        }
        for hint in &self.cluster_hints {
            push_u32(&mut bytes, hint.cluster_id);
            push_u32(&mut bytes, hint.flags);
            push_u32(&mut bytes, hint.priority);
            push_u32(&mut bytes, 0);
        }
        debug_assert_eq!(bytes.len(), len);
        Ok(bytes)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, ClusterDirectoryError> {
        if data.len() < HEADER_SIZE {
            return Err(ClusterDirectoryError::InvalidData(format!(
                "section too short for 40-byte header: got {}",
                data.len()
            )));
        }
        let version = read_u32(data, 0);
        if version != CLUSTER_DIRECTORY_VERSION {
            return Err(ClusterDirectoryError::VersionMismatch {
                version,
                expected: CLUSTER_DIRECTORY_VERSION,
            });
        }
        let runtime_cell_count = read_u32(data, 4);
        let cluster_count = read_u32(data, 8);
        let resource_count = read_u32(data, 12);
        let member_count = read_u32(data, 16);
        let range_count = read_u32(data, 20);
        let primitive_limit = read_u32(data, 24);
        let cell_limit = read_u32(data, 28);
        let seam_portal_count = read_u32(data, 32);
        let cluster_hint_count = read_u32(data, 36);
        let expected = checked_wire_len(
            usize_count(cluster_count)?,
            usize_count(resource_count)?,
            usize_count(member_count)?,
            usize_count(range_count)?,
            usize_count(seam_portal_count)?,
            usize_count(cluster_hint_count)?,
        )?;
        if data.len() != expected {
            return Err(ClusterDirectoryError::InvalidData(format!(
                "length mismatch: expected {expected}, got {}",
                data.len()
            )));
        }

        let mut clusters = try_vec(cluster_count, "cluster records")?;
        let mut cursor = HEADER_SIZE;
        for _ in 0..cluster_count {
            clusters.push(super::ClusterRecord {
                bounds_min: [
                    read_f32(data, cursor),
                    read_f32(data, cursor + 4),
                    read_f32(data, cursor + 8),
                ],
                bounds_max: [
                    read_f32(data, cursor + 12),
                    read_f32(data, cursor + 16),
                    read_f32(data, cursor + 20),
                ],
                member_start: read_u32(data, cursor + 24),
                member_count: read_u32(data, cursor + 28),
                range_start: read_u32(data, cursor + 32),
                range_count: read_u32(data, cursor + 36),
                primitive_count: read_u32(data, cursor + 40),
                flags: read_u32(data, cursor + 44),
            });
            cursor += CLUSTER_RECORD_SIZE;
        }
        let mut resources = try_vec(resource_count, "resource records")?;
        for _ in 0..resource_count {
            let reserved = read_u32(data, cursor + 20);
            if reserved != 0 {
                return Err(ClusterDirectoryError::InvalidData(format!(
                    "resource reserved field must be zero, got {reserved}"
                )));
            }
            resources.push(ClusterResourceRecord {
                section_id: read_u32(data, cursor),
                domain: ClusterResourceDomain::parse(read_u32(data, cursor + 4))?,
                dimensions: [
                    read_u32(data, cursor + 8),
                    read_u32(data, cursor + 12),
                    read_u32(data, cursor + 16),
                ],
            });
            cursor += RESOURCE_RECORD_SIZE;
        }
        let mut members = try_vec(member_count, "members")?;
        for _ in 0..member_count {
            members.push(read_u32(data, cursor));
            cursor += MEMBER_RECORD_SIZE;
        }
        let mut ranges = try_vec(range_count, "ranges")?;
        for _ in 0..range_count {
            let reserved = read_u32(data, cursor + 20);
            if reserved != 0 {
                return Err(ClusterDirectoryError::InvalidData(format!(
                    "range reserved field must be zero, got {reserved}"
                )));
            }
            ranges.push(ClusterRangeRecord {
                resource_index: read_u32(data, cursor),
                start: read_u32(data, cursor + 4),
                count: read_u32(data, cursor + 8),
                owner_cluster_id: read_u32(data, cursor + 12),
                role: ClusterRangeRole::parse(read_u32(data, cursor + 16))?,
            });
            cursor += RANGE_RECORD_SIZE;
        }
        let mut seam_portal_ids = try_vec(seam_portal_count, "seam portal ids")?;
        for _ in 0..seam_portal_count {
            seam_portal_ids.push(read_u32(data, cursor));
            cursor += MEMBER_RECORD_SIZE;
        }
        let mut cluster_hints = try_vec(cluster_hint_count, "cluster hints")?;
        for hint_index in 0..cluster_hint_count {
            let reserved = read_u32(data, cursor + 12);
            if reserved != 0 {
                return Err(ClusterDirectoryError::ClusterHintReserved {
                    hint: hint_index,
                    reserved,
                });
            }
            cluster_hints.push(ClusterHintRecord {
                cluster_id: read_u32(data, cursor),
                flags: read_u32(data, cursor + 4),
                priority: read_u32(data, cursor + 8),
            });
            cursor += CLUSTER_HINT_RECORD_SIZE;
        }
        debug_assert_eq!(cursor, expected);
        let section = Self {
            runtime_cell_count,
            primitive_limit,
            cell_limit,
            clusters,
            resources,
            members,
            ranges,
            seam_portal_ids,
            cluster_hints,
        };
        section.validate_structure()?;
        Ok(section)
    }
}

fn checked_wire_len(
    clusters: usize,
    resources: usize,
    members: usize,
    ranges: usize,
    seam_portals: usize,
    cluster_hints: usize,
) -> Result<usize, ClusterDirectoryError> {
    HEADER_SIZE
        .checked_add(
            clusters
                .checked_mul(CLUSTER_RECORD_SIZE)
                .ok_or(ClusterDirectoryError::SizeOverflow("cluster bytes"))?,
        )
        .and_then(|value| value.checked_add(resources.checked_mul(RESOURCE_RECORD_SIZE)?))
        .and_then(|value| value.checked_add(members.checked_mul(MEMBER_RECORD_SIZE)?))
        .and_then(|value| value.checked_add(ranges.checked_mul(RANGE_RECORD_SIZE)?))
        .and_then(|value| value.checked_add(seam_portals.checked_mul(MEMBER_RECORD_SIZE)?))
        .and_then(|value| value.checked_add(cluster_hints.checked_mul(CLUSTER_HINT_RECORD_SIZE)?))
        .ok_or(ClusterDirectoryError::SizeOverflow("section byte length"))
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        data[offset..offset + 4]
            .try_into()
            .expect("validated section length"),
    )
}

fn read_f32(data: &[u8], offset: usize) -> f32 {
    f32::from_bits(read_u32(data, offset))
}
