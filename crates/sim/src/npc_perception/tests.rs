// Perception step tests on synthetic id-51 levels: a row of equal cells along
// x, located by x alone, with hand-built residency sets.
// See: context/lib/entity_model.md §7c

use super::test_support::{Row, centre, residency, spawn_at, three_arenas};
use super::*;
use postretro_entities::components::ghost_link::GhostLinkComponent;
use postretro_foundation::GhostColorFilter;

const SCALE: u32 = CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;

/// A corridor of `count` cells `width` metres long. Each camera cell sees
/// `visible` cells either side at lead 0; every other cell is mandatory at the
/// portal-path distance to its near edge. Baked out to 32 m.
fn corridor(count: usize, width: f32, visible: usize) -> Row {
    Row {
        width,
        residency: Some(residency(count, 32.0, |camera| {
            (0..count)
                .filter_map(|cell| {
                    let hops = camera.abs_diff(cell);
                    let lead = if hops <= visible {
                        0.0
                    } else {
                        (hops - 1) as f32 * width
                    };
                    (lead <= 32.0).then_some((cell, lead))
                })
                .collect()
        })),
    }
}

fn perception(row: &Row, lead_metres: f32) -> NpcPerception {
    let count = row
        .residency
        .as_ref()
        .map_or(1, CellResidencySetSection::camera_cell_count);
    NpcPerception::new(
        count,
        resolve_ai_lead(
            Some(lead_metres),
            row.residency.as_ref().map(|set| set.max_lead),
        ),
        GhostLinkCells::default(),
    )
}

/// Which of `cells` the step perceives.
fn perceived(state: &NpcPerception, cells: impl IntoIterator<Item = usize>) -> Vec<usize> {
    cells
        .into_iter()
        .filter(|&cell| state.cell_perceived(cell))
        .collect()
}

#[test]
fn ai_lead_is_the_authored_value_or_16_m_capped_at_the_baked_maximum() {
    assert_eq!(resolve_ai_lead(None, Some(32 * SCALE)), 16 * SCALE);
    assert_eq!(resolve_ai_lead(None, None), 16 * SCALE);
    assert_eq!(resolve_ai_lead(Some(24.0), Some(32 * SCALE)), 24 * SCALE);
    assert_eq!(resolve_ai_lead(Some(0.0), Some(32 * SCALE)), 0);
    // An authored lead past the bake is capped (and warned); the default is
    // capped silently.
    assert_eq!(resolve_ai_lead(Some(40.0), Some(32 * SCALE)), 32 * SCALE);
    assert_eq!(resolve_ai_lead(None, Some(8 * SCALE)), 8 * SCALE);
}

#[test]
fn ai_lead_bounds_the_union_and_zero_keeps_the_visible_set_alone() {
    // 4 m cells; the camera cell sees one cell either side.
    let row = corridor(20, 4.0, 1);
    let mut registry = EntityRegistry::new();
    let pawn = spawn_at(&mut registry, centre(&row, 10));

    let mut state = perception(&row, 8.0);
    state.step(&row, &registry, [pawn]);
    // Lead 8 m reaches cells whose near edge is within 8 m: two hops past the
    // visible set each way.
    assert_eq!(perceived(&state, 0..20), (7..=13).collect::<Vec<_>>());

    let mut state = perception(&row, 0.0);
    state.step(&row, &registry, [pawn]);
    assert_eq!(perceived(&state, 0..20), vec![9, 10, 11]);
}

#[test]
fn ai_lead_never_reads_streamings_lead() {
    // The AI lead is its own constant and its own resolved value: the step
    // takes no streaming input, so a streaming retune moves no NPC.
    assert_eq!(DEFAULT_AI_LEAD_METRES, 16.0);
    let row = corridor(20, 4.0, 1);
    let state = perception(&row, DEFAULT_AI_LEAD_METRES);
    assert_eq!(state.lead(), 16 * SCALE);
}

#[test]
fn two_players_two_arenas_third_dormant() {
    let row = three_arenas();
    let mut registry = EntityRegistry::new();
    let first = spawn_at(&mut registry, centre(&row, 3));
    let second = spawn_at(&mut registry, centre(&row, 15));
    let npcs: Vec<EntityId> = [5, 18, 25]
        .into_iter()
        .map(|cell| spawn_at(&mut registry, centre(&row, cell)))
        .collect();

    let mut state = perception(&row, 16.0);
    state.step(&row, &registry, [first, second]);
    let seen: Vec<bool> = npcs
        .iter()
        .map(|&npc| {
            let position = registry.get_component::<Transform>(npc).unwrap().position;
            state.npc_perceived(&row, npc, position)
        })
        .collect();
    assert_eq!(seen, vec![true, true, false]);
    assert_eq!(state.perceives_all(), None);
}

#[test]
fn a_teleported_pawn_is_located_where_it_stands_on_that_step() {
    let row = three_arenas();
    let mut registry = EntityRegistry::new();
    let pawn = spawn_at(&mut registry, centre(&row, 3));
    let mut state = perception(&row, 16.0);
    state.step(&row, &registry, [pawn]);
    assert!(!state.cell_perceived(25));

    // The step runs after movement and teleports: the arrival arena is
    // perceived on the same tick.
    registry
        .set_component(
            pawn,
            Transform {
                position: centre(&row, 24),
                ..Transform::default()
            },
        )
        .unwrap();
    state.step(&row, &registry, [pawn]);
    assert!(state.cell_perceived(25));
    assert!(!state.cell_perceived(5));
}

/// The fastest authored travel: reference-enemy's release runs at activity
/// speed 2 over `moveSpeed` 3.
const FASTEST_AUTHORED_TRAVEL_MPS: f32 = 6.0;
/// One dormant step: 2 Hz.
const DORMANT_STEP_SECONDS: f32 = 0.5;

#[test]
fn fastest_dormant_walker_wakes_inside_lead() {
    // 1 m cells; the player's cell sees 3 cells either side.
    let row = corridor(80, 1.0, 3);
    let mut registry = EntityRegistry::new();
    let pawn = spawn_at(&mut registry, centre(&row, 10));
    let walker = spawn_at(&mut registry, centre(&row, 70));
    let mut state = perception(&row, DEFAULT_AI_LEAD_METRES);
    let step = FASTEST_AUTHORED_TRAVEL_MPS * DORMANT_STEP_SECONDS;

    let mut x = centre(&row, 70).x;
    loop {
        state.step(&row, &registry, [pawn]);
        if state.npc_perceived(&row, walker, Vec3::new(x, 0.0, 0.0)) {
            break;
        }
        x -= step;
        assert!(x > 0.0, "the walker reached the player unperceived");
    }
    let cell = row.locate_cell(Vec3::new(x, 0.0, 0.0));
    let visible_edge = 10 + 3;
    // Perceived while still in the lead ring, at least a whole dormant step
    // short of the player's visible set.
    assert!(
        cell as f32 - visible_edge as f32 >= step,
        "perceived in cell {cell}, within one step ({step} m) of the visible set"
    );
}

#[test]
fn fastest_dormant_walker_entering_a_far_visible_cell_through_a_thin_cell_wakes_next_tick() {
    // Cell 40 is a far vista the player's cell sees at lead 0; cell 39 is a
    // thin cell the bake never reaches.
    let row = Row {
        width: 1.0,
        residency: Some(residency(60, 32.0, |camera| {
            let mut set = vec![(camera, 0.0)];
            if camera == 0 {
                set.push((40, 0.0));
            }
            set
        })),
    };
    for lead in [DEFAULT_AI_LEAD_METRES, 0.0] {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_at(&mut registry, centre(&row, 0));
        let walker = EntityId::from_raw(9_999);
        let mut state = perception(&row, lead);

        state.step(&row, &registry, [pawn]);
        assert!(!state.npc_perceived(&row, walker, centre(&row, 39)));
        // The dormant step lands it inside the vista; the next tick's lookup
        // re-locates it there.
        state.step(&row, &registry, [pawn]);
        assert!(state.npc_perceived(&row, walker, centre(&row, 40)));
    }
}

#[test]
fn moved_npcs_are_relocated_and_a_reused_slot_reads_its_own_cell() {
    let row = three_arenas();
    let mut registry = EntityRegistry::new();
    let pawn = spawn_at(&mut registry, centre(&row, 3));
    let npc = spawn_at(&mut registry, centre(&row, 25));
    let mut state = perception(&row, 16.0);

    state.step(&row, &registry, [pawn]);
    assert!(!state.npc_perceived(&row, npc, centre(&row, 25)));
    // A script, mover or knockback move: the new position re-locates.
    state.step(&row, &registry, [pawn]);
    assert!(state.npc_perceived(&row, npc, centre(&row, 6)));

    // A despawned NPC's slot reused: a new id, a fresh lookup, no inherited
    // cell.
    registry.despawn(npc).unwrap();
    let reused = spawn_at(&mut registry, centre(&row, 25));
    assert_ne!(reused, npc);
    state.step(&row, &registry, [pawn]);
    assert!(!state.npc_perceived(&row, reused, centre(&row, 25)));
    // The departed entry is pruned once a step passes without a lookup.
    state.step(&row, &registry, [pawn]);
    state.step(&row, &registry, [pawn]);
    assert_eq!(state.tracked_npcs(), 0);
}

#[test]
fn ghost_link_couples_its_cells_only_while_armed() {
    let row = three_arenas();
    let mut ghost = GhostLinkCells::default();
    // Link 0: a window in arena 0 (cells 8, 9) onto arena 2 (cells 20, 21).
    ghost.push_cells(0, &[8, 9], &[20, 21]);
    let mut registry = EntityRegistry::new();
    let pawn = spawn_at(&mut registry, centre(&row, 3));
    let link = spawn_at(&mut registry, Vec3::ZERO);
    let mut component = GhostLinkComponent {
        index: 0,
        source_entity: 0,
        armed: true,
        members: 0,
        filter: GhostColorFilter::default(),
    };
    registry.set_component(link, component).unwrap();
    let mut state = NpcPerception::new(30, 16 * SCALE, ghost);

    state.step(&row, &registry, [pawn]);
    assert_eq!(perceived(&state, 20..30), vec![20, 21]);
    assert!(state.cell_perceived(9));

    // The far side's player perceives the near side through the link too.
    let far = spawn_at(&mut registry, centre(&row, 25));
    state.step(&row, &registry, [far]);
    assert_eq!(perceived(&state, 0..10), vec![8, 9]);

    component.armed = false;
    registry.set_component(link, component).unwrap();
    state.step(&row, &registry, [pawn]);
    assert_eq!(perceived(&state, 20..30), Vec::<usize>::new());
}

#[test]
fn ghost_link_cells_come_from_the_volume_and_its_mapped_image() {
    let mut ghost = GhostLinkCells::default();
    let cells = |i: usize| {
        Some((
            Vec3::new(i as f32 * 2.0, 0.0, 0.0),
            Vec3::new(i as f32 * 2.0 + 2.0, 2.0, 2.0),
        ))
    };
    // A volume over x 3..5 overlaps cells 1 and 2; its image 40 m on, cells
    // 21 and 22. Solid cell 22 is skipped.
    let bounds = (0..30).map(move |i| if i == 22 { None } else { cells(i) });
    ghost.push(
        0,
        &[(Vec3::new(3.0, 0.5, 0.5), Vec3::new(5.0, 1.5, 1.5))],
        &[(Vec3::new(43.0, 0.5, 0.5), Vec3::new(45.0, 1.5, 1.5))],
        bounds,
    );
    assert_eq!(ghost.cells, vec![1, 2, 21]);
    assert_eq!(ghost.pairs[0].volume, 0..2);
    assert_eq!(ghost.pairs[0].image, 2..3);
}

#[test]
fn perceivers_are_every_player_pawn_whatever_its_life_state() {
    let row = three_arenas();
    let mut registry = EntityRegistry::new();
    // A dead or down pawn is still a pawn in the registry: it perceives.
    let pawn = spawn_at(&mut registry, centre(&row, 13));
    registry
        .set_component(
            pawn,
            postretro_entities::components::health::HealthComponent {
                max: 100.0,
                current: 0.0,
                hitbox: None,
                death_handled: true,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .unwrap();
    let mut state = perception(&row, 16.0);
    state.step(&row, &registry, [pawn]);
    assert!(state.cell_perceived(15));
    assert!(!state.cell_perceived(5));
}

#[test]
fn perceivers_absent_run_every_npc_perceived() {
    let row = three_arenas();
    let mut registry = EntityRegistry::new();
    let npc = spawn_at(&mut registry, centre(&row, 25));
    let mut state = perception(&row, 16.0);
    // No pawn: before first spawn, every seat loading or reconnecting.
    state.step(&row, &registry, []);
    assert_eq!(state.perceives_all(), Some(PerceiveAll::NoPerceiver));
    assert!(state.npc_perceived(&row, npc, centre(&row, 25)));

    // A pawn id whose entity is gone counts as no pawn.
    let gone = spawn_at(&mut registry, centre(&row, 3));
    registry.despawn(gone).unwrap();
    state.step(&row, &registry, [gone]);
    assert_eq!(state.perceives_all(), Some(PerceiveAll::NoPerceiver));
}

#[test]
fn perceivers_outside_every_camera_cell_run_every_npc_perceived() {
    // Cells 5 and 6 are solid: no camera set.
    let row = Row {
        width: 1.0,
        residency: Some(residency(10, 32.0, |camera| {
            if camera == 5 || camera == 6 {
                Vec::new()
            } else {
                vec![(camera, 0.0)]
            }
        })),
    };
    let mut registry = EntityRegistry::new();
    let outside = spawn_at(&mut registry, centre(&row, 5));
    let also_outside = spawn_at(&mut registry, centre(&row, 6));
    let mut state = perception(&row, 16.0);
    state.step(&row, &registry, [outside, also_outside]);
    assert_eq!(
        state.perceives_all(),
        Some(PerceiveAll::PerceiverOutsideCameraCells)
    );
    assert!(state.cell_perceived(8));
}

// Regression: the first perceiver in a cell with no camera set ran every NPC
// on the map perceived, so one co-op player dead in an exterior void woke
// every arena for their whole respawn delay.
#[test]
fn a_perceiver_outside_every_camera_cell_is_skipped_while_another_resolves() {
    // Arenas of ten cells each seeing only itself; cell 25 is solid.
    let row = Row {
        width: 2.0,
        residency: Some(residency(30, 32.0, |camera| {
            if camera == 25 {
                return Vec::new();
            }
            let arena = camera / 10;
            (arena * 10..arena * 10 + 10)
                .map(|cell| (cell, 0.0))
                .collect()
        })),
    };
    let mut registry = EntityRegistry::new();
    let in_void = spawn_at(&mut registry, centre(&row, 25));
    let in_arena = spawn_at(&mut registry, centre(&row, 3));
    // Either order: a skipped perceiver never cuts the union short.
    for perceivers in [[in_void, in_arena], [in_arena, in_void]] {
        let mut state = perception(&row, 16.0);
        state.step(&row, &registry, perceivers);
        assert_eq!(state.perceives_all(), None);
        assert_eq!(perceived(&state, 0..30), (0..10).collect::<Vec<_>>());
    }
}

#[test]
fn perceivers_on_a_level_without_id_51_run_every_npc_perceived() {
    let mut registry = EntityRegistry::new();
    let pawn = spawn_at(&mut registry, Vec3::ZERO);
    for row in [
        Row {
            width: 1.0,
            residency: None,
        },
        // A section with no camera sets: every cell solid or exterior.
        Row {
            width: 1.0,
            residency: Some(residency(4, 32.0, |_| Vec::new())),
        },
    ] {
        let mut state = perception(&row, 16.0);
        state.step(&row, &registry, [pawn]);
        assert_eq!(state.perceives_all(), Some(PerceiveAll::NoResidencySet));
        assert!(state.npc_perceived(&row, EntityId::from_raw(7), Vec3::new(3.0, 0.0, 0.0)));
    }
}

#[test]
fn union_allocates_nothing() {
    let row = three_arenas();
    let mut registry = EntityRegistry::new();
    let pawns: Vec<EntityId> = [3, 15]
        .into_iter()
        .map(|cell| spawn_at(&mut registry, centre(&row, cell)))
        .collect();
    let npcs: Vec<(EntityId, Vec3)> = (0..30)
        .map(|cell| {
            let position = centre(&row, cell);
            (spawn_at(&mut registry, position), position)
        })
        .collect();
    let link = spawn_at(&mut registry, Vec3::ZERO);
    registry
        .set_component(
            link,
            GhostLinkComponent {
                index: 0,
                source_entity: 0,
                armed: true,
                members: 0,
                filter: GhostColorFilter::default(),
            },
        )
        .unwrap();
    let mut ghost = GhostLinkCells::default();
    ghost.push_cells(0, &[8, 9], &[20, 21]);
    let mut state = NpcPerception::new(30, 16 * SCALE, ghost);
    // Warm: the table has seen every NPC.
    state.step(&row, &registry, pawns.iter().copied());
    for &(npc, position) in &npcs {
        state.npc_perceived(&row, npc, position);
    }

    let probe = crate::alloc_probe::AllocSnapshot::arm();
    for tick in 0..8 {
        state.step(&row, &registry, pawns.iter().copied());
        for &(npc, position) in &npcs {
            // Half the NPCs move each tick and re-locate.
            let offset = if tick % 2 == 0 { 0.1 } else { 0.0 };
            state.npc_perceived(&row, npc, position + Vec3::new(offset, 0.0, 0.0));
        }
    }
    assert_eq!(probe.allocs_since(), 0);
}
