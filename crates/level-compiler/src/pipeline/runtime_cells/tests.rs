// Runtime-cell rows: the cell view, cell-id portals and every consumer that
// reads them, on fixture maps compiled in process and loaded by the runtime.
// See: context/plans/in-progress/occluder-first-partition/index.md §Acceptance

use std::collections::HashSet;

use glam::{DVec3, Vec3};
use postretro_level_format::{SectionId, read_container};
use postretro_visibility::VisibilityPath;

use super::test_fixtures::{Cells, MapText, Untiered, compile, engine, load, quake_triple, walk};
use super::{RuntimeCells, classify};
use crate::affinity_grid::{light_portal_reach, portal_adjacency};
use crate::cell_residency_bake::teleport_routes::{TeleportResidencyInput, build_routes};
use crate::map_data::MapLight;
use crate::partition::occluder_classify::{OccluderVerdict, see_over_override};
use crate::partition::{BspChild, CellTree, OccluderTier, TierLeaf, find_leaf_for_point};
use crate::portals;

// ---- fixtures ----

/// Lightmap density for compiles whose rows read no texel: ten times the
/// default texel pitch keeps each bake small.
const COARSE: &[&str] = &["--lightmap-density", "0.25"];

/// Hall 1600 × 1600 × 512 u (40.6 × 40.6 × 13 m) crossed by a retaining wall
/// 1024 u long and 256 u tall (26 × 6.5 m) with a lane past each end, and
/// red-pillar stand-ins (40 × 40 × 144 u, 1.0 × 3.7 m) beyond it: the
/// occlusion-test layout.
fn occlusion_hall() -> MapText {
    let mut map = MapText::new();
    map.hall([1600.0, 1600.0, 512.0])
        .solid([288.0, 784.0, 0.0], [1312.0, 816.0, 256.0]);
    for x in [600.0, 900.0, 1200.0] {
        for y in [1000.0, 1300.0] {
            map.solid([x, y, 0.0], [x + 40.0, y + 40.0, 144.0]);
        }
    }
    map.light([800.0, 400.0, 400.0], 2000.0);
    map
}

/// Index of the retaining wall in `occlusion_hall`'s brushes.
const RETAINING_WALL: usize = 6;

/// Behind the retaining wall at eye height (1.5 m), facing it.
const BEHIND_WALL_EYE: [f64; 3] = [800.0, 300.0, 60.0];
const NORTH: [f64; 3] = [0.0, 1.0, 0.0];

/// Beyond the wall: a floor point between pillars, just over a pillar top,
/// and high above the wall top.
const BEYOND_FLOOR: [f64; 3] = [800.0, 1150.0, 4.0];
const BEYOND_PILLAR_TOP: [f64; 3] = [620.0, 1020.0, 148.0];
const BEYOND_ABOVE_WALL: [f64; 3] = [800.0, 1150.0, 480.0];

/// The red-pillar stand-in at the centre of `pillar_room`.
const PILLAR_MIN: [f64; 3] = [300.0, 300.0, 0.0];
const PILLAR_MAX: [f64; 3] = [340.0, 340.0, 144.0];
const PILLAR_CENTRE: [f64; 3] = [320.0, 320.0, 72.0];

/// Hall 640 × 640 × 256 u (16.3 × 16.3 × 6.5 m) with one red-pillar stand-in
/// at its centre and a static light: one interior cell over several BSP
/// leaves.
fn pillar_room() -> MapText {
    let mut map = MapText::new();
    map.hall([640.0, 640.0, 256.0])
        .solid(PILLAR_MIN, PILLAR_MAX)
        .light([160.0, 160.0, 200.0], 800.0);
    map
}

/// `teleport_rooms` with a fog volume in one corner of the first room.
fn pillar_rooms_with_fog() -> MapText {
    let mut map = teleport_rooms();
    map.brush_entity(
        "fog_volume",
        &[("density", "0.5")],
        &[([8.0, 8.0, 8.0], [48.0, 48.0, 48.0])],
    );
    map
}

// ---- guards every fixture shares ----

/// No runtime portal names a solid or exterior cell, and every endpoint is
/// in range (pins R3, R22).
fn assert_portals_join_interior_cells(cells: &Cells) {
    for (index, portal) in cells.runtime.portals.iter().enumerate() {
        for cell in [portal.front_leaf, portal.back_leaf] {
            assert!(
                cells.is_interior_cell(cell),
                "runtime portal {index} names cell {cell}, which is solid or exterior"
            );
        }
        assert_ne!(portal.front_leaf, portal.back_leaf);
    }
}

/// The view locates every point where the full tree does, solid for solid
/// and exterior for exterior.
fn assert_view_classifies_like_the_full_tree(cells: &Cells) {
    let tree = cells.full_tree();
    for (leaf, entry) in tree.leaves.iter().enumerate() {
        let cell = cells.runtime.cells.leaf_cell[leaf];
        let centre = entry.bounds.centroid();
        if find_leaf_for_point(tree, centre) != leaf {
            continue;
        }
        assert_eq!(find_leaf_for_point(cells.cell_tree(), centre), cell);
        assert_eq!(cells.cell_tree().leaves[cell].is_solid, entry.is_solid);
        assert_eq!(
            cells.runtime.cells.exterior_cells.contains(&cell),
            cells.full_exterior.contains(&leaf)
        );
    }
}

// ---- cell view ----

#[test]
fn cell_tree_groups_each_occluder_tier_leaf_into_one_interior_cell() {
    let cells = Cells::of(&occlusion_hall());
    assert_view_classifies_like_the_full_tree(&cells);
    assert_portals_join_interior_cells(&cells);

    let view = &cells.runtime.cells;
    let tier = &cells.partition.tier;
    for (leaf, entry) in cells.full_tree().leaves.iter().enumerate() {
        let cell = view.leaf_cell[leaf];
        if entry.is_solid || cells.full_exterior.contains(&leaf) {
            assert_eq!(
                cells.members(cell),
                vec![leaf],
                "solid and exterior stay alone"
            );
        } else {
            assert_eq!(Some(cell), view.tier_leaf_cell[tier.leaf_tier_leaf[leaf]]);
            let bounds = &cells.cell_tree().leaves[cell].bounds;
            assert!(bounds.min.cmple(entry.bounds.min).all());
            assert!(bounds.max.cmpge(entry.bounds.max).all());
        }
    }
    assert!(
        view.interior_cell_count() < cells.full_tree().leaves.len() - view.exterior_cells.len(),
        "exempt pillars group several leaves into one cell"
    );
}

#[test]
fn all_occluding_fixture_keeps_each_empty_leaf_its_own_cell() {
    // Pin R2: a hall split by a full-width wall short of the ceiling, every
    // brush occluding.
    let mut map = MapText::new();
    map.hall([640.0, 320.0, 160.0])
        .solid([320.0, 0.0, 0.0], [332.0, 320.0, 120.0]);
    let verdicts = classify(&map.parse().brush_volumes)
        .expect("classifies")
        .verdicts;
    assert!(
        verdicts.iter().all(|&v| v == OccluderVerdict::Occludes),
        "precondition: every brush occludes, got {verdicts:?}"
    );

    let cells = Cells::of(&map);
    let view = &cells.runtime.cells;
    assert_eq!(view.tree.leaves.len(), cells.full_tree().leaves.len());
    for leaf in 0..cells.full_tree().leaves.len() {
        assert_eq!(view.leaf_cell[leaf], leaf, "leaf {leaf} reads its own id");
    }
    assert!(!cells.runtime.portals.is_empty());
    assert_portals_join_interior_cells(&cells);

    let compiled = compile(&map, COARSE);
    assert_eq!(compiled.world.cells.len(), cells.full_tree().leaves.len());
}

// ---- occlusion (C1) ----

#[test]
fn walk_behind_retaining_wall_hides_floor_and_pillar_tops_beyond_it() {
    let map = occlusion_hall();
    let brushes = map.parse().brush_volumes;
    let verdicts = classify(&brushes).expect("classifies").verdicts;
    assert_eq!(verdicts[RETAINING_WALL], OccluderVerdict::Occludes);
    for (pillar, &verdict) in verdicts.iter().enumerate().skip(RETAINING_WALL + 1) {
        assert_ne!(verdict, OccluderVerdict::Occludes, "pillar {pillar}");
    }

    let compiled = compile(&map, COARSE);
    let world = &compiled.world;
    let cell = |q: [f64; 3]| world.locate_cell(engine(q).as_vec3()) as u32;
    let (visible, path) = walk(world, BEHIND_WALL_EYE, NORTH);
    assert!(matches!(path, VisibilityPath::PrlPortal { .. }), "{path:?}");
    assert!(
        !visible.contains(&cell(BEYOND_FLOOR)),
        "the floor beyond the wall draws from behind it"
    );
    assert!(
        !visible.contains(&cell(BEYOND_PILLAR_TOP)),
        "a pillar top beyond the wall draws from behind it"
    );
    assert!(
        visible.contains(&cell(BEYOND_ABOVE_WALL)),
        "the space above the wall top beyond it must stay visible"
    );

    // Today's build: its column cells beyond the wall are portal-visible
    // over it, so their floors or pillar tops draw.
    let before = Untiered::of(&map.parse());
    let before_world = before.visibility_world();
    let (before_visible, _) = walk(&before_world, BEHIND_WALL_EYE, NORTH);
    let hidden_drawn = [BEYOND_FLOOR, BEYOND_PILLAR_TOP]
        .iter()
        .filter(|&&q| before_visible.contains(&(before.cell_at(q) as u32)))
        .count();
    println!(
        "occlusion hall: after {} cells / {} portals, {} visible; before {} cells / {} \
         portals, {} visible, {hidden_drawn} of 2 hidden points' cells drawn",
        world.cells.len(),
        world.portals.len(),
        visible.len(),
        before_world.cells.len(),
        before_world.portals.len(),
        before_visible.len(),
    );
    assert!(
        hidden_drawn > 0,
        "the pre-change build was expected to draw a hidden floor or pillar-top cell"
    );
}

// ---- teleporters (C2, C3) ----

/// Two rooms either side of an occluding wall that stops 56 u under the
/// ceiling, each with a pillar, and a seamless teleporter whose source region
/// and arrival image each straddle their room's pillar planes.
fn teleport_rooms() -> MapText {
    let mut map = MapText::new();
    map.hall([1280.0, 640.0, 256.0])
        .solid([640.0, 0.0, 0.0], [656.0, 640.0, 200.0])
        .solid(PILLAR_MIN, PILLAR_MAX)
        .solid([940.0, 300.0, 0.0], [980.0, 340.0, 144.0])
        .light([160.0, 160.0, 200.0], 600.0)
        .light([1100.0, 160.0, 200.0], 600.0)
        .brush_entity(
            "trigger_teleport",
            &[("target", "out"), ("mode", "seamless"), ("landmark", "in")],
            &[([280.0, 200.0, 0.0], [360.0, 260.0, 64.0])],
        )
        .point_entity(
            "teleport_destination",
            &[
                ("name", "in"),
                ("origin", &quake_triple([320.0, 230.0, 32.0])),
                ("angles", "0 0 0"),
            ],
        )
        .point_entity(
            "teleport_destination",
            &[
                ("name", "out"),
                ("origin", &quake_triple([960.0, 230.0, 32.0])),
                ("angles", "0 0 0"),
            ],
        );
    map
}

/// Interior full-tree leaves a Quake-unit box's interior lattice touches.
fn leaves_in_box(cells: &Cells, min: [f64; 3], max: [f64; 3]) -> HashSet<usize> {
    let mut leaves = HashSet::new();
    for i in 1..8 {
        for j in 1..8 {
            for k in 1..8 {
                let t = |a: f64, b: f64, n: i32| a + (b - a) * f64::from(n) / 8.0;
                let q = [
                    t(min[0], max[0], i),
                    t(min[1], max[1], j),
                    t(min[2], max[2], k),
                ];
                let leaf = cells.leaf_at(q);
                if cells.is_interior_cell(cells.runtime.cells.leaf_cell[leaf]) {
                    leaves.insert(leaf);
                }
            }
        }
    }
    leaves
}

#[test]
fn seamless_teleporter_across_several_leaves_couples_each_cell_once() {
    // Pin R9 and the one-hull-slot-per-cell guard.
    let cells = Cells::of(&teleport_rooms());
    assert_portals_join_interior_cells(&cells);
    let source_leaves = leaves_in_box(&cells, [280.0, 200.0, 0.0], [360.0, 260.0, 64.0]);
    let image_leaves = leaves_in_box(&cells, [920.0, 200.0, 0.0], [1000.0, 260.0, 64.0]);
    assert!(
        source_leaves.len() > 1,
        "the source region crosses several leaves"
    );
    assert!(
        image_leaves.len() > 1,
        "the arrival image crosses several leaves"
    );
    let source_cell = cells.cell_at([320.0, 230.0, 32.0]);
    let arrival_cell = cells.cell_at([960.0, 230.0, 32.0]);
    for leaf in &source_leaves {
        assert_eq!(cells.runtime.cells.leaf_cell[*leaf], source_cell);
    }
    for leaf in &image_leaves {
        assert_eq!(cells.runtime.cells.leaf_cell[*leaf], arrival_cell);
    }

    let (cells_section, _, _) = cells.sections();
    let world = cells.visibility_world();
    let teleporters = &cells.map_data.teleporters;
    let routes = build_routes(
        TeleportResidencyInput {
            section: teleporters,
            tree: cells.cell_tree(),
        },
        &cells_section,
        &world,
    )
    .expect("one hull slot per cell");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].source_cells, vec![source_cell as u32]);
    assert_eq!(routes[0].arrival_cells, vec![arrival_cell as u32]);

    // The full tree has a hull slot per leaf, not per cell: the guard bites.
    assert!(
        build_routes(
            TeleportResidencyInput {
                section: teleporters,
                tree: cells.full_tree(),
            },
            &cells_section,
            &world,
        )
        .is_err()
    );

    let compiled = compile(&teleport_rooms(), COARSE);
    assert!(
        compiled.world.cell_residency_set.is_some(),
        "id 51 bakes with its teleport routes"
    );
}

// ---- multi-leaf cells (C4, C5, F1) ----

#[test]
fn multi_leaf_cell_holds_every_member_leaf_in_each_consumer() {
    // Pin R20.
    let map = pillar_rooms_with_fog();
    let cells = Cells::of(&map);
    assert_portals_join_interior_cells(&cells);
    assert!(
        !cells.runtime.portals.is_empty(),
        "the rooms share a portal"
    );
    let cell = cells.cell_at([160.0, 160.0, 100.0]);
    let members = cells.members(cell);
    assert!(
        members.len() > 1,
        "the pillar splits the room's cell into leaves"
    );

    // Fog: a volume touching only some member leaves sets the cell's bit.
    let fog = &cells.map_data.fog_volumes;
    let leaf_masks = crate::fog_cell_masks::bake_fog_cell_masks(cells.full_tree(), fog)
        .expect("fog masks")
        .masks;
    let fogged = members
        .iter()
        .filter(|&&leaf| leaf_masks[leaf] & 1 != 0)
        .count();
    assert!(
        fogged > 0 && fogged < members.len(),
        "fog touches some members only"
    );
    let cell_masks = crate::fog_cell_masks::bake_fog_cell_masks(cells.cell_tree(), fog)
        .expect("fog masks")
        .masks;
    assert_eq!(cell_masks[cell] & 1, 1, "the cell's fog bit is set");

    // Bounds enclose every member.
    let bounds = &cells.cell_tree().leaves[cell].bounds;
    for &leaf in &members {
        let member = &cells.full_tree().leaves[leaf].bounds;
        assert!(bounds.min.cmple(member.min).all() && bounds.max.cmpge(member.max).all());
    }

    // Faces, draw list and lightmap blocks, read from the compiled level.
    let member_faces: usize = members
        .iter()
        .map(|&leaf| cells.full_tree().leaves[leaf].face_indices.len())
        .sum();
    let faced_members = members
        .iter()
        .filter(|&&leaf| !cells.full_tree().leaves[leaf].face_indices.is_empty())
        .count();
    assert!(faced_members > 1, "several member leaves own faces");

    let compiled = compile(&map, COARSE);
    let world = &compiled.world;
    let loaded_cell = world.locate_cell(engine([160.0, 160.0, 100.0]).as_vec3());
    assert_eq!(
        loaded_cell, cell,
        "the compile assigns the in-process cell ids"
    );
    let data = &world.cells[cell];
    assert_eq!(
        data.face_count as usize, member_faces,
        "the face range holds every leaf's"
    );
    let range = data.face_start as usize..(data.face_start + data.face_count) as usize;
    for (face, meta) in world.face_meta.iter().enumerate() {
        assert_eq!(meta.leaf_index as usize == cell, range.contains(&face));
    }
    for &leaf in &members {
        let member = &cells.full_tree().leaves[leaf].bounds;
        assert!(
            data.bounds_min
                .cmple(member.min.as_vec3() + Vec3::splat(1e-4))
                .all()
        );
        assert!(
            data.bounds_max
                .cmpge(member.max.as_vec3() - Vec3::splat(1e-4))
                .all()
        );
    }

    let draw_index = world.cell_draw_index.as_ref().expect("CellDrawIndex");
    let spans = &draw_index.spans[draw_index.cell_span_offset[cell] as usize
        ..draw_index.cell_span_offset[cell + 1] as usize];
    let drawn: HashSet<usize> = spans
        .iter()
        .flat_map(|span| span.leaf_start as usize..(span.leaf_start + span.leaf_count) as usize)
        .collect();
    let owned: HashSet<usize> = world
        .bvh
        .leaves
        .iter()
        .enumerate()
        .filter(|(_, leaf)| leaf.cell_id as usize == cell && leaf.index_count > 0)
        .map(|(index, _)| index)
        .collect();
    assert!(!owned.is_empty());
    assert_eq!(
        drawn, owned,
        "the draw list holds every BVH leaf of the cell"
    );

    let lightmap = world.lightmap.as_ref().expect("Lightmap blocks");
    for &bvh_leaf in &owned {
        let leaf = &world.bvh.leaves[bvh_leaf];
        let indices = &world.indices
            [leaf.index_offset as usize..(leaf.index_offset + leaf.index_count) as usize];
        for &index in indices {
            let block = world.vertices[index as usize].lightmap_block;
            assert!(block > 0, "every face of the cell has a lightmap block");
            assert_eq!(lightmap.records[block as usize - 1].cell_id as usize, cell);
        }
    }

    // Reachability: cells a portal joins share a component.
    let visibility = world.cell_visibility.as_ref().expect("CellVisibility");
    let components = visibility.component_ids();
    assert!(!world.portals.is_empty());
    for portal in &world.portals {
        assert_eq!(components[portal.front_cell], components[portal.back_cell]);
    }
}

#[test]
fn pillar_interior_stays_a_solid_cell_that_buries_lights_and_blocks_the_player() {
    let mut map = pillar_room();
    map.light(PILLAR_CENTRE, 300.0);
    let cells = Cells::of(&map);
    let solid = cells.cell_at(PILLAR_CENTRE);
    assert!(cells.cell_tree().leaves[solid].is_solid);
    assert_eq!(cells.members(solid).len(), 1);

    let buried =
        crate::buried_lights::BuriedLights::classify(cells.full_tree(), &cells.map_data.lights);
    let pillar_light = cells
        .map_data
        .lights
        .iter()
        .position(|light| (light.origin - engine(PILLAR_CENTRE)).length() < 1e-6)
        .expect("the buried light parses");
    assert!(buried.is_buried(pillar_light));

    // The pillar's bottom face and the floor beneath it are culled.
    let footprint_min = engine(PILLAR_MIN);
    let footprint_max = engine([PILLAR_MAX[0], PILLAR_MAX[1], 0.0]);
    let (lo, hi) = (
        footprint_min.min(footprint_max),
        footprint_min.max(footprint_max),
    );
    for face in &cells.partition.result.faces {
        let centroid = face.vertices.iter().sum::<DVec3>() / face.vertices.len() as f64;
        let under_pillar = centroid.y.abs() < 1e-6
            && centroid.x > lo.x
            && centroid.x < hi.x
            && centroid.z > lo.z
            && centroid.z < hi.z;
        assert!(
            !under_pillar,
            "a face buried under the pillar survives: {centroid}"
        );
    }

    let compiled = compile(&map, COARSE);
    let world = &compiled.world;
    let centre = engine(PILLAR_CENTRE).as_vec3();
    assert!(world.cells[world.locate_cell(centre)].is_solid);
    assert_eq!(
        world.lights.len(),
        cells.map_data.lights.len() - 1,
        "the buried light is excluded"
    );

    let collision = postretro_physics::collision::CollisionWorld::from_level(world)
        .expect("collision world builds");
    let combined =
        postretro_physics::collision::moving::CombinedCollisionWorld::static_only(&collision);
    let capsule = postretro_foundation::CapsuleParams {
        radius: 0.2,
        half_height: 0.3,
        eye_height: 0.4,
    };
    assert!(!postretro_physics::collision::capsule_fits_combined(
        &combined, centre, &capsule
    ));
    let open = engine([160.0, 160.0, 40.0]).as_vec3();
    assert!(postretro_physics::collision::capsule_fits_combined(
        &combined, open, &capsule
    ));
}

#[test]
fn loader_accepts_multi_leaf_cells_and_rejects_a_face_naming_another_cell() {
    let map = pillar_room();
    let cells = Cells::of(&map);
    let cell = cells.cell_at([160.0, 160.0, 100.0]);
    assert!(cells.members(cell).len() > 1);
    let compiled = compile(&map, COARSE);
    let other = (0..compiled.world.cells.len())
        .find(|&c| c != cell && compiled.world.cells[c].face_count > 0)
        .unwrap_or_else(|| {
            (0..compiled.world.cells.len())
                .find(|&c| c != cell)
                .expect("another cell")
        });

    let bytes = std::fs::read(&compiled.prl).expect("compiled PRL");
    let meta = read_container(&mut std::io::Cursor::new(&bytes)).expect("container");
    let sections: Vec<postretro_level_format::SectionBlob> = meta
        .sections
        .iter()
        .map(|entry| {
            let start = entry.offset as usize;
            let mut data = bytes[start..start + entry.size as usize].to_vec();
            if entry.section_id == SectionId::Geometry as u32 {
                let mut geometry =
                    postretro_level_format::geometry::GeometrySection::from_bytes(&data)
                        .expect("geometry decodes");
                let face = geometry
                    .faces
                    .iter()
                    .position(|face| face.leaf_index as usize == cell)
                    .expect("the cell owns a face");
                geometry.faces[face].leaf_index = other as u32;
                data = geometry.to_bytes();
            }
            postretro_level_format::SectionBlob {
                section_id: entry.section_id,
                version: entry.version,
                data,
            }
        })
        .collect();
    let tampered = compiled.prl.with_file_name("tampered.prl");
    let mut file = std::fs::File::create(&tampered).expect("tampered PRL");
    postretro_level_format::write_prl(&mut file, &sections).expect("tampered PRL writes");
    drop(file);
    let error = load(&tampered).expect_err("a face naming another cell must not load");
    assert!(
        error.to_string().contains("leaf_index"),
        "rejected for the face's cell: {error}"
    );
}

// ---- guards on seals (C6, C7, C8, C9) ----

/// Rooms either side of two walls 44 u (1.1 m) apart that stop 56 u under
/// the ceiling; the slot between them is filled by touching exempt blocks.
fn filled_slot() -> MapText {
    let mut map = MapText::new();
    map.hall([640.0, 640.0, 256.0])
        .solid([0.0, 200.0, 0.0], [640.0, 216.0, 200.0])
        .solid([0.0, 260.0, 0.0], [640.0, 276.0, 200.0]);
    for x in (0..16).map(|i| f64::from(i) * 40.0) {
        map.solid([x, 216.0, 0.0], [x + 40.0, 260.0, 200.0]);
    }
    map.light([320.0, 100.0, 200.0], 600.0);
    map
}

/// A window 40 u (1 m) to the void near one end of an occluding wall,
/// plugged flush with the wall's outer face by an exempt brush half its
/// depth.
fn plugged_window() -> MapText {
    let mut map = MapText::new();
    let t = 16.0;
    map.solid([-t, -t, -t], [400.0 + t, 400.0 + t, 0.0])
        .solid([-t, -t, 256.0], [400.0 + t, 400.0 + t, 256.0 + t])
        .solid([-t, -t, 0.0], [0.0, 400.0 + t, 256.0])
        .solid([400.0, -t, 0.0], [400.0 + t, 400.0 + t, 256.0])
        .solid([0.0, -t, 0.0], [400.0, 0.0, 256.0])
        .solid([0.0, 400.0, 0.0], [340.0, 400.0 + t, 256.0])
        .solid([380.0, 400.0, 0.0], [400.0, 400.0 + t, 256.0])
        .solid([340.0, 400.0, 0.0], [380.0, 400.0 + t, 100.0])
        .solid([340.0, 400.0, 140.0], [380.0, 400.0 + t, 256.0])
        .solid([340.0, 408.0, 100.0], [380.0, 400.0 + t, 140.0])
        .light([200.0, 200.0, 200.0], 600.0);
    map
}

/// An inner doorway 40 u (1 m) wide near one end of an occluding wall,
/// plugged flush with the wall's near face by an exempt brush half its
/// depth.
fn plugged_doorway() -> MapText {
    let mut map = MapText::new();
    map.hall([640.0, 640.0, 256.0])
        .solid([0.0, 320.0, 0.0], [560.0, 336.0, 256.0])
        .solid([600.0, 320.0, 0.0], [640.0, 336.0, 256.0])
        .solid([560.0, 320.0, 100.0], [600.0, 336.0, 256.0])
        .solid([560.0, 320.0, 0.0], [600.0, 328.0, 100.0])
        .light([320.0, 160.0, 200.0], 600.0)
        .light([320.0, 480.0, 200.0], 600.0);
    map
}

#[test]
fn no_runtime_portal_leads_into_solid_or_joins_interior_to_exterior() {
    // Pins R3 and R22.
    let slot = Cells::of(&filled_slot());
    let slot_leaf = slot.leaf_at([300.0, 238.0, 100.0]);
    let slot_tier_leaf = slot.partition.tier.leaf_tier_leaf[slot_leaf];
    assert!(slot.cell_tree().leaves[slot.cell_at([300.0, 238.0, 100.0])].is_solid);
    assert_eq!(
        slot.runtime.cells.tier_leaf_cell[slot_tier_leaf], None,
        "the filled region gets no interior cell"
    );
    for map in [filled_slot(), plugged_window(), plugged_doorway()] {
        let cells = Cells::of(&map);
        assert_portals_join_interior_cells(&cells);
        assert_view_classifies_like_the_full_tree(&cells);
        let compiled = compile(&map, COARSE);
        for portal in &compiled.world.portals {
            for cell in [portal.front_cell, portal.back_cell] {
                let data = &compiled.world.cells[cell];
                assert!(!data.is_solid && !data.is_exterior);
            }
        }
    }
    let compiled = compile(&filled_slot(), COARSE);
    let (visible, _) = walk(&compiled.world, [320.0, 100.0, 60.0], NORTH);
    assert!(
        visible
            .iter()
            .all(|&c| !compiled.world.cells[c as usize].is_solid)
    );
}

/// A hall whose east wall opens on a 40 u (1 m) alcove sealed from the void
/// only by an exempt back brush; the alcove's sides, roof and floor are
/// exempt too.
fn exempt_sealed_alcove() -> MapText {
    let mut map = MapText::new();
    let t = 16.0;
    map.solid([-t, -t, -t], [400.0 + t, 640.0 + t, 0.0])
        .solid([-t, -t, 256.0], [400.0 + t, 640.0 + t, 256.0 + t])
        .solid([-t, -t, 0.0], [0.0, 640.0 + t, 256.0])
        .solid([0.0, -t, 0.0], [400.0, 0.0, 256.0])
        .solid([0.0, 640.0, 0.0], [400.0, 640.0 + t, 256.0])
        .solid([400.0, -t, 0.0], [416.0, 300.0, 256.0])
        .solid([400.0, 340.0, 0.0], [416.0, 640.0 + t, 256.0])
        .solid([400.0, 300.0, 100.0], [416.0, 340.0, 256.0])
        .solid([416.0, 284.0, 0.0], [456.0, 300.0, 100.0])
        .solid([416.0, 340.0, 0.0], [456.0, 356.0, 100.0])
        .solid([416.0, 284.0, 100.0], [456.0, 356.0, 116.0])
        .solid([416.0, 284.0, -t], [456.0, 356.0, 0.0])
        .solid([448.0, 300.0, 0.0], [456.0, 340.0, 100.0])
        .light([200.0, 320.0, 200.0], 800.0);
    map
}

const ALCOVE_POINT: [f64; 3] = [432.0, 320.0, 50.0];
const BEHIND_ALCOVE: [f64; 3] = [480.0, 320.0, 50.0];

#[test]
fn alcove_sealed_only_by_an_exempt_brush_stays_sealed_drawn_and_visible() {
    // Pin R15.
    let map = exempt_sealed_alcove();
    let brushes = map.parse().brush_volumes;
    let verdicts = classify(&brushes).expect("classifies").verdicts;
    assert_ne!(
        verdicts[brushes.len() - 1],
        OccluderVerdict::Occludes,
        "the back is exempt"
    );

    let cells = Cells::of(&map);
    assert_portals_join_interior_cells(&cells);
    let alcove = cells.cell_at(ALCOVE_POINT);
    assert!(cells.is_interior_cell(alcove), "no leak into the alcove");
    assert!(cells.is_interior_cell(cells.cell_at([200.0, 320.0, 60.0])));
    // The back's inner face (Quake -x, engine +z) is extracted, fronting
    // interior space, so geometry keeps it.
    let back_face = engine([448.0, 320.0, 50.0]);
    assert!(
        cells.partition.result.faces.iter().any(|face| {
            let centroid = face.vertices.iter().sum::<DVec3>() / face.vertices.len() as f64;
            let front = find_leaf_for_point(cells.cell_tree(), centroid + face.normal * 1e-3);
            (centroid.z - back_face.z).abs() < 1e-6
                && face.normal.dot(DVec3::Z) > 0.99
                && cells.is_interior_cell(front)
        }),
        "the back's inner face is drawn"
    );

    let compiled = compile(&map, COARSE);
    let world = &compiled.world;
    let loaded_alcove = world.locate_cell(engine(ALCOVE_POINT).as_vec3());
    assert!(world.cells[loaded_alcove].is_drawable, "the alcove draws");
    let (visible, _) = walk(world, [200.0, 320.0, 60.0], [1.0, 0.0, 0.0]);
    assert!(visible.contains(&(loaded_alcove as u32)));

    let behind = engine(BEHIND_ALCOVE).as_vec3();
    let behind_cell = &world.cells[world.locate_cell(behind)];
    assert!(behind_cell.is_exterior || behind_cell.is_solid);
    let collision = postretro_physics::collision::CollisionWorld::from_level(world)
        .expect("collision world builds");
    let combined =
        postretro_physics::collision::moving::CombinedCollisionWorld::static_only(&collision);
    let capsule = postretro_foundation::CapsuleParams {
        radius: 0.2,
        half_height: 0.3,
        eye_height: 0.4,
    };
    assert!(!postretro_physics::collision::capsule_fits_combined(
        &combined, behind, &capsule
    ));
}

/// Interior non-solid leaves the full-tree flood leaves interior.
fn interior_leaf_count(tree: &crate::partition::BspTree, exterior: &HashSet<usize>) -> usize {
    (0..tree.leaves.len())
        .filter(|&leaf| !tree.leaves[leaf].is_solid && !exterior.contains(&leaf))
        .count()
}

/// Sampled points whose exterior class differs between the two-tier build
/// and today's: a lattice of 12 per axis over the brushes' bounds.
fn exterior_reclassified(cells: &Cells, before: &Untiered) -> Vec<DVec3> {
    let mut bounds = crate::partition::Aabb::empty();
    for brush in &cells.map_data.brush_volumes {
        bounds.expand_aabb(&brush.aabb);
    }
    let mut flipped = Vec::new();
    for i in 0..12 {
        for j in 0..12 {
            for k in 0..12 {
                let f = |n: i32| (f64::from(n) + 0.5) / 12.0;
                let p = bounds.min + (bounds.max - bounds.min) * DVec3::new(f(i), f(j), f(k));
                let after = find_leaf_for_point(cells.full_tree(), p);
                let today = find_leaf_for_point(&before.tree, p);
                if cells.full_tree().leaves[after].is_solid || before.tree.leaves[today].is_solid {
                    continue;
                }
                if cells.full_exterior.contains(&after) != before.exterior.contains(&today) {
                    flipped.push(p);
                }
            }
        }
    }
    flipped
}

#[test]
fn leaks_and_exterior_classes_match_the_pre_change_build() {
    // Pin R14: a leak through a 40 × 40 u (1.03 m², over four times the
    // portal-area cull) hole is still a leak; sealed fixtures stay sealed and
    // classify sampled points as today's build does.
    let mut leaky = MapText::new();
    let t = 16.0;
    leaky
        .solid([-t, -t, -t], [400.0 + t, 400.0 + t, 0.0])
        .solid([-t, -t, 200.0], [400.0 + t, 400.0 + t, 200.0 + t])
        .solid([-t, -t, 0.0], [0.0, 400.0 + t, 200.0])
        .solid([0.0, -t, 0.0], [400.0, 0.0, 200.0])
        .solid([0.0, 400.0, 0.0], [400.0, 400.0 + t, 200.0])
        .solid([400.0, -t, 0.0], [400.0 + t, 180.0, 200.0])
        .solid([400.0, 220.0, 0.0], [400.0 + t, 400.0 + t, 200.0])
        .solid([400.0, 180.0, 0.0], [400.0 + t, 220.0, 60.0])
        .solid([400.0, 180.0, 100.0], [400.0 + t, 220.0, 200.0]);
    let cells = Cells::of(&leaky);
    let before = Untiered::of(&cells.map_data);
    assert_eq!(
        interior_leaf_count(&before.tree, &before.exterior),
        0,
        "today's build leaks"
    );
    assert_eq!(
        interior_leaf_count(cells.full_tree(), &cells.full_exterior),
        0,
        "the leak is still reported"
    );
    assert_eq!(cells.runtime.cells.interior_cell_count(), 0);

    let fixtures = [
        ("occlusion hall", occlusion_hall()),
        ("pillar rooms", pillar_rooms_with_fog()),
        ("teleport rooms", teleport_rooms()),
        ("filled slot", filled_slot()),
        ("plugged window", plugged_window()),
        ("plugged doorway", plugged_doorway()),
        ("exempt alcove", exempt_sealed_alcove()),
        ("closet", sealed_closet()),
    ];
    let occlusion_test = {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/dev/maps/occlusion-test.map");
        crate::parse::parse_map_file(&path, crate::map_format::MapFormat::IdTech2)
            .expect("occlusion-test.map parses")
    };
    let sealed = fixtures
        .into_iter()
        .map(|(name, map)| (name, map.parse()))
        .chain([("occlusion-test.map", occlusion_test)]);
    for (name, map_data) in sealed {
        let cells = Cells::of_map_data(map_data);
        let before = Untiered::of(&cells.map_data);
        assert!(
            interior_leaf_count(&before.tree, &before.exterior) > 0,
            "{name}: sealed today"
        );
        assert!(
            interior_leaf_count(cells.full_tree(), &cells.full_exterior) > 0,
            "{name}: still sealed"
        );
        let flipped = exterior_reclassified(&cells, &before);
        assert!(
            flipped.is_empty(),
            "{name}: points change exterior class: {flipped:?}"
        );
    }
}

/// A sealed closet 44 × 44 × 120 u (1.1 × 1.1 × 3 m) of slabs meeting edge
/// to edge: every brush is under the floor on a broad axis.
fn sealed_closet() -> MapText {
    let mut map = MapText::new();
    let t = 12.0;
    map.solid([0.0, 0.0, -t], [44.0, 44.0, 0.0])
        .solid([0.0, 0.0, 120.0], [44.0, 44.0, 120.0 + t])
        .solid([-t, 0.0, 0.0], [0.0, 44.0, 120.0])
        .solid([44.0, 0.0, 0.0], [44.0 + t, 44.0, 120.0])
        .solid([0.0, -t, 0.0], [44.0, 0.0, 120.0])
        .solid([0.0, 44.0, 0.0], [44.0, 44.0 + t, 120.0]);
    map
}

#[test]
fn sealed_map_without_occluders_is_one_cell_with_no_portals() {
    // Pin R1.
    let map = sealed_closet();
    let verdicts = classify(&map.parse().brush_volumes)
        .expect("classifies")
        .verdicts;
    assert!(verdicts.iter().all(|&v| v != OccluderVerdict::Occludes));
    let cells = Cells::of(&map);
    assert_eq!(cells.runtime.cells.interior_cell_count(), 1);
    assert!(cells.runtime.portals.is_empty());

    let compiled = compile(&map, COARSE);
    let world = &compiled.world;
    assert!(world.portals.is_empty());
    let inside = world.locate_cell(engine([22.0, 22.0, 60.0]).as_vec3());
    assert!(world.cells[inside].is_drawable);
    let (visible, path) = walk(world, [22.0, 4.0, 60.0], NORTH);
    assert!(
        matches!(path, VisibilityPath::NoPortalsFallback),
        "{path:?}"
    );
    assert!(visible.contains(&(inside as u32)));
}

// ---- lights (C16, C17) ----

/// A static point light at engine `origin`.
fn light_at(origin: DVec3) -> MapLight {
    let mut map = MapText::new();
    map.hall([64.0, 64.0, 64.0])
        .light([32.0, 32.0, 32.0], 300.0);
    let mut light = map.parse().lights.remove(0);
    light.origin = origin;
    light
}

#[test]
fn light_just_outside_a_pillar_records_and_floods_the_room_cell() {
    // Pin R11: 0.02 u (0.5 mm) off the pillar's +x face.
    let cells = Cells::of(&pillar_room());
    let origin = engine([PILLAR_MAX[0] + 0.02, 320.0, 72.0]);
    let lights = vec![light_at(origin)];
    let buried = crate::buried_lights::BuriedLights::classify(cells.full_tree(), &lights);
    assert!(!buried.is_buried(0), "the light is kept");

    let far_side = cells.cell_at([200.0, 320.0, 72.0]);
    assert!(cells.is_interior_cell(far_side));
    let namespace = crate::light_namespaces::AlphaLightsNs::from_lights(&lights);
    let section = crate::pack::encode_alpha_lights(&namespace, cells.cell_tree());
    assert_eq!(section.lights[0].leaf_index as usize, far_side);
    let before = Untiered::of(&cells.map_data);
    assert_ne!(
        find_leaf_for_point(&before.tree, origin),
        before.cell_at([200.0, 320.0, 72.0]),
        "today the light sits in a sliver beside the pillar"
    );

    let adjacency = portal_adjacency(&cells.runtime.portals);
    let reach = light_portal_reach(
        &lights[0],
        cells.cell_tree(),
        &cells.runtime.cells.exterior_cells,
        &adjacency,
    )
    .expect("an interior light floods");
    assert!(reach.contains(&far_side));
}

#[test]
fn light_on_an_occluder_plane_records_the_front_cell_and_floods_behind() {
    // Pin R10: on the retaining wall's top plane, beyond the wall in open
    // space.
    let cells = Cells::of(&occlusion_hall());
    let beyond = engine([800.0, 1150.0, 256.0]);
    let tier = &cells.partition.tier;
    let (node, plane) = cells
        .full_tree()
        .nodes
        .iter()
        .enumerate()
        .find(|(index, node)| {
            tier.node_is_tier[*index]
                && node.plane_normal == DVec3::Y
                && (node.plane_distance - beyond.y).abs() < 1e-6
        })
        .expect("the wall top is an occluder-tier plane");
    let origin = DVec3::new(beyond.x, plane.plane_distance, beyond.z);
    assert!(node < cells.full_tree().nodes.len());
    let front = find_leaf_for_point(cells.cell_tree(), origin + DVec3::Y * 0.01);
    let back = find_leaf_for_point(cells.cell_tree(), origin - DVec3::Y * 0.01);
    assert_ne!(front, back);
    assert!(cells.is_interior_cell(front) && cells.is_interior_cell(back));

    let lights = vec![light_at(origin)];
    let namespace = crate::light_namespaces::AlphaLightsNs::from_lights(&lights);
    let section = crate::pack::encode_alpha_lights(&namespace, cells.cell_tree());
    assert_eq!(section.lights[0].leaf_index as usize, front);
    let adjacency = portal_adjacency(&cells.runtime.portals);
    let reach = light_portal_reach(
        &lights[0],
        cells.cell_tree(),
        &cells.runtime.cells.exterior_cells,
        &adjacency,
    )
    .expect("an interior light floods");
    assert!(reach.contains(&back));
}

// ---- format (F2) ----

#[test]
fn oversize_face_in_a_multi_leaf_cell_keeps_every_piece_in_that_cell() {
    // Pin R21: a 2100 u (53 m) hall, past one 2048-texel pool layer at the
    // default density, split lengthwise beneath the tier by an exempt curb.
    let mut map = MapText::new();
    map.hall([2100.0, 80.0, 100.0])
        .solid([0.0, 30.0, 0.0], [2100.0, 42.0, 12.0])
        .light([1050.0, 60.0, 80.0], 2400.0);
    let cells = Cells::of(&map);
    let cell = cells.cell_at([1050.0, 15.0, 50.0]);
    assert!(cells.members(cell).len() > 1, "the curb splits the cell");
    let pre_cut = crate::geometry::extract_geometry(
        &cells.partition.result.faces,
        cells.cell_tree(),
        &cells.runtime.cells.exterior_cells,
    );
    let pre_cut_faces = pre_cut
        .geometry
        .faces
        .iter()
        .filter(|face| face.leaf_index as usize == cell)
        .count();

    let compiled = compile(&map, &[]);
    let world = &compiled.world;
    let data = &world.cells[cell];
    assert!(
        data.face_count as usize > pre_cut_faces,
        "the face cut splits faces inside the cell ({} vs {pre_cut_faces})",
        data.face_count
    );
    let range = data.face_start as usize..(data.face_start + data.face_count) as usize;
    for (face, meta) in world.face_meta.iter().enumerate() {
        assert_eq!(
            meta.leaf_index as usize == cell,
            range.contains(&face),
            "face {face}: the cell's faces stay one contiguous range"
        );
    }
}

// ---- cache keys (F6) ----

/// A coarser tier over the same tree: the smallest tier sub-tree holding two
/// or more interior tier leaves becomes one tier leaf, its tier nodes
/// ordinary nodes. The full tree and its leaves are untouched.
fn merge_one_tier_subtree(cells: &Cells) -> OccluderTier {
    let tree = cells.full_tree();
    let tier = &cells.partition.tier;
    // Tier leaves under a tier node, depth-first.
    fn tier_leaves_under(
        tree: &crate::partition::BspTree,
        tier: &OccluderTier,
        child: &BspChild,
        out: &mut Vec<usize>,
    ) {
        match child {
            BspChild::Node(node) if tier.node_is_tier[*node] => {
                tier_leaves_under(tree, tier, &tree.nodes[*node].front, out);
                tier_leaves_under(tree, tier, &tree.nodes[*node].back, out);
            }
            other => out.push(tier.tier_leaf_of(tree, other)),
        }
    }
    let interior = |t: &usize| cells.runtime.cells.tier_leaf_cell[*t].is_some();
    let (node, merged) = (0..tree.nodes.len())
        .filter(|&node| tier.node_is_tier[node])
        .map(|node| {
            let mut under = Vec::new();
            tier_leaves_under(tree, tier, &BspChild::Node(node), &mut under);
            (node, under)
        })
        .filter(|(_, under)| under.iter().filter(|t| interior(t)).count() >= 2)
        .min_by_key(|(_, under)| under.len())
        .expect("a tier node above two interior tier leaves");

    // The merged region's planes: the node's own region, from its ancestors.
    let mut planes = Vec::new();
    let mut child = node;
    while let Some(parent) = tree.nodes[child].parent {
        let p = &tree.nodes[parent];
        let on_front = matches!(p.front, BspChild::Node(n) if n == child);
        planes.push(if on_front {
            (p.plane_normal, p.plane_distance)
        } else {
            (-p.plane_normal, -p.plane_distance)
        });
        child = parent;
    }
    planes.reverse();

    let mut node_is_tier = tier.node_is_tier.clone();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        node_is_tier[n] = false;
        for c in [&tree.nodes[n].front, &tree.nodes[n].back] {
            if let BspChild::Node(c) = c
                && tier.node_is_tier[*c]
            {
                stack.push(*c);
            }
        }
    }
    let mut bounds = crate::partition::Aabb::empty();
    let (mut start, mut end) = (usize::MAX, 0);
    for &t in &merged {
        bounds.expand_aabb(&tier.leaves[t].bounds);
        start = start.min(tier.leaves[t].leaf_range.start);
        end = end.max(tier.leaves[t].leaf_range.end);
    }
    let range = start..end;
    let first = merged[0];
    let mut leaves = Vec::new();
    let mut renumber = vec![0; tier.leaves.len()];
    for (index, leaf) in tier.leaves.iter().enumerate() {
        if merged.contains(&index) && index != first {
            continue;
        }
        renumber[index] = leaves.len();
        leaves.push(if index == first {
            TierLeaf {
                subtree: BspChild::Node(node),
                leaf_range: range.clone(),
                defining_planes: planes.clone(),
                bounds: bounds.clone(),
            }
        } else {
            leaf.clone()
        });
    }
    for &t in &merged {
        renumber[t] = renumber[first];
    }
    OccluderTier {
        node_is_tier,
        leaves,
        leaf_tier_leaf: tier.leaf_tier_leaf.iter().map(|&t| renumber[t]).collect(),
    }
}

/// The cache keys of every whole-section stage that reads cells or portals,
/// the geometry hash the SH, direct-SH, scatter, navmesh and chunk keys fold
/// (face cell ids and cell-major order), and the SDF key, which reads only
/// full-tree solidity.
#[derive(Debug, PartialEq, Eq)]
struct StageKeys {
    cell_visibility: String,
    cell_residency_set: String,
    chunk_light_list: String,
    geometry: [u8; 32],
    sdf: String,
}

fn stage_keys(cells: &Cells, view: &CellTree, runtime_portals: &[portals::Portal]) -> StageKeys {
    let geometry = crate::geometry::extract_geometry(
        &cells.partition.result.faces,
        &view.tree,
        &view.exterior_cells,
    );
    let (bvh, primitives, _) = crate::bvh_build::build_bvh(&geometry).expect("BVH builds");
    let lights = crate::light_namespaces::AlphaLightsNs::from_lights(&cells.map_data.lights);
    let chunk_inputs = crate::chunk_light_list_bake::ChunkLightListInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        lights: &lights,
        tree: &view.tree,
        portals: runtime_portals,
        exterior_leaves: &view.exterior_cells,
    };
    let vis = crate::visibility::encode_vis(&view.tree, &view.exterior_cells);
    let portals_section = crate::pack::encode_portals(runtime_portals).expect("portals encode");
    let cells_section =
        crate::pack::encode_cells(&vis.leaves_section, &portals_section, &view.exterior_cells)
            .expect("cells encode");
    let locator = crate::pack::encode_cell_locator(&view.tree).expect("locator encodes");
    StageKeys {
        cell_visibility: crate::cell_visibility_bake::cell_visibility_cache_key(
            &view.tree,
            runtime_portals,
            crate::cell_visibility_bake::CELL_VISIBILITY_STAGE_VERSION,
        )
        .as_filename(),
        cell_residency_set: crate::cell_residency_bake::cell_residency_set_cache_key(
            &cells_section,
            &portals_section,
            &locator,
            &cells.map_data.teleporters,
            Some(&view.tree),
        )
        .as_filename(),
        chunk_light_list: crate::chunk_light_list_bake::chunk_light_list_cache_key(
            &chunk_inputs,
            crate::chunk_light_list_bake::DEFAULT_CELL_SIZE_METERS,
            crate::chunk_light_list_bake::DEFAULT_PER_CHUNK_LIGHT_CAP,
        )
        .as_filename(),
        geometry: crate::sh_group::geometry_content_hash(&geometry),
        sdf: crate::sdf_bake::cache_key(
            &geometry,
            cells.full_tree(),
            &crate::sdf_bake::SdfConfig::default(),
        )
        .as_filename(),
    }
}

fn keys_of(cells: &Cells) -> StageKeys {
    stage_keys(cells, &cells.runtime.cells, &cells.runtime.portals)
}

#[test]
fn cell_and_portal_stage_keys_miss_on_regrouped_cells_or_classification_only() {
    // Pin R16.
    let map = occlusion_hall();
    let first = Cells::of(&map);
    let unchanged = keys_of(&first);
    assert_eq!(
        unchanged,
        keys_of(&Cells::of(&map)),
        "an unchanged rebuild hits"
    );

    // Same BSP leaves, cells grouped differently.
    let coarse = merge_one_tier_subtree(&first);
    let regrouped = RuntimeCells::build(first.full_tree(), &coarse, &first.full_exterior, 0);
    assert!(regrouped.cells.tree.leaves.len() < first.cell_tree().leaves.len());
    let keys = stage_keys(&first, &regrouped.cells, &regrouped.portals);
    assert_ne!(keys.cell_visibility, unchanged.cell_visibility);
    assert_ne!(keys.cell_residency_set, unchanged.cell_residency_set);
    assert_ne!(keys.chunk_light_list, unchanged.chunk_light_list);
    assert_ne!(keys.geometry, unchanged.geometry);
    assert_eq!(
        keys.sdf, unchanged.sdf,
        "the solidity-only SDF keeps hitting"
    );

    // Every brush's geometry fixed, the retaining wall exempted by a raised
    // see-over height.
    let overridden = see_over_override::with(7.0, || {
        let verdicts = classify(&first.map_data.brush_volumes)
            .expect("classifies")
            .verdicts;
        assert_ne!(verdicts[RETAINING_WALL], OccluderVerdict::Occludes);
        Cells::of(&map)
    });
    let keys = keys_of(&overridden);
    assert_ne!(keys.cell_visibility, unchanged.cell_visibility);
    assert_ne!(keys.cell_residency_set, unchanged.cell_residency_set);
    assert_ne!(keys.chunk_light_list, unchanged.chunk_light_list);
    assert_ne!(keys.geometry, unchanged.geometry);
}
