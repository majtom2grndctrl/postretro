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
///
/// Handedness convention: `left`/`right` are oriented for `region_a → region_b`
/// traversal — an agent crossing that way has `left` on its left in the XZ
/// funnel projection (X east, Z north). Equivalently: a constant-X portal
/// crossed toward +X stores `left` at the greater-Z endpoint; a constant-Z
/// portal crossed toward +Z stores `left` at the lesser-X endpoint. Crossing
/// `region_b → region_a` swaps the two (the runtime `oriented_portals` swap).
/// The bake emits endpoints in this order and the runtime funnel's turn tests
/// depend on it; a section that violates it string-pulls to the wrong portal
/// jamb. The `NAVMESH_VERSION` bump to 2 pins this convention — sections baked
/// under version 1 carried unconditionally z-ascending constant-X endpoints and
/// are rejected by `from_bytes`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavPortal {
    pub region_a: u32,
    pub region_b: u32,
    pub left: [f32; 3],
    pub right: [f32; 3],
}

/// A mover gate: `portal` refuses crossings while mover `mover_id` is docked at
/// `waypoint`. `portal` indexes the section's final sorted `portals` array.
/// `waypoint` is the index along the mover's resolved `path` → `next` chain,
/// the runtime's `segment_index`. Records carry identity only; the runtime
/// switches them from dock state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NavMoverGate {
    pub mover_id: u32,
    pub waypoint: u32,
    pub portal: u32,
}

/// A mover link: while mover `mover_id` is docked at `waypoint`, its walkable
/// top joins `region` across the boarding edge `left`–`right`. The edge lies on
/// the mover surface the link leaves, so its height tells that surface apart
/// from any other the mover has at this dock. `left`/`right` follow the
/// [`NavPortal`] handedness convention for mover → region travel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavMoverLink {
    pub mover_id: u32,
    pub waypoint: u32,
    pub region: u32,
    pub left: [f32; 3],
    pub right: [f32; 3],
}

/// NavMesh section: one navigation graph per map, baked for the canonical agent.
///
/// On-disk layout (all little-endian), in body order:
///   u16 version                (section-internal; current version 3)
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
///   u32 gate_count
///   NavMoverGate * gate_count  (12 bytes each; see `GATE_STRIDE`)
///   u32 link_count
///   NavMoverLink * link_count  (36 bytes each; see `LINK_STRIDE`)
///
/// Determinism (the build cache keys on these bytes): callers must hand
/// `to_bytes` regions sorted ascending by `(z0, x0, x1, z1, floor_y_min)` and
/// unique, and portals sorted ascending by `(region_a, region_b)` then by
/// `left.x, left.y, left.z` under f32 total order (`to_bits`). Fixed field
/// order plus pre-sorted inputs make `to_bytes` deterministic. `to_bytes` writes
/// gates sorted by `(mover_id, waypoint, portal)` and links by `(mover_id,
/// waypoint, region, left)` with `left` compared by `to_bits`, both unique.
///
/// `portal_count == 0` is valid — a single isolated region. A *present* section
/// always carries at least one region (`region_count >= 1`); `from_bytes`
/// rejects a present-but-region-less section.
#[derive(Debug, Clone, PartialEq)]
pub struct NavMeshSection {
    /// Section-internal serialized-version gate (the body's first field,
    /// distinct from the container `SectionEntry` version). Bump it when the
    /// body layout or a serialized-data semantic contract changes.
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
    /// Mover gates. Empty on a map with no movers, or decoded from version 2.
    pub gates: Vec<NavMoverGate>,
    /// Mover links. Empty on a map with no movers, or decoded from version 2.
    pub links: Vec<NavMoverLink>,
}

/// Current section-internal version emitted by [`NavMeshSection::to_bytes`].
///
/// Version 3 appends the mover gate and link tables. Version 2 pinned the
/// [`NavPortal`] handedness convention; its body is version 3's without the
/// tables, so `from_bytes` still reads it, as a mesh with no mover records.
/// Version 1 emitted constant-X portal endpoints in a fixed z-ascending order
/// regardless of `region_a`, so its portals disagree with the runtime funnel on
/// which jamb is "left"; `from_bytes` rejects it and any unknown version so a
/// stale section degrades loudly (warn-and-ignore → no navigation) rather than
/// pathing on wrong-handed portals.
pub const NAVMESH_VERSION: u16 = 3;

/// The last version without mover tables: still decodes, with a rebake warning.
const NAVMESH_VERSION_WITHOUT_MOVER_TABLES: u16 = 2;

/// Header bytes before the region array: version(2) + origin(12) + cell_size(4)
/// + dim_x(4) + dim_z(4) + agent params(16) + region_count(4).
pub const HEADER_SIZE: usize = 2 + 12 + 4 + 4 + 4 + 16 + 4;
pub const REGION_STRIDE: usize = 24;
/// Bytes for the `portal_count` u32 that precedes the portal array.
pub const PORTAL_COUNT_SIZE: usize = 4;
pub const PORTAL_STRIDE: usize = 32;
/// Bytes for the `gate_count` or `link_count` u32 that precedes each mover table.
pub const MOVER_TABLE_COUNT_SIZE: usize = 4;
pub const GATE_STRIDE: usize = 12;
pub const LINK_STRIDE: usize = 36;

impl NavMeshSection {
    pub fn byte_len(&self) -> usize {
        body_size(
            self.regions.len(),
            self.portals.len(),
            sorted_unique_gates(&self.gates).len(),
            sorted_unique_links(&self.links).len(),
        )
    }

    /// Always writes the current version, whatever `version` holds: a section
    /// decoded from version 2 re-encodes as version 3 with empty tables.
    pub fn to_bytes(&self) -> Vec<u8> {
        let region_count = self.regions.len() as u32;
        let portal_count = self.portals.len() as u32;
        let gates = sorted_unique_gates(&self.gates);
        let links = sorted_unique_links(&self.links);

        let size = body_size(
            self.regions.len(),
            self.portals.len(),
            gates.len(),
            links.len(),
        );
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&NAVMESH_VERSION.to_le_bytes());
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

        buf.extend_from_slice(&(gates.len() as u32).to_le_bytes());
        for gate in &gates {
            buf.extend_from_slice(&gate.mover_id.to_le_bytes());
            buf.extend_from_slice(&gate.waypoint.to_le_bytes());
            buf.extend_from_slice(&gate.portal.to_le_bytes());
        }

        buf.extend_from_slice(&(links.len() as u32).to_le_bytes());
        for link in &links {
            buf.extend_from_slice(&link.mover_id.to_le_bytes());
            buf.extend_from_slice(&link.waypoint.to_le_bytes());
            buf.extend_from_slice(&link.region.to_le_bytes());
            for coordinate in link.left.iter().chain(&link.right) {
                buf.extend_from_slice(&coordinate.to_le_bytes());
            }
        }

        debug_assert_eq!(buf.len(), size);
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(short("navmesh section too short for header".to_string()));
        }

        let version = u16::from_le_bytes([data[0], data[1]]);
        // Version is the first semantic gate: a stale section (e.g. version 1's
        // wrong-handed constant-X portals) must report the version mismatch, not
        // a downstream structural error. The runtime loader turns this rejection
        // into warn-and-ignore → no navigation, the intended loud degradation.
        if version != NAVMESH_VERSION && version != NAVMESH_VERSION_WITHOUT_MOVER_TABLES {
            return Err(invalid(format!(
                "navmesh section version {version}, expected {NAVMESH_VERSION} — recompile the map"
            )));
        }
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

        let (gates, links) = if version == NAVMESH_VERSION {
            read_mover_tables(data, offset)?
        } else {
            log::warn!(
                "[NavMesh] section version {version} has no mover gates or links, so enemy routes ignore doors and lifts — rebake the map"
            );
            (Vec::new(), Vec::new())
        };

        Ok(Self {
            version: NAVMESH_VERSION,
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
            gates,
            links,
        })
    }
}

fn body_size(regions: usize, portals: usize, gates: usize, links: usize) -> usize {
    HEADER_SIZE
        + regions * REGION_STRIDE
        + PORTAL_COUNT_SIZE
        + portals * PORTAL_STRIDE
        + MOVER_TABLE_COUNT_SIZE
        + gates * GATE_STRIDE
        + MOVER_TABLE_COUNT_SIZE
        + links * LINK_STRIDE
}

/// `gates` in wire order: sorted by `(mover_id, waypoint, portal)`, unique.
pub fn sorted_unique_gates(gates: &[NavMoverGate]) -> Vec<NavMoverGate> {
    let mut sorted = gates.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

/// Order and identity by bit pattern, so the wire order holds for every f32.
/// `right` only breaks ties the wire format leaves open, keeping output stable.
fn link_key(link: &NavMoverLink) -> (u32, u32, u32, [u32; 3], [u32; 3]) {
    (
        link.mover_id,
        link.waypoint,
        link.region,
        link.left.map(f32::to_bits),
        link.right.map(f32::to_bits),
    )
}

/// `links` in wire order: sorted by `(mover_id, waypoint, region, left)`, unique.
pub fn sorted_unique_links(links: &[NavMoverLink]) -> Vec<NavMoverLink> {
    let mut sorted = links.to_vec();
    sorted.sort_unstable_by_key(link_key);
    sorted.dedup_by_key(|link| link_key(link));
    sorted
}

/// Decode the two mover tables starting at `offset`. Counts and strides only:
/// mover, waypoint, portal and region references are checked after mover spawn,
/// since this section cannot see the movers.
fn read_mover_tables(
    data: &[u8],
    offset: usize,
) -> crate::Result<(Vec<NavMoverGate>, Vec<NavMoverLink>)> {
    let gates_start = offset + MOVER_TABLE_COUNT_SIZE;
    if data.len() < gates_start {
        return Err(short(format!(
            "navmesh section too short: need {gates_start} bytes for gate_count, got {}",
            data.len()
        )));
    }
    let gate_count = read_u32(data, offset) as usize;
    let gates_end = gates_start + gate_count * GATE_STRIDE;
    let links_start = gates_end + MOVER_TABLE_COUNT_SIZE;
    if data.len() < links_start {
        return Err(short(format!(
            "navmesh section too short: need {links_start} bytes for gates + link_count, got {}",
            data.len()
        )));
    }
    let link_count = read_u32(data, gates_end) as usize;
    let links_end = links_start + link_count * LINK_STRIDE;
    if data.len() < links_end {
        return Err(short(format!(
            "navmesh section too short: need {links_end} bytes for links, got {}",
            data.len()
        )));
    }

    let gates = data[gates_start..gates_end]
        .as_chunks::<GATE_STRIDE>()
        .0
        .iter()
        .map(|entry| NavMoverGate {
            mover_id: read_u32(entry, 0),
            waypoint: read_u32(entry, 4),
            portal: read_u32(entry, 8),
        })
        .collect();
    let links = data[links_start..links_end]
        .as_chunks::<LINK_STRIDE>()
        .0
        .iter()
        .map(|entry| NavMoverLink {
            mover_id: read_u32(entry, 0),
            waypoint: read_u32(entry, 4),
            region: read_u32(entry, 8),
            left: read_vec3(entry, 12),
            right: read_vec3(entry, 24),
        })
        .collect();
    Ok((gates, links))
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
    use postretro_test_log_capture::LogCapture;

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
            gates: Vec::new(),
            links: Vec::new(),
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
            gates: Vec::new(),
            links: Vec::new(),
        }
    }

    /// `stacked_region_section` with a door gating its portal at both of two
    /// waypoints and a lift linking each region.
    fn mover_record_section() -> NavMeshSection {
        let mut section = stacked_region_section();
        section.gates = vec![
            NavMoverGate {
                mover_id: 2,
                waypoint: 0,
                portal: 0,
            },
            NavMoverGate {
                mover_id: 2,
                waypoint: 1,
                portal: 0,
            },
        ];
        section.links = vec![
            NavMoverLink {
                mover_id: 5,
                waypoint: 0,
                region: 0,
                left: [1.0, 0.0, 2.0],
                right: [1.0, 0.0, 3.0],
            },
            NavMoverLink {
                mover_id: 5,
                waypoint: 1,
                region: 1,
                left: [3.0, 0.5, 6.0],
                right: [3.0, 0.5, 5.0],
            },
        ];
        section
    }

    /// `section`'s body as version 2 wrote it: no mover tables, version field 2.
    fn version_2_bytes(section: &NavMeshSection) -> Vec<u8> {
        let portals_end = HEADER_SIZE
            + section.regions.len() * REGION_STRIDE
            + PORTAL_COUNT_SIZE
            + section.portals.len() * PORTAL_STRIDE;
        let mut bytes = section.to_bytes();
        bytes.truncate(portals_end);
        bytes[0..2].copy_from_slice(&2u16.to_le_bytes());
        bytes
    }

    #[test]
    fn single_region_round_trips() {
        let section = single_region_section();
        let bytes = section.to_bytes();
        let restored = NavMeshSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn mover_tables_round_trip_at_version_3() {
        let section = mover_record_section();
        let bytes = section.to_bytes();
        assert_eq!(&bytes[0..2], &3u16.to_le_bytes());
        assert_eq!(section.byte_len(), bytes.len());
        assert_eq!(NavMeshSection::from_bytes(&bytes).unwrap(), section);
    }

    #[test]
    fn to_bytes_writes_mover_tables_sorted_and_unique() {
        let sorted = mover_record_section();
        let mut shuffled = sorted.clone();
        shuffled.gates.reverse();
        shuffled.gates.push(sorted.gates[0]);
        shuffled.links.reverse();
        shuffled.links.push(sorted.links[1]);

        assert_eq!(shuffled.byte_len(), sorted.byte_len());
        assert_eq!(shuffled.to_bytes(), sorted.to_bytes());
    }

    #[test]
    fn links_at_one_dock_and_region_sort_by_left_bit_pattern() {
        let mut section = stacked_region_section();
        let link = |left_x: f32| NavMoverLink {
            mover_id: 1,
            waypoint: 0,
            region: 0,
            left: [left_x, 0.0, 0.0],
            right: [0.0, 0.0, 1.0],
        };
        // -1.0's bit pattern has the sign bit set, so it sorts after 2.0.
        section.links = vec![link(-1.0), link(2.0)];

        let restored = NavMeshSection::from_bytes(&section.to_bytes()).unwrap();

        assert_eq!(restored.links, vec![link(2.0), link(-1.0)]);
    }

    #[test]
    fn section_without_movers_writes_count_zero_for_both_tables() {
        let section = stacked_region_section();
        let bytes = section.to_bytes();
        let tables = &bytes[bytes.len() - 2 * MOVER_TABLE_COUNT_SIZE..];
        assert_eq!(tables, &[0u8; 8]);
    }

    #[test]
    fn version_2_body_loads_with_empty_tables_and_one_rebake_warning() {
        let section = stacked_region_section();
        let bytes = version_2_bytes(&section);
        let capture = LogCapture::start();

        let restored = NavMeshSection::from_bytes(&bytes).unwrap();

        assert_eq!(restored, section, "decodes as version 3, tables empty");
        capture.assert_logged_once(log::Level::Warn, "rebake the map");
    }

    #[test]
    fn version_3_section_loads_without_a_rebake_warning() {
        let bytes = mover_record_section().to_bytes();
        let capture = LogCapture::start();

        NavMeshSection::from_bytes(&bytes).unwrap();

        capture.assert_not_logged(log::Level::Warn, "rebake the map");
    }

    #[test]
    fn version_2_section_re_encodes_as_version_3() {
        let section = stacked_region_section();
        let decoded = NavMeshSection::from_bytes(&version_2_bytes(&section)).unwrap();
        assert_eq!(decoded.to_bytes(), section.to_bytes());
    }

    #[test]
    fn rejects_every_truncation_of_the_mover_tables() {
        let section = mover_record_section();
        let bytes = section.to_bytes();
        let tables_start = bytes.len()
            - 2 * MOVER_TABLE_COUNT_SIZE
            - section.gates.len() * GATE_STRIDE
            - section.links.len() * LINK_STRIDE;
        for len in tables_start..bytes.len() {
            let err = NavMeshSection::from_bytes(&bytes[..len]).unwrap_err();
            assert!(matches!(err, FormatError::Io(_)), "len {len}: {err:?}");
        }
    }

    #[test]
    fn unchecked_mover_references_decode_for_the_post_spawn_check() {
        // Mover, waypoint, portal and region references are judged after mover
        // spawn, so a record no level could satisfy still decodes here.
        let mut section = stacked_region_section();
        section.gates = vec![NavMoverGate {
            mover_id: 900,
            waypoint: 900,
            portal: 900,
        }];
        section.links = vec![NavMoverLink {
            mover_id: 900,
            waypoint: 900,
            region: 900,
            left: [0.0; 3],
            right: [0.0; 3],
        }];
        let restored = NavMeshSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored, section);
    }

    #[test]
    fn from_bytes_rejects_stale_section_version() {
        // Regression: a version 1 section has wrong-handed constant-X portals.
        // Loading it must report the version mismatch (loud degradation), not
        // silently path on inverted portals.
        let mut bytes = stacked_region_section().to_bytes();
        // version is the u16 at offset 0.
        bytes[0..2].copy_from_slice(&1u16.to_le_bytes());
        let err = NavMeshSection::from_bytes(&bytes).unwrap_err();
        match &err {
            FormatError::Io(io) => {
                assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
                assert!(
                    io.to_string().contains("version"),
                    "message names the version mismatch: {io}"
                );
            }
            other => panic!("expected InvalidData version error, got {other:?}"),
        }
    }

    #[test]
    fn byte_len_matches_navmesh_with_portal_payload() {
        let section = stacked_region_section();
        let bytes = section.to_bytes();
        assert_eq!(section.byte_len(), bytes.len());
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
            + section.portals.len() * PORTAL_STRIDE
            + 2 * MOVER_TABLE_COUNT_SIZE;
        assert_eq!(bytes.len(), expected);
        assert_eq!(HEADER_SIZE, 46);
        assert_eq!(REGION_STRIDE, 24);
        assert_eq!(PORTAL_STRIDE, 32);
        assert_eq!(GATE_STRIDE, 12);
        assert_eq!(LINK_STRIDE, 36);
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
