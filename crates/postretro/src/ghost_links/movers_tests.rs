// Mover ghost membership: `ghost_tags` resolution, bounds-center containment, per-link
// listing, two links and two tags, the movers category, and the unmatched-tag warning.
// See: context/lib/entity_model.md §7d

use std::collections::BTreeSet;

use glam::{Quat, Vec3};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, GhostLinkComponent,
    KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, Transform,
};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::geometry::Vertex;
use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld, LoadedKinematicMover};
use postretro_test_log_capture::LogCapture;

use super::super::clocks::{ClockSource, PoseStamps, PresentationClocks};
use super::super::eligibility::{CandidateView, GhostCategory};
use super::super::membership::ramp_step;
use super::super::opt_ins_test_fixtures::player_opted_in;
use super::super::{GhostFrame, GhostLinks, GhostMember, spawn_loaded_ghost_links};
use super::UNMATCHED_GHOST_TAG_WARNING;

const FRAME: f32 = 1.0 / 240.0;
/// Link 0 covers x in [0, 10]; link 1 overlaps it over x in [5, 15]. They map
/// into different copies, so one mover seen through both draws at two places.
const LINK_0: (Vec3, Vec3) = (Vec3::new(0.0, -5.0, -5.0), Vec3::new(10.0, 5.0, 5.0));
const LINK_1: (Vec3, Vec3) = (Vec3::new(5.0, -5.0, -5.0), Vec3::new(15.0, 5.0, 5.0));
const LINK_0_TARGET: Vec3 = Vec3::new(100.0, 0.0, 0.0);
const LINK_1_TARGET: Vec3 = Vec3::new(0.0, 0.0, 100.0);

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

fn link(source: u32, volume: (Vec3, Vec3), target: Vec3, ghost_tags: &[&str]) -> GhostLinkRecord {
    GhostLinkRecord {
        source_entity: source,
        enabled_on_spawn: true,
        members: GhostMembers::all(),
        style: String::new(),
        brushes: vec![box_brush(volume)],
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        target_origin: target.to_array(),
        target_yaw: 0.0,
        ghost_tags: ghost_tags.iter().map(|tag| tag.to_string()).collect(),
    }
}

fn vertex(position: [f32; 3]) -> Vertex {
    Vertex::new(
        position,
        [0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        true,
        [0.0, 0.0],
        0,
    )
}

/// A loaded mover whose local bounds span `local` (origin-relative, as the
/// compiler emits them), carrying `tags`.
fn loaded_mover(mover_id: u32, tags: &[&str], local: (Vec3, Vec3)) -> LoadedKinematicMover {
    let mut mover = crate::runtime_movers::tests::mover(1);
    mover.mover_id = mover_id;
    mover.tags = tags.iter().map(|tag| tag.to_string()).collect();
    mover.vertices = vec![
        vertex(local.0.to_array()),
        vertex([local.1.x, local.0.y, local.0.z]),
        vertex(local.1.to_array()),
    ];
    mover
}

/// One mover id per loaded mover, in order: 1, 2, 3...
fn world(links: Vec<GhostLinkRecord>, movers: Vec<LoadedKinematicMover>) -> LevelWorld {
    let mut world = LevelWorld::new_visibility_only(
        vec![CellData {
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
    world.kinematic_geometry.movers = movers;
    world
}

struct Level {
    world: LevelWorld,
    registry: EntityRegistry,
    ghosts: GhostLinks,
    links: Vec<EntityId>,
}

impl Level {
    fn new(world: LevelWorld) -> Self {
        let mut registry = EntityRegistry::new();
        spawn_loaded_ghost_links(&mut registry, &world);
        let mut ghosts = GhostLinks::install(&world, &registry);
        ghosts.refresh_opt_ins(&player_opted_in());
        let mut links: Vec<_> = registry
            .iter_with_kind(ComponentKind::GhostLink)
            .map(|(id, value)| match value {
                ComponentValue::GhostLink(link) => (link.index, id),
                _ => unreachable!("ghost-link column"),
            })
            .collect();
        links.sort();
        let mut level = Self {
            world,
            registry,
            ghosts,
            links: links.into_iter().map(|(_, id)| id).collect(),
        };
        // The viewer, outside every volume: the camera follows it and it never
        // ghosts for itself.
        let viewer = level.pawn(Vec3::new(-50.0, 0.0, 0.0));
        level.registry.mark_local_player_pawn(viewer).unwrap();
        level
    }

    /// The level's mover `mover_id`, spawned as every peer spawns it from the
    /// level file, presented at `origin`.
    fn mover(&mut self, mover_id: u32, origin: Vec3) -> EntityId {
        let loaded = self
            .world
            .kinematic_geometry
            .movers
            .iter()
            .find(|mover| mover.mover_id == mover_id)
            .expect("fixture mover");
        let tags = loaded.tags.clone();
        let id = self
            .registry
            .try_spawn(Transform::default(), &tags)
            .unwrap();
        self.place(id, origin);
        let component = KinematicMoverComponent::new(
            mover_id,
            KinematicMoverConfig {
                waypoints: vec![origin, origin + Vec3::Y],
                waypoint_names: vec!["closed".into(), "open".into()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::PingPong,
                started: false,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        self.registry.set_component(id, component).unwrap();
        id
    }

    fn pawn(&mut self, at: Vec3) -> EntityId {
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
                    spawn_path: DescriptorSpawnPath::NetworkSlot,
                },
            )
            .unwrap();
        id
    }

    fn place(&mut self, id: EntityId, at: Vec3) {
        self.registry
            .set_presentation_transform(
                id,
                Transform {
                    position: at,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
            )
            .unwrap();
    }

    /// Change link `index`'s shown categories as a host-tick primitive does,
    /// then complete the tick.
    fn set_members(&mut self, index: usize, members: GhostMembers) {
        let id = self.links[index];
        let mut component = *self
            .registry
            .get_component::<GhostLinkComponent>(id)
            .unwrap();
        component.members = members.bits();
        self.registry.set_component(id, component).unwrap();
        self.ghosts.record_after_fixed_tick(&self.registry, None);
    }

    fn frame_in(&mut self, view: CandidateView<'_>) {
        let completed_ticks = self.ghosts.completed_ticks();
        self.ghosts.update(
            &self.registry,
            GhostFrame {
                clocks: PresentationClocks::compute(
                    ClockSource::SinglePlayer { completed_ticks },
                    0.5,
                ),
                alpha: 0.5,
                elapsed_seconds: FRAME,
                view,
                stamps: PoseStamps::None,
            },
        );
    }

    fn frame(&mut self) {
        self.frame_in(CandidateView::Authoritative);
    }

    fn ghosts_of(&self, entity: EntityId) -> Vec<GhostMember> {
        self.ghosts
            .members()
            .iter()
            .filter(|member| member.entity == entity)
            .copied()
            .collect()
    }
}

/// A barrier whose geometry sits 4 m along +x from its origin: its bounds
/// center, not its origin, decides membership.
const OFFSET_BOUNDS: (Vec3, Vec3) = (Vec3::new(3.5, -1.0, -0.5), Vec3::new(4.5, 1.0, 0.5));

// Host and connected client: a mover's presented bounds
// center just inside the volume ghosts it and just outside does not, whatever
// its origin; the client loads the same mover from the level file and decides
// the same way.
#[test]
fn a_movers_bounds_center_just_inside_the_volume_ghosts_it_and_just_outside_does_not() {
    let world = world(
        vec![link(4, LINK_0, LINK_0_TARGET, &["barrier"])],
        vec![loaded_mover(1, &["barrier"], OFFSET_BOUNDS)],
    );
    let client = crate::netcode::ClientReplication::default();
    for view in [CandidateView::Authoritative, CandidateView::Client(&client)] {
        let mut level = Level::new(world_clone(&world));
        // Origin outside (x < 0) but center at x = 9.99: inside.
        let inside = level.mover(1, Vec3::new(5.99, 0.0, 0.0));
        level.frame_in(view);
        let ghosts = level.ghosts_of(inside);
        assert_eq!(ghosts.len(), 1, "center just inside");
        assert_eq!(ghosts[0].category, GhostCategory::Movers);
        assert!(!ghosts[0].frozen);

        // Origin inside (x = 6.01) but center at x = 10.01: outside.
        let mut level = Level::new(world_clone(&world));
        let outside = level.mover(1, Vec3::new(6.01, 0.0, 0.0));
        level.frame_in(view);
        assert!(level.ghosts_of(outside).is_empty(), "center just outside");

        let mut level = Level::new(world_clone(&world));
        let origin_out = level.mover(1, Vec3::new(-3.0, 0.0, 0.0));
        level.frame_in(view);
        assert_eq!(
            level.ghosts_of(origin_out).len(),
            1,
            "origin outside, bounds center at x = 1 inside"
        );
    }
}

/// A fresh install of the same level, as a second peer loads it.
fn world_clone(world: &LevelWorld) -> LevelWorld {
    self::world(
        world.ghost_links.links.clone(),
        world.kinematic_geometry.movers.clone(),
    )
}

// A mover carrying a listed tag ghosts; an untagged mover
// beside it does not, and neither does a mover whose tag no link lists, while
// the ghosted partner pawn beside them ghosts.
#[test]
fn a_mover_carrying_a_listed_tag_ghosts_while_an_untagged_partner_mover_does_not() {
    let world = world(
        vec![link(4, LINK_0, LINK_0_TARGET, &["barrier"])],
        vec![
            loaded_mover(1, &["barrier"], OFFSET_BOUNDS),
            loaded_mover(2, &[], OFFSET_BOUNDS),
            loaded_mover(3, &["lift"], OFFSET_BOUNDS),
        ],
    );
    let mut level = Level::new(world);
    let barrier = level.mover(1, Vec3::new(1.0, 0.0, 0.0));
    let untagged = level.mover(2, Vec3::new(1.0, 0.0, 1.0));
    let lift = level.mover(3, Vec3::new(1.0, 0.0, -1.0));
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0));
    level.frame();
    assert_eq!(level.ghosts_of(barrier).len(), 1);
    assert_eq!(level.ghosts_of(partner).len(), 1);
    assert!(level.ghosts_of(untagged).is_empty());
    assert!(level.ghosts_of(lift).is_empty());
}

// The same tagged barrier inside two links ghosts through the
// link that lists its tag and not the other.
#[test]
fn a_barrier_inside_two_links_ghosts_only_through_the_link_listing_its_tag() {
    let world = world(
        vec![
            link(4, LINK_0, LINK_0_TARGET, &["barrier"]),
            link(5, LINK_1, LINK_1_TARGET, &["lift"]),
        ],
        vec![
            loaded_mover(1, &["barrier"], OFFSET_BOUNDS),
            loaded_mover(2, &["lift"], OFFSET_BOUNDS),
        ],
    );
    let mut level = Level::new(world);
    // Both centers at x = 8, inside both volumes.
    let barrier = level.mover(1, Vec3::new(4.0, 0.0, 0.0));
    let lift = level.mover(2, Vec3::new(4.0, 0.0, 1.0));
    level.frame();
    let barrier_links: Vec<u16> = level.ghosts_of(barrier).iter().map(|g| g.link).collect();
    let lift_links: Vec<u16> = level.ghosts_of(lift).iter().map(|g| g.link).collect();
    assert_eq!(barrier_links, [0]);
    assert_eq!(lift_links, [1]);
}

// A mover inside two armed links that both list its tag
// draws two ghosts at two transforms, each its link's mapping of one pose.
#[test]
fn a_mover_inside_two_links_listing_its_tag_ghosts_twice_at_two_transforms() {
    let world = world(
        vec![
            link(4, LINK_0, LINK_0_TARGET, &["barrier"]),
            link(5, LINK_1, LINK_1_TARGET, &["barrier"]),
        ],
        vec![loaded_mover(1, &["barrier"], OFFSET_BOUNDS)],
    );
    let mut level = Level::new(world);
    let origin = Vec3::new(4.0, 0.0, 0.0);
    let barrier = level.mover(1, origin);
    level.frame();
    let ghosts = level.ghosts_of(barrier);
    assert_eq!(ghosts.len(), 2, "P27: one ghost per link");
    assert!(ghosts.iter().all(|ghost| ghost.source.position == origin));
    assert_eq!(ghosts[0].mapped.position, origin + LINK_0_TARGET);
    assert_eq!(ghosts[1].mapped.position, origin + LINK_1_TARGET);
}

// A mover matching two tags of one link ghosts once
// through it.
#[test]
fn a_mover_matching_two_tags_of_one_link_ghosts_once() {
    let world = world(
        vec![link(4, LINK_0, LINK_0_TARGET, &["barrier", "vault"])],
        vec![loaded_mover(1, &["vault", "barrier"], OFFSET_BOUNDS)],
    );
    let mut level = Level::new(world);
    let barrier = level.mover(1, Vec3::new(1.0, 0.0, 0.0));
    level.frame();
    assert_eq!(level.ghosts_of(barrier).len(), 1, "P39");
}

// Movers: with `movers` off, a link's mover ghosts fade at
// the rate cap while its other categories' members stay; turned back on, the
// mover ramps back in from its presented value.
#[test]
fn turning_the_movers_category_off_fades_movers_at_the_rate_cap_while_pawns_stay() {
    let world = world(
        vec![link(4, LINK_0, LINK_0_TARGET, &["barrier"])],
        vec![loaded_mover(1, &["barrier"], OFFSET_BOUNDS)],
    );
    let mut level = Level::new(world);
    let barrier = level.mover(1, Vec3::new(1.0, 0.0, 0.0));
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0));
    level.frame();
    let membership = |level: &Level, entity| level.ghosts_of(entity).first().map(|g| g.membership);
    assert_eq!(membership(&level, barrier), Some(1.0), "armed on spawn");

    level.set_members(0, GhostMembers::all() - GhostMembers::MOVERS);
    let step = ramp_step(FRAME);
    let mut fade = Vec::new();
    for _ in 0..4 {
        level.frame();
        fade.push(membership(&level, barrier).unwrap());
        assert_eq!(membership(&level, partner), Some(1.0), "players stay");
        assert!(level.ghosts_of(barrier)[0].frozen);
    }
    for (frame, presented) in fade.iter().enumerate() {
        let expected = 1.0 - step * (frame + 1) as f32;
        assert!((presented - expected).abs() < 1e-5, "{fade:?}");
    }

    level.set_members(0, GhostMembers::all());
    level.frame();
    let resumed = membership(&level, barrier).unwrap();
    assert!(
        (resumed - (fade[3] + step)).abs() < 1e-5,
        "reverses from presented"
    );
}

fn unmatched_warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .iter()
        .filter(|record| {
            record.level == log::Level::Warn && record.message.contains(UNMATCHED_GHOST_TAG_WARNING)
        })
        .map(|record| record.message.clone())
        .collect()
}

// A `ghost_tags` entry matching no mover warns once at level
// install, on the host and on a client (both install from the same level file
// through the same install), and the level loads: the link's matched tags still
// ghost their movers.
#[test]
fn an_unmatched_ghost_tag_warns_once_at_install_on_host_and_client_and_the_level_loads() {
    let world = world(
        vec![link(4, LINK_0, LINK_0_TARGET, &["barrier", "missing"])],
        vec![loaded_mover(1, &["barrier"], OFFSET_BOUNDS)],
    );
    let client = crate::netcode::ClientReplication::default();
    for view in [CandidateView::Authoritative, CandidateView::Client(&client)] {
        let capture = LogCapture::start();
        let mut level = Level::new(world_clone(&world));
        let warned = unmatched_warnings(&capture);
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(warned[0].contains("`missing`"), "{}", warned[0]);
        assert!(!warned[0].contains("`barrier`"));

        let barrier = level.mover(1, Vec3::new(1.0, 0.0, 0.0));
        for _ in 0..3 {
            level.frame_in(view);
        }
        assert_eq!(
            level.ghosts_of(barrier).len(),
            1,
            "the level loads and ghosts"
        );
        assert_eq!(
            unmatched_warnings(&capture).len(),
            1,
            "once, at install only"
        );
    }
}

// A headless capture installs and evaluates once, with no fixed tick and no
// descriptor opt-ins: a tagged mover inside a link armed on spawn presents at
// full, one inside a link disarmed on spawn does not ghost, and a pawn never
// ghosts in a capture.
#[cfg(feature = "capture")]
#[test]
fn capture_evaluation_settles_movers_of_links_armed_on_spawn_and_ghosts_no_pawn() {
    let mut disarmed = link(5, LINK_1, LINK_1_TARGET, &["barrier"]);
    disarmed.enabled_on_spawn = false;
    let world = world(
        vec![link(4, LINK_0, LINK_0_TARGET, &["barrier"]), disarmed],
        vec![loaded_mover(1, &["barrier"], OFFSET_BOUNDS)],
    );
    let mut level = Level::new(world);
    // Center at x = 8, inside both volumes.
    let barrier = level.mover(1, Vec3::new(4.0, 0.0, 0.0));
    let partner = level.pawn(Vec3::new(8.0, 0.0, 0.0));
    // As the capture builds it: installed, never refreshed against descriptors.
    level.ghosts = GhostLinks::install(&level.world, &level.registry);
    level.ghosts.evaluate_for_capture(&level.registry);
    let ghosts = level.ghosts_of(barrier);
    assert_eq!(ghosts.len(), 1, "only the link armed on spawn shows it");
    assert_eq!(ghosts[0].link, 0);
    assert_eq!(ghosts[0].membership, 1.0, "full on the capture's one frame");
    assert!(
        level.ghosts_of(partner).is_empty(),
        "no opt-ins in a capture"
    );
}
