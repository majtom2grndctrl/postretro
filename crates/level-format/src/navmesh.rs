// NavMesh section: baked walkable regions joined by portals (the pathfinding query surface).
// See: context/lib/build_pipeline.md §Navigation bake

use crate::FormatError;

/// One walkable region: a cell-space rectangle on the navigation grid plus the
/// floor-height band it spans. Coordinates are in grid cells, min inclusive /
/// max exclusive (`x0 <= x < x1`, `z0 <= z < z1`). World-space cell data and
/// per-cell spans are never serialized — only the grid header is — so a region
/// is reconstructed from its rectangle and the shared grid origin/cell size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavRegion {
    pub x0: u32,
    pub z0: u32,
    pub x1: u32,
    pub z1: u32,
    pub floor_y_min: f32,
    pub floor_y_max: f32,
}

/// One portal: a shared traversable edge between two regions. `left`/`right`
/// are the world-space endpoints of the portal segment (the funnel constraint
/// for path smoothing). `region_a < region_b` and both index the section's
/// final sorted `regions` array.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavPortal {
    pub region_a: u32,
    pub region_b: u32,
    pub left: [f32; 3],
    pub right: [f32; 3],
}

/// NavMesh section: one navigation graph per map, baked for the canonical agent.
///
/// On-disk layout (all little-endian), in body order:
///   u16 version                (section-internal; starts at 1)
///   f32 origin[3]              (grid min corner, world space)
///   f32 cell_size
///   u32 dim_x
///   u32 dim_z
///   f32 agent_radius
///   f32 agent_height
///   f32 step_height
///   f32 max_slope_deg
///   u32 region_count
///   NavRegion * region_count   (24 bytes each; see `REGION_STRIDE`)
///   u32 portal_count
///   NavPortal * portal_count   (32 bytes each; see `PORTAL_STRIDE`)
///
/// Determinism (the build cache keys on these bytes): callers must hand
/// `to_bytes` regions sorted ascending by `(z0, x0, x1, z1, floor_y_min)` and
/// unique, and portals sorted ascending by `(region_a, region_b)` then by
/// `left.x, left.y, left.z` under f32 total order (`to_bits`). Fixed field
/// order plus pre-sorted inputs make `to_bytes` deterministic.
///
/// `portal_count == 0` is valid — a single isolated region. A *present* section
/// always carries at least one region (`region_count >= 1`); `from_bytes`
/// rejects a present-but-region-less section.
#[derive(Debug, Clone, PartialEq)]
pub struct NavMeshSection {
    /// Section-internal format version (the body's first field, distinct from
    /// the container `SectionEntry` version). Bumped when this body layout
    /// changes.
    pub version: u16,
    /// World-space minimum corner of the navigation grid.
    pub origin: [f32; 3],
    pub cell_size: f32,
    pub dim_x: u32,
    pub dim_z: u32,
    pub agent_radius: f32,
    pub agent_height: f32,
    pub step_height: f32,
    pub max_slope_deg: f32,
    /// Sorted, unique walkable regions. Portals index this array.
    pub regions: Vec<NavRegion>,
    /// Sorted region-to-region portals. May be empty (single isolated region).
    pub portals: Vec<NavPortal>,
}

/// Current section-internal version emitted by [`NavMeshSection::to_bytes`].
pub const NAVMESH_VERSION: u16 = 1;

/// Header bytes before the region array: version(2) + origin(12) + cell_size(4)
/// + dim_x(4) + dim_z(4) + agent params(16) + region_count(4).
pub const HEADER_SIZE: usize = 2 + 12 + 4 + 4 + 4 + 16 + 4;
pub const REGION_STRIDE: usize = 24;
/// Bytes for the `portal_count` u32 that precedes the portal array.
pub const PORTAL_COUNT_SIZE: usize = 4;
pub const PORTAL_STRIDE: usize = 32;

impl NavMeshSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let region_count = self.regions.len() as u32;
        let portal_count = self.portals.len() as u32;

        let size = HEADER_SIZE
            + (self.regions.len() * REGION_STRIDE)
            + PORTAL_COUNT_SIZE
            + (self.portals.len() * PORTAL_STRIDE);
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&self.version.to_le_bytes());
        buf.extend_from_slice(&self.origin[0].to_le_bytes());
        buf.extend_from_slice(&self.origin[1].to_le_bytes());
        buf.extend_from_slice(&self.origin[2].to_le_bytes());
        buf.extend_from_slice(&self.cell_size.to_le_bytes());
        buf.extend_from_slice(&self.dim_x.to_le_bytes());
        buf.extend_from_slice(&self.dim_z.to_le_bytes());
        buf.extend_from_slice(&self.agent_radius.to_le_bytes());
        buf.extend_from_slice(&self.agent_height.to_le_bytes());
        buf.extend_from_slice(&self.step_height.to_le_bytes());
        buf.extend_from_slice(&self.max_slope_deg.to_le_bytes());
        buf.extend_from_slice(&region_count.to_le_bytes());

        for region in &self.regions {
            buf.extend_from_slice(&region.x0.to_le_bytes());
            buf.extend_from_slice(&region.z0.to_le_bytes());
            buf.extend_from_slice(&region.x1.to_le_bytes());
            buf.extend_from_slice(&region.z1.to_le_bytes());
            buf.extend_from_slice(&region.floor_y_min.to_le_bytes());
            buf.extend_from_slice(&region.floor_y_max.to_le_bytes());
        }

        buf.extend_from_slice(&portal_count.to_le_bytes());

        for portal in &self.portals {
            buf.extend_from_slice(&portal.region_a.to_le_bytes());
            buf.extend_from_slice(&portal.region_b.to_le_bytes());
            buf.extend_from_slice(&portal.left[0].to_le_bytes());
            buf.extend_from_slice(&portal.left[1].to_le_bytes());
            buf.extend_from_slice(&portal.left[2].to_le_bytes());
            buf.extend_from_slice(&portal.right[0].to_le_bytes());
            buf.extend_from_slice(&portal.right[1].to_le_bytes());
            buf.extend_from_slice(&portal.right[2].to_le_bytes());
        }

        debug_assert_eq!(buf.len(), size);
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(short("navmesh section too short for header".to_string()));
        }

        let version = u16::from_le_bytes([data[0], data[1]]);
        let origin = read_vec3(data, 2);
        let cell_size = read_f32(data, 14);
        let dim_x = read_u32(data, 18);
        let dim_z = read_u32(data, 22);
        let agent_radius = read_f32(data, 26);
        let agent_height = read_f32(data, 30);
        let step_height = read_f32(data, 34);
        let max_slope_deg = read_f32(data, 38);
        let region_count = read_u32(data, 42) as usize;

        // A present section is never region-less; portal_count == 0 is fine.
        if region_count == 0 {
            return Err(invalid(
                "navmesh section present but region_count is 0".to_string(),
            ));
        }

        let regions_end = HEADER_SIZE + region_count * REGION_STRIDE;
        let portal_count_end = regions_end + PORTAL_COUNT_SIZE;
        if data.len() < portal_count_end {
            return Err(short(format!(
                "navmesh section too short: need {portal_count_end} bytes for regions + portal_count, got {}",
                data.len()
            )));
        }

        let mut regions = Vec::with_capacity(region_count);
        let mut offset = HEADER_SIZE;
        for _ in 0..region_count {
            regions.push(NavRegion {
                x0: read_u32(data, offset),
                z0: read_u32(data, offset + 4),
                x1: read_u32(data, offset + 8),
                z1: read_u32(data, offset + 12),
                floor_y_min: read_f32(data, offset + 16),
                floor_y_max: read_f32(data, offset + 20),
            });
            offset += REGION_STRIDE;
        }

        let portal_count = read_u32(data, regions_end) as usize;
        offset = portal_count_end;

        let expected_size = portal_count_end + portal_count * PORTAL_STRIDE;
        if data.len() < expected_size {
            return Err(short(format!(
                "navmesh section too short: need {expected_size} bytes, got {}",
                data.len()
            )));
        }

        let region_count_u32 = region_count as u32;
        let mut portals = Vec::with_capacity(portal_count);
        for _ in 0..portal_count {
            let region_a = read_u32(data, offset);
            let region_b = read_u32(data, offset + 4);
            if region_a >= region_b {
                return Err(invalid(format!(
                    "navmesh portal endpoints not ordered: region_a {region_a} >= region_b {region_b}"
                )));
            }
            if region_b >= region_count_u32 {
                return Err(invalid(format!(
                    "navmesh portal references region {region_b} out of range (region_count {region_count})"
                )));
            }
            portals.push(NavPortal {
                region_a,
                region_b,
                left: read_vec3(data, offset + 8),
                right: read_vec3(data, offset + 20),
            });
            offset += PORTAL_STRIDE;
        }

        Ok(Self {
            version,
            origin,
            cell_size,
            dim_x,
            dim_z,
            agent_radius,
            agent_height,
            step_height,
            max_slope_deg,
            regions,
            portals,
        })
    }
}

fn short(msg: String) -> FormatError {
    FormatError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, msg))
}

fn invalid(msg: String) -> FormatError {
    FormatError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_vec3(data: &[u8], at: usize) -> [f32; 3] {
    [
        read_f32(data, at),
        read_f32(data, at + 4),
        read_f32(data, at + 8),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_region_section() -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [-4.0, 0.0, -8.0],
            cell_size: 0.5,
            dim_x: 32,
            dim_z: 64,
            agent_radius: 0.3,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: vec![NavRegion {
                x0: 0,
                z0: 0,
                x1: 8,
                z1: 8,
                floor_y_min: 0.0,
                floor_y_max: 0.25,
            }],
            portals: Vec::new(),
        }
    }

    /// Two regions stacked in z (z0 differs), joined by one portal.
    fn stacked_region_section() -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            dim_x: 16,
            dim_z: 16,
            agent_radius: 0.35,
            agent_height: 2.0,
            step_height: 0.5,
            max_slope_deg: 50.0,
            // Sorted ascending by (z0, x0, x1, z1, floor_y_min).
            regions: vec![
                NavRegion {
                    x0: 0,
                    z0: 0,
                    x1: 4,
                    z1: 4,
                    floor_y_min: 0.0,
                    floor_y_max: 0.1,
                },
                NavRegion {
                    x0: 0,
                    z0: 4,
                    x1: 4,
                    z1: 8,
                    floor_y_min: 0.5,
                    floor_y_max: 0.6,
                },
            ],
            portals: vec![NavPortal {
                region_a: 0,
                region_b: 1,
                left: [0.0, 0.05, 4.0],
                right: [4.0, 0.55, 4.0],
            }],
        }
    }

    #[test]
    fn single_region_round_trips() {
        let section = single_region_section();
        let bytes = section.to_bytes();
        let restored = NavMeshSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn stacked_regions_with_portal_round_trip() {
        let section = stacked_region_section();
        let bytes = section.to_bytes();
        let restored = NavMeshSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn to_bytes_is_deterministic_for_equal_inputs() {
        // The build cache keys on these bytes, so two encodes of equal data
        // must be byte-identical.
        let a = stacked_region_section();
        let b = stacked_region_section();
        assert_eq!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn version_is_first_body_field() {
        let section = single_region_section();
        let bytes = section.to_bytes();
        assert_eq!(&bytes[0..2], &NAVMESH_VERSION.to_le_bytes());
    }

    #[test]
    fn byte_layout_size_matches_strides() {
        let section = stacked_region_section();
        let bytes = section.to_bytes();
        let expected = HEADER_SIZE
            + section.regions.len() * REGION_STRIDE
            + PORTAL_COUNT_SIZE
            + section.portals.len() * PORTAL_STRIDE;
        assert_eq!(bytes.len(), expected);
        assert_eq!(HEADER_SIZE, 46);
        assert_eq!(REGION_STRIDE, 24);
        assert_eq!(PORTAL_STRIDE, 32);
    }

    #[test]
    fn no_portal_single_region_is_valid() {
        let section = single_region_section();
        assert!(section.portals.is_empty());
        let restored = NavMeshSection::from_bytes(&section.to_bytes()).unwrap();
        assert!(restored.portals.is_empty());
        assert_eq!(restored.regions.len(), 1);
    }

    #[test]
    fn rejects_present_section_with_zero_regions() {
        let mut bytes = single_region_section().to_bytes();
        // region_count is the u32 at offset 42.
        bytes[42..46].copy_from_slice(&0u32.to_le_bytes());
        let err = NavMeshSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_truncated_header() {
        let err = NavMeshSection::from_bytes(&[0u8; 8]).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_truncated_portal_body() {
        let section = stacked_region_section();
        let bytes = section.to_bytes();
        // Drop the last portal's worth of bytes.
        let truncated = &bytes[..bytes.len() - PORTAL_STRIDE];
        let err = NavMeshSection::from_bytes(truncated).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_unordered_portal_endpoints() {
        let mut section = stacked_region_section();
        // region_a must be < region_b; flip them to violate the invariant.
        section.portals[0].region_a = 1;
        section.portals[0].region_b = 0;
        let bytes = section.to_bytes();
        let err = NavMeshSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_portal_region_out_of_range() {
        let mut section = stacked_region_section();
        section.portals[0].region_b = 9; // only 2 regions exist
        let bytes = section.to_bytes();
        let err = NavMeshSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }
}
