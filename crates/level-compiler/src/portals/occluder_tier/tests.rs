use std::collections::HashSet;

use super::*;
use crate::map_data::{BrushPlane, BrushVolume};
use crate::map_format::MapFormat;
use crate::partition::occluder_classify::{
    OccluderClassification, OccluderVerdict, OpenSpace, classify_occluders,
};
use crate::partition::{
    Aabb, TierRole, build_bsp_from_brushes, build_bsp_with_occluder_tier, find_leaf_for_point,
};
use crate::portals::generate_portals;
use crate::visibility::find_exterior_leaves;

fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
    let planes = [
        (DVec3::X, max.x),
        (DVec3::NEG_X, -min.x),
        (DVec3::Y, max.y),
        (DVec3::NEG_Y, -min.y),
        (DVec3::Z, max.z),
        (DVec3::NEG_Z, -min.z),
    ]
    .into_iter()
    .map(|(normal, distance)| BrushPlane { normal, distance })
    .collect();
    BrushVolume {
        planes,
        sides: Vec::new(),
        aabb: Aabb { min, max },
    }
}

/// Sealed hall of 0.5 m slabs, interior `[0, size]`.
fn hall(size: DVec3) -> Vec<BrushVolume> {
    let t = 0.5;
    let s = size;
    vec![
        box_brush(DVec3::new(-t, -t, -t), DVec3::new(s.x + t, 0.0, s.z + t)),
        box_brush(
            DVec3::new(-t, s.y, -t),
            DVec3::new(s.x + t, s.y + t, s.z + t),
        ),
        box_brush(DVec3::new(-t, 0.0, -t), DVec3::new(0.0, s.y, s.z + t)),
        box_brush(DVec3::new(s.x, 0.0, -t), DVec3::new(s.x + t, s.y, s.z + t)),
        box_brush(DVec3::new(0.0, 0.0, -t), DVec3::new(s.x, s.y, 0.0)),
        box_brush(DVec3::new(0.0, 0.0, s.z), DVec3::new(s.x, s.y, s.z + t)),
    ]
}

/// Today's tree, its portals and exterior flood, then classification.
struct FirstPass {
    leaf_count: usize,
    portal_count: usize,
    classification: OccluderClassification,
}

fn first_pass(brushes: &[BrushVolume]) -> FirstPass {
    let tree = build_bsp_from_brushes(brushes).expect("first-pass tree builds");
    let portals = generate_portals(&tree);
    let exterior = find_exterior_leaves(&tree, &portals);
    let space = OpenSpace::new(&tree, &exterior);
    FirstPass {
        leaf_count: tree.leaves.len(),
        portal_count: portals.len(),
        classification: classify_occluders(brushes, &space),
    }
}

/// The two-tier tree, its full portal set and exterior flood, its cells and
/// their occluder-tier portals.
struct TieredBuild {
    tree: BspTree,
    tier: OccluderTier,
    full_portal_count: usize,
    exterior: HashSet<usize>,
    cells: CellTree,
    tier_portals: Vec<Portal>,
}

fn tiered_build(brushes: &[BrushVolume], roles: &[TierRole]) -> TieredBuild {
    let (tree, tier) =
        build_bsp_with_occluder_tier(brushes, roles, &[], &[], None).expect("two-tier tree builds");
    let portals = generate_portals(&tree);
    let exterior = find_exterior_leaves(&tree, &portals);
    let pockets = interior_pockets(&tree, &tier, &exterior);
    let cells = CellTree::build(&tree, &tier, &exterior, &pockets);
    let tier_portals = generate_runtime_portals(&tree, &tier, &cells);
    TieredBuild {
        tree,
        tier,
        full_portal_count: portals.len(),
        exterior,
        cells,
        tier_portals,
    }
}

impl TieredBuild {
    fn cell_at(&self, point: DVec3) -> usize {
        self.cells.leaf_cell[find_leaf_for_point(&self.tree, point)]
    }

    fn is_interior_at(&self, point: DVec3) -> bool {
        let leaf = find_leaf_for_point(&self.tree, point);
        !self.tree.leaves[leaf].is_solid && !self.exterior.contains(&leaf)
    }

    /// Every tier portal joins interior cells of two distinct tier leaves
    /// and lies on a tier node's plane.
    fn assert_tier_portals_well_formed(&self) {
        let tier_leaf_of = |cell: usize| {
            self.cells
                .tier_leaf_cells
                .iter()
                .position(|cells| cells.contains(&cell))
                .unwrap_or_else(|| panic!("tier portal names cell {cell}, which is not interior"))
        };
        for portal in &self.tier_portals {
            assert_ne!(
                tier_leaf_of(portal.front_leaf),
                tier_leaf_of(portal.back_leaf)
            );
            let on_tier_plane = self.tree.nodes.iter().enumerate().any(|(idx, node)| {
                self.tier.node_is_tier[idx]
                    && portal
                        .polygon
                        .iter()
                        .all(|p| (p.dot(node.plane_normal) - node.plane_distance).abs() < 1e-6)
            });
            assert!(on_tier_plane, "tier portal off every tier node plane");
        }
    }
}

#[test]
fn unclassified_build_keeps_todays_tree() {
    // The two-tier builder with no occluder must make today's tree, so an
    // unwired branch compiles every map as main does.
    let mut brushes = hall(DVec3::new(20.0, 6.0, 20.0));
    brushes.push(box_brush(
        DVec3::new(8.0, 0.0, 8.0),
        DVec3::new(9.0, 3.7, 9.0),
    ));

    let today = build_bsp_from_brushes(&brushes).expect("tree builds");
    let (tiered, tier) = build_bsp_with_occluder_tier(
        &brushes,
        &vec![TierRole::Exempt; brushes.len()],
        &[],
        &[],
        None,
    )
    .expect("tree builds");

    assert_eq!(today.nodes.len(), tiered.nodes.len());
    assert_eq!(today.leaves.len(), tiered.leaves.len());
    for (a, b) in today.nodes.iter().zip(&tiered.nodes) {
        assert_eq!(
            (a.plane_normal, a.plane_distance),
            (b.plane_normal, b.plane_distance)
        );
    }
    assert_eq!(tier.leaves.len(), 1, "no occluder: one tier leaf");
    assert!(tier.node_is_tier.iter().all(|&in_tier| !in_tier));
}

#[test]
fn exempt_pillar_adds_no_tier_portal_beside_an_occluding_wall() {
    // Hall 40 × 13 × 40 m crossed by a 26 m wall, 6.5 m tall, with a red
    // pillar beyond it: the wall bounds tier leaves; the pillar only splits
    // beneath them.
    let size = DVec3::new(40.0, 13.0, 40.0);
    let wall = box_brush(DVec3::new(7.0, 0.0, 20.0), DVec3::new(33.0, 6.5, 20.8));
    let pillar = box_brush(DVec3::new(15.0, 0.0, 28.0), DVec3::new(16.0, 3.7, 29.0));
    let without: Vec<BrushVolume> = hall(size).into_iter().chain([wall.clone()]).collect();
    let with: Vec<BrushVolume> = without.iter().cloned().chain([pillar]).collect();

    let roles_without = first_pass(&without).classification.tier_roles();
    let roles_with = first_pass(&with).classification.tier_roles();
    assert_eq!(roles_with[6], TierRole::Occluder, "the wall occludes");
    assert_eq!(roles_with[7], TierRole::Exempt, "the pillar stays exempt");

    let build_without = tiered_build(&without, &roles_without);
    let build_with = tiered_build(&with, &roles_with);
    build_with.assert_tier_portals_well_formed();

    assert_eq!(
        build_with.tier.leaves.len(),
        build_without.tier.leaves.len()
    );
    assert_eq!(
        build_with.tier_portals.len(),
        build_without.tier_portals.len()
    );
    assert!(
        build_with.tree.leaves.len() > build_without.tree.leaves.len(),
        "the pillar still splits the full tree"
    );

    let below = DVec3::new(20.0, 6.4, 30.0);
    let above = DVec3::new(20.0, 6.6, 30.0);
    assert!(build_with.is_interior_at(below) && build_with.is_interior_at(above));
    assert_ne!(build_with.cell_at(below), build_with.cell_at(above));
}

/// Engine point of a Quake-unit point in occlusion-test.map.
fn quake_point(x: f64, y: f64, z: f64) -> DVec3 {
    crate::parse::quake_to_engine(DVec3::new(x, y, z)) * MapFormat::IdTech2.units_to_meters()
}

fn approx_aabb(aabb: &Aabb, min: DVec3, max: DVec3) -> bool {
    (aabb.min - min.min(max)).abs().max_element() < 1e-6
        && (aabb.max - max.max(min)).abs().max_element() < 1e-6
}

/// First node at which the two points' descents part, with its plane.
fn parting_node(tree: &BspTree, a: DVec3, b: DVec3) -> Option<usize> {
    let mut child = BspChild::Node(0);
    while let BspChild::Node(idx) = child {
        let node = &tree.nodes[idx];
        let side = |p: DVec3| p.dot(node.plane_normal) - node.plane_distance >= 0.0;
        if side(a) != side(b) {
            return Some(idx);
        }
        child = if side(a) {
            node.front.clone()
        } else {
            node.back.clone()
        };
    }
    None
}

// The brief's first slice: occluder-first selection must carry the retaining
// wall's top plane across the hall beyond it before anything else cuts it.
#[test]
fn occlusion_test_wall_top_bounds_occluder_tier_leaves_beyond_the_wall() {
    let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps/occlusion-test.map");
    let map_data = crate::parse::parse_map_file(&map_path, MapFormat::IdTech2)
        .expect("occlusion-test.map parses");
    let brushes = &map_data.brush_volumes;

    let pass = first_pass(brushes);
    let verdicts = &pass.classification.verdicts;

    // R24: the retaining wall (Quake [0,1184,0]..[1024,1216,256]) occludes and
    // every red pillar stays exempt. Pillars are the red brushes with a 40 u
    // (1.016 m) square footprint; the Cornell room's red wall is not one.
    let wall = brushes
        .iter()
        .position(|b| {
            approx_aabb(
                &b.aabb,
                quake_point(0.0, 1184.0, 0.0),
                quake_point(1024.0, 1216.0, 256.0),
            )
        })
        .expect("retaining wall is in the brush set");
    assert_eq!(verdicts[wall], OccluderVerdict::Occludes);
    let red: Vec<usize> = (0..brushes.len())
        .filter(|&i| {
            let footprint = brushes[i].aabb.max - brushes[i].aabb.min;
            !brushes[i].sides.is_empty()
                && brushes[i]
                    .sides
                    .iter()
                    .all(|s| s.texture == "debug_wall_red")
                && (footprint.x - 1.016).abs() < 1e-3
                && (footprint.z - 1.016).abs() < 1e-3
        })
        .collect();
    assert!(!red.is_empty(), "occlusion-test has red pillars");
    for &i in &red {
        assert_ne!(
            verdicts[i],
            OccluderVerdict::Occludes,
            "red pillar brush {i} must stay exempt"
        );
    }

    let flags = pass.classification.occluder_flags();
    let build = tiered_build(brushes, &pass.classification.tier_roles());
    build.assert_tier_portals_well_formed();

    // Beyond the wall (north, Quake +y), mid-span, just below and just above
    // the wall top.
    let below = quake_point(512.0, 1400.0, 252.0);
    let above = quake_point(512.0, 1400.0, 260.0);
    assert!(build.is_interior_at(below) && build.is_interior_at(above));
    let parting = parting_node(&build.tree, below, above).expect("points part somewhere");
    let parting_plane = &build.tree.nodes[parting];

    let interior_tier_leaves = build
        .tier
        .leaves
        .iter()
        .filter(|leaf| {
            leaf.leaf_range
                .clone()
                .any(|l| !build.tree.leaves[l].is_solid && !build.exterior.contains(&l))
        })
        .count();
    println!(
        "occlusion-test: {} brushes, {} occlude, {} red pillars exempt\n\
         today:    {} leaves, {} portals\n\
         two-tier: {} leaves, {} full-tree portals; {} tier nodes, {} tier leaves \
         ({} with interior), {} tier portals\n\
         below/above wall top part at node {} (tier: {}), plane n={:?} d={:.4}",
        brushes.len(),
        flags.iter().filter(|&&f| f).count(),
        red.len(),
        pass.leaf_count,
        pass.portal_count,
        build.tree.leaves.len(),
        build.full_portal_count,
        build.tier.node_is_tier.iter().filter(|&&t| t).count(),
        build.tier.leaves.len(),
        interior_tier_leaves,
        build.tier_portals.len(),
        parting,
        build.tier.node_is_tier[parting],
        parting_plane.plane_normal,
        parting_plane.plane_distance,
    );

    assert_ne!(
        build.cell_at(below),
        build.cell_at(above),
        "points below and above the wall top beyond it share an occluder-tier leaf"
    );
}

#[test]
fn portal_corners_snap_onto_axis_planes_within_clipping_dust_only() {
    // Clipping the base winding leaves corners about 1e-13 m off the planes
    // that cut them; a closed door must still cover a portal its face bounds.
    let floor = (DVec3::Y, 1.6256);
    let oblique = (DVec3::new(1.0, 1.0, 0.0).normalize(), 2.0);
    let dusty = DVec3::new(19.5072, 1.6256 - 3e-13, 88.5952);
    let snapped = snap_to_bounding_planes(dusty, [floor, oblique].iter());
    assert_eq!(snapped.y, 1.6256);
    assert_eq!((snapped.x, snapped.z), (dusty.x, dusty.z));

    let open_gap = DVec3::new(19.5072, 1.6256 - 1e-6, 88.5952);
    assert_eq!(snap_to_bounding_planes(open_gap, [floor].iter()), open_gap);
}

#[test]
fn portal_corners_snap_onto_oblique_planes_within_clipping_dust() {
    // A door turned 45° in plan: its face is the portal's plane, and its
    // edge and the floor are further planes bounding the portal's leaves.
    // Dust off each must go, whichever side it left the corner; which side
    // of the face the corner rounds to is sealing's to read
    // (`portal_on_an_oblique_door_face_within_parse_noise_either_side_is_sealed`).
    let face_normal = DVec3::new(1.0, 0.0, 1.0).normalize();
    let edge_normal = DVec3::new(1.0, 0.0, -1.0).normalize();
    let corner = DVec3::new(7.3, 1.2, 4.1);
    let face = (face_normal, corner.dot(face_normal));
    let edge = (edge_normal, corner.dot(edge_normal));
    let floor = (DVec3::Y, 1.2);
    // The leaf behind the face carries it reversed.
    let behind = (-face_normal, -face.1);
    for (face_dust, edge_dust, floor_dust) in [
        (3e-13, -2e-13, 1e-13),
        (-3e-13, 2e-13, -1e-13),
        (-7e-10, 5e-10, 4e-10),
        (0.0, 0.0, 0.0),
    ] {
        let dusty =
            corner + face_normal * face_dust + edge_normal * edge_dust + DVec3::Y * floor_dust;
        let snapped = snap_to_bounding_planes(dusty, [face, edge, floor, behind].iter());
        assert!(
            (snapped.dot(face_normal) - face.1).abs() < 1e-14,
            "corner dusted {face_dust:e} off the face lands on it"
        );
        assert!((snapped.dot(edge_normal) - edge.1).abs() < 1e-14);
        assert_eq!(snapped.y, 1.2, "the floor still takes the corner exactly");
        assert!((snapped - corner).length() < 1e-12);
    }
}

/// The 24 orders of four values.
fn orders(values: [f64; 4]) -> Vec<[f64; 4]> {
    let mut all = Vec::new();
    for a in 0..4 {
        for b in (0..4).filter(|&b| b != a) {
            for c in (0..4).filter(|&c| c != a && c != b) {
                let d = 6 - a - b - c;
                all.push([values[a], values[b], values[c], values[d]]);
            }
        }
    }
    all
}

#[test]
fn coplanar_hull_of_dusty_fragments_covers_the_whole_opening() {
    // A 1.5 × 2 m opening on a wall turned 45° in plan, and on one at an
    // arbitrary angle, split by a horizontal or a vertical plane into two
    // fragments. The four corners on each line the split crosses carry
    // clipping dust across that line, in every order. The hull must stay the
    // whole opening and hold every corner: ordered by the dust, the chain
    // dropped a real extreme corner as collinear.
    let (width, height) = (1.5, 2.0);
    let dust = [-2e-12, -1e-12, 1e-12, 2e-12];
    for angle in [std::f64::consts::FRAC_PI_4, 1.1] {
        let normal = DVec3::new(angle.cos(), 0.0, angle.sin());
        let along = DVec3::new(-angle.sin(), 0.0, angle.cos());
        let origin = DVec3::new(3.25, 0.5, -7.75);
        for split_vertical in [false, true] {
            // Corners as (along, up), the two fragments in turn; the corner
            // indices on each line the split crosses; the dust's direction.
            let (corners, lines, across) = if split_vertical {
                let s = 0.6;
                (
                    [
                        (0.0, 0.0),
                        (s, 0.0),
                        (s, height),
                        (0.0, height),
                        (s, 0.0),
                        (width, 0.0),
                        (width, height),
                        (s, height),
                    ],
                    [[0, 1, 4, 5], [3, 2, 7, 6]],
                    DVec3::Y,
                )
            } else {
                let h = 1.2;
                (
                    [
                        (0.0, 0.0),
                        (width, 0.0),
                        (width, h),
                        (0.0, h),
                        (0.0, h),
                        (width, h),
                        (width, height),
                        (0.0, height),
                    ],
                    [[0, 3, 4, 7], [1, 2, 5, 6]],
                    along,
                )
            };
            for first in orders(dust) {
                for second in orders(dust) {
                    let mut points: Vec<DVec3> = corners
                        .iter()
                        .map(|&(s, up)| origin + along * s + DVec3::Y * up)
                        .collect();
                    for (line, offsets) in lines.iter().zip([first, second]) {
                        for (&index, offset) in line.iter().zip(offsets) {
                            points[index] += across * offset;
                        }
                    }
                    let context = format!(
                        "angle {angle}, split vertical {split_vertical}, dust {first:?} {second:?}"
                    );
                    let hull = coplanar_hull(&points, normal)
                        .unwrap_or_else(|| panic!("no hull ({context})"));
                    assert!(
                        (polygon_area(&hull) - width * height).abs() < 1e-9,
                        "hull area {} is not the opening's ({context})",
                        polygon_area(&hull)
                    );
                    for point in &points {
                        let inside = (0..hull.len()).all(|i| {
                            let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
                            (b - a).cross(*point - a).dot(normal) >= -1e-9
                        });
                        assert!(inside, "hull leaves out {point:?} ({context})");
                    }
                }
            }
        }
    }
}

#[test]
fn window_split_below_the_tier_into_pieces_under_the_cull_keeps_its_portal() {
    // A 1 × 0.5 m window through an occluding wall, with thin exempt posts
    // standing flush against one face. The posts' planes cut the window's
    // tier piece below the tier into strips each under the portal-area cull;
    // the cull applies to the tier piece and the pair's portal, so the strips
    // between the posts must still be covered by the window's portal.
    let size = DVec3::new(20.0, 6.0, 20.0);
    let mut brushes = hall(size);
    let walls = brushes.len();
    let (z0, z1) = (10.0, 10.5);
    let (x0, x1, y0, y1) = (15.0, 16.0, 2.0, 2.5);
    brushes.extend([
        box_brush(DVec3::new(0.0, 0.0, z0), DVec3::new(x0, size.y, z1)),
        box_brush(DVec3::new(x1, 0.0, z0), DVec3::new(size.x, size.y, z1)),
        box_brush(DVec3::new(x0, 0.0, z0), DVec3::new(x1, y0, z1)),
        box_brush(DVec3::new(x0, y1, z0), DVec3::new(x1, size.y, z1)),
    ]);
    let posts: Vec<f64> = vec![15.17, 15.36, 15.55, 15.74, 15.93];
    for &x in &posts {
        brushes.push(box_brush(
            DVec3::new(x, 0.0, z0 - 0.5),
            DVec3::new(x + 0.02, size.y, z0),
        ));
    }
    // The wall is one facade section: its bounding face, the posts' side,
    // and its box edges split at occluder rank, its far face and the
    // window's jambs only beneath the tier. Planes run +X, -X, +Y, -Y, +Z,
    // -Z.
    let mut roles = vec![TierRole::Exempt; brushes.len()];
    let outlines = [
        [false, true, true, true, false, true],
        [true, false, true, true, false, true],
        [false, false, false, true, false, true],
        [false, false, true, false, false, true],
    ];
    for (role, outline) in roles[walls..walls + 4].iter_mut().zip(outlines) {
        *role = TierRole::SectionMember(outline.to_vec());
    }

    let build = tiered_build(&brushes, &roles);
    build.assert_tier_portals_well_formed();

    // Strip centres between the posts, on the wall face the posts touch.
    let mut edges = vec![x0];
    for &x in &posts {
        edges.extend([x, x + 0.02]);
    }
    edges.push(x1);
    let strips: Vec<DVec3> = edges
        .chunks(2)
        .map(|pair| DVec3::new((pair[0] + pair[1]) / 2.0, (y0 + y1) / 2.0, z0))
        .collect();
    assert!(
        strips
            .iter()
            .all(|s| build.is_interior_at(s - DVec3::Z * 0.01))
    );

    // The posts part the window's fragments into openings each under the
    // cull, so they share one hull.
    let room = build.cell_at(DVec3::new(15.5, 2.25, 5.0));
    let window = build.cell_at(DVec3::new(15.5, 2.25, (z0 + z1) / 2.0));
    assert_ne!(room, window);
    let across: Vec<&Portal> = build
        .tier_portals
        .iter()
        .filter(|p| {
            (p.front_leaf, p.back_leaf) == (room, window)
                || (p.front_leaf, p.back_leaf) == (window, room)
        })
        .collect();
    assert_eq!(
        across.len(),
        1,
        "the window keeps one portal into the room the posts stand in"
    );
    let portal = across[0];
    let covers = |point: DVec3| {
        let n = portal.polygon.len();
        let normal = (portal.polygon[1] - portal.polygon[0])
            .cross(portal.polygon[2] - portal.polygon[0])
            .normalize();
        (0..n).all(|i| {
            let (a, b) = (portal.polygon[i], portal.polygon[(i + 1) % n]);
            (b - a).cross(point - a).dot(normal) >= -1e-9
        })
    };
    for strip in &strips {
        assert!(
            covers(*strip),
            "window portal misses the strip at {strip:?}"
        );
    }
}
