// Interior pockets: one runtime cell per connected pocket of an occluder-tier
// leaf, on a fixture modelled on stress-street's lane cells.
// See: context/plans/in-progress/occluder-first-partition/plan.md §Block (Task 9), resolved

use std::collections::HashMap;

use postretro_visibility::VisibilityPath;

use super::RuntimeCells;
use super::test_fixtures::{Cells, MapText, compile, engine, quake_triple, walk};
use super::tests::stage_keys;
use crate::cell_residency_bake::teleport_routes::{TeleportResidencyInput, build_routes};
use crate::partition::{CellTree, InteriorPockets};
use crate::portals;

/// A hall 960 × 512 × 192 u (24.4 × 13 × 4.9 m) split by an occluding wall
/// at y 256–272 whose middle piece runs x 144–816, with full-height doorways
/// either side of it, so the north room's middle region (x 144–816) is one
/// occluder-tier leaf. A block fills that region's middle (x 330–630) from
/// the wall to the north wall and floor to ceiling. Its broad faces are its
/// top and bottom, which touch ceiling and floor, so it stays exempt (as
/// stress-street's `solid_fill` brush 903 does) and parts the region into a
/// west and an east pocket, each opening onto the room beside it. No cap or
/// grid line crosses the region.
fn lane_pockets(block: &[([f64; 3], [f64; 3])]) -> MapText {
    let mut map = MapText::new();
    map.hall([960.0, 512.0, 192.0])
        .solid([0.0, 256.0, 0.0], [80.0, 272.0, 192.0])
        .solid([144.0, 256.0, 0.0], [816.0, 272.0, 192.0])
        .solid([880.0, 256.0, 0.0], [960.0, 272.0, 192.0])
        .light([480.0, 128.0, 160.0], 800.0);
    for &(min, max) in block {
        map.solid(min, max);
    }
    map
}

/// Lightmap density for the compile, whose rows read no texel.
const COARSE: &[&str] = &["--lightmap-density", "0.25"];

const BLOCK: ([f64; 3], [f64; 3]) = ([330.0, 272.0, 0.0], [630.0, 512.0, 192.0]);

/// The block pierced along x by a tunnel 8 × 8 u (0.2 × 0.2 m, 0.04 m²,
/// under the 0.1 m² portal-area cull) at y 380–388, z 40–48.
const CRACKED_BLOCK: [([f64; 3], [f64; 3]); 4] = [
    ([330.0, 272.0, 0.0], [630.0, 512.0, 40.0]),
    ([330.0, 272.0, 48.0], [630.0, 512.0, 192.0]),
    ([330.0, 272.0, 40.0], [630.0, 380.0, 48.0]),
    ([330.0, 388.0, 40.0], [630.0, 512.0, 48.0]),
];

const WEST_POCKET: [f64; 3] = [240.0, 400.0, 64.0];
const EAST_POCKET: [f64; 3] = [720.0, 400.0, 64.0];
/// In the south room, in front of the east doorway.
const SOUTH_EYE: [f64; 3] = [848.0, 128.0, 64.0];

/// The grouping before cells followed connectivity: one interior cell per
/// occluder-tier leaf, over the same tree.
fn one_cell_per_tier_leaf(cells: &Cells) -> RuntimeCells {
    let (tree, tier, exterior) = (
        cells.full_tree(),
        &cells.partition.tier,
        &cells.full_exterior,
    );
    let grouped = CellTree::build(
        tree,
        tier,
        exterior,
        &InteriorPockets::one_per_tier_leaf(tree, tier, exterior),
    );
    let portals = portals::generate_runtime_portals(tree, tier, &grouped);
    RuntimeCells {
        cells: grouped,
        portals,
    }
}

#[test]
fn pockets_a_solid_parts_in_one_tier_leaf_are_separate_cells_with_no_portal_between() {
    let map = lane_pockets(&[BLOCK]);
    let cells = Cells::of(&map);
    let tier = &cells.partition.tier;
    let (west, east) = (cells.leaf_at(WEST_POCKET), cells.leaf_at(EAST_POCKET));
    assert_eq!(
        tier.leaf_tier_leaf[west], tier.leaf_tier_leaf[east],
        "one occluder-tier leaf holds both pockets"
    );
    let (west_cell, east_cell) = (cells.cell_at(WEST_POCKET), cells.cell_at(EAST_POCKET));
    assert_ne!(west_cell, east_cell, "each pocket is its own cell");
    assert!(cells.is_interior_cell(west_cell) && cells.is_interior_cell(east_cell));
    assert_eq!(
        cells.runtime.cells.tier_leaf_cells[tier.leaf_tier_leaf[west]],
        {
            let mut both = vec![west_cell, east_cell];
            both.sort_unstable();
            both
        }
    );
    assert_eq!(cells.runtime.cells.split_tier_leaf_count(), 1);
    assert!(
        !cells.runtime.portals.iter().any(|portal| {
            [portal.front_leaf, portal.back_leaf] == [west_cell, east_cell]
                || [portal.front_leaf, portal.back_leaf] == [east_cell, west_cell]
        }),
        "no portal joins the two pockets"
    );
    for cell in [west_cell, east_cell] {
        assert!(
            cells
                .runtime
                .portals
                .iter()
                .any(|portal| portal.front_leaf == cell || portal.back_leaf == cell),
            "each pocket opens onto the room beside it"
        );
        let hull = &cells.runtime.cells.hulls[cell];
        assert_eq!(
            hull.pieces.len(),
            cells.members(cell).len(),
            "a pocket's hull is its member leaves' hulls"
        );
    }
    let inside = |cell: usize, q: [f64; 3]| {
        let bounds = &cells.cell_tree().leaves[cell].bounds;
        let p = engine(q);
        bounds.min.cmple(p).all() && bounds.max.cmpge(p).all()
    };
    let block_centre = [480.0, 400.0, 96.0];
    assert!(
        !inside(west_cell, block_centre) && !inside(west_cell, EAST_POCKET),
        "the west pocket's bounds stop at the block"
    );
    assert!(
        !inside(east_cell, block_centre) && !inside(east_cell, WEST_POCKET),
        "the east pocket's bounds stop at the block"
    );

    // Before cells followed connectivity, the tier leaf was one cell.
    let before = one_cell_per_tier_leaf(&cells);
    assert_eq!(
        before.cells.leaf_cell[west], before.cells.leaf_cell[east],
        "the pre-pocket grouping joins the pockets"
    );
    assert_eq!(
        before.cells.tree.leaves.len() + 1,
        cells.cell_tree().leaves.len()
    );
    assert_eq!(before.portals.len(), cells.runtime.portals.len());

    // Compiled and loaded, each pocket locates to its own cell, and the walk
    // reaches the other pocket only through real portals.
    let compiled = compile(&map, COARSE);
    let world = &compiled.world;
    let located = |q: [f64; 3]| world.locate_cell(engine(q).as_vec3());
    assert_eq!(located(WEST_POCKET), west_cell);
    assert_eq!(located(EAST_POCKET), east_cell);

    let (visible, path) = walk(world, WEST_POCKET, [1.0, 0.0, 0.0]);
    assert!(matches!(path, VisibilityPath::PrlPortal { .. }), "{path:?}");
    assert!(visible.contains(&(west_cell as u32)));
    assert!(
        !visible.contains(&(east_cell as u32)),
        "the east pocket draws from the west one, through the block"
    );

    let (visible, path) = walk(world, SOUTH_EYE, [0.0, 1.0, 0.0]);
    assert!(matches!(path, VisibilityPath::PrlPortal { .. }), "{path:?}");
    assert!(
        visible.contains(&(east_cell as u32)),
        "the east pocket is visible through the east doorway"
    );
    assert!(
        !visible.contains(&(west_cell as u32)),
        "the west pocket draws through the east doorway, behind the block"
    );
}

#[test]
fn a_crack_under_the_portal_area_cull_keeps_its_pockets_one_cell() {
    // Pockets join through any shared face above clipping dust, so a crack
    // the cull would drop still joins them, as the tier leaf's one cell did:
    // the walk sees through it no less than before.
    let cells = Cells::of(&lane_pockets(&CRACKED_BLOCK));
    let tier = &cells.partition.tier;
    let (west, east) = (cells.leaf_at(WEST_POCKET), cells.leaf_at(EAST_POCKET));
    assert_eq!(tier.leaf_tier_leaf[west], tier.leaf_tier_leaf[east]);
    assert_eq!(
        cells.cell_at(WEST_POCKET),
        cells.cell_at(EAST_POCKET),
        "the tunnel joins the pockets into one cell"
    );
    assert_eq!(cells.runtime.cells.split_tier_leaf_count(), 0);
    let before = one_cell_per_tier_leaf(&cells);
    assert_eq!(
        before.cells.tree.leaves.len(),
        cells.cell_tree().leaves.len()
    );
    assert_eq!(before.portals.len(), cells.runtime.portals.len());
}

/// `lane_pockets` with a fog volume in the east pocket and a seamless
/// teleporter from the west pocket arriving in the east one.
fn lane_pockets_with_fog_and_teleporter() -> MapText {
    let mut map = lane_pockets(&[BLOCK]);
    map.brush_entity(
        "fog_volume",
        &[("density", "0.5")],
        &[([680.0, 420.0, 8.0], [760.0, 480.0, 48.0])],
    )
    .brush_entity(
        "trigger_teleport",
        &[("target", "out"), ("mode", "seamless"), ("landmark", "in")],
        &[([200.0, 360.0, 0.0], [280.0, 440.0, 64.0])],
    )
    .point_entity(
        "teleport_destination",
        &[
            ("name", "in"),
            ("origin", &quake_triple([240.0, 400.0, 32.0])),
            ("angles", "0 0 0"),
        ],
    )
    .point_entity(
        "teleport_destination",
        &[
            ("name", "out"),
            ("origin", &quake_triple([720.0, 400.0, 32.0])),
            ("angles", "0 0 0"),
        ],
    );
    map
}

#[test]
fn fog_and_teleport_hulls_of_a_split_tier_leaf_read_each_pocket_alone() {
    let cells = Cells::of(&lane_pockets_with_fog_and_teleporter());
    let (west_cell, east_cell) = (cells.cell_at(WEST_POCKET), cells.cell_at(EAST_POCKET));
    assert_ne!(west_cell, east_cell);

    // Fog in the east pocket sets only its cell's bit.
    let fog = &cells.map_data.fog_volumes;
    let masks = crate::fog_cell_masks::bake_fog_cell_masks(
        cells.cell_tree(),
        &cells.runtime.cells.hulls,
        fog,
    )
    .expect("fog masks")
    .masks;
    assert_eq!(masks[east_cell] & 1, 1, "the fogged pocket's bit is set");
    assert_eq!(masks[west_cell] & 1, 0, "the pocket beside it stays clear");
    let before = one_cell_per_tier_leaf(&cells);
    let before_masks =
        crate::fog_cell_masks::bake_fog_cell_masks(&before.cells.tree, &before.cells.hulls, fog)
            .expect("fog masks")
            .masks;
    let merged = before.cells.leaf_cell[cells.leaf_at(WEST_POCKET)];
    assert_eq!(
        before_masks[merged] & 1,
        1,
        "the pre-pocket cell fogs both pockets"
    );

    // The teleporter couples its source pocket to its arrival pocket only:
    // one hull slot per cell, each hull its pocket's pieces.
    let (cells_section, _, _) = cells.sections();
    let world = cells.visibility_world();
    let routes = build_routes(
        TeleportResidencyInput {
            section: &cells.map_data.teleporters,
            hulls: &cells.runtime.cells.hulls,
        },
        &cells_section,
        &world,
    )
    .expect("one hull slot per cell");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].source_cells, vec![west_cell as u32]);
    assert_eq!(routes[0].arrival_cells, vec![east_cell as u32]);
}

#[test]
fn cell_and_portal_stage_keys_miss_when_pockets_regroup_the_same_leaves() {
    // Pin R16: the same BSP leaves, grouped by pocket or by tier leaf.
    let cells = Cells::of(&lane_pockets_with_fog_and_teleporter());
    let pockets = stage_keys(&cells, &cells.runtime.cells, &cells.runtime.portals);
    let before = one_cell_per_tier_leaf(&cells);
    let merged = stage_keys(&cells, &before.cells, &before.portals);
    assert_ne!(pockets.cell_visibility, merged.cell_visibility);
    assert_ne!(pockets.cell_residency_set, merged.cell_residency_set);
    assert_ne!(pockets.chunk_light_list, merged.chunk_light_list);
    assert_ne!(
        pockets.lightmap_atlas_layout, merged.lightmap_atlas_layout,
        "the lightmap layer keys fold the block cell ids"
    );
    assert_ne!(pockets.geometry, merged.geometry);
    assert_eq!(
        pockets.sdf, merged.sdf,
        "the solidity-only SDF keeps hitting"
    );
}

#[test]
fn pockets_do_not_depend_on_brush_order() {
    let map = lane_pockets(&[BLOCK]);
    let lattice: Vec<[f64; 3]> = (0..24)
        .flat_map(|i| {
            (0..13).flat_map(move |j| {
                (0..4).map(move |k| {
                    [
                        20.0 + 40.0 * f64::from(i),
                        20.0 + 40.0 * f64::from(j),
                        24.0 + 48.0 * f64::from(k),
                    ]
                })
            })
        })
        .collect();
    // Cells, interior cells and portals, and the lattice's partition into
    // cells, labelled by first appearance so ids may differ.
    let shape = |reverse: bool| {
        let mut map_data = map.parse();
        if reverse {
            map_data.brush_volumes.reverse();
        }
        let cells = Cells::of_map_data(map_data);
        let mut label: HashMap<usize, usize> = HashMap::new();
        let partition: Vec<usize> = lattice
            .iter()
            .map(|&q| {
                let next = label.len();
                *label.entry(cells.cell_at(q)).or_insert(next)
            })
            .collect();
        (
            cells.cell_tree().leaves.len(),
            cells.runtime.cells.interior_cell_count(),
            cells.runtime.cells.split_tier_leaf_count(),
            cells.runtime.portals.len(),
            partition,
        )
    };
    let (forward, reversed) = (shape(false), shape(true));
    assert_eq!(forward.2, 1, "the block parts one tier leaf");
    assert_eq!(forward, reversed);
}
