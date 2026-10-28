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
use crate::partition::occluder_classify::{OccluderVerdict, OpenSpace, classify_occluders};
use crate::partition::{
    BspTree, OccluderTier, build_bsp_from_brushes, build_bsp_with_occluder_tier,
    find_leaf_for_point,
};
use crate::portals::generate_portals;
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

    pub(crate) fn classify(&self, brushes: &[BrushVolume]) -> Vec<OccluderVerdict> {
        classify_occluders(brushes, &OpenSpace::new(&self.tree, &self.exterior)).verdicts
    }
}

/// Classify `brushes` against their own first pass.
pub(crate) fn classify(brushes: &[BrushVolume]) -> Vec<OccluderVerdict> {
    FirstPass::of(brushes).classify(brushes)
}

pub(crate) fn occluder_flags(verdicts: &[OccluderVerdict]) -> Vec<bool> {
    verdicts
        .iter()
        .map(|&verdict| verdict == OccluderVerdict::Occludes)
        .collect()
}

/// The two-tier tree on given occluder flags, with its exterior flood. An
/// occluder-tier leaf stands in for the runtime cell it becomes.
pub(crate) struct TierBuild {
    pub(crate) tree: BspTree,
    pub(crate) tier: OccluderTier,
    pub(crate) exterior: HashSet<usize>,
}

impl TierBuild {
    pub(crate) fn new(brushes: &[BrushVolume], occluders: &[bool]) -> Self {
        let (tree, tier) =
            build_bsp_with_occluder_tier(brushes, occluders).expect("two-tier tree builds");
        let portals = generate_portals(&tree);
        let exterior = find_exterior_leaves(&tree, &portals);
        Self {
            tree,
            tier,
            exterior,
        }
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
    let flags = occluder_flags(&classify(&fixture.brushes));
    let build = TierBuild::new(&fixture.brushes, &flags);
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
