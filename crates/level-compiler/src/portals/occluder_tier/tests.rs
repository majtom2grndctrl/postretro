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

/// The two-tier tree, its full portal set and exterior flood, and its
/// occluder-tier portals.
struct TieredBuild {
    tree: BspTree,
    tier: OccluderTier,
    full_portal_count: usize,
    exterior: HashSet<usize>,
    tier_portals: Vec<TierPortal>,
}

fn tiered_build(brushes: &[BrushVolume], roles: &[TierRole]) -> TieredBuild {
    let (tree, tier) = build_bsp_with_occluder_tier(brushes, roles).expect("two-tier tree builds");
    let portals = generate_portals(&tree);
    let exterior = find_exterior_leaves(&tree, &portals);
    let tier_portals = generate_occluder_tier_portals(&tree, &tier, &exterior);
    TieredBuild {
        tree,
        tier,
        full_portal_count: portals.len(),
        exterior,
        tier_portals,
    }
}

impl TieredBuild {
    fn tier_leaf_at(&self, point: DVec3) -> usize {
        self.tier.leaf_tier_leaf[find_leaf_for_point(&self.tree, point)]
    }

    fn is_interior_at(&self, point: DVec3) -> bool {
        let leaf = find_leaf_for_point(&self.tree, point);
        !self.tree.leaves[leaf].is_solid && !self.exterior.contains(&leaf)
    }

    /// Every tier portal joins two distinct tier leaves, each holding an
    /// interior leaf, and lies on a tier node's plane.
    fn assert_tier_portals_well_formed(&self) {
        for portal in &self.tier_portals {
            assert_ne!(portal.front_tier_leaf, portal.back_tier_leaf);
            for tier_leaf in [portal.front_tier_leaf, portal.back_tier_leaf] {
                let range = self.tier.leaves[tier_leaf].leaf_range.clone();
                assert!(
                    range
                        .into_iter()
                        .any(|leaf| !self.tree.leaves[leaf].is_solid
                            && !self.exterior.contains(&leaf)),
                    "tier portal names tier leaf {tier_leaf} with no interior leaf"
                );
            }
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
    let (tiered, tier) =
        build_bsp_with_occluder_tier(&brushes, &vec![TierRole::Exempt; brushes.len()])
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
    assert_ne!(
        build_with.tier_leaf_at(below),
        build_with.tier_leaf_at(above)
    );
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
        build.tier_leaf_at(below),
        build.tier_leaf_at(above),
        "points below and above the wall top beyond it share an occluder-tier leaf"
    );
}
