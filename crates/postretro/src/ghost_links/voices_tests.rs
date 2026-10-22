// Ghost-voice admission through installed links and a real registry: styles, the fire-time
// point, member clocks, the viewer's own sounds, impacts and their shot owners, movers, and
// admission beside refused real voices.
// See: context/lib/audio.md §1 (Ghost bus) · context/lib/entity_model.md §7d

use std::collections::BTreeSet;

use glam::Vec3;
use postretro_audio::{SoundAnchor, SoundRequest};
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{
    ComponentKind, Emitter, EntityId, EntityRegistry, GhostLinkComponent, KinematicMoverComponent,
    KinematicMoverConfig, KinematicMoverMode, Transform, WeaponEmission,
};
use postretro_foundation::{
    ActivationLane, GhostStyle, GhostStyleDescriptor, GhostStyleRegistry, ShotId,
};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
use postretro_level_loader::{CellData, CellLocatorChild, LevelWorld, LoadedKinematicMover};
use postretro_physics::kinematic_mover::MoverEventKind;

use super::super::clocks::{ClockSource, PoseStamps, PresentationClocks};
use super::super::eligibility::CandidateView;
use super::super::opt_ins_test_fixtures::{data_registry, descriptor, projectile_weapon};
use super::super::{GhostFrame, GhostLinks, spawn_loaded_ghost_links};
use super::{GHOST_BUS, GhostVoicePlan, LocalSound, VoiceFrame};
use crate::runtime_movers::KinematicMoverRenderCollector;
use crate::sound_events::{AnchorScene, frozen_sound, resolve_mover_edges};

const FRAME: f32 = 1.0 / 240.0;
/// The link volume: x in [0, 10], mapped 100 m along +x into the other copy.
const VOLUME: (Vec3, Vec3) = (Vec3::new(0.0, -5.0, -5.0), Vec3::new(10.0, 5.0, 5.0));
const COPY_OFFSET: Vec3 = Vec3::new(100.0, 0.0, 0.0);
const MOVER_ID: u32 = 7;

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

/// The styles a link may name: `echo` carries sound, `impacts` carries impact
/// sound too, `quiet` carries none. An empty name is the floor style.
fn styles() -> GhostStyleRegistry {
    let style = |name: &str, audio, impact_audio| {
        Ok(GhostStyleDescriptor {
            name: name.to_string(),
            style: GhostStyle {
                audio,
                impact_audio,
                ..GhostStyle::FLOOR
            },
        })
    };
    GhostStyleRegistry::from_declarations(
        "test",
        [
            style("echo", true, false),
            style("impacts", true, true),
            style("quiet", false, true),
        ],
    )
}

/// A mover with geometry bounding `[0, 1]³` about its origin, tagged `barrier`.
fn barrier_mover() -> LoadedKinematicMover {
    let mut mover = crate::runtime_movers::tests::mover(1);
    mover.mover_id = MOVER_ID;
    mover.tags = vec!["barrier".to_string()];
    mover
}

fn world(style: &str) -> LevelWorld {
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
    world.ghost_links.links = vec![GhostLinkRecord {
        source_entity: 4,
        enabled_on_spawn: true,
        members: GhostMembers::all(),
        style: style.to_string(),
        brushes: vec![box_brush(VOLUME)],
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        target_origin: COPY_OFFSET.to_array(),
        target_yaw: 0.0,
        ghost_tags: vec!["barrier".to_string()],
    }];
    world.kinematic_geometry.movers = vec![barrier_mover()];
    world
}

/// One installed link (armed on spawn) naming `style`, with the player and the
/// `rocket` weapon's projectiles opted in, and a viewer outside the volume.
struct Level {
    world: LevelWorld,
    registry: EntityRegistry,
    ghosts: GhostLinks,
    link: EntityId,
    viewer: EntityId,
    movers: KinematicMoverRenderCollector,
}

impl Level {
    fn new(style: &str) -> Self {
        let world = world(style);
        let mut registry = EntityRegistry::new();
        spawn_loaded_ghost_links(&mut registry, &world);
        let mut ghosts = GhostLinks::install(&world, &registry);
        ghosts.bind_styles(&world, &styles());
        ghosts.refresh_opt_ins(&data_registry(vec![
            descriptor("player", true),
            projectile_weapon("rocket", true),
        ]));
        let link = registry
            .iter_with_kind(ComponentKind::GhostLink)
            .map(|(id, _)| id)
            .next()
            .expect("installed link");
        let mut level = Self {
            world,
            registry,
            ghosts,
            link,
            viewer: EntityId::from_raw(0),
            movers: KinematicMoverRenderCollector::new(),
        };
        level.viewer = level.pawn(Vec3::new(-50.0, 0.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
        level.registry.mark_local_player_pawn(level.viewer).unwrap();
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

    fn projectile(&mut self, owner: EntityId, at: Vec3) -> EntityId {
        let id = self.registry.spawn(Transform::default());
        self.place(id, at);
        let projectile = ProjectileComponent {
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
            credit_source: "rocket".to_string(),
            owner_pawn: owner,
            owner_weapon: owner,
            attackerless: false,
            spawned: false,
            predicted_shot_id: None,
            elapsed_flight_age: 0.0,
            flipbook_active: false,
            impact_light: None,
            splash: None,
            source_weapon: Some("rocket".to_string()),
            activation: None,
        };
        self.registry.set_component(id, projectile).unwrap();
        id
    }

    /// The level's tagged mover, spawned as every peer spawns it, opening
    /// toward `origin + Y` and playing `door_open` when it opens.
    fn mover(&mut self, origin: Vec3) -> EntityId {
        let id = self
            .registry
            .try_spawn(Transform::default(), &["barrier".to_string()])
            .unwrap();
        self.place(id, origin);
        let mut component = KinematicMoverComponent::new(
            MOVER_ID,
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
        component.open_sound = Some("door_open".to_string());
        self.registry.set_component(id, component).unwrap();
        id
    }

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

    fn set_armed(&mut self, armed: bool) {
        let mut link = *self
            .registry
            .get_component::<GhostLinkComponent>(self.link)
            .unwrap();
        link.armed = armed;
        self.registry.set_component(self.link, link).unwrap();
    }

    /// A descriptor sound at `emitter`, anchored as the app drain anchors it.
    fn sound_at(&mut self, key: &str, emitter: &Emitter) -> LocalSound {
        let mut scene = AnchorScene {
            registry: &self.registry,
            world: Some(&self.world),
            movers: &mut self.movers,
        };
        frozen_sound(key, emitter, &mut scene).into()
    }

    fn fire_from(&mut self, entity: EntityId) -> LocalSound {
        let origin = self
            .registry
            .get_component::<Transform>(entity)
            .unwrap()
            .position;
        self.sound_at("rifle_fire", &Emitter::Entity { id: entity, origin })
    }

    fn voices_with(&self, source: ClockSource, sound: &LocalSound) -> Vec<SoundRequest> {
        let frame = VoiceFrame {
            clocks: PresentationClocks::compute(source, 0.0),
            view: CandidateView::Authoritative,
            stamps: PoseStamps::None,
        };
        let mut out = Vec::new();
        self.ghosts
            .ghost_voices(&self.registry, &frame, sound, &mut out);
        out
    }

    fn voices(&self, sound: &LocalSound) -> Vec<SoundRequest> {
        let completed_ticks = self.ghosts.completed_ticks();
        self.voices_with(ClockSource::SinglePlayer { completed_ticks }, sound)
    }

    fn membership_frame(&mut self) {
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
}

fn point_voice(sound: &str, point: Vec3) -> SoundRequest {
    SoundRequest {
        bus: GHOST_BUS.to_string(),
        sound: sound.to_string(),
        looping: false,
        anchor: Some(SoundAnchor::Point(point.to_array())),
    }
}

fn eye_of(level: &Level, pawn: EntityId) -> Vec3 {
    match level.clone_anchor(pawn) {
        Some(SoundAnchor::Entity { point, .. }) => Vec3::from_array(point),
        other => panic!("a pawn sound anchors at the pawn: {other:?}"),
    }
}

impl Level {
    fn clone_anchor(&self, pawn: EntityId) -> Option<SoundAnchor> {
        let mut movers = KinematicMoverRenderCollector::new();
        let mut scene = AnchorScene {
            registry: &self.registry,
            world: Some(&self.world),
            movers: &mut movers,
        };
        let origin = self
            .registry
            .get_component::<Transform>(pawn)
            .unwrap()
            .position;
        frozen_sound("probe", &Emitter::Entity { id: pawn, origin }, &mut scene).anchor
    }
}

// A fire sound at a ghosted pawn admits one second voice, on the Ghost
// bus at the mapped fire-time point, under a style that carries sound, and
// none under one that does not (the floor style included).
#[test]
fn a_member_fire_sound_admits_a_ghost_voice_only_under_a_sound_carrying_style() {
    let mut level = Level::new("echo");
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    let fire = level.fire_from(partner);
    let eye = eye_of(&level, partner);
    assert_eq!(
        level.voices(&fire),
        vec![point_voice("rifle_fire", eye + COPY_OFFSET)],
        "one Ghost-bus voice at the mapped fire-time point"
    );

    for style in ["quiet", ""] {
        let mut level = Level::new(style);
        let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
        let fire = level.fire_from(partner);
        assert!(
            level.voices(&fire).is_empty(),
            "style `{style}` carries no sound"
        );
    }
}

// Admission reads the fire-time point and armed state, never the
// presented ramp: a sound fired just inside while the member's ghost fades out
// still admits, one fired just outside does not, and a disarmed link admits none.
#[test]
fn admission_reads_the_fire_time_point_and_armed_state_not_the_presented_ramp() {
    let mut level = Level::new("echo");
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    level.membership_frame();
    level.place(partner, Vec3::new(-0.5, 0.0, 0.0));
    level.membership_frame();
    let fading = level
        .ghosts
        .members()
        .iter()
        .find(|member| member.entity == partner)
        .expect("the ghost fades out");
    assert!(fading.frozen && fading.membership < 1.0);

    let at = |point: Vec3| -> LocalSound {
        SoundRequest {
            bus: "sfx".to_string(),
            sound: "rifle_fire".to_string(),
            looping: false,
            anchor: Some(SoundAnchor::Entity {
                key: crate::sound_events::entity_key(partner),
                point: point.to_array(),
            }),
        }
        .into()
    };
    let inside = Vec3::new(0.01, 1.0, 0.0);
    assert_eq!(
        level.voices(&at(inside)),
        vec![point_voice("rifle_fire", inside + COPY_OFFSET)]
    );
    assert!(level.voices(&at(Vec3::new(-0.01, 1.0, 0.0))).is_empty());

    // Disarmed during one tick; the next frame presents that tick's end.
    level.set_armed(false);
    level.ghosts.record_after_fixed_tick(&level.registry, None);
    level.ghosts.record_after_fixed_tick(&level.registry, None);
    assert!(
        level.voices(&at(inside)).is_empty(),
        "a disarmed link admits none"
    );
}

// The viewer's own fire, and a sound at its own projectile, admit
// no ghost voice on its machine, while the partner's do.
#[test]
fn the_viewers_own_sounds_never_ghost_on_its_machine() {
    let mut level = Level::new("echo");
    let viewer = level.viewer;
    level.place(viewer, Vec3::new(3.0, 0.0, 0.0));
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    let own_rocket = level.projectile(viewer, Vec3::new(4.0, 0.0, 0.0));
    let partner_rocket = level.projectile(partner, Vec3::new(5.0, 0.0, 0.0));

    let own_fire = level.fire_from(viewer);
    let own_flight = level.fire_from(own_rocket);
    assert!(level.voices(&own_fire).is_empty());
    assert!(level.voices(&own_flight).is_empty());
    let partner_fire = level.fire_from(partner);
    let partner_flight = level.fire_from(partner_rocket);
    assert_eq!(level.voices(&partner_fire).len(), 1);
    assert_eq!(level.voices(&partner_flight).len(), 1);
}

// A ghost voice is a fixed point, not a follower: after its member
// despawns and its link disarms, nothing re-judges it and it plays out where it
// was admitted (the audio crate proves a point voice plays out on the bus).
#[test]
fn an_admitted_ghost_voice_stays_at_its_mapped_point_after_the_member_and_link_go() {
    let mut level = Level::new("echo");
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    let fire = level.fire_from(partner);
    let admitted = level.voices(&fire);
    let [voice] = admitted.as_slice() else {
        panic!("one ghost voice: {admitted:?}");
    };
    level.registry.despawn(partner).unwrap();
    level.set_armed(false);
    assert!(
        matches!(voice.anchor, Some(SoundAnchor::Point(point)) if Vec3::from_array(point).x > 100.0),
        "a point in the other copy, carrying no entity key to follow: {voice:?}"
    );
}

// A mover's open sound ghosts on the host under a sound-carrying style,
// at the mapped center of its bounds, through the link that lists its tag.
#[test]
fn a_tagged_movers_open_sound_ghosts_on_the_host() {
    let mut level = Level::new("echo");
    let door = level.mover(Vec3::new(4.0, 0.0, 0.0));
    let edges = {
        let mut scene = AnchorScene {
            registry: &level.registry,
            world: Some(&level.world),
            movers: &mut level.movers,
        };
        resolve_mover_edges(&[(MoverEventKind::Opened, MOVER_ID)], &mut scene)
    };
    let open: LocalSound = edges[0].sound_request().expect("open sound").into();
    let Some(SoundAnchor::Entity { key, point }) = open.request.anchor.clone() else {
        panic!("a mover sound anchors at the mover");
    };
    assert_eq!(key, crate::sound_events::entity_key(door));
    assert_eq!(
        level.voices(&open),
        vec![point_voice(
            "door_open",
            Vec3::from_array(point) + COPY_OFFSET
        )]
    );
}

fn shot(pawn: u32, start_tick: u32) -> ShotId {
    ShotId::from_parts(pawn, start_tick, ActivationLane::Primary, 0)
}

fn impact(owner: Option<EntityId>, contacts: &[Vec3]) -> LocalSound {
    LocalSound {
        request: SoundRequest {
            bus: "sfx".to_string(),
            sound: "rifle_impact".to_string(),
            looping: false,
            anchor: Some(SoundAnchor::Contacts(
                contacts.iter().map(|contact| contact.to_array()).collect(),
            )),
        },
        shot_owner: owner,
    }
}

// An impact ghosts only under a style carrying both sound and impact
// sound, from a shot whose owner is a member; contacts outside the volume drop;
// an impact with no shot owner never ghosts; an owner outside the volume
// admits none.
#[test]
fn impacts_ghost_only_from_a_member_owners_shot_under_impact_audio_and_drop_outside_contacts() {
    let inside = Vec3::new(6.0, 0.0, 0.0);
    let outside = Vec3::new(20.0, 0.0, 0.0);

    let mut level = Level::new("impacts");
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    let voices = level.voices(&impact(Some(partner), &[inside, outside]));
    assert_eq!(
        voices,
        vec![SoundRequest {
            bus: GHOST_BUS.to_string(),
            sound: "rifle_impact".to_string(),
            looping: false,
            anchor: Some(SoundAnchor::Contacts(vec![
                (inside + COPY_OFFSET).to_array()
            ])),
        }],
        "the inside contact, mapped; the outside one dropped"
    );
    assert!(level.voices(&impact(Some(partner), &[outside])).is_empty());
    assert!(
        level.voices(&impact(None, &[inside])).is_empty(),
        "no shot id, no owner"
    );
    level.place(partner, Vec3::new(-3.0, 0.0, 0.0));
    assert!(
        level.voices(&impact(Some(partner), &[inside])).is_empty(),
        "an owner outside the volume is no member"
    );

    let mut level = Level::new("echo");
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    assert!(
        level.voices(&impact(Some(partner), &[inside])).is_empty(),
        "`audio` without `impactAudio` never ghosts an impact"
    );
}

// Impact owners come from the shot ids this machine saw fired, matched
// whole. The host's own raw shot id (pawn 0) is never read as network id 0,
// and an id two owners fired under is attributed to neither, so the host's own
// impact is never attributed to another entity.
#[test]
fn shot_owners_match_whole_ids_and_never_attribute_the_hosts_impact_to_another_entity() {
    let mut level = Level::new("impacts");
    let host = level.viewer;
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::NetworkSlot);

    // The partner's pawn holds network id 0; its shot is normalized to it.
    level.ghosts.record_shot_owner(shot(0, 40), partner);
    // The host's own shot on another tick, raw: pawn 0.
    level.ghosts.record_shot_owner(shot(0, 41), host);
    assert_eq!(level.ghosts.shot_owner(Some(shot(0, 40))), Some(partner));
    assert_eq!(level.ghosts.shot_owner(Some(shot(0, 41))), Some(host));
    assert_eq!(
        level.ghosts.shot_owner(Some(shot(0, 99))),
        None,
        "never seen fired"
    );
    assert_eq!(level.ghosts.shot_owner(None), None);

    // Both fire under one whole id: the impact belongs to neither.
    level.ghosts.record_shot_owner(shot(0, 42), partner);
    level.ghosts.record_shot_owner(shot(0, 42), host);
    assert_eq!(level.ghosts.shot_owner(Some(shot(0, 42))), None);
    let owner = level.ghosts.shot_owner(Some(shot(0, 42)));
    assert!(
        level
            .voices(&impact(owner, &[Vec3::new(6.0, 0.0, 0.0)]))
            .is_empty()
    );
}

// The app records owners from this frame's fire emissions before attributing
// impacts: a same-frame hitscan impact and a later flight's impact both find
// the shooter. A level with no link records nothing.
#[test]
fn app_records_shot_owners_from_fire_emissions_on_a_level_with_links() {
    let mut app = crate::startup::lifecycle::tests::test_app();
    let shooter = EntityId::from_raw(12);
    let fire = WeaponEmission {
        sounds: None,
        action: None,
        shot_id: Some(shot(0, 7)),
        address: "activate",
        emitter: Emitter::Entity {
            id: shooter,
            origin: Vec3::ZERO,
        },
        weapon: None,
    };
    let unattributed = WeaponEmission {
        shot_id: None,
        ..fire.clone()
    };
    app.record_ghost_shot_owners(&[fire.clone(), unattributed.clone()], &[]);
    assert_eq!(
        app.ghost_links.shot_owner(Some(shot(0, 7))),
        None,
        "no link, no ledger"
    );

    let world = world("impacts");
    let mut registry = EntityRegistry::new();
    spawn_loaded_ghost_links(&mut registry, &world);
    app.ghost_links = GhostLinks::install(&world, &registry);
    app.record_ghost_shot_owners(&[fire, unattributed], &[]);
    assert_eq!(app.ghost_links.shot_owner(Some(shot(0, 7))), Some(shooter));
}

// Regression: the host records each shot under two ids, so a 256-entry ledger
// kept only 128 shots and evicted a slow projectile's fire entry mid-battle.
#[test]
fn the_shot_owner_ledger_keeps_a_long_flights_owner_through_a_busy_battle() {
    let mut level = Level::new("impacts");
    let shooter = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::NetworkSlot);
    let other = level.pawn(Vec3::new(3.0, 0.0, 0.0), DescriptorSpawnPath::NetworkSlot);
    // The slow projectile's shot, raw and normalized, then 1500 more shots.
    level.ghosts.record_shot_owner(shot(0, 1), shooter);
    level.ghosts.record_shot_owner(shot(9, 1), shooter);
    for tick in 2..1502 {
        level.ghosts.record_shot_owner(shot(0, tick), other);
        level.ghosts.record_shot_owner(shot(5, tick), other);
    }
    assert_eq!(level.ghosts.shot_owner(Some(shot(9, 1))), Some(shooter));
    assert_eq!(level.ghosts.shot_owner(Some(shot(5, 1501))), Some(other));
}

// A member's sound refused at the SFX cap admits no ghost voice;
// one admitted beside it plays its ghost right after it.
#[test]
fn a_member_sound_refused_at_the_sfx_cap_starts_no_ghost_voice() {
    let mut level = Level::new("echo");
    let partner = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::PlayerSpawn);
    let refused = level.fire_from(partner);
    let admitted = level.sound_at(
        "rifle_reload",
        &Emitter::Entity {
            id: partner,
            origin: Vec3::new(2.0, 0.0, 0.0),
        },
    );
    let sounds = vec![refused, admitted];
    let plan = GhostVoicePlan {
        voices: sounds
            .iter()
            .enumerate()
            .flat_map(|(index, sound)| {
                level
                    .voices(sound)
                    .into_iter()
                    .map(move |voice| (index, voice))
            })
            .collect(),
    };
    // The SFX bus has one free voice left and the first request finds it full.
    let mut sfx_free = 0;
    let mut played = Vec::new();
    plan.play_beside(sounds, |request, _| {
        let admitted = if request.bus == "sfx" {
            let free = sfx_free > 0 || request.sound == "rifle_reload";
            sfx_free = 0;
            free
        } else {
            true
        };
        played.push((request.bus.clone(), request.sound.clone(), admitted));
        admitted
    });
    assert_eq!(
        played,
        vec![
            ("sfx".to_string(), "rifle_fire".to_string(), false),
            ("sfx".to_string(), "rifle_reload".to_string(), true),
            (GHOST_BUS.to_string(), "rifle_reload".to_string(), true),
        ]
    );
}

// Clock level: a voice reads its member's link state at the clock
// that member presents at, so a cue for a delay-buffered remote arriving just
// after an arm admits exactly when the shooter's ghost presents armed, not at
// host-now.
#[test]
fn a_remote_members_voice_reads_armed_state_at_its_presentation_clock() {
    let mut level = Level::new("echo");
    level.set_armed(false);
    level
        .ghosts
        .record_after_fixed_tick(&level.registry, Some(100));
    level.set_armed(true);
    // The arm applied during tick 100 carries snapshot stamp 101.
    level
        .ghosts
        .record_after_fixed_tick(&level.registry, Some(101));
    let shooter = level.pawn(Vec3::new(2.0, 0.0, 0.0), DescriptorSpawnPath::NetworkSlot);
    let fire = level.fire_from(shooter);
    let client = |render: f64| ClockSource::Client {
        render_server_tick: Some(render),
        estimated_server_tick: Some(103.0),
        latest_snapshot_tick: Some(103),
    };
    assert!(
        level.voices_with(client(99.5), &fire).is_empty(),
        "the shooter still presents before the arm, though host-now is past it"
    );
    assert_eq!(level.voices_with(client(100.5), &fire).len(), 1);

    // The ghost presents on the same frame the voice admits.
    for (render, armed) in [(99.5, false), (100.5, true)] {
        level.ghosts.update(
            &level.registry,
            GhostFrame {
                clocks: PresentationClocks::compute(client(render), 0.0),
                alpha: 0.0,
                elapsed_seconds: FRAME,
                view: CandidateView::Authoritative,
                stamps: PoseStamps::None,
            },
        );
        let targeted = level
            .ghosts
            .members()
            .iter()
            .any(|member| member.entity == shooter && !member.frozen);
        assert_eq!(targeted, armed, "render tick {render}");
    }
}

// Regression: an impact's owner was judged at its simulated pose, so on the host
// seat a client pawn's impact and its delayed ghost disagreed near the volume's
// edge for the presentation delay. The owner is judged where its ghost presents.
#[test]
fn an_impact_owner_on_the_host_seat_is_judged_at_its_presented_pose() {
    let inside = Vec3::new(2.0, 0.0, 0.0);
    let outside = Vec3::new(-3.0, 0.0, 0.0);
    let contact = Vec3::new(6.0, 0.0, 0.0);
    for (presented, simulated, ghosts) in [(outside, inside, false), (inside, outside, true)] {
        let mut level = Level::new("impacts");
        let partner = level.pawn(presented, DescriptorSpawnPath::NetworkSlot);
        let mut owners = crate::netcode::MovementOwners::default();
        owners.set_for_test(partner, 8);
        let mut allocator = crate::netcode::NetworkIdAllocator::for_test();
        allocator.stamp(partner);
        let mut buffer = crate::netcode::RemoteInterpolationBuffer::default();
        for tick in 1..=4 {
            crate::netcode::host_record_client_pawn_poses(
                &mut buffer,
                &owners,
                &allocator,
                &level.registry,
                tick,
            );
        }
        // The host simulated the pawn on; the seat still presents it delayed.
        level.place(partner, simulated);
        let frame = VoiceFrame {
            clocks: PresentationClocks::compute(ClockSource::Host { tick: 5 }, 0.0),
            view: CandidateView::Authoritative,
            stamps: PoseStamps::HostSeat {
                buffer: &buffer,
                allocator: &allocator,
                tick: 5,
                alpha: 0.0,
            },
        };
        let mut out = Vec::new();
        level.ghosts.ghost_voices(
            &level.registry,
            &frame,
            &impact(Some(partner), &[contact]),
            &mut out,
        );
        assert_eq!(
            !out.is_empty(),
            ghosts,
            "presented at {presented}, simulated at {simulated}"
        );
    }
}

#[test]
fn a_point_anchored_or_unanchored_sound_never_ghosts() {
    let level = Level::new("impacts");
    for anchor in [None, Some(SoundAnchor::Point([2.0, 0.0, 0.0]))] {
        let sound: LocalSound = SoundRequest {
            bus: "sfx".to_string(),
            sound: "teleport_arrive".to_string(),
            looping: false,
            anchor,
        }
        .into();
        assert!(level.voices(&sound).is_empty());
    }
}
