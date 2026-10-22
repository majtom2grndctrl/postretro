// Ghost membership through installed links and a real registry: volumes, roles, clocks,
// link-state edges, re-tenanting, despawn and teleport-out fades.
// See: context/lib/entity_model.md §7d · context/lib/rendering_pipeline.md §7.9

use std::collections::BTreeSet;

use glam::Vec3;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{GhostLinkComponent, Transform};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
use postretro_level_loader::CellLocatorChild;

use super::clocks::ClockSource;
use super::*;

const FRAME: f32 = 1.0 / 240.0;
/// Link 0 covers x in [0, 10]; link 1 overlaps it over x in [5, 15].
const LINK_0: (Vec3, Vec3) = (Vec3::new(0.0, 0.0, -5.0), Vec3::new(10.0, 4.0, 5.0));
const LINK_1: (Vec3, Vec3) = (Vec3::new(5.0, 0.0, -5.0), Vec3::new(15.0, 4.0, 5.0));

fn box_brush((min, max): (Vec3, Vec3)) -> EnvironmentBrush {
    let planes = (0..3)
        .flat_map(|axis| {
            let mut normal = [0.0; 3];
            normal[axis] = 1.0;
            let mut negative = normal;
            negative[axis] = -1.0;
            [
                EnvironmentPlane {
                    normal,
                    distance: max[axis],
                },
                EnvironmentPlane {
                    normal: negative,
                    distance: -min[axis],
                },
            ]
        })
        .collect();
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

fn link_record(
    source_entity: u32,
    volume: (Vec3, Vec3),
    enabled_on_spawn: bool,
    target: Vec3,
    target_yaw: f32,
) -> GhostLinkRecord {
    GhostLinkRecord {
        source_entity,
        enabled_on_spawn,
        members: GhostMembers::all(),
        style: String::new(),
        brushes: vec![box_brush(volume)],
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        target_origin: target.to_array(),
        target_yaw,
        ghost_tags: Vec::new(),
    }
}

fn world(links: Vec<GhostLinkRecord>) -> LevelWorld {
    let mut world = LevelWorld::new_visibility_only(
        vec![postretro_level_loader::CellData {
            bounds_min: Vec3::splat(-500.0),
            bounds_max: Vec3::splat(500.0),
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: false,
        }],
        Vec::new(),
        CellLocatorChild::Cell(0),
        Vec::new(),
        Vec::new(),
        false,
    )
    .unwrap();
    world.ghost_links.links = links;
    world
}

/// A level with link 0 (disarmed unless `armed`) and link 1 (armed), installed
/// through the level-install spawn and binding.
struct Level {
    registry: EntityRegistry,
    ghosts: GhostLinks,
    links: Vec<EntityId>,
}

impl Level {
    fn new(link_0_armed: bool) -> Self {
        let world = world(vec![
            link_record(4, LINK_0, link_0_armed, Vec3::new(100.0, 0.0, 0.0), 1.0),
            link_record(5, LINK_1, true, Vec3::new(0.0, 0.0, 100.0), 0.0),
        ]);
        let mut registry = EntityRegistry::new();
        spawn_loaded_ghost_links(&mut registry, &world);
        let mut ghosts = GhostLinks::install(&world, &registry);
        // The player descriptor opts in; the engine never ghosts a pawn by kind.
        ghosts.refresh_opt_ins(&super::opt_ins_test_fixtures::player_opted_in());
        let mut links: Vec<_> = registry
            .iter_with_kind(ComponentKind::GhostLink)
            .map(|(id, value)| match value {
                ComponentValue::GhostLink(link) => (link.index, id),
                _ => unreachable!("ghost-link column"),
            })
            .collect();
        links.sort();
        let mut level = Self {
            registry,
            ghosts,
            links: links.into_iter().map(|(_, id)| id).collect(),
        };
        // The viewer, outside every volume: the camera follows it and it never
        // ghosts for itself.
        let viewer = level.pawn(Vec3::new(-50.0, 1.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
        level.registry.mark_local_player_pawn(viewer).unwrap();
        level
    }

    fn pawn(&mut self, at: Vec3, spawn_path: DescriptorSpawnPath) -> EntityId {
        let id = self.registry.spawn(Transform::default());
        self.place(id, at);
        self.registry
            .set_component(
                id,
                postretro_foundation::PlayerMovementComponent::from_descriptor(
                    &crate::mod_digest::tests::movement_descriptor(),
                ),
            )
            .unwrap();
        self.registry
            .set_component(
                id,
                DescriptorProvenance {
                    canonical_name: "player".into(),
                    owned_components: BTreeSet::new(),
                    map_overrides: BTreeSet::new(),
                    spawn_path,
                },
            )
            .unwrap();
        id
    }

    /// Present `id` at `at` at any alpha, as a teleport cut or a presentation write does.
    fn place(&mut self, id: EntityId, at: Vec3) {
        self.registry
            .set_presentation_transform(
                id,
                Transform {
                    position: at,
                    ..Default::default()
                },
            )
            .unwrap();
    }

    /// Apply `armed` to link `index`'s component, as a host-tick primitive does.
    fn set_armed(&mut self, index: usize, armed: bool) {
        let id = self.links[index];
        let mut link = *self
            .registry
            .get_component::<GhostLinkComponent>(id)
            .unwrap();
        link.armed = armed;
        self.registry.set_component(id, link).unwrap();
    }

    /// One completed single-player fixed tick.
    fn complete_tick(&mut self) {
        self.ghosts.record_after_fixed_tick(&self.registry, None);
    }

    fn frame(&mut self, source: ClockSource, alpha: f32, elapsed_seconds: f32) {
        self.ghosts.update(
            &self.registry,
            GhostFrame {
                clocks: PresentationClocks::compute(source, alpha),
                alpha,
                elapsed_seconds,
                view: CandidateView::Authoritative,
                stamps: super::clocks::PoseStamps::None,
            },
        );
    }

    fn single_player_frame(&mut self, alpha: f32) {
        let completed_ticks = self.ghosts.completed_ticks();
        self.frame(ClockSource::SinglePlayer { completed_ticks }, alpha, FRAME);
    }

    fn ghost(&self, link: u16, entity: EntityId) -> Option<GhostMember> {
        self.ghosts
            .members()
            .iter()
            .find(|member| member.link == link && member.entity == entity)
            .copied()
    }
}

fn inside_link_0_only() -> Vec3 {
    Vec3::new(2.0, 1.0, 0.0)
}

#[test]
fn install_binds_each_link_with_its_spawn_state_and_mapping() {
    let level = Level::new(false);
    assert_eq!(level.links.len(), 2);
    assert!(!level.ghosts.links[0].history.latest().armed);
    assert!(level.ghosts.links[1].history.latest().armed);
    assert_eq!(level.ghosts.links[1].entity, Some(level.links[1]));
    let mapped = level
        .ghosts
        .mapping(0)
        .unwrap()
        .point(Vec3::new(1.0, 0.0, 0.0));
    assert!(
        (mapped
            - SeamlessMapping::new(Vec3::ZERO, 0.0, Vec3::new(100.0, 0.0, 0.0), 1.0)
                .point(Vec3::new(1.0, 0.0, 0.0)))
        .length()
            < 1e-5
    );
}

#[test]
fn partner_pawn_ghosts_inside_the_volume_and_not_just_outside_it() {
    let mut level = Level::new(true);
    let viewer = level.pawn(Vec3::new(3.0, 1.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    level.registry.mark_local_player_pawn(viewer).unwrap();
    let partner = level.pawn(Vec3::new(0.01, 1.0, 0.0), DescriptorSpawnPath::NetworkSlot);
    level.frame(ClockSource::Host { tick: 50 }, 0.5, FRAME);
    assert!(
        level.ghost(0, partner).is_some(),
        "a body point just inside link 0 ghosts"
    );
    assert!(
        level
            .ghosts
            .members()
            .iter()
            .all(|member| member.entity != viewer),
        "the viewer's own pawn never ghosts on its own machine"
    );

    level.place(partner, Vec3::new(-0.01, 1.0, 0.0));
    level.frame(ClockSource::Host { tick: 50 }, 0.5, FRAME);
    let fading = level.ghost(0, partner).expect("an exit fades out");
    assert!(fading.frozen, "just outside is not a target");
    assert!(
        (fading.source.position - Vec3::new(0.01, 1.0, 0.0)).length() < 1e-6,
        "the fade holds the last inside pose"
    );
}

#[test]
fn pawn_inside_two_armed_links_draws_two_ghosts_at_two_transforms() {
    let mut level = Level::new(true);
    let partner = level.pawn(Vec3::new(7.0, 1.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    level.single_player_frame(0.0);
    let through_0 = level.ghost(0, partner).expect("ghost through link 0");
    let through_1 = level.ghost(1, partner).expect("ghost through link 1");
    assert_eq!(level.ghosts.members().len(), 2);
    assert!((through_0.mapped.position - through_1.mapped.position).length() > 1.0);
    assert!(
        (through_0.mapped.position - through_0.mapping.point(Vec3::new(7.0, 1.0, 0.0))).length()
            < 1e-5
    );
}

#[test]
fn link_armed_on_spawn_presents_full_membership_on_the_first_frame_after_install() {
    let mut level = Level::new(false);
    let partner = level.pawn(Vec3::new(12.0, 1.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    level.single_player_frame(0.0);
    assert_eq!(level.ghost(1, partner).unwrap().membership, 1.0, "P26");
    assert!(level.ghost(0, partner).is_none());
}

#[test]
fn arm_on_tick_n_and_disarm_on_tick_n_plus_1_rise_then_fall_at_the_rate_cap() {
    let mut level = Level::new(false);
    let partner = level.pawn(inside_link_0_only(), DescriptorSpawnPath::PlayerSpawn);
    let step = membership::ramp_step(FRAME);
    let alphas = [0.0, 0.25, 0.5, 0.75];
    let mut trace = Vec::new();
    let mut run_tick = |level: &mut Level, armed: Option<bool>| {
        if let Some(armed) = armed {
            level.set_armed(0, armed);
        }
        level.complete_tick();
        for alpha in alphas {
            level.single_player_frame(alpha);
            trace.push(level.ghost(0, partner).map(|ghost| ghost.membership));
        }
    };
    run_tick(&mut level, None);
    run_tick(&mut level, Some(true));
    run_tick(&mut level, Some(false));
    run_tick(&mut level, None);
    let expected = [
        // Before the arm.
        None,
        None,
        None,
        None,
        // Armed during this tick: presents once alpha leaves its start.
        None,
        Some(0.0),
        Some(step),
        Some(2.0 * step),
        // Disarmed during this tick: one more armed frame, then the fall.
        Some(3.0 * step),
        Some(2.0 * step),
        Some(step),
        None,
        None,
        None,
        None,
        None,
    ];
    assert_eq!(trace.len(), expected.len());
    for (frame, (actual, expected)) in trace.iter().zip(expected).enumerate() {
        match (actual, expected) {
            (None, None) => {}
            (Some(actual), Some(expected)) => assert!(
                (actual - expected).abs() < 1e-6,
                "frame {frame}: {actual} != {expected}"
            ),
            _ => panic!("frame {frame}: {actual:?} != {expected:?}"),
        }
    }
}

#[test]
fn arm_and_disarm_in_one_tick_start_no_ramp_and_present_no_ghost() {
    let mut level = Level::new(false);
    let partner = level.pawn(inside_link_0_only(), DescriptorSpawnPath::PlayerSpawn);
    level.set_armed(0, true);
    level.set_armed(0, false);
    level.complete_tick();
    for _ in 0..3 {
        for alpha in [0.0, 0.5, 0.99] {
            level.single_player_frame(alpha);
            assert!(level.ghost(0, partner).is_none(), "P2");
        }
        level.complete_tick();
    }
}

#[test]
fn rearm_during_a_ramp_out_ramps_up_from_the_presented_value() {
    let mut level = Level::new(false);
    let partner = level.pawn(inside_link_0_only(), DescriptorSpawnPath::PlayerSpawn);
    let step = membership::ramp_step(FRAME);
    level.single_player_frame(0.5);
    level.set_armed(0, true);
    level.complete_tick();
    for _ in 0..30 {
        level.single_player_frame(0.5);
    }
    let peak = level.ghost(0, partner).unwrap().membership;
    level.set_armed(0, false);
    level.complete_tick();
    for _ in 0..10 {
        level.single_player_frame(0.5);
    }
    let faded = level.ghost(0, partner).unwrap().membership;
    assert!((faded - (peak - 10.0 * step)).abs() < 1e-5);
    level.set_armed(0, true);
    level.complete_tick();
    level.single_player_frame(0.5);
    let resumed = level.ghost(0, partner).unwrap();
    assert!(!resumed.frozen);
    assert!((resumed.membership - (faded + step)).abs() < 1e-5, "P4");
}

#[test]
fn host_seat_reads_link_state_at_each_members_own_presentation_clock() {
    let mut level = Level::new(false);
    let host_pawn = level.pawn(inside_link_0_only(), DescriptorSpawnPath::PlayerSpawn);
    let slot_pawn = level.pawn(Vec3::new(3.0, 1.0, 0.0), DescriptorSpawnPath::NetworkSlot);
    // Armed during the tick labelled 100: stamped 101, the post-increment tick.
    level.set_armed(0, true);
    level
        .ghosts
        .record_after_fixed_tick(&level.registry, Some(101));
    level.frame(ClockSource::Host { tick: 101 }, 0.5, FRAME);
    assert!(
        level.ghost(0, host_pawn).is_some(),
        "the simulated pawn presents the end of tick 100"
    );
    assert!(
        level.ghost(0, slot_pawn).is_none(),
        "the delayed client pawn still presents tick 98's end"
    );
    for tick in 102..=103 {
        level
            .ghosts
            .record_after_fixed_tick(&level.registry, Some(tick));
        level.frame(ClockSource::Host { tick }, 0.5, FRAME);
    }
    assert!(
        level.ghost(0, slot_pawn).is_some(),
        "the arm reaches the client pawn with the pose it describes"
    );
}

#[test]
fn ramp_continues_through_a_pause_and_a_hitch_steps_at_most_the_ceiling() {
    let mut level = Level::new(false);
    let partner = level.pawn(inside_link_0_only(), DescriptorSpawnPath::PlayerSpawn);
    // Past the install frame, so the arm ramps from 0.
    level.single_player_frame(0.5);
    level.set_armed(0, true);
    level.complete_tick();
    level.single_player_frame(0.5);
    // Paused: no fixed tick runs, frames keep presenting.
    for frame in 1..=5 {
        level.single_player_frame(0.5);
        let membership = level.ghost(0, partner).unwrap().membership;
        assert!(
            (membership - frame as f32 * membership::ramp_step(FRAME)).abs() < 1e-6,
            "P42: the ramp runs on presented-frame time"
        );
    }
    let before = level.ghost(0, partner).unwrap().membership;
    let completed_ticks = level.ghosts.completed_ticks();
    level.frame(ClockSource::SinglePlayer { completed_ticks }, 0.5, 2.0);
    let after = level.ghost(0, partner).unwrap().membership;
    assert!(
        (after - before - 4.0 / 30.0).abs() < 1e-6,
        "P30: a 2 s hitch steps the 1/30 s ceiling"
    );
}

#[test]
fn retenanted_slot_starts_a_new_ghost_at_zero_while_the_old_one_fades() {
    let mut level = Level::new(true);
    let first = level.pawn(inside_link_0_only(), DescriptorSpawnPath::PlayerSpawn);
    for _ in 0..40 {
        level.single_player_frame(0.5);
    }
    assert!(level.ghost(0, first).unwrap().membership > 0.5);
    level.registry.despawn(first).unwrap();
    let second = level.pawn(inside_link_0_only(), DescriptorSpawnPath::PlayerSpawn);
    assert_eq!(
        second.to_raw() & 0xFFFF,
        first.to_raw() & 0xFFFF,
        "the new tenant reuses the freed slot"
    );
    assert_ne!(second, first, "under a new generation");
    level.single_player_frame(0.5);
    assert_eq!(level.ghost(0, second).unwrap().membership, 0.0, "P19");
    let old = level
        .ghost(0, first)
        .expect("P20: the despawned member fades");
    assert!(old.frozen);
    assert!((old.source.position - inside_link_0_only()).length() < 1e-6);
}

#[test]
fn member_teleported_out_fades_at_its_last_in_volume_mapped_pose() {
    let mut level = Level::new(true);
    let partner = level.pawn(inside_link_0_only(), DescriptorSpawnPath::PlayerSpawn);
    for _ in 0..40 {
        level.single_player_frame(0.5);
    }
    let last_inside = level.ghost(0, partner).unwrap().mapped;
    // A seamless cut far outside every volume.
    let arrival = Vec3::new(200.0, 1.0, 0.0);
    level.place(partner, arrival);
    let mapping = level.ghosts.mapping(0).unwrap();
    for _ in 0..10 {
        level.single_player_frame(0.5);
        let ghost = level.ghost(0, partner).unwrap();
        assert!(ghost.frozen, "P1");
        assert_eq!(ghost.mapped, last_inside);
        assert!((ghost.mapped.position - mapping.point(arrival)).length() > 1.0);
    }
}

#[test]
fn client_presents_spawn_state_until_a_received_record_and_again_after_reset() {
    let mut level = Level::new(false);
    // A slot pawn presents on the remote lane, as every display copy on a client does.
    let partner = level.pawn(inside_link_0_only(), DescriptorSpawnPath::NetworkSlot);
    let client_frame = |level: &mut Level, render_server_tick| {
        level.frame(
            ClockSource::Client {
                render_server_tick: Some(render_server_tick),
                estimated_server_tick: Some(render_server_tick + 4.0),
                latest_snapshot_tick: None,
            },
            0.0,
            FRAME,
        );
    };
    client_frame(&mut level, 200.0);
    assert!(
        level.ghost(0, partner).is_none(),
        "local spawn state: disarmed"
    );
    // Replication lands the host's arm, stamped 205.
    level
        .ghosts
        .history_mut(0)
        .unwrap()
        .push(205, LinkState::new(true, GhostMembers::all()));
    client_frame(&mut level, 204.0);
    assert!(
        level.ghost(0, partner).is_none(),
        "not yet at the remotes' clock"
    );
    client_frame(&mut level, 204.5);
    assert!(
        level.ghost(0, partner).is_some(),
        "presents with stamp 205's poses"
    );
    for _ in 0..10 {
        client_frame(&mut level, 204.5);
    }
    // Demotion drops received state: back to spawn state, through the ramp.
    level.ghosts.history_mut(0).unwrap().reset_to_spawn();
    client_frame(&mut level, 205.0);
    assert!(level.ghost(0, partner).unwrap().frozen);
}
