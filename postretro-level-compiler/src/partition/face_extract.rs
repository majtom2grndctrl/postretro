// Brush-side face extraction: two-pass projection that walks each brush
// side through the BSP tree, accumulates a per-side visible hull from the
// fragments that survive into empty leaves, then distributes those hulls
// back through the tree to emit one `Face` per empty-leaf fragment.
//
// Plane-equality routing (rather than a numeric front/back test) sidesteps
// every epsilon problem that would otherwise arise when a polygon sits
// exactly on a splitting plane — and the splitter pool is built from the
// same brush-side planes the walker reads back, so equivalence is exact.
//
// Algorithm shape mirrors id Tech 4's `ClipSideByTree_r` /
// `PutWindingIntoAreas_r` (Doom 3 GPL dmap, `usurface.cpp`).
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::DVec3;

use super::types::*;
use crate::geometry_utils::split_polygon;
use crate::map_data::{BrushVolume, Face};

/// Tolerance for polygon splits during Pass 1 / Pass 2 recursion. Matches the
/// generous epsilon the BSP builder uses for face classification.
const SPLIT_EPSILON: f64 = 0.1;

/// Squared L2 tolerance for plane-equivalence tests between a brush side and
/// a BSP node's splitting plane. A 1e-3 effective L2 tolerance (1e-6 squared)
/// is tight on purpose: the splitter pool is built from the same brush-side
/// planes the walker compares against, so the test checks for exact reuse —
/// not approximate parallelism.
const PLANE_NORMAL_EPSILON_SQ: f64 = 1e-6;
const PLANE_DISTANCE_EPSILON: f64 = 1e-4;

/// Tolerance for deduping coplanar brush sides at leaf emission. Looser than
/// the node-equality tolerance because two independently authored brushes
/// may carry slightly different plane coefficients that still collapse to
/// the same visual surface.
const COPLANAR_NORMAL_EPSILON: f64 = 1e-4;
const COPLANAR_DISTANCE_EPSILON: f64 = 1e-3;

/// Diagnostic record emitted when the coplanar dedup rule drops a brush
/// side whose texture differs from the winning side's. Stricter than dmap,
/// which has no tiebreaker — surfacing the conflict gives content authors
/// immediate feedback on overlapping brushes that would otherwise ship as
/// non-deterministic z-fighting.
#[derive(Debug, Clone)]
pub struct CoplanarConflict {
    pub winner_brush: usize,
    pub loser_brush: usize,
    pub winner_texture: String,
    pub loser_texture: String,
}

/// Output of the two-pass face extraction: the emitted face polygons (with
/// `leaf.face_indices` on the tree already populated) plus any texture
/// conflicts the coplanar dedup rule surfaced. Callers log conflicts via
/// `log::warn!` on the boundary; tests inspect the returned vec.
#[derive(Debug, Default)]
pub struct FaceExtractionResult {
    pub faces: Vec<Face>,
    pub coplanar_conflicts: Vec<CoplanarConflict>,
}

/// Extract the final world face list from a completed BSP tree.
///
/// Two passes:
/// 1. Walk each brush side through the tree, accumulating a visible hull
///    from every fragment that survives clipping into an empty leaf.
/// 2. Walk each visible hull back through the tree, emitting one `Face`
///    per empty-leaf fragment and recording its index on that leaf.
///
/// The tree's `leaves[i].face_indices` are populated in place; the
/// returned vec owns the face polygons themselves.
pub fn extract_faces(tree: &mut BspTree, brushes: &[BrushVolume]) -> FaceExtractionResult {
    // Empty tree: nothing to project against.
    if tree.leaves.is_empty() {
        return FaceExtractionResult::default();
    }

    // Clear any prior face assignments so this pass is authoritative.
    for leaf in tree.leaves.iter_mut() {
        leaf.face_indices.clear();
    }

    // Pass 1: build each brush side's visible hull.
    let mut side_records = collect_side_records(brushes);
    for record in side_records.iter_mut() {
        let polygon = record.source_polygon();
        clip_side_by_tree(
            tree,
            tree_root(tree),
            &polygon,
            record.plane_normal,
            record.plane_distance,
            &mut record.visible_hull,
        );
    }

    // Pass 2: distribute surviving hulls into empty leaves. Coplanar dedup
    // runs at leaf emission: when a hull fragment would land in a leaf that
    // already has a face on the same oriented plane, the lower-brush-index
    // side wins. This is stricter than dmap (which has no tiebreaker) and
    // scoped correctly — non-overlapping coplanar sides end up in different
    // leaves and never collide.
    let mut faces: Vec<Face> = Vec::new();
    let mut coplanar_conflicts: Vec<CoplanarConflict> = Vec::new();
    for record in &side_records {
        if record.visible_hull.is_empty() {
            continue;
        }
        put_hull_into_areas(
            tree,
            tree_root(tree),
            &record.visible_hull,
            record,
            &mut faces,
            &mut coplanar_conflicts,
        );
    }

    for conflict in &coplanar_conflicts {
        log::warn!(
            "[Compiler] Coplanar brush sides with mismatched textures: \
             brush {} (tex '{}') wins over brush {} (tex '{}'). \
             Check brush placement — overlapping coplanar surfaces should \
             carry the same material.",
            conflict.winner_brush,
            conflict.winner_texture,
            conflict.loser_brush,
            conflict.loser_texture,
        );
    }

    FaceExtractionResult {
        faces,
        coplanar_conflicts,
    }
}

/// Per-side state carried through Pass 1 and Pass 2.
struct SideRecord {
    brush_index: usize,
    plane_normal: DVec3,
    plane_distance: f64,
    texture: String,
    tex_projection: crate::map_data::TextureProjection,
    source_vertices: Vec<DVec3>,
    /// Accumulated convex hull of every fragment that landed in an empty
    /// leaf during Pass 1.
    visible_hull: Vec<DVec3>,
}

impl SideRecord {
    fn source_polygon(&self) -> Vec<DVec3> {
        self.source_vertices.clone()
    }
}

fn collect_side_records(brushes: &[BrushVolume]) -> Vec<SideRecord> {
    let mut records = Vec::new();
    for (brush_index, brush) in brushes.iter().enumerate() {
        for side in &brush.sides {
            if side.vertices.len() < 3 {
                continue;
            }
            records.push(SideRecord {
                brush_index,
                plane_normal: side.normal,
                plane_distance: side.distance,
                texture: side.texture.clone(),
                tex_projection: side.tex_projection.clone(),
                source_vertices: side.vertices.clone(),
                visible_hull: Vec::new(),
            });
        }
    }
    records
}

/// Returns the root child handle. Empty-node tree (single-leaf or empty)
/// is handled explicitly because the walkers key off node indices.
fn tree_root(tree: &BspTree) -> BspChild {
    if tree.nodes.is_empty() {
        // Single-leaf tree: descent routes straight to leaf 0.
        BspChild::Leaf(0)
    } else {
        BspChild::Node(0)
    }
}

/// Plane relationship between a brush side and a BSP node's splitting plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaneMatch {
    /// Same oriented plane — route polygon to the front child only.
    Same,
    /// Same plane, opposite orientation — route polygon to the back child only.
    Opposite,
    /// Distinct planes — split normally.
    Different,
}

fn classify_plane_vs_node(
    side_normal: DVec3,
    side_distance: f64,
    node_normal: DVec3,
    node_distance: f64,
) -> PlaneMatch {
    if (side_normal - node_normal).length_squared() < PLANE_NORMAL_EPSILON_SQ
        && (side_distance - node_distance).abs() < PLANE_DISTANCE_EPSILON
    {
        return PlaneMatch::Same;
    }
    if (side_normal + node_normal).length_squared() < PLANE_NORMAL_EPSILON_SQ
        && (side_distance + node_distance).abs() < PLANE_DISTANCE_EPSILON
    {
        return PlaneMatch::Opposite;
    }
    PlaneMatch::Different
}

/// Pass 1 recursion: walk a brush side's polygon down the tree, accumulating
/// a visible hull from every fragment that reaches an empty leaf.
fn clip_side_by_tree(
    tree: &BspTree,
    child: BspChild,
    polygon: &[DVec3],
    side_normal: DVec3,
    side_distance: f64,
    visible_hull: &mut Vec<DVec3>,
) {
    if polygon.len() < 3 {
        return;
    }

    match child {
        BspChild::Leaf(idx) => {
            if !tree.leaves[idx].is_solid {
                hull_union_into(visible_hull, polygon, side_normal);
            }
            // Solid leaf: fragment is buried. Drop it.
        }
        BspChild::Node(idx) => {
            let node = &tree.nodes[idx];
            match classify_plane_vs_node(
                side_normal,
                side_distance,
                node.plane_normal,
                node.plane_distance,
            ) {
                PlaneMatch::Same => {
                    // Polygon lies on this node's plane. Route to front
                    // child only — by convention the polygon's outward
                    // normal matches the plane's +normal direction.
                    clip_side_by_tree(
                        tree,
                        node.front.clone(),
                        polygon,
                        side_normal,
                        side_distance,
                        visible_hull,
                    );
                }
                PlaneMatch::Opposite => {
                    // Polygon lies on this node's plane with the opposite
                    // orientation — its outward normal points "backwards"
                    // through the node plane. Route to back.
                    clip_side_by_tree(
                        tree,
                        node.back.clone(),
                        polygon,
                        side_normal,
                        side_distance,
                        visible_hull,
                    );
                }
                PlaneMatch::Different => {
                    let (front, back) = split_polygon(
                        polygon,
                        node.plane_normal,
                        node.plane_distance,
                        SPLIT_EPSILON,
                    );
                    if let Some(front_poly) = front {
                        clip_side_by_tree(
                            tree,
                            node.front.clone(),
                            &front_poly,
                            side_normal,
                            side_distance,
                            visible_hull,
                        );
                    }
                    if let Some(back_poly) = back {
                        clip_side_by_tree(
                            tree,
                            node.back.clone(),
                            &back_poly,
                            side_normal,
                            side_distance,
                            visible_hull,
                        );
                    }
                }
            }
        }
    }
}

/// Pass 2 recursion: walk a brush side's visible hull down the tree,
/// emitting one `Face` per empty-leaf fragment. Routing mirrors Pass 1.
///
/// At leaf emission, runs the containment-aware coplanar resolution rule
/// (see the in-function block comment for the rationale).
fn put_hull_into_areas(
    tree: &mut BspTree,
    child: BspChild,
    polygon: &[DVec3],
    record: &SideRecord,
    faces: &mut Vec<Face>,
    conflicts: &mut Vec<CoplanarConflict>,
) {
    if polygon.len() < 3 {
        return;
    }

    match child {
        BspChild::Leaf(idx) => {
            if tree.leaves[idx].is_solid {
                return;
            }

            // Coplanar dedup rule: containment-aware, not lower-brush-index.
            //
            // Two brush sides on the same oriented plane can reach the same
            // leaf with *different* clipped polygons — the leaf's footprint
            // on the plane can extend past one source's coverage, leaving
            // each brush's contribution as a different shape. A naive
            // wholesale-replace tiebreak ("lower brush index wins, overwrite
            // the slot") then trades a large polygon for a small one and
            // leaves a hole in the world. The previous rule did exactly
            // this and silently dropped ~63% of test-3.map's faces.
            //
            // The rule here keeps coverage by checking containment instead
            // of brush index:
            //   • Incoming polygon is fully inside an existing one → drop
            //     the incoming as redundant (the larger polygon already
            //     covers it).
            //   • Existing polygon is fully inside the incoming one →
            //     unlink the existing from this leaf and emit the incoming
            //     (the new one supersedes the smaller patch).
            //   • Neither contains the other (partial overlap or disjoint)
            //     → emit both and accept z-fighting in any overlap region.
            //
            // The third case is deliberate: partially-overlapping coplanar
            // brushes are a real authoring error this compiler will not
            // try to outsmart, and merging them would require general 2D
            // polygon union. The visible z-fight is the diagnostic.
            //
            // The texture-mismatch warning still fires when a containment
            // resolution drops a side whose texture differs from the
            // surviving side. The warning is "stricter than dmap" — Doom 3
            // has no dedup at all — but it surfaces overlapping brushes
            // with mismatched materials so authors can fix them.
            //
            // Removed entries are unlinked from `leaf.face_indices` but
            // left in the `faces` vec as orphans. The downstream geometry
            // pass walks faces via leaf face-index lists, so orphans are
            // never drawn or packed. Compacting `faces` would invalidate
            // every other leaf's indices and is not worth the bookkeeping
            // for a handful of dropped entries per map.
            let coplanar_existing: Vec<usize> = tree.leaves[idx]
                .face_indices
                .iter()
                .copied()
                .filter(|&fi| {
                    planes_match_oriented(
                        faces[fi].normal,
                        faces[fi].distance,
                        record.plane_normal,
                        record.plane_distance,
                    )
                })
                .collect();

            for &existing_idx in &coplanar_existing {
                if convex_contains(&faces[existing_idx].vertices, polygon, record.plane_normal) {
                    if faces[existing_idx].texture != record.texture {
                        conflicts.push(CoplanarConflict {
                            winner_brush: faces[existing_idx].brush_index,
                            loser_brush: record.brush_index,
                            winner_texture: faces[existing_idx].texture.clone(),
                            loser_texture: record.texture.clone(),
                        });
                    }
                    return;
                }
            }

            let mut newly_orphaned: Vec<usize> = Vec::new();
            for &existing_idx in &coplanar_existing {
                if convex_contains(polygon, &faces[existing_idx].vertices, record.plane_normal) {
                    if faces[existing_idx].texture != record.texture {
                        conflicts.push(CoplanarConflict {
                            winner_brush: record.brush_index,
                            loser_brush: faces[existing_idx].brush_index,
                            winner_texture: record.texture.clone(),
                            loser_texture: faces[existing_idx].texture.clone(),
                        });
                    }
                    newly_orphaned.push(existing_idx);
                }
            }
            if !newly_orphaned.is_empty() {
                tree.leaves[idx]
                    .face_indices
                    .retain(|fi| !newly_orphaned.contains(fi));
            }

            let face_idx = faces.len();
            faces.push(Face {
                vertices: polygon.to_vec(),
                normal: record.plane_normal,
                distance: record.plane_distance,
                texture: record.texture.clone(),
                tex_projection: record.tex_projection.clone(),
                brush_index: record.brush_index,
            });
            tree.leaves[idx].face_indices.push(face_idx);
        }
        BspChild::Node(idx) => {
            // Snapshot node fields so we can release the borrow before
            // recursing with `&mut tree`.
            let node_normal;
            let node_distance;
            let node_front;
            let node_back;
            {
                let node = &tree.nodes[idx];
                node_normal = node.plane_normal;
                node_distance = node.plane_distance;
                node_front = node.front.clone();
                node_back = node.back.clone();
            }

            match classify_plane_vs_node(
                record.plane_normal,
                record.plane_distance,
                node_normal,
                node_distance,
            ) {
                PlaneMatch::Same => {
                    put_hull_into_areas(tree, node_front, polygon, record, faces, conflicts);
                }
                PlaneMatch::Opposite => {
                    put_hull_into_areas(tree, node_back, polygon, record, faces, conflicts);
                }
                PlaneMatch::Different => {
                    let (front, back) =
                        split_polygon(polygon, node_normal, node_distance, SPLIT_EPSILON);
                    if let Some(front_poly) = front {
                        put_hull_into_areas(
                            tree,
                            node_front,
                            &front_poly,
                            record,
                            faces,
                            conflicts,
                        );
                    }
                    if let Some(back_poly) = back {
                        put_hull_into_areas(tree, node_back, &back_poly, record, faces, conflicts);
                    }
                }
            }
        }
    }
}

/// Union a polygon fragment into the running visible hull for a brush side.
///
/// Coplanarity is guaranteed by Pass 1's routing: when the walker hits a
/// node whose plane equals the side's plane (same or opposite orientation),
/// it routes the polygon to a single child without splitting, so no
/// fragment ever leaves the side's plane. The accumulated hull and every
/// incoming fragment therefore live in the same 2D subspace and a plain
/// 2D convex hull on the union of points is the correct accumulation —
/// no separate coplanarity test required.
///
/// Strategy:
/// 1. Build an orthonormal basis for the plane from `side_normal`.
/// 2. Project both the existing hull vertices and the incoming fragment
///    vertices to 2D.
/// 3. Re-run a 2D convex hull (monotone chain) on the combined set.
/// 4. Lift the result back to 3D by inverting the projection.
fn hull_union_into(hull: &mut Vec<DVec3>, fragment: &[DVec3], side_normal: DVec3) {
    if fragment.len() < 3 {
        return;
    }

    // Build an orthonormal basis (u, v) spanning the plane perpendicular
    // to side_normal. Pick the axis least aligned with the normal to avoid
    // a degenerate cross product.
    let n = side_normal.normalize_or_zero();
    if n.length_squared() < 0.5 {
        // Degenerate normal — skip this fragment rather than crash.
        return;
    }
    let helper = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let u = n.cross(helper).normalize();
    let v = n.cross(u).normalize();

    // Use the first hull point (or fragment[0] if hull is empty) as the
    // origin so 2D coordinates stay near zero and the hull arithmetic is
    // well-conditioned.
    let origin = if hull.is_empty() {
        fragment[0]
    } else {
        hull[0]
    };

    let mut points_2d: Vec<(f64, f64)> = Vec::with_capacity(hull.len() + fragment.len());
    for &p in hull.iter() {
        let d = p - origin;
        points_2d.push((d.dot(u), d.dot(v)));
    }
    for &p in fragment.iter() {
        let d = p - origin;
        points_2d.push((d.dot(u), d.dot(v)));
    }

    let hull_2d = monotone_chain_hull(&points_2d);
    if hull_2d.len() < 3 {
        return;
    }

    // Lift back to 3D.
    hull.clear();
    for (x, y) in hull_2d {
        hull.push(origin + u * x + v * y);
    }
}

/// 2D convex hull via Andrew's monotone chain. O(n log n). Produces vertices
/// in counter-clockwise order starting from the lowest-leftmost point. The
/// returned vec does not repeat the starting point.
fn monotone_chain_hull(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    const POINT_EPSILON: f64 = 1e-9;

    let mut pts: Vec<(f64, f64)> = points.to_vec();
    pts.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < POINT_EPSILON && (a.1 - b.1).abs() < POINT_EPSILON);

    if pts.len() < 3 {
        return pts;
    }

    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| -> f64 {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };

    let mut lower: Vec<(f64, f64)> = Vec::new();
    for &p in pts.iter() {
        while lower.len() >= 2
            && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= POINT_EPSILON
        {
            lower.pop();
        }
        lower.push(p);
    }

    let mut upper: Vec<(f64, f64)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2
            && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= POINT_EPSILON
        {
            upper.pop();
        }
        upper.push(p);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn planes_match_oriented(n1: DVec3, d1: f64, n2: DVec3, d2: f64) -> bool {
    (n1 - n2).length_squared() < COPLANAR_NORMAL_EPSILON * COPLANAR_NORMAL_EPSILON
        && (d1 - d2).abs() < COPLANAR_DISTANCE_EPSILON
}

/// True when convex polygon `inner` is fully contained in convex polygon
/// `outer`. Both polygons must lie on the same plane and use the same
/// orientation, given by `plane_normal` (CCW when viewed from +normal).
///
/// Vertices exactly on an edge count as inside; the tolerance is in
/// length units (perpendicular distance from each edge), so identical
/// polygons and floating-point-noisy duplicates both pass cleanly.
fn convex_contains(outer: &[DVec3], inner: &[DVec3], plane_normal: DVec3) -> bool {
    // Tolerance is in meters (engine units). 1 mm of slack absorbs the
    // numerical noise from coordinate transforms and split clipping
    // without admitting visibly-outside points.
    const INSIDE_TOLERANCE: f64 = 1e-3;

    if outer.len() < 3 || inner.is_empty() {
        return false;
    }

    for &p in inner {
        for i in 0..outer.len() {
            let a = outer[i];
            let b = outer[(i + 1) % outer.len()];
            let edge = b - a;
            let edge_len = edge.length();
            if edge_len < 1e-12 {
                continue;
            }
            // For a CCW polygon viewed from +plane_normal, the inside of
            // edge a→b is where cross(edge, p - a) · plane_normal ≥ 0.
            // Dividing by edge length converts the cross magnitude to a
            // perpendicular distance.
            let signed_distance = edge.cross(p - a).dot(plane_normal) / edge_len;
            if signed_distance < -INSIDE_TOLERANCE {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{BrushPlane, BrushSide, TextureProjection};
    use crate::partition::brush_bsp::build_bsp_from_brushes;

    fn tex_projection() -> TextureProjection {
        TextureProjection::default()
    }

    /// Build a box brush with the six axis-aligned sides carrying their
    /// vertex polygons. Used as the canonical face-extraction test fixture
    /// because the BSP-builder tests' `box_brush` leaves `sides` empty.
    fn box_brush_with_sides(min: DVec3, max: DVec3, texture: &str) -> BrushVolume {
        box_brush_with_sides_per_face(
            min,
            max,
            [texture, texture, texture, texture, texture, texture],
        )
    }

    fn box_brush_with_sides_per_face(min: DVec3, max: DVec3, textures: [&str; 6]) -> BrushVolume {
        // Axis-aligned quads, winding so the polygon's normal matches the
        // outward face normal via right-hand rule.
        let sides = vec![
            // +X
            BrushSide {
                vertices: vec![
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(max.x, max.y, min.z),
                ],
                normal: DVec3::X,
                distance: max.x,
                texture: textures[0].to_string(),
                tex_projection: tex_projection(),
            },
            // -X
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(min.x, max.y, max.z),
                    DVec3::new(min.x, min.y, max.z),
                ],
                normal: DVec3::NEG_X,
                distance: -min.x,
                texture: textures[1].to_string(),
                tex_projection: tex_projection(),
            },
            // +Y
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                ],
                normal: DVec3::Y,
                distance: max.y,
                texture: textures[2].to_string(),
                tex_projection: tex_projection(),
            },
            // -Y
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, min.y, min.z),
                ],
                normal: DVec3::NEG_Y,
                distance: -min.y,
                texture: textures[3].to_string(),
                tex_projection: tex_projection(),
            },
            // +Z
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                ],
                normal: DVec3::Z,
                distance: max.z,
                texture: textures[4].to_string(),
                tex_projection: tex_projection(),
            },
            // -Z
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                ],
                normal: DVec3::NEG_Z,
                distance: -min.z,
                texture: textures[5].to_string(),
                tex_projection: tex_projection(),
            },
        ];

        BrushVolume {
            planes: vec![
                BrushPlane {
                    normal: DVec3::X,
                    distance: max.x,
                },
                BrushPlane {
                    normal: DVec3::NEG_X,
                    distance: -min.x,
                },
                BrushPlane {
                    normal: DVec3::Y,
                    distance: max.y,
                },
                BrushPlane {
                    normal: DVec3::NEG_Y,
                    distance: -min.y,
                },
                BrushPlane {
                    normal: DVec3::Z,
                    distance: max.z,
                },
                BrushPlane {
                    normal: DVec3::NEG_Z,
                    distance: -min.z,
                },
            ],
            sides,
            aabb: Aabb { min, max },
        }
    }

    fn face_count_in_leaves(tree: &BspTree) -> usize {
        tree.leaves.iter().map(|l| l.face_indices.len()).sum()
    }

    #[test]
    fn monotone_chain_hull_of_square_recovers_four_corners() {
        let pts = vec![
            (0.0, 0.0),
            (2.0, 0.0),
            (2.0, 2.0),
            (0.0, 2.0),
            // Interior point — should be discarded.
            (1.0, 1.0),
        ];
        let hull = monotone_chain_hull(&pts);
        assert_eq!(hull.len(), 4, "hull of a square has 4 extreme points");
    }

    #[test]
    fn hull_union_merges_two_coplanar_fragments() {
        // Two rectangles sharing an edge on X=0..5, Y=0..2 and X=0..5, Y=2..4.
        // Union should be a single X=0..5, Y=0..4 rectangle.
        let mut hull: Vec<DVec3> = Vec::new();
        let frag_a = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(5.0, 0.0, 0.0),
            DVec3::new(5.0, 2.0, 0.0),
            DVec3::new(0.0, 2.0, 0.0),
        ];
        let frag_b = vec![
            DVec3::new(0.0, 2.0, 0.0),
            DVec3::new(5.0, 2.0, 0.0),
            DVec3::new(5.0, 4.0, 0.0),
            DVec3::new(0.0, 4.0, 0.0),
        ];
        hull_union_into(&mut hull, &frag_a, DVec3::Z);
        hull_union_into(&mut hull, &frag_b, DVec3::Z);

        assert_eq!(hull.len(), 4, "union of abutting rectangles is a rectangle");
        let min_y = hull.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_y = hull.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
        assert!((min_y - 0.0).abs() < 1e-6);
        assert!((max_y - 4.0).abs() < 1e-6);
    }

    #[test]
    fn single_box_brush_emits_six_faces() {
        let brushes = vec![box_brush_with_sides(
            DVec3::splat(0.0),
            DVec3::splat(10.0),
            "wall",
        )];
        let mut tree = build_bsp_from_brushes(&brushes).expect("single box should build");
        let result = extract_faces(&mut tree, &brushes);

        assert_eq!(
            result.faces.len(),
            6,
            "simple box has six visible quad sides — got {}",
            result.faces.len()
        );
        assert!(result.coplanar_conflicts.is_empty());

        // Every face should be referenced by exactly one leaf, and that leaf
        // must be empty.
        assert_eq!(
            face_count_in_leaves(&tree),
            6,
            "every face must appear in exactly one leaf"
        );
        for leaf in &tree.leaves {
            if !leaf.face_indices.is_empty() {
                assert!(
                    !leaf.is_solid,
                    "only empty leaves should carry face indices"
                );
            }
        }
    }

    #[test]
    fn narrow_air_gap_produces_facing_walls() {
        // Two brushes separated by a 2-unit gap in Z. The gap is an empty
        // leaf sandwiched between two solid brushes.
        let brushes = vec![
            box_brush_with_sides(
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(20.0, 20.0, 10.0),
                "wall",
            ),
            box_brush_with_sides(
                DVec3::new(0.0, 0.0, 12.0),
                DVec3::new(20.0, 20.0, 22.0),
                "wall",
            ),
        ];
        let mut tree = build_bsp_from_brushes(&brushes).expect("narrow gap should build");
        let result = extract_faces(&mut tree, &brushes);

        // Both brushes carry six visible sides from the outside, plus each
        // brush has one side facing the gap — so the total face count is
        // 2 * 6 = 12 visible sides, no duplicates.
        assert_eq!(
            result.faces.len(),
            12,
            "two disjoint boxes with an air gap should emit 12 faces"
        );

        // At least one face should point +Z (brush A's top) and at least one
        // should point -Z (brush B's bottom) — these are the gap-facing walls.
        let mut has_plus_z_at_top_of_a = false;
        let mut has_neg_z_at_bottom_of_b = false;
        for face in &result.faces {
            if face.normal.z > 0.99 && (face.distance - 10.0).abs() < 1e-6 {
                has_plus_z_at_top_of_a = true;
            }
            if face.normal.z < -0.99 && (face.distance - (-12.0)).abs() < 1e-6 {
                has_neg_z_at_bottom_of_b = true;
            }
        }
        assert!(
            has_plus_z_at_top_of_a,
            "brush A should emit a +Z face on top at Z=10"
        );
        assert!(
            has_neg_z_at_bottom_of_b,
            "brush B should emit a -Z face on bottom at Z=12"
        );
    }

    #[test]
    fn abutting_brushes_do_not_emit_shared_boundary_face() {
        // Two boxes sharing the X=10 plane. The touching sides have
        // opposite normals (brush A's +X and brush B's -X), both buried in
        // solid. Pass 1 leaves both with empty hulls and Pass 2 emits
        // nothing from either — the shared plane contributes zero output
        // faces. Every surviving face is an outward-facing side of one of
        // the boxes.
        let brushes = vec![
            box_brush_with_sides(
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(10.0, 10.0, 10.0),
                "wall",
            ),
            box_brush_with_sides(
                DVec3::new(10.0, 0.0, 0.0),
                DVec3::new(20.0, 10.0, 10.0),
                "wall",
            ),
        ];
        let mut tree = build_bsp_from_brushes(&brushes).expect("abutting boxes should build");
        let result = extract_faces(&mut tree, &brushes);

        // No face should lie on the shared X=10 plane with an outward normal
        // of +X (that would be brush A's buried side) or -X (brush B's).
        for face in &result.faces {
            let on_shared_plane =
                face.normal.x.abs() > 0.99 && (face.distance.abs() - 10.0).abs() < 1e-6;
            assert!(
                !on_shared_plane,
                "no face should lie on the shared X=10 plane (normal={:?}, distance={})",
                face.normal, face.distance
            );
        }
    }

    #[test]
    fn coplanar_identical_brushes_dedup_to_first_arrival() {
        // Two identical boxes placed at the same location. Every side is
        // coplanar and same-oriented with its counterpart on the other
        // brush, and the polygons are mutually contained (identical
        // shape). Containment-aware dedup drops the second arrival; in
        // practice that is brush 0 because Pass 2 iterates side records in
        // brush order. Conflict diagnostics still fire because textures
        // differ.
        let brushes = vec![
            box_brush_with_sides(
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(10.0, 10.0, 10.0),
                "brush_zero",
            ),
            box_brush_with_sides(
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(10.0, 10.0, 10.0),
                "brush_one",
            ),
        ];
        let mut tree = build_bsp_from_brushes(&brushes).expect("stacked boxes should build");
        let result = extract_faces(&mut tree, &brushes);

        // Exactly six visible faces (a single box's worth), all from brush 0.
        assert_eq!(
            result.faces.len(),
            6,
            "stacked identical boxes should dedup to a single box's six faces"
        );
        for face in &result.faces {
            assert_eq!(
                face.brush_index, 0,
                "lower-index brush should win the coplanar tiebreak"
            );
            assert_eq!(
                face.texture, "brush_zero",
                "surviving face should carry the winner's texture"
            );
        }

        // One conflict per side pair (six sides).
        assert_eq!(
            result.coplanar_conflicts.len(),
            6,
            "each of the six coplanar same-orientation pairs should emit a texture conflict"
        );
        for conflict in &result.coplanar_conflicts {
            assert_eq!(conflict.winner_brush, 0);
            assert_eq!(conflict.loser_brush, 1);
            assert_eq!(conflict.winner_texture, "brush_zero");
            assert_eq!(conflict.loser_texture, "brush_one");
        }
    }

    #[test]
    fn spatially_disjoint_coplanar_sides_both_emit() {
        // Two boxes at the same Z range but offset in X with a gap between
        // them in X. Both have +Z sides coplanar at Z=10 but the hulls are
        // spatially disjoint (X=0..5 vs X=10..15). Each should emit its own
        // face; neither should be dropped by the dedup rule.
        //
        // The BSP tree splits along X=5 and X=10 — adjacent planes from the
        // two brushes — so the above-world empty space is partitioned into
        // three regions: X<5, 5<X<10, and X>10. Brush 0's +Z hull lands in
        // the first empty region; brush 1's +Z hull lands in the third. They
        // never collide, so the dedup rule (currently leaf-time) does not
        // spuriously drop either side.
        let brushes = vec![
            box_brush_with_sides(
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(5.0, 5.0, 10.0),
                "wall",
            ),
            box_brush_with_sides(
                DVec3::new(10.0, 0.0, 0.0),
                DVec3::new(15.0, 5.0, 10.0),
                "wall",
            ),
        ];
        let mut tree = build_bsp_from_brushes(&brushes).expect("two disjoint boxes should build");
        let result = extract_faces(&mut tree, &brushes);

        // Each box contributes six faces. No dedup should fire because
        // the hulls are spatially disjoint.
        assert_eq!(
            result.faces.len(),
            12,
            "two disjoint boxes should emit 12 faces total with no false dedup"
        );
        assert!(
            result.coplanar_conflicts.is_empty(),
            "disjoint hulls should not produce coplanar conflicts"
        );

        // Both brushes should own at least one +Z face.
        let plus_z_from_zero = result
            .faces
            .iter()
            .any(|f| f.brush_index == 0 && f.normal.z > 0.99);
        let plus_z_from_one = result
            .faces
            .iter()
            .any(|f| f.brush_index == 1 && f.normal.z > 0.99);
        assert!(plus_z_from_zero, "brush 0 should own a +Z face");
        assert!(plus_z_from_one, "brush 1 should own a +Z face");
    }

    #[test]
    fn convex_contains_recognises_inner_outer_relationships() {
        // All polygons live on Z=0 with +Z normal, CCW from above.
        let normal = DVec3::Z;
        let big = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(10.0, 0.0, 0.0),
            DVec3::new(10.0, 10.0, 0.0),
            DVec3::new(0.0, 10.0, 0.0),
        ];
        let small_inside = vec![
            DVec3::new(2.0, 2.0, 0.0),
            DVec3::new(8.0, 2.0, 0.0),
            DVec3::new(8.0, 8.0, 0.0),
            DVec3::new(2.0, 8.0, 0.0),
        ];
        let partial_overlap = vec![
            DVec3::new(5.0, 5.0, 0.0),
            DVec3::new(15.0, 5.0, 0.0),
            DVec3::new(15.0, 15.0, 0.0),
            DVec3::new(5.0, 15.0, 0.0),
        ];
        let disjoint = vec![
            DVec3::new(20.0, 20.0, 0.0),
            DVec3::new(25.0, 20.0, 0.0),
            DVec3::new(25.0, 25.0, 0.0),
            DVec3::new(20.0, 25.0, 0.0),
        ];

        assert!(convex_contains(&big, &small_inside, normal));
        assert!(!convex_contains(&small_inside, &big, normal));
        assert!(!convex_contains(&big, &partial_overlap, normal));
        assert!(!convex_contains(&partial_overlap, &big, normal));
        assert!(!convex_contains(&big, &disjoint, normal));
        // Identical polygons must mutually contain each other so the
        // first-arrival dedup path resolves duplicate fragments cleanly.
        assert!(convex_contains(&big, &big, normal));
    }

    #[test]
    fn coplanar_matching_textures_emit_no_conflict() {
        // Same stacked-brush configuration, but both brushes use the same
        // texture. Dedup still runs (six faces, all from brush 0), but no
        // conflict is reported because the textures match.
        let brushes = vec![
            box_brush_with_sides(
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(10.0, 10.0, 10.0),
                "wall",
            ),
            box_brush_with_sides(
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(10.0, 10.0, 10.0),
                "wall",
            ),
        ];
        let mut tree = build_bsp_from_brushes(&brushes).expect("matched boxes should build");
        let result = extract_faces(&mut tree, &brushes);

        assert_eq!(result.faces.len(), 6);
        assert!(
            result.coplanar_conflicts.is_empty(),
            "matching textures should not emit a conflict"
        );
    }
}
