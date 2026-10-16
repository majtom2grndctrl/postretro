// Reflection-probe splitting: water a capture point fails leaves as a new probe sharing its box, round by round.
// See: context/lib/resource_management.md §5.1

use std::ops::Range;

use glam::{DVec2, Vec3};
use rayon::prelude::*;

use crate::governor::Governor;

use super::capture_point::{PointClassifier, capture_point_over, nearest_point_on_polygon};
use super::capture_visibility::{DistanceTile, face_by_key};
use super::hit::SceneTracer;
use super::placement::{PLANE_TOLERANCE_M, PlacedProbe, UpFacingFace};
use super::split_triggers::{FaceVerdict, failing_faces, judge_face, mirror_fan};

/// Rounds of splitting. Probes a round makes are captured, then tested in
/// the next; the last round's probes are captured and never tested.
pub(crate) const MAX_SPLIT_ROUNDS: u32 = 3;
/// Failing faces on one plane whose outlines come this close (horizontal, m)
/// are one piece of water and leave as one probe. Pieces a pillar or a push
/// cut separates touch along their cut; water on either side of a pillar
/// does not, so each side gets its own capture point.
pub(crate) const ADJACENT_FACE_M: f64 = 1.0e-3;
/// A split whose capture point lands this close (m) to its parent's would
/// fail as its parent did, so it is never made.
const SAME_POINT_M: f32 = 1.0e-3;

/// A probe of the stage: as placed, or split from another.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StageProbe {
    pub placed: PlacedProbe,
    /// The probe this one split from; `None` for authored and derived ones.
    pub parent: Option<usize>,
}

/// What splitting reads besides the probes and their tiles.
pub(crate) struct SplitScene<'a> {
    pub tracer: &'a SceneTracer<'a>,
    pub classifier: &'a PointClassifier<'a>,
    /// Every up-facing fluid face, in id-52 order.
    pub faces: &'a [UpFacingFace],
    pub max_distance: f32,
    pub governor: &'a Governor,
}

/// Test each probe in `batch` against its own distance tile (`tiles`, the
/// batch's in order) and append the probes its failing water makes, in parent
/// order, then group order; return their index range. Each parent keeps its
/// box, point and capture, and only its bound faces shrink: the faces it
/// keeps passed against that capture, so retesting it would change nothing.
/// Verdicts are computed in parallel and decided serially in probe order,
/// so the result never depends on the worker count.
pub(crate) fn split_round(
    probes: &mut Vec<StageProbe>,
    batch: Range<usize>,
    tiles: &[DistanceTile],
    scene: &SplitScene<'_>,
) -> Range<usize> {
    let fan = mirror_fan();
    let jobs: Vec<(usize, (u32, u32))> = batch
        .clone()
        .flat_map(|i| probes[i].placed.faces.iter().map(move |&key| (i, key)))
        .collect();
    let placed = &*probes;
    let batch_start = batch.start;
    let verdicts: Vec<FaceVerdict> = jobs
        .par_iter()
        .map(|&(probe, key)| {
            let _permit = scene.governor.enter();
            let face = face_by_key(scene.faces, key).expect("a bound face is an up-facing face");
            judge_face(
                scene.tracer,
                &tiles[probe - batch_start],
                placed[probe].placed.capture_point_vec3(),
                face,
                &fan,
                scene.max_distance,
            )
        })
        .collect();

    let start = probes.len();
    let mut next = 0;
    for parent in batch {
        let count = probes[parent].placed.faces.len();
        let children = split_probe(
            parent,
            &probes[parent],
            &verdicts[next..next + count],
            scene,
        );
        next += count;
        for child in children {
            let kept = &mut probes[parent].placed;
            kept.faces.retain(|key| !child.placed.faces.contains(key));
            kept.bound_area = bound_area(&kept.faces, scene.faces);
            probes.push(child);
        }
    }
    start..probes.len()
}

/// The probes `probe`'s failing water makes: one per connected group of its
/// failing faces, in order of each group's first face. When every face
/// fails, the parent keeps the face its point serves the largest fraction
/// of (ties to the earlier face), so it still binds water, and the water a
/// new point can serve better leaves. A split whose capture point
/// would drop is never made; its water stays with the parent.
fn split_probe(
    index: usize,
    probe: &StageProbe,
    verdicts: &[FaceVerdict],
    scene: &SplitScene<'_>,
) -> Vec<StageProbe> {
    let mut failing = failing_faces(verdicts);
    for (verdict, fails) in verdicts.iter().zip(&failing) {
        log::debug!(
            "[ReflectionProbes] split test, probe of entity {} at {:?}: face {:?} seen {:.2}, \
             hidden rays {:.3}{}",
            probe.placed.source_entity,
            probe.placed.capture_point,
            verdict.key,
            verdict.seen_fraction(),
            verdict.hidden_share(),
            if *fails { ", fails" } else { "" },
        );
    }
    if failing.iter().all(|&f| f) {
        let best = (1..verdicts.len()).fold(0, |best, i| {
            if verdicts[i].served_fraction() > verdicts[best].served_fraction() {
                i
            } else {
                best
            }
        });
        failing[best] = false;
    }
    let water: Vec<&UpFacingFace> = verdicts
        .iter()
        .zip(&failing)
        .filter(|&(_, &fails)| fails)
        .map(|(verdict, _)| {
            face_by_key(scene.faces, verdict.key).expect("a bound face is an up-facing face")
        })
        .collect();
    connected_groups(&water)
        .into_iter()
        .filter_map(|group| {
            let members: Vec<&UpFacingFace> = group.iter().map(|&i| water[i]).collect();
            split_over(index, &probe.placed, &members, scene.classifier)
        })
        .collect()
}

/// A probe over `members` sharing `parent`'s box and source entity, its
/// capture point 2 mm above their horizontal area centroid, moved onto their
/// water by the nearest-point rule; `None` when that point would drop.
fn split_over(
    parent: usize,
    placed: &PlacedProbe,
    members: &[&UpFacingFace],
    classifier: &PointClassifier<'_>,
) -> Option<StageProbe> {
    let plane = members
        .iter()
        .map(|m| m.geometry.plane_height())
        .fold(f64::INFINITY, f64::min);
    // A nearest-point move past the wall of a face straddling an authored
    // box clamps back inside: box projection needs the point in its box.
    let point = capture_point_over(members, plane)
        .as_vec3()
        .clamp(Vec3::from(placed.box_min), Vec3::from(placed.box_max));
    if point.distance(placed.capture_point_vec3()) < SAME_POINT_M {
        return None;
    }
    if let Some(fault) = classifier.fault(point.as_dvec3()) {
        log::info!(
            "[ReflectionProbes] a split of the probe of entity {} would capture at {:?}, {fault}; \
             its water stays with that probe",
            placed.source_entity,
            point.to_array(),
        );
        return None;
    }
    let mut faces: Vec<(u32, u32)> = members.iter().map(|m| (m.volume, m.face)).collect();
    faces.sort_unstable();
    Some(StageProbe {
        placed: PlacedProbe {
            source_entity: placed.source_entity,
            box_min: placed.box_min,
            box_max: placed.box_max,
            capture_point: point.to_array(),
            bound_area: members.iter().map(|m| m.geometry.area).sum(),
            faces,
        },
        parent: Some(parent),
    })
}

/// Summed area of `keys`' faces.
pub(crate) fn bound_area(keys: &[(u32, u32)], faces: &[UpFacingFace]) -> f64 {
    keys.iter()
        .filter_map(|&key| face_by_key(faces, key))
        .map(|face| face.geometry.area)
        .sum()
}

/// Connected components of `water` (faces in id-52 order) under "on one
/// plane and within [`ADJACENT_FACE_M`]", as ascending indices, ordered by
/// first member.
fn connected_groups(water: &[&UpFacingFace]) -> Vec<Vec<usize>> {
    let mut root: Vec<usize> = (0..water.len()).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    for a in 0..water.len() {
        for b in a + 1..water.len() {
            let same_plane = (water[a].geometry.plane_height() - water[b].geometry.plane_height())
                .abs()
                < PLANE_TOLERANCE_M;
            if same_plane && outlines_touch(&water[a].outline, &water[b].outline) {
                let (ra, rb) = (find(&mut root, a), find(&mut root, b));
                root[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut slot = vec![usize::MAX; water.len()];
    for i in 0..water.len() {
        let r = find(&mut root, i);
        if slot[r] == usize::MAX {
            slot[r] = groups.len();
            groups.push(Vec::new());
        }
        groups[slot[r]].push(i);
    }
    groups
}

/// Whether two horizontal outlines touch: some vertex of one lies within
/// [`ADJACENT_FACE_M`] of the other. Pieces sharing a cut share a vertex or
/// put one on the other's edge, so this finds every shared edge.
fn outlines_touch(a: &[DVec2], b: &[DVec2]) -> bool {
    let near = |points: &[DVec2], outline: &[DVec2]| {
        points
            .iter()
            .any(|&p| nearest_point_on_polygon(p, outline).1 <= ADJACENT_FACE_M)
    };
    let bounds = |points: &[DVec2]| {
        points.iter().fold(
            (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        )
    };
    let ((alo, ahi), (blo, bhi)) = (bounds(a), bounds(b));
    let gap = (blo - ahi).max(alo - bhi);
    if gap.max_element() > ADJACENT_FACE_M {
        return false;
    }
    near(a, b) || near(b, a)
}

#[cfg(test)]
mod tests {
    use super::super::placement::FaceGeometry;
    use super::*;
    use glam::DVec3;

    fn face(face: u32, x0: f64, z0: f64, x1: f64, z1: f64, y: f64) -> UpFacingFace {
        let points = [
            DVec3::new(x0, y, z0),
            DVec3::new(x0, y, z1),
            DVec3::new(x1, y, z1),
            DVec3::new(x1, y, z0),
        ];
        UpFacingFace {
            volume: 0,
            face,
            geometry: FaceGeometry::from_polygon(&points).unwrap(),
            outline: points.iter().map(|p| DVec2::new(p.x, p.z)).collect(),
        }
    }

    // Two hidden pieces on either side of a pillar leave as two probes; pieces
    // sharing a cut, or a corner, leave as one; a piece at another height
    // never joins.
    #[test]
    fn failing_water_groups_by_touching_pieces_on_one_plane() {
        let west = face(0, -2.0, -1.0, -0.5, 1.0, 0.5);
        let east = face(1, 0.5, -1.0, 2.0, 1.0, 0.5);
        let south = face(2, -0.5, -1.0, 0.5, -0.5, 0.5);
        let corner = face(3, 2.0, 1.0, 3.0, 2.0, 0.5);
        let above = face(4, 2.0, -1.0, 3.0, 1.0, 1.5);
        assert_eq!(connected_groups(&[&west, &east]), vec![vec![0], vec![1]]);
        assert_eq!(
            connected_groups(&[&west, &east, &south]),
            vec![vec![0, 1, 2]],
            "a strip sharing both cuts joins them"
        );
        assert_eq!(
            connected_groups(&[&east, &corner, &above, &west]),
            vec![vec![0, 1], vec![2], vec![3]]
        );
    }
}
