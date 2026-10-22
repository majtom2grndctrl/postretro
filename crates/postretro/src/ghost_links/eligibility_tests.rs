// Ghost eligibility from descriptor opt-ins: categories, roles, the viewer's own shots,
// the install warning and committed-reload re-resolution through the membership ramp.
// See: context/lib/entity_model.md §7d

use std::collections::BTreeSet;

use glam::Vec3;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{DataRegistry, EntityId, EntityRegistry, Transform};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
use postretro_level_loader::{CellLocatorChild, LevelWorld};
use postretro_test_log_capture::LogCapture;

use super::super::clocks::{ClockSource, PoseStamps, PresentationClocks};
use super::super::membership::ramp_step;
use super::super::movers::GhostMovers;
use super::super::opt_ins::{NO_OPT_IN_WARNING, warn_if_links_without_opt_ins};
use super::super::opt_ins_test_fixtures::{data_registry, descriptor, projectile_weapon};
use super::super::{GhostFrame, GhostLinks, GhostMember, spawn_loaded_ghost_links};
use super::*;

const FRAME: f32 = 1.0 / 240.0;
/// The armed link covers x in [0, 10] and maps into a copy 100 m along +x.
const VOLUME: (Vec3, Vec3) = (Vec3::new(0.0, 0.0, -5.0), Vec3::new(10.0, 4.0, 5.0));
const INSIDE: Vec3 = Vec3::new(2.0, 1.0, 0.0);
const OUTSIDE: Vec3 = Vec3::new(-20.0, 1.0, 0.0);

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

fn world() -> LevelWorld {
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
    world.ghost_links.links = vec![GhostLinkRecord {
        source_entity: 4,
        enabled_on_spawn: true,
        members: GhostMembers::all(),
        style: String::new(),
        brushes: vec![box_brush(VOLUME)],
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        target_origin: [100.0, 0.0, 0.0],
        target_yaw: 0.0,
        ghost_tags: Vec::new(),
    }];
    world
}

/// One armed link showing every category, a viewer pawn outside it, and this
/// peer's committed descriptors.
pub(super) struct Scene {
    pub registry: EntityRegistry,
    pub ghosts: GhostLinks,
    pub data: DataRegistry,
    pub viewer: EntityId,
}

impl Scene {
    pub fn new(descriptors: Vec<postretro_entities::EntityTypeDescriptor>) -> Self {
        let world = world();
        let mut registry = EntityRegistry::new();
        spawn_loaded_ghost_links(&mut registry, &world);
        let ghosts = GhostLinks::install(&world, &registry);
        let viewer = registry.spawn(Transform::default());
        let mut scene = Self {
            registry,
            ghosts,
            data: data_registry(descriptors),
            viewer,
        };
        scene.dress_pawn(viewer, OUTSIDE, DescriptorSpawnPath::PlayerSpawn);
        scene.registry.mark_local_player_pawn(viewer).unwrap();
        scene
    }

    /// Present `id` at `at` at any alpha.
    pub fn place(&mut self, id: EntityId, at: Vec3) {
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

    fn provenance(&mut self, id: EntityId, name: &str, spawn_path: DescriptorSpawnPath) {
        self.registry
            .set_component(
                id,
                DescriptorProvenance {
                    canonical_name: name.into(),
                    owned_components: BTreeSet::new(),
                    map_overrides: BTreeSet::new(),
                    spawn_path,
                },
            )
            .unwrap();
    }

    /// A pawn of the `player` descriptor.
    pub fn pawn(&mut self, at: Vec3, spawn_path: DescriptorSpawnPath) -> EntityId {
        let id = self.registry.spawn(Transform::default());
        self.dress_pawn(id, at, spawn_path);
        id
    }

    fn dress_pawn(&mut self, id: EntityId, at: Vec3, spawn_path: DescriptorSpawnPath) {
        self.place(id, at);
        self.registry
            .set_component(
                id,
                postretro_foundation::PlayerMovementComponent::from_descriptor(
                    &crate::mod_digest::tests::movement_descriptor(),
                ),
            )
            .unwrap();
        self.provenance(id, "player", spawn_path);
    }

    /// A descriptor entity (a monster, a prop) materialized from `name`.
    pub fn entity(&mut self, name: &str, at: Vec3, spawn_path: DescriptorSpawnPath) -> EntityId {
        let id = self.registry.spawn(Transform::default());
        self.place(id, at);
        self.provenance(id, name, spawn_path);
        id
    }

    /// A gameplay projectile `owner` fired from weapon descriptor `weapon`.
    pub fn projectile(&mut self, weapon: &str, owner: EntityId, at: Vec3) -> EntityId {
        let id = self.registry.spawn(Transform::default());
        self.place(id, at);
        self.registry
            .set_component(
                id,
                ProjectileComponent {
                    source_sounds: None,
                    predicted_visible: true,
                    source_action: None,
                    source_shot: None,
                    direction: Vec3::X.to_array(),
                    speed: 30.0,
                    radius: 0.1,
                    remaining_range: 64.0,
                    remaining_lifetime: 2.0,
                    damage: 10.0,
                    knockback_impulse: [0.0; 3],
                    credit_source: weapon.to_string(),
                    owner_pawn: owner,
                    // An enemy's projectile carries the enemy as its damage
                    // provenance, never a weapon entity.
                    owner_weapon: owner,
                    attackerless: false,
                    spawned: false,
                    predicted_shot_id: None,
                    elapsed_flight_age: 0.0,
                    flipbook_active: false,
                    impact_light: None,
                    splash: None,
                    source_weapon: Some(weapon.to_string()),
                    activation: None,
                },
            )
            .unwrap();
        id
    }

    /// A host presentation flight for weapon `weapon`: a remote client's fire
    /// with its body, or (`body` false) the hidden mirror of a gameplay projectile.
    pub fn presentation_projectile(&mut self, weapon: &str, at: Vec3, body: bool) -> EntityId {
        let id = self.entity(weapon, at, DescriptorSpawnPath::ProjectilePresentation);
        if body {
            self.registry
                .set_component(id, MeshComponent::stateless("models/rocket.glb".into()))
                .unwrap();
        }
        id
    }

    /// Commit a new descriptor snapshot, as a staged reload's commit does.
    pub fn reload(&mut self, descriptors: Vec<postretro_entities::EntityTypeDescriptor>) {
        self.data.replace_entity_types(descriptors);
    }

    /// One presented frame on the host (single player stands in for its clocks).
    pub fn frame(&mut self) {
        self.ghosts.refresh_opt_ins(&self.data);
        let completed_ticks = self.ghosts.completed_ticks();
        self.ghosts.update(
            &self.registry,
            GhostFrame {
                clocks: PresentationClocks::compute(
                    ClockSource::SinglePlayer { completed_ticks },
                    0.0,
                ),
                alpha: 0.0,
                elapsed_seconds: FRAME,
                view: CandidateView::Authoritative,
                stamps: PoseStamps::None,
            },
        );
    }

    pub fn ghost(&self, entity: EntityId) -> Option<GhostMember> {
        self.ghosts
            .members()
            .iter()
            .find(|member| member.entity == entity)
            .copied()
    }

    pub fn ghosted(&self) -> Vec<(EntityId, GhostCategory)> {
        self.ghosts
            .members()
            .iter()
            .map(|member| (member.entity, member.category))
            .collect()
    }
}

/// An untagged kinematic mover: no descriptor, so no descriptor opt-in reaches it.
fn untagged_mover(scene: &mut Scene, at: Vec3) -> EntityId {
    let id = scene.registry.spawn(Transform::default());
    scene.place(id, at);
    scene
        .registry
        .set_component(
            id,
            postretro_entities::KinematicMoverComponent::new(
                0,
                postretro_entities::KinematicMoverConfig {
                    waypoints: vec![at, at + Vec3::Y],
                    waypoint_names: vec!["closed".into(), "open".into()],
                    speed_mps: 1.0,
                    wait_ms: 0.0,
                    mode: postretro_entities::KinematicMoverMode::PingPong,
                    started: false,
                    spin_axis: Vec3::ZERO,
                    initial_spin_rate_rad_s: 0.0,
                    spin_accel_rad_s2: 0.0,
                    carry_yaw: false,
                },
            ),
        )
        .unwrap();
    id
}

// Host: the partner pawn ghosts while an untagged mover and
// a monster whose descriptor does not set `ghost` beside it do not; a monster
// whose descriptor sets `ghost` ghosts as a descriptor entity.
#[test]
fn partner_and_opted_in_monster_ghost_while_an_unopted_monster_and_untagged_mover_do_not() {
    let mut scene = Scene::new(vec![
        descriptor("player", true),
        descriptor("imp", false),
        descriptor("phase_wraith", true),
    ]);
    let partner = scene.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
    let imp = scene.entity("imp", INSIDE + Vec3::X, DescriptorSpawnPath::MapPlacement);
    let mover = untagged_mover(&mut scene, INSIDE + Vec3::Z);
    let wraith = scene.entity(
        "phase_wraith",
        INSIDE - Vec3::Z,
        DescriptorSpawnPath::MapPlacement,
    );
    scene.frame();

    let mut expected = vec![
        (partner, GhostCategory::Players),
        (wraith, GhostCategory::Entities),
    ];
    expected.sort();
    assert_eq!(scene.ghosted(), expected);
    assert!(scene.ghost(imp).is_none() && scene.ghost(mover).is_none());
}

// A player descriptor that omits `ghost` puts no pawn in any
// ghost set, whichever path materialized it; the engine never ghosts by kind.
#[test]
fn a_player_descriptor_omitting_ghost_puts_no_pawn_in_any_ghost_set() {
    let mut scene = Scene::new(vec![descriptor("player", false)]);
    let host_pawn = scene.pawn(INSIDE, DescriptorSpawnPath::PlayerSpawn);
    let slot_pawn = scene.pawn(INSIDE + Vec3::X, DescriptorSpawnPath::NetworkSlot);
    scene.frame();
    assert!(scene.ghost(host_pawn).is_none() && scene.ghost(slot_pawn).is_none());
    assert!(scene.ghosted().is_empty());

    // A client's copy of the same pawn reads the same descriptor.
    let opt_ins = opt_ins_from(&scene.data);
    assert_eq!(
        classify(
            &scene.registry,
            slot_pawn,
            CandidateOrigin::Replicated(ReplicatedDescriptorClass::Pawn("player")),
            &opt_ins,
            &GhostMovers::default(),
        ),
        None
    );
}

fn opt_ins_from(data: &DataRegistry) -> GhostOptIns {
    let mut opt_ins = GhostOptIns::default();
    opt_ins.refresh(data);
    opt_ins
}

fn warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn && record.message.contains("[Ghost]"))
        .map(|record| record.message)
        .collect()
}

// A level with links while no loaded descriptor sets
// top-level `ghost` warns once at install, so a forgotten opt-in is not
// silent, even when a weapon's `projectile.ghost` opts its shots in or a link's
// `ghost_tags` matches a mover; the warning names what can still ghost, or
// says nothing can. Install runs the same check on host and client.
#[test]
fn links_with_no_ghost_opted_descriptor_warn_once_at_install() {
    let shots = || {
        vec![
            descriptor("player", false),
            projectile_weapon("rocket_launcher", true),
        ]
    };
    for (links, movers_ghost, descriptors, still_ghost) in [
        (
            2,
            true,
            vec![descriptor("player", false)],
            "only tagged movers can",
        ),
        (
            2,
            true,
            shots(),
            "only tagged movers and projectiles whose weapon sets `projectile.ghost` can",
        ),
        (
            2,
            false,
            shots(),
            "only projectiles whose weapon sets `projectile.ghost` can",
        ),
        (
            2,
            false,
            vec![descriptor("player", false)],
            "nothing can ghost",
        ),
        (
            1,
            false,
            vec![descriptor("player", false)],
            "has 1 ghost link but",
        ),
        (
            2,
            false,
            vec![descriptor("player", false)],
            "has 2 ghost links but",
        ),
    ] {
        let capture = LogCapture::start();
        warn_if_links_without_opt_ins(links, movers_ghost, &data_registry(descriptors));
        let warned = warnings(&capture);
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(warned[0].contains(NO_OPT_IN_WARNING), "{}", warned[0]);
        assert!(warned[0].contains(still_ghost), "{}", warned[0]);
        if !movers_ghost {
            assert!(!warned[0].contains("movers"), "{}", warned[0]);
        }
    }

    for (links, descriptors) in [
        (2, vec![descriptor("player", true)]),
        (
            2,
            vec![
                descriptor("phase_wraith", true),
                projectile_weapon("rocket_launcher", true),
            ],
        ),
        (0, vec![descriptor("player", false)]),
    ] {
        let capture = LogCapture::start();
        warn_if_links_without_opt_ins(links, true, &data_registry(descriptors));
        assert!(warnings(&capture).is_empty(), "{links} links");
    }
}

// Regression: a dropped default-loadout weapon ghosted on clients, which receive
// it under its descriptor class, but never on the host.
#[test]
fn a_dropped_default_weapon_ghosts_alike_on_host_and_client_while_a_carried_one_does_not() {
    let mut scene = Scene::new(vec![descriptor("rifle", true)]);
    let dropped = scene.entity("rifle", INSIDE, DescriptorSpawnPath::DefaultWeapon);
    let carried = scene.entity(
        "rifle",
        INSIDE + Vec3::X,
        DescriptorSpawnPath::DefaultWeapon,
    );
    let mut inventory = Inventory::default();
    inventory.wieldables[0] = Some(carried);
    let holder = scene.viewer;
    scene.registry.set_component(holder, inventory).unwrap();
    scene.frame();
    assert_eq!(
        scene.ghosted(),
        vec![(dropped, GhostCategory::Entities)],
        "the host ghosts the dropped weapon; the carried one draws with its holder"
    );

    // A client holds only the dropped weapon, as a `descriptor:` display copy.
    let opt_ins = opt_ins_from(&scene.data);
    assert_eq!(
        classify(
            &scene.registry,
            dropped,
            CandidateOrigin::Replicated(ReplicatedDescriptorClass::Descriptor("rifle")),
            &opt_ins,
            &GhostMovers::default(),
        ),
        Some(GhostCategory::Entities)
    );
}

// A spawner releasing a ghost-opted descriptor inside an
// armed link ghosts the spawn, its membership ramping in from 0.
#[test]
fn a_runtime_spawn_of_an_opted_in_descriptor_ramps_in_from_zero() {
    let mut scene = Scene::new(vec![descriptor("phase_wraith", true)]);
    scene.frame();
    let spawn = scene.entity("phase_wraith", INSIDE, DescriptorSpawnPath::RuntimeSpawn);
    scene.frame();
    let first = scene
        .ghost(spawn)
        .expect("the spawn ghosts on its first frame");
    assert_eq!(first.category, GhostCategory::Entities);
    assert_eq!(first.membership, 0.0, "a new ghost starts at 0");
    scene.frame();
    let second = scene.ghost(spawn).unwrap();
    assert!((second.membership - ramp_step(FRAME)).abs() < 1e-6);
}

// A projectile ghosts by its weapon's `projectile.ghost`
// whoever fired it, an unghosted monster included; one whose weapon omits it
// does not. A remote client's fire flown on the host ghosts by the same rule;
// the host's bodiless mirror of its own gameplay projectile never doubles it.
#[test]
fn projectiles_ghost_by_their_weapon_opt_in_whoever_fired_them() {
    let mut scene = Scene::new(vec![
        descriptor("player", false),
        descriptor("imp", false),
        projectile_weapon("rocket_launcher", true),
        projectile_weapon("plasma_rifle", false),
    ]);
    let partner = scene.pawn(OUTSIDE + Vec3::Z, DescriptorSpawnPath::NetworkSlot);
    let imp = scene.entity("imp", INSIDE, DescriptorSpawnPath::MapPlacement);
    let partner_rocket = scene.projectile("rocket_launcher", partner, INSIDE + Vec3::X);
    let imp_rocket = scene.projectile("rocket_launcher", imp, INSIDE + Vec3::Y);
    let imp_plasma = scene.projectile("plasma_rifle", imp, INSIDE + Vec3::Z);
    let remote_rocket = scene.presentation_projectile("rocket_launcher", INSIDE - Vec3::Z, true);
    let mirror = scene.presentation_projectile("rocket_launcher", INSIDE + Vec3::X, false);
    scene.frame();

    let mut expected = vec![
        (partner_rocket, GhostCategory::Projectiles),
        (imp_rocket, GhostCategory::Projectiles),
        (remote_rocket, GhostCategory::Projectiles),
    ];
    expected.sort();
    assert_eq!(scene.ghosted(), expected);
    assert!(
        scene.ghost(imp).is_none(),
        "the unghosted shooter stays real-only"
    );
    assert!(scene.ghost(imp_plasma).is_none() && scene.ghost(mirror).is_none());
}

// The viewer's own projectiles never ghost on its machine,
// while the partner's do.
#[test]
fn the_viewers_own_projectiles_are_absent_from_its_own_ghost_set() {
    let mut scene = Scene::new(vec![projectile_weapon("rocket_launcher", true)]);
    let partner = scene.pawn(OUTSIDE + Vec3::Z, DescriptorSpawnPath::NetworkSlot);
    let viewer = scene.viewer;
    let own = scene.projectile("rocket_launcher", viewer, INSIDE);
    let partners = scene.projectile("rocket_launcher", partner, INSIDE + Vec3::X);
    scene.frame();
    assert!(scene.ghost(own).is_none());
    assert!(scene.ghost(partners).is_some());
}

// Regression: when the camera came to follow a pawn whose projectile was
// ghosting, the new viewer saw its own projectile's ghost fade out over the
// rate cap.
#[test]
fn a_viewer_switch_drops_the_new_viewers_projectile_ghost_on_that_frame() {
    let mut scene = Scene::new(vec![projectile_weapon("rocket_launcher", true)]);
    let partner = scene.pawn(OUTSIDE + Vec3::Z, DescriptorSpawnPath::NetworkSlot);
    let rocket = scene.projectile("rocket_launcher", partner, INSIDE);
    let other = scene.pawn(OUTSIDE - Vec3::Z, DescriptorSpawnPath::NetworkSlot);
    let others = scene.projectile("rocket_launcher", other, INSIDE + Vec3::X);
    scene.frame();
    assert!(scene.ghost(rocket).is_some());

    scene.registry.mark_local_player_pawn(partner).unwrap();
    scene.frame();
    assert!(scene.ghost(rocket).is_none(), "no fade frame");
    assert!(
        scene.ghost(others).is_some(),
        "another pawn's projectile keeps ghosting"
    );
}

// A ghost-opted projectile keeps ghosting while its
// origin is inside after its owner leaves the volume.
#[test]
fn a_ghost_opted_projectile_keeps_ghosting_after_its_owner_leaves() {
    let mut scene = Scene::new(vec![
        descriptor("player", true),
        projectile_weapon("rocket_launcher", true),
    ]);
    let partner = scene.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
    let rocket = scene.projectile("rocket_launcher", partner, INSIDE + Vec3::X);
    scene.frame();
    assert!(scene.ghost(partner).is_some() && scene.ghost(rocket).is_some());

    scene.place(partner, OUTSIDE - Vec3::Z);
    scene.frame();
    assert!(scene.ghost(partner).unwrap().frozen, "the owner fades out");
    let rocket_ghost = scene.ghost(rocket).unwrap();
    assert!(!rocket_ghost.frozen && rocket_ghost.membership == 1.0);
}

// A committed reload that changes a descriptor's
// `ghost` or a weapon's `projectile.ghost` re-resolves membership on the next
// frame through the membership ramp, never a cut.
#[test]
fn a_committed_reload_flipping_an_opt_in_re_resolves_through_the_ramp() {
    let quiet = || {
        vec![
            descriptor("phase_wraith", false),
            projectile_weapon("rocket_launcher", false),
        ]
    };
    let loud = || {
        vec![
            descriptor("phase_wraith", true),
            projectile_weapon("rocket_launcher", true),
        ]
    };
    let mut scene = Scene::new(quiet());
    let wraith = scene.entity("phase_wraith", INSIDE, DescriptorSpawnPath::MapPlacement);
    let rocket = scene.projectile("rocket_launcher", wraith, INSIDE + Vec3::X);
    scene.frame();
    assert!(scene.ghosted().is_empty());

    scene.reload(loud());
    scene.frame();
    for entity in [wraith, rocket] {
        assert_eq!(
            scene.ghost(entity).unwrap().membership,
            0.0,
            "ramps in from 0"
        );
    }
    for _ in 0..3 {
        scene.frame();
    }
    let risen = scene.ghost(wraith).unwrap().membership;
    assert!((risen - 3.0 * ramp_step(FRAME)).abs() < 1e-5);
    assert_eq!(scene.ghost(rocket).unwrap().membership, risen);

    scene.reload(quiet());
    scene.frame();
    for entity in [wraith, rocket] {
        let fading = scene.ghost(entity).expect("an opt-out fades, never cuts");
        assert!(fading.frozen);
        assert!((fading.membership - 2.0 * ramp_step(FRAME)).abs() < 1e-5);
    }
}

// Client: display copies classify by the replicated
// class they were materialized from, against this peer's own descriptors, and
// follow its own committed reload.
#[test]
fn client_display_copies_classify_by_class_against_local_descriptors() {
    let scene = Scene::new(vec![]);
    let any = scene.viewer;
    let classify_all = |data: &DataRegistry| {
        let opt_ins = opt_ins_from(data);
        [
            ReplicatedDescriptorClass::Pawn("player"),
            ReplicatedDescriptorClass::Descriptor("phase_wraith"),
            ReplicatedDescriptorClass::Descriptor("imp"),
            ReplicatedDescriptorClass::Projectile("rocket_launcher"),
            ReplicatedDescriptorClass::Projectile("plasma_rifle"),
        ]
        .map(|class| {
            classify(
                &scene.registry,
                any,
                CandidateOrigin::Replicated(class),
                &opt_ins,
                &GhostMovers::default(),
            )
        })
    };
    let mut data = data_registry(vec![
        descriptor("player", true),
        descriptor("phase_wraith", true),
        descriptor("imp", false),
        projectile_weapon("rocket_launcher", true),
        projectile_weapon("plasma_rifle", false),
    ]);
    assert_eq!(
        classify_all(&data),
        [
            Some(GhostCategory::Players),
            Some(GhostCategory::Entities),
            None,
            Some(GhostCategory::Projectiles),
            None,
        ]
    );

    // The client's own reload flipping only `projectile.ghost` changes its
    // projectile ghosting and nothing else.
    data.replace_entity_types(vec![
        descriptor("player", true),
        descriptor("phase_wraith", true),
        descriptor("imp", false),
        projectile_weapon("rocket_launcher", false),
        projectile_weapon("plasma_rifle", true),
    ]);
    assert_eq!(
        classify_all(&data),
        [
            Some(GhostCategory::Players),
            Some(GhostCategory::Entities),
            None,
            None,
            Some(GhostCategory::Projectiles),
        ]
    );
}

#[path = "interaction_tests.rs"]
mod interaction;
