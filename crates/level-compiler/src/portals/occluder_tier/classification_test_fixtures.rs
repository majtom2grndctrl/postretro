// Harness for the occluder classification rows: the first pass with its
// exterior flood, the two-tier build, and point location to occluder-tier
// leaves. It lives on the prl-build side because the library target compiles
// `partition` without portals, the exterior flood or the map parser.
// See: context/plans/in-progress/occluder-first-partition/index.md §Acceptance

use std::collections::HashSet;
use std::path::PathBuf;

use glam::DVec3;

use crate::map_data::BrushVolume;
use crate::map_format::MapFormat;
use crate::partition::occluder_classify::test_fixtures::Fixture;
use crate::partition::occluder_classify::{
    OccluderClassification, OccluderVerdict, OpenSpace, classify_occluders,
};
use crate::partition::{
    BspTree, OccluderTier, TierRole, build_bsp_from_brushes, build_bsp_with_occluder_tier,
    find_leaf_for_point,
};
use crate::portals::generate_portals;
use crate::portals::occluder_tier::generate_occluder_tier_portals;
use crate::visibility::find_exterior_leaves;

/// The first pass the classifier reads: today's tree of every brush and its
/// exterior flood.
pub(crate) struct FirstPass {
    pub(crate) tree: BspTree,
    pub(crate) exterior: HashSet<usize>,
}

impl FirstPass {
    pub(crate) fn of(brushes: &[BrushVolume]) -> Self {
        let tree = build_bsp_from_brushes(brushes).expect("first-pass tree builds");
        let portals = generate_portals(&tree);
        let exterior = find_exterior_leaves(&tree, &portals);
        Self { tree, exterior }
    }

    pub(crate) fn classification(&self, brushes: &[BrushVolume]) -> OccluderClassification {
        classify_occluders(brushes, &OpenSpace::new(&self.tree, &self.exterior))
    }
}

/// Each brush's verdict alone, against the brushes' own first pass.
pub(crate) fn classify(brushes: &[BrushVolume]) -> Vec<OccluderVerdict> {
    classification(brushes).verdicts
}

/// Brushes and clusters classified against the brushes' own first pass.
pub(crate) fn classification(brushes: &[BrushVolume]) -> OccluderClassification {
    FirstPass::of(brushes).classification(brushes)
}

/// The two-tier tree on given tier roles, with its exterior flood. An
/// occluder-tier leaf stands in for the runtime cell it becomes.
pub(crate) struct TierBuild {
    pub(crate) tree: BspTree,
    pub(crate) tier: OccluderTier,
    pub(crate) exterior: HashSet<usize>,
}

impl TierBuild {
    pub(crate) fn new(brushes: &[BrushVolume], roles: &[TierRole]) -> Self {
        let (tree, tier) =
            build_bsp_with_occluder_tier(brushes, roles, &[]).expect("two-tier tree builds");
        let portals = generate_portals(&tree);
        let exterior = find_exterior_leaves(&tree, &portals);
        Self {
            tree,
            tier,
            exterior,
        }
    }

    /// The two tiers on `brushes`' own classification, clusters included.
    pub(crate) fn classified(brushes: &[BrushVolume]) -> Self {
        Self::new(brushes, &classification(brushes).tier_roles())
    }

    /// Occluder-tier leaves holding interior space, and tier portals: the
    /// empty cells and portals a brush that occludes nothing leaves as they
    /// were.
    pub(crate) fn interior_cells_and_portals(&self) -> (usize, usize) {
        let interior = self
            .tier
            .leaves
            .iter()
            .filter(|leaf| {
                leaf.leaf_range
                    .clone()
                    .any(|l| !self.tree.leaves[l].is_solid && !self.exterior.contains(&l))
            })
            .count();
        let portals = generate_occluder_tier_portals(&self.tree, &self.tier, &self.exterior);
        (interior, portals.len())
    }

    /// Occluder-tier leaf holding `point`, which must lie in interior space.
    pub(crate) fn cell_at(&self, point: DVec3) -> usize {
        let leaf = find_leaf_for_point(&self.tree, point);
        assert!(
            !self.tree.leaves[leaf].is_solid && !self.exterior.contains(&leaf),
            "point {point} is not in interior space"
        );
        self.tier.leaf_tier_leaf[leaf]
    }
}

/// Classify `fixture`, assert every pin, and return the verdicts.
pub(crate) fn assert_pins(fixture: &Fixture) -> Vec<OccluderVerdict> {
    let verdicts = classify(&fixture.brushes);
    for pin in &fixture.pins {
        assert_eq!(
            verdicts[pin.brush], pin.verdict,
            "{}: {} (brush {})",
            fixture.name, pin.label, pin.brush
        );
    }
    verdicts
}

/// Build the two tiers on `fixture`'s own classification and assert each of
/// its `apart` pairs locates to different cells.
pub(crate) fn assert_apart(fixture: &Fixture) {
    assert!(
        !fixture.apart.is_empty(),
        "{}: no points to part",
        fixture.name
    );
    let build = TierBuild::classified(&fixture.brushes);
    for &(a, b) in &fixture.apart {
        assert_ne!(
            build.cell_at(a),
            build.cell_at(b),
            "{}: {a} and {b} share a cell",
            fixture.name
        );
    }
}

/// occlusion-test.map's brushes, through parse and unit scale.
pub(crate) fn occlusion_test_brushes() -> Vec<BrushVolume> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/maps/occlusion-test.map");
    crate::parse::parse_map_file(&path, MapFormat::IdTech2)
        .expect("occlusion-test.map parses")
        .brush_volumes
}

/// Index of the brush whose AABB is the Quake-unit box `min..max`.
pub(crate) fn brush_at_quake_box(brushes: &[BrushVolume], min: [f64; 3], max: [f64; 3]) -> usize {
    let engine = |p: [f64; 3]| {
        crate::parse::quake_to_engine(DVec3::from_array(p)) * MapFormat::IdTech2.units_to_meters()
    };
    let (a, b) = (engine(min), engine(max));
    let (lo, hi) = (a.min(b), a.max(b));
    brushes
        .iter()
        .position(|brush| {
            (brush.aabb.min - lo).abs().max_element() < 1e-6
                && (brush.aabb.max - hi).abs().max_element() < 1e-6
        })
        .unwrap_or_else(|| panic!("no brush spans Quake {min:?}..{max:?}"))
}
