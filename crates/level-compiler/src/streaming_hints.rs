//! Resolve compiler-only SH streaming authoring into finalized runtime IDs.
//!
//! Source-format brush entities are translated into [`MapStreamingHintRegion`]
//! before this module runs. This is deliberately the later, engine-space seam:
//! portal IDs and cell IDs do not exist until the BSP and portal passes finish.

use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::portals::PortalsSection;

use crate::geometry_utils::clip_winding_to_half_spaces;
use crate::map_data::{MapStreamingHintRegion, MapStreamingPriorityRegion};
use crate::partition::Aabb;
use crate::portals::{PORTAL_EPSILON, Portal};

/// A portal/cluster-policy input expressed in finalized runtime ID spaces.
///
/// All lists are canonical: portal and pinned-cell IDs are ascending and
/// unique, while `cell_priorities` is ascending by cell ID and omits priority
/// zero. Keeping the resolved form compiler-owned prevents source-format
/// brush vocabulary from leaking into packing or the shared PRL format.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedStreamingHints {
    pub(crate) seam_portal_ids: Vec<u32>,
    pub(crate) pinned_cell_ids: Vec<u32>,
    pub(crate) cell_priorities: Vec<(u32, u32)>,
}

const MIN_PORTAL_INTERSECTION_AREA_M2: f64 = 1.0e-12;

/// Resolve parsed compiler-only regions after portal and cell IDs are final.
///
/// Portal geometry stays at the compiler's f64 precision for the exact clip;
/// the accompanying packed section supplies the ID space consumed by the
/// directory. Cell overlap intentionally uses packed f32 bounds because those
/// are the runtime bounds named by the emitted directory.
pub(crate) fn resolve_streaming_hints(
    streaming_seam_regions: &[MapStreamingHintRegion],
    stream_resident_regions: &[MapStreamingHintRegion],
    stream_priority_regions: &[MapStreamingPriorityRegion],
    generated_portals: &[Portal],
    packed_portals: &PortalsSection,
    cells: &CellsSection,
) -> anyhow::Result<ResolvedStreamingHints> {
    if streaming_seam_regions.is_empty()
        && stream_resident_regions.is_empty()
        && stream_priority_regions.is_empty()
    {
        return Ok(ResolvedStreamingHints::default());
    }

    validate_packed_portal_ids(generated_portals, packed_portals)?;

    let mut seam_portal_ids = BTreeSet::new();
    for region in streaming_seam_regions {
        let bounds = checked_region_bounds(region, "streaming_seam_volume")?;
        let planes = checked_region_planes(region, "streaming_seam_volume")?;
        let mut matched = false;
        for (portal_index, portal) in generated_portals.iter().enumerate() {
            let Some(portal_bounds) = valid_portal_bounds(portal) else {
                continue;
            };
            if !bounds.intersects(&portal_bounds) || !seam_matches_portal(portal, &planes) {
                continue;
            }
            seam_portal_ids.insert(u32::try_from(portal_index)?);
            matched = true;
        }
        anyhow::ensure!(
            matched,
            "streaming_seam_volume {} matches no generated portal with positive-area through-hull overlap",
            hint_location(region),
        );
    }

    let mut pinned_cell_ids = BTreeSet::new();
    for region in stream_resident_regions {
        let bounds = checked_region_bounds(region, "stream_resident_volume")?;
        let matched = runtime_cells_with_positive_overlap(&bounds, cells);
        anyhow::ensure!(
            !matched.is_empty(),
            "stream_resident_volume {} overlaps no runtime cell by positive volume",
            hint_location(region),
        );
        pinned_cell_ids.extend(matched);
    }

    let mut cell_priorities = BTreeMap::<u32, u32>::new();
    for priority_region in stream_priority_regions {
        let region = &priority_region.region;
        let bounds = checked_region_bounds(region, "stream_priority_region")?;
        let matched = runtime_cells_with_positive_overlap(&bounds, cells);
        anyhow::ensure!(
            !matched.is_empty(),
            "stream_priority_region {} overlaps no runtime cell by positive volume",
            hint_location(region),
        );
        let priority = u32::from(priority_region.priority);
        anyhow::ensure!(
            priority <= 3,
            "stream_priority_region {} has priority {priority} outside 0..=3",
            hint_location(region),
        );
        if priority != 0 {
            for cell_id in matched {
                cell_priorities
                    .entry(cell_id)
                    .and_modify(|current| *current = (*current).max(priority))
                    .or_insert(priority);
            }
        }
    }

    Ok(ResolvedStreamingHints {
        seam_portal_ids: seam_portal_ids.into_iter().collect(),
        pinned_cell_ids: pinned_cell_ids.into_iter().collect(),
        cell_priorities: cell_priorities.into_iter().collect(),
    })
}

fn validate_packed_portal_ids(
    generated_portals: &[Portal],
    packed_portals: &PortalsSection,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        generated_portals.len() == packed_portals.portals.len(),
        "generated portal count {} differs from packed portal count {} while resolving streaming hints",
        generated_portals.len(),
        packed_portals.portals.len(),
    );
    for (portal_id, (generated, packed)) in generated_portals
        .iter()
        .zip(&packed_portals.portals)
        .enumerate()
    {
        let vertex_end = packed
            .vertex_start
            .checked_add(packed.vertex_count)
            .ok_or_else(|| anyhow::anyhow!("packed portal {portal_id} vertex range overflows"))?;
        anyhow::ensure!(
            usize::try_from(vertex_end)? <= packed_portals.vertices.len(),
            "packed portal {portal_id} vertex range exceeds packed vertices",
        );
        anyhow::ensure!(
            usize::try_from(packed.vertex_count)? == generated.polygon.len()
                && packed.front_leaf == u32::try_from(generated.front_leaf)?
                && packed.back_leaf == u32::try_from(generated.back_leaf)?,
            "generated portal {portal_id} does not match its packed runtime portal ID",
        );
    }
    Ok(())
}

fn checked_region_bounds(region: &MapStreamingHintRegion, classname: &str) -> anyhow::Result<Aabb> {
    let bounds = Aabb {
        min: DVec3::from_array(region.min.map(f64::from)),
        max: DVec3::from_array(region.max.map(f64::from)),
    };
    anyhow::ensure!(
        bounds.is_valid()
            && (bounds.max.x - bounds.min.x) > 0.0
            && (bounds.max.y - bounds.min.y) > 0.0
            && (bounds.max.z - bounds.min.z) > 0.0,
        "{classname} {} has an invalid non-positive-volume resolved hull AABB",
        hint_location(region),
    );
    anyhow::ensure!(
        DVec3::from_array(region.source_location.map(f64::from)).is_finite(),
        "{classname} has a non-finite source location",
    );
    Ok(bounds)
}

fn checked_region_planes(
    region: &MapStreamingHintRegion,
    classname: &str,
) -> anyhow::Result<Vec<(DVec3, f64)>> {
    anyhow::ensure!(
        region.planes.len() >= 4,
        "{classname} {} has fewer than four hull planes",
        hint_location(region),
    );
    region
        .planes
        .iter()
        .enumerate()
        .map(|(plane_index, plane)| {
            let normal = DVec3::new(
                f64::from(plane[0]),
                f64::from(plane[1]),
                f64::from(plane[2]),
            );
            let distance = f64::from(plane[3]);
            anyhow::ensure!(
                normal.is_finite()
                    && distance.is_finite()
                    && normal.length_squared() > f64::EPSILON,
                "{classname} {} has invalid hull plane {plane_index}",
                hint_location(region),
            );
            Ok((normal, distance))
        })
        .collect()
}

fn runtime_cells_with_positive_overlap(bounds: &Aabb, cells: &CellsSection) -> Vec<u32> {
    cells
        .cells
        .iter()
        .enumerate()
        .filter_map(|(cell_id, cell)| {
            let cell_bounds = Aabb {
                min: DVec3::from_array(cell.bounds_min.map(f64::from)),
                max: DVec3::from_array(cell.bounds_max.map(f64::from)),
            };
            positive_volume_overlap(bounds, &cell_bounds)
                .then(|| u32::try_from(cell_id).ok())
                .flatten()
        })
        .collect()
}

fn positive_volume_overlap(left: &Aabb, right: &Aabb) -> bool {
    left.max.x.min(right.max.x) > left.min.x.max(right.min.x)
        && left.max.y.min(right.max.y) > left.min.y.max(right.min.y)
        && left.max.z.min(right.max.z) > left.min.z.max(right.min.z)
}

fn valid_portal_bounds(portal: &Portal) -> Option<Aabb> {
    portal_plane(&portal.polygon).map(|_| Aabb::from_points(&portal.polygon))
}

/// A seam must overlap a true portal opening, not merely touch its polygon.
///
/// The first clip deliberately matches portal-generation tolerance. The exact
/// second pass certifies that tolerance did not turn a gap or a boundary touch
/// into an authored seam. A strict interior point in the exact intersection
/// then proves the convex hull has nonzero extent on both portal-plane sides.
fn seam_matches_portal(portal: &Portal, hull_planes: &[(DVec3, f64)]) -> bool {
    let Some((portal_normal, _portal_distance)) = portal_plane(&portal.polygon) else {
        return false;
    };
    let inside_planes: Vec<_> = hull_planes
        .iter()
        .map(|&(normal, distance)| (-normal, -distance))
        .collect();
    let Some(tolerant_intersection) =
        clip_winding_to_half_spaces(portal.polygon.clone(), &inside_planes, PORTAL_EPSILON)
    else {
        return false;
    };
    if polygon_area(&tolerant_intersection) <= MIN_PORTAL_INTERSECTION_AREA_M2 {
        return false;
    }
    let Some(exact_intersection) =
        clip_winding_to_half_spaces(portal.polygon.clone(), &inside_planes, 0.0)
    else {
        return false;
    };
    polygon_area(&exact_intersection) > MIN_PORTAL_INTERSECTION_AREA_M2
        && hull_has_depth_on_both_portal_sides(&exact_intersection, portal_normal, hull_planes)
}

fn hull_has_depth_on_both_portal_sides(
    intersection: &[DVec3],
    portal_normal: DVec3,
    hull_planes: &[(DVec3, f64)],
) -> bool {
    let point = intersection.iter().copied().sum::<DVec3>() / intersection.len() as f64;
    if !point.is_finite() {
        return false;
    }

    let mut positive_depth = f64::INFINITY;
    let mut negative_depth = f64::INFINITY;
    for &(normal, distance) in hull_planes {
        let slack = distance - normal.dot(point);
        if !slack.is_finite() || slack <= 0.0 {
            return false;
        }
        let normal_motion = normal.dot(portal_normal);
        if normal_motion > 0.0 {
            positive_depth = positive_depth.min(slack / normal_motion);
        } else if normal_motion < 0.0 {
            negative_depth = negative_depth.min(slack / -normal_motion);
        }
    }
    positive_depth.is_finite()
        && positive_depth > 0.0
        && negative_depth.is_finite()
        && negative_depth > 0.0
}

/// Validate generated portal geometry before it can turn an authored hint into
/// a directory seam. The generated portal contract is convex and planar; a
/// broken caller conservatively gets no match rather than a guessed cut.
fn portal_plane(vertices: &[DVec3]) -> Option<(DVec3, f64)> {
    if vertices.len() < 3 || vertices.iter().any(|vertex| !vertex.is_finite()) {
        return None;
    }
    let mut unnormalized_normal = DVec3::ZERO;
    for index in 0..vertices.len() {
        unnormalized_normal += vertices[index].cross(vertices[(index + 1) % vertices.len()]);
    }
    if unnormalized_normal.length_squared() <= f64::EPSILON {
        return None;
    }
    let normal = unnormalized_normal.normalize();
    let distance = normal.dot(vertices[0]);
    if vertices
        .iter()
        .any(|vertex| (normal.dot(*vertex) - distance).abs() > PORTAL_EPSILON)
    {
        return None;
    }
    let mut winding_sign = 0.0_f64;
    for index in 0..vertices.len() {
        let edge = vertices[(index + 1) % vertices.len()] - vertices[index];
        if edge.length_squared() <= f64::EPSILON {
            return None;
        }
        let turn = edge
            .cross(vertices[(index + 2) % vertices.len()] - vertices[(index + 1) % vertices.len()])
            .dot(normal);
        if turn.abs() <= f64::EPSILON {
            continue;
        }
        if winding_sign != 0.0 && turn.signum() != winding_sign {
            return None;
        }
        winding_sign = turn.signum();
    }
    (winding_sign != 0.0 && polygon_area(vertices) > MIN_PORTAL_INTERSECTION_AREA_M2)
        .then_some((normal, distance))
}

fn polygon_area(vertices: &[DVec3]) -> f64 {
    if vertices.len() < 3 {
        return 0.0;
    }
    let mut area = DVec3::ZERO;
    for index in 1..vertices.len() - 1 {
        area += (vertices[index] - vertices[0]).cross(vertices[index + 1] - vertices[0]);
    }
    area.length() * 0.5
}

fn hint_location(region: &MapStreamingHintRegion) -> String {
    format!(
        "at ({:.3}, {:.3}, {:.3}) m",
        region.source_location[0], region.source_location[1], region.source_location[2]
    )
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Instant;

    use super::*;
    use postretro_level_format::cells::{CELL_FLAG_DRAWABLE, CellRecord};
    use postretro_level_format::cluster_directory::{
        CLUSTER_HINT_FLAG_PINNED, ClusterDirectorySection,
    };
    use postretro_level_format::portals::PortalRecord;
    use postretro_level_format::{SectionId, read_container, validate_container_bounds};

    fn box_region(min: [f32; 3], max: [f32; 3]) -> MapStreamingHintRegion {
        MapStreamingHintRegion {
            min,
            max,
            planes: vec![
                [1.0, 0.0, 0.0, max[0]],
                [-1.0, 0.0, 0.0, -min[0]],
                [0.0, 1.0, 0.0, max[1]],
                [0.0, -1.0, 0.0, -min[1]],
                [0.0, 0.0, 1.0, max[2]],
                [0.0, 0.0, -1.0, -min[2]],
            ],
            source_location: [0.0, 0.0, 0.0],
        }
    }

    fn cell(min_x: f32, max_x: f32) -> CellRecord {
        CellRecord {
            bounds_min: [min_x, -1.0, -1.0],
            bounds_max: [max_x, 1.0, 1.0],
            flags: CELL_FLAG_DRAWABLE,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
        }
    }

    fn portal_fixture() -> (Vec<Portal>, PortalsSection, CellsSection) {
        let polygon = vec![
            DVec3::new(0.0, -1.0, -1.0),
            DVec3::new(0.0, 1.0, -1.0),
            DVec3::new(0.0, 1.0, 1.0),
            DVec3::new(0.0, -1.0, 1.0),
        ];
        let portals = vec![Portal {
            polygon: polygon.clone(),
            front_leaf: 0,
            back_leaf: 1,
        }];
        let packed = PortalsSection {
            vertices: polygon
                .iter()
                .map(|point| [point.x as f32, point.y as f32, point.z as f32])
                .collect(),
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 4,
                front_leaf: 0,
                back_leaf: 1,
            }],
        };
        let cells = CellsSection {
            cells: vec![cell(-1.0, 0.0), cell(0.0, 1.0)],
            portal_refs: Vec::new(),
        };
        (portals, packed, cells)
    }

    #[test]
    fn resolves_portal_seams_and_canonical_cell_policy() {
        let (portals, packed, cells) = portal_fixture();
        let hints = resolve_streaming_hints(
            &[box_region([-0.25, -0.75, -0.75], [0.25, 0.75, 0.75])],
            &[box_region([0.1, -0.5, -0.5], [0.9, 0.5, 0.5])],
            &[
                MapStreamingPriorityRegion {
                    region: box_region([0.1, -0.5, -0.5], [0.9, 0.5, 0.5]),
                    priority: 2,
                },
                MapStreamingPriorityRegion {
                    region: box_region([0.2, -0.5, -0.5], [0.8, 0.5, 0.5]),
                    priority: 3,
                },
                MapStreamingPriorityRegion {
                    region: box_region([-0.9, -0.5, -0.5], [-0.1, 0.5, 0.5]),
                    priority: 0,
                },
            ],
            &portals,
            &packed,
            &cells,
        )
        .unwrap();

        assert_eq!(hints.seam_portal_ids, vec![0]);
        assert_eq!(hints.pinned_cell_ids, vec![1]);
        assert_eq!(hints.cell_priorities, vec![(1, 3)]);
    }

    #[test]
    fn no_hints_bypass_packed_portal_validation() {
        let (portals, mut packed, cells) = portal_fixture();
        packed.portals.clear();

        let hints = resolve_streaming_hints(&[], &[], &[], &portals, &packed, &cells)
            .expect("no authored hints must preserve the existing no-hint compiler path");

        assert_eq!(hints, ResolvedStreamingHints::default());
    }

    #[test]
    fn seam_face_contact_does_not_match() {
        let (portals, packed, cells) = portal_fixture();
        let error = resolve_streaming_hints(
            &[box_region([0.0, -0.75, -0.75], [0.25, 0.75, 0.75])],
            &[],
            &[],
            &portals,
            &packed,
            &cells,
        )
        .expect_err("one-sided hull must not produce a seam");
        assert!(
            error.to_string().contains("streaming_seam_volume")
                && error.to_string().contains("matches no generated portal"),
            "error must name the authored hint: {error}",
        );
    }

    #[test]
    fn f32_slanted_near_miss_does_not_survive_zero_epsilon_certification() {
        let (portals, packed, cells) = portal_fixture();
        // The slanted lower bound leaves a 0.005 m gap at the portal's
        // nearest corner. That is inside PORTAL_EPSILON for the broad pass,
        // but the source f32 plane must still fail the exact certification.
        let region = MapStreamingHintRegion {
            min: [-0.01, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
            planes: vec![
                [1.0, 0.0, 0.0, 1.0],
                [-1.0, -0.25, 0.0, -0.255],
                [0.0, 1.0, 0.0, 1.0],
                [0.0, -1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0, 1.0],
                [0.0, 0.0, -1.0, 1.0],
            ],
            source_location: [0.0, 0.0, 0.0],
        };
        let error = resolve_streaming_hints(&[region], &[], &[], &portals, &packed, &cells)
            .expect_err("the exact f32 hull plane leaves no portal-area overlap");
        assert!(error.to_string().contains("matches no generated portal"));
    }

    #[test]
    fn cell_face_contact_does_not_count_as_positive_volume_overlap() {
        let (portals, packed, cells) = portal_fixture();
        let error = resolve_streaming_hints(
            &[],
            &[box_region([1.0, -0.5, -0.5], [2.0, 0.5, 0.5])],
            &[],
            &portals,
            &packed,
            &cells,
        )
        .expect_err("cell face contact is not positive volume overlap");
        assert!(error.to_string().contains("stream_resident_volume"));
    }

    #[test]
    fn hinted_doorway_compiler_format_loader_round_trip_is_deterministic() {
        let output_dir = tempfile::tempdir().expect("temporary PRL output directory");
        let first = output_dir.path().join("hinted-door-a.prl");
        let second = output_dir.path().join("hinted-door-b.prl");
        let committed_fixture = committed_hinted_doorway_fixture();

        compile_hinted_doorway(&first);
        let first_directory = directory_from_prl(&first);
        assert!(
            !first_directory.seam_portal_ids.is_empty(),
            "the authored doorway seam must resolve to at least one portal ID",
        );
        let pinned_hint = first_directory
            .cluster_hints
            .iter()
            .find(|hint| hint.flags & CLUSTER_HINT_FLAG_PINNED != 0 && hint.priority == 0)
            .expect("the near-side resident region must emit its own pinned cluster hint");
        assert!(
            first_directory
                .cluster_hints
                .iter()
                .any(|hint| hint.flags == 0 && hint.priority == 3),
            "the far-side priority region must emit a separate unpinned cluster hint",
        );
        let loaded =
            postretro_level_loader::load_prl(first.to_str().expect("temporary path is UTF-8"))
                .expect(
                    "loader must validate the compiler-produced v2 directory and id-50 payload",
                );
        let manifest = loaded
            .sh_stream_manifest()
            .expect("compiler-produced v2 directory must produce a streaming manifest");
        let doorway_seam = manifest
            .seam_portals()
            .iter()
            .find(|seam| seam.front_cluster_id == pinned_hint.cluster_id)
            .expect("the pinned near cluster must have an authored doorway seam");
        assert!(
            !first_directory.cluster_hints.iter().any(|hint| {
                hint.cluster_id == doorway_seam.back_cluster_id
                    && hint.flags & CLUSTER_HINT_FLAG_PINNED != 0
            }),
            "the doorway's exact far endpoint must remain unpinned",
        );

        compile_hinted_doorway(&second);
        for section_id in [SectionId::ClusterDirectory, SectionId::ClusterShPayloads] {
            assert_eq!(
                section_from_container_meta(&first, section_id),
                section_from_container_meta(&second, section_id),
                "two --no-cache bakes must preserve exact {section_id:?} bytes",
            );
            assert_eq!(
                section_from_container_meta(&first, section_id),
                section_from_container_meta(&committed_fixture, section_id),
                "the committed fixture must remain synchronized with exact {section_id:?} bytes",
            );
        }
    }

    fn compile_hinted_doorway(output: &Path) {
        let map = workspace_root().join("content/dev/maps/sh-streaming-hinted-door.map");
        let args = crate::parse_args_from(
            [
                map.to_str().expect("fixture path is UTF-8").to_owned(),
                "--no-cache".to_owned(),
                "--sh-probe-spacing".to_owned(),
                "4".to_owned(),
                "-o".to_owned(),
                output.to_str().expect("temporary path is UTF-8").to_owned(),
            ]
            .into_iter(),
        )
        .expect("hinted doorway arguments must parse");
        let started = Instant::now();
        let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
            crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
        );
        crate::pipeline::run(
            &args,
            None,
            started,
            reporter,
            Arc::new(crate::governor::Governor::new(1, false)),
        )
        .expect("hinted doorway fixture must compile");
    }

    fn committed_hinted_doorway_fixture() -> PathBuf {
        workspace_root().join("content/dev/maps/test-fixtures/sh-streaming-hinted-door.prl")
    }

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|path| path.parent())
            .expect("workspace root")
            .to_path_buf()
    }

    fn directory_from_prl(path: &Path) -> ClusterDirectorySection {
        ClusterDirectorySection::from_bytes(&section_from_container_meta(
            path,
            SectionId::ClusterDirectory,
        ))
        .expect("compiled section 49 must decode")
    }

    /// Extract a section by the production container table's offset and size.
    /// This deliberately avoids an extraction CLI so the cold-bake determinism
    /// test exercises the same PRL framing contract the loader consumes.
    fn section_from_container_meta(path: &Path, section_id: SectionId) -> Vec<u8> {
        let bytes = std::fs::read(path).expect("compiled PRL must be readable");
        let mut cursor = Cursor::new(&bytes);
        let meta = read_container(&mut cursor).expect("compiled PRL container must decode");
        validate_container_bounds(
            &meta,
            u64::try_from(bytes.len()).expect("file size fits u64"),
        )
        .expect("compiled PRL container bounds must validate");
        let entry = meta
            .find_section(section_id as u32)
            .unwrap_or_else(|| panic!("compiled PRL is missing {section_id:?}"));
        let start = usize::try_from(entry.offset).expect("section offset fits usize");
        let end = start
            .checked_add(usize::try_from(entry.size).expect("section size fits usize"))
            .expect("section range does not overflow usize");
        bytes
            .get(start..end)
            .expect("validated section range is available")
            .to_vec()
    }
}
