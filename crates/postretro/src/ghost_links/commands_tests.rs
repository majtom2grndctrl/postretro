// Ghost-link script steps end to end: registered primitives and the real sequence dispatcher,
// next-tick apply, history and per-frame membership, and per-field look resolution.
// See: context/lib/scripting.md §12 (Entity addressing) · context/lib/entity_model.md §7d

use std::collections::BTreeSet;

use glam::Vec3;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, NamedReaction, ReactionDescriptor, SequenceStep,
    SequenceTarget, Transform,
};
use postretro_foundation::{
    ARM_GHOST_LINK, DISARM_GHOST_LINK, DescriptorError, GhostStyle, GhostStyleDescriptor,
    GhostStyleRegistry, SET_GHOST_LINK_COLOR_FILTER, SET_GHOST_LINK_MEMBERS,
};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
use postretro_level_loader::{CellLocatorChild, LevelWorld};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_test_log_capture::LogCapture;
use serde_json::{Value, json};

use super::super::clocks::{ClockSource, PoseStamps, PresentationClocks};
use super::super::eligibility::{CandidateView, GhostCategory};
use super::super::membership::ramp_step;
use super::super::opt_ins_test_fixtures::{data_registry, descriptor, projectile_weapon};
use super::super::{GhostFrame, GhostLinks, GhostMember, spawn_loaded_ghost_links};
use super::*;

const FRAME: f32 = 1.0 / 240.0;
const FRAMES_PER_TICK: usize = 4;
const VOLUME: (Vec3, Vec3) = (Vec3::new(0.0, 0.0, -5.0), Vec3::new(10.0, 4.0, 5.0));
const INSIDE: Vec3 = Vec3::new(2.0, 1.0, 0.0);
const ALSO_INSIDE: Vec3 = Vec3::new(4.0, 1.0, 0.0);
const OUTSIDE: Vec3 = Vec3::new(-20.0, 1.0, 0.0);

const ECHO: GhostStyle = GhostStyle {
    tint: [0.6, 0.8, 1.0],
    opacity: 0.45,
    saturation: 0.2,
    audio: true,
    impact_audio: false,
};

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

fn world(armed: bool, members: GhostMembers) -> LevelWorld {
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
        enabled_on_spawn: armed,
        members,
        style: "echo".to_string(),
        brushes: vec![box_brush(VOLUME)],
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        target_origin: [100.0, 0.0, 0.0],
        target_yaw: 0.0,
        ghost_tags: Vec::new(),
    }];
    world
}

fn styles(echo: GhostStyle) -> GhostStyleRegistry {
    GhostStyleRegistry::from_declarations(
        "test",
        [Ok::<_, DescriptorError>(GhostStyleDescriptor {
            name: "echo".to_string(),
            style: echo,
        })],
    )
}

/// One installed link driven only through the registered member primitives,
/// as a level script's reactions drive it on the host.
struct Host {
    ctx: ScriptCtx,
    ghosts: GhostLinks,
    steps: SequencedPrimitiveRegistry,
    link: EntityId,
    viewer: EntityId,
    /// Script time, advanced per presented frame unless paused.
    time: f64,
}

impl Host {
    fn new(armed: bool, members: GhostMembers) -> Self {
        let world = world(armed, members);
        let ctx = ScriptCtx::new();
        spawn_loaded_ghost_links(&mut ctx.registry.borrow_mut(), &world);
        let mut ghosts = GhostLinks::install(&world, &ctx.registry.borrow());
        ghosts.bind_styles(&world, &styles(ECHO));
        ghosts.refresh_opt_ins(&data_registry(vec![
            descriptor("player", true),
            descriptor("wraith", true),
            projectile_weapon("launcher", true),
        ]));
        let link = ctx
            .registry
            .borrow()
            .iter_with_kind(ComponentKind::GhostLink)
            .find_map(|(id, value)| matches!(value, ComponentValue::GhostLink(_)).then_some(id))
            .expect("install spawned the link");
        let mut steps = SequencedPrimitiveRegistry::new();
        postretro_sim::ghost_link_commands::register_sequenced_ghost_link_primitives(
            &mut steps,
            ctx.clone(),
        );
        let mut host = Self {
            ctx,
            ghosts,
            steps,
            link,
            viewer: EntityId::from_raw(0),
            time: 0.0,
        };
        host.viewer = host.pawn(OUTSIDE, DescriptorSpawnPath::PlayerSpawn);
        host.ctx
            .registry
            .borrow_mut()
            .mark_local_player_pawn(host.viewer)
            .unwrap();
        host
    }

    fn place(&self, id: EntityId, at: Vec3) {
        self.ctx
            .registry
            .borrow_mut()
            .set_presentation_transform(
                id,
                Transform {
                    position: at,
                    ..Default::default()
                },
            )
            .unwrap();
    }

    fn spawn(&self, at: Vec3, name: &str, spawn_path: DescriptorSpawnPath) -> EntityId {
        let id = self.ctx.registry.borrow_mut().spawn(Transform::default());
        self.place(id, at);
        self.ctx
            .registry
            .borrow_mut()
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
        id
    }

    fn pawn(&self, at: Vec3, spawn_path: DescriptorSpawnPath) -> EntityId {
        let id = self.spawn(at, "player", spawn_path);
        self.ctx
            .registry
            .borrow_mut()
            .set_component(
                id,
                postretro_foundation::PlayerMovementComponent::from_descriptor(
                    &crate::mod_digest::tests::movement_descriptor(),
                ),
            )
            .unwrap();
        id
    }

    fn projectile(&self, owner: EntityId, at: Vec3) -> EntityId {
        let id = self.ctx.registry.borrow_mut().spawn(Transform::default());
        self.place(id, at);
        self.ctx
            .registry
            .borrow_mut()
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
                    credit_source: "launcher".to_string(),
                    owner_pawn: owner,
                    owner_weapon: owner,
                    spawned: false,
                    predicted_shot_id: None,
                    elapsed_flight_age: 0.0,
                    flipbook_active: false,
                    impact_light: None,
                    splash: None,
                    source_weapon: Some("launcher".to_string()),
                    activation: None,
                },
            )
            .unwrap();
        id
    }

    /// A member step, run as the sequence dispatcher runs it.
    fn step(&self, primitive: &str, args: Value) {
        self.steps.get(primitive).expect("registered")(self.link, &args).expect("a valid step");
    }

    /// One completed authoritative tick in single player: its start applies
    /// the queued steps, its end records the link state.
    fn tick(&mut self) {
        self.ctx.presentation_script_time.set(self.time);
        self.ghosts.apply_pending_commands(&self.ctx);
        self.ghosts
            .record_after_fixed_tick(&self.ctx.registry.borrow(), None);
    }

    /// One presented frame; `advance` moves script time (false: paused).
    fn present(&mut self, advance: bool) {
        if advance {
            self.time += f64::from(FRAME);
        }
        self.ctx.presentation_script_time.set(self.time);
        self.ghosts.set_look_time(self.time);
        let completed_ticks = self.ghosts.completed_ticks();
        self.ghosts.update(
            &self.ctx.registry.borrow(),
            GhostFrame {
                clocks: PresentationClocks::compute(
                    ClockSource::SinglePlayer { completed_ticks },
                    0.5,
                ),
                alpha: 0.5,
                elapsed_seconds: FRAME,
                view: CandidateView::Authoritative,
                stamps: PoseStamps::None,
            },
        );
    }

    fn frame(&mut self) {
        self.present(true);
    }

    /// One tick followed by its presented frames.
    fn tick_and_present(&mut self) {
        self.tick();
        for _ in 0..FRAMES_PER_TICK {
            self.frame();
        }
    }

    fn ghost(&self, entity: EntityId) -> Option<GhostMember> {
        self.ghosts
            .members()
            .iter()
            .find(|member| member.entity == entity)
            .copied()
    }

    fn membership(&self, entity: EntityId) -> f32 {
        self.ghost(entity).map_or(0.0, |ghost| ghost.membership)
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

// AC13 / P3: `arm` on tick N and `disarm` on tick N+1 ramp presented
// membership up and back down at the rate cap, through the real steps.
#[test]
fn arm_then_disarm_on_the_next_tick_ramps_up_and_back_down_at_the_rate_cap() {
    let mut host = Host::new(false, GhostMembers::all());
    let partner = host.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
    host.frame(); // the install frame
    let step = ramp_step(FRAME);

    host.step(ARM_GHOST_LINK, json!({}));
    host.tick();
    let mut previous = 0.0;
    for frame in 0..FRAMES_PER_TICK {
        host.frame();
        let membership = host
            .ghost(partner)
            .expect("armed: the partner ghosts")
            .membership;
        let expected = if frame == 0 { 0.0 } else { previous + step };
        assert!(close(membership, expected), "frame {frame}: {membership}");
        previous = membership;
    }
    assert!(
        previous > 0.0 && previous < 1.0,
        "a one-tick rise never reaches full"
    );

    host.step(DISARM_GHOST_LINK, json!({}));
    host.tick();
    for _ in 0..FRAMES_PER_TICK {
        host.frame();
        let membership = host.membership(partner);
        assert!(
            close(membership, (previous - step).max(0.0)),
            "{membership}"
        );
        previous = membership;
    }
    assert_eq!(previous, 0.0, "the fall lands on 0 at the rate cap");
}

// AC13 / P2: `arm` and `disarm` in one tick start no ramp and present no ghost.
#[test]
fn arm_and_disarm_in_one_tick_present_no_ghost() {
    let mut host = Host::new(false, GhostMembers::all());
    let partner = host.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
    host.frame();
    host.step(ARM_GHOST_LINK, json!({}));
    host.step(DISARM_GHOST_LINK, json!({}));
    for _ in 0..3 {
        host.tick_and_present();
        assert!(host.ghost(partner).is_none(), "no frame presents a ghost");
    }
}

// AC18: with one category off, that category's members fade at the rate cap
// and the other categories' members stay, for players, entities and
// projectiles (movers are Task 7's draw).
#[test]
fn a_category_turned_off_fades_its_members_while_the_others_stay() {
    for (field, off) in [
        ("players", GhostCategory::Players),
        ("entities", GhostCategory::Entities),
        ("projectiles", GhostCategory::Projectiles),
    ] {
        let mut host = Host::new(true, GhostMembers::all());
        let partner = host.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
        let wraith = host.spawn(ALSO_INSIDE, "wraith", DescriptorSpawnPath::MapPlacement);
        let rocket = host.projectile(partner, INSIDE);
        let members = [
            (partner, GhostCategory::Players),
            (wraith, GhostCategory::Entities),
            (rocket, GhostCategory::Projectiles),
        ];
        host.frame();
        for (entity, category) in members {
            let ghost = host
                .ghost(entity)
                .expect("every category ghosts at install");
            assert_eq!((ghost.category, ghost.membership), (category, 1.0));
        }
        host.step(SET_GHOST_LINK_MEMBERS, json!({ field: false }));
        host.tick_and_present();
        for (entity, category) in members {
            let membership = host.membership(entity);
            if category == off {
                assert!(
                    close(membership, 1.0 - FRAMES_PER_TICK as f32 * ramp_step(FRAME)),
                    "{field}: fades at the rate cap, got {membership}"
                );
                assert!(host.ghost(entity).unwrap().frozen);
            } else {
                assert_eq!(membership, 1.0, "{field}: {category:?} stays");
            }
        }
    }
}

// AC19 / P44 / P45: the map's `ghost_members` default holds from the first
// frame; off then on in one tick ends in the last call's state with no ramp;
// `setMembers` and `arm` on one tick present together; a flip mid-ramp
// reverses from the presented value.
#[test]
fn member_categories_follow_the_map_default_then_the_last_call_of_each_tick() {
    let mut host = Host::new(true, GhostMembers::ENTITIES);
    let partner = host.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
    let wraith = host.spawn(ALSO_INSIDE, "wraith", DescriptorSpawnPath::MapPlacement);
    host.frame();
    assert_eq!(host.membership(wraith), 1.0, "entities shown from frame 1");
    assert!(host.ghost(partner).is_none(), "players hidden from frame 1");

    host.step(SET_GHOST_LINK_MEMBERS, json!({ "entities": false }));
    host.step(SET_GHOST_LINK_MEMBERS, json!({ "entities": true }));
    host.tick_and_present();
    assert_eq!(host.membership(wraith), 1.0, "off then on: no ramp");
    assert!(!host.ghost(wraith).unwrap().frozen);

    // A disarmed link: showing players and arming on one tick present together.
    let mut host = Host::new(false, GhostMembers::ENTITIES);
    let partner = host.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
    host.frame();
    host.step(SET_GHOST_LINK_MEMBERS, json!({ "players": true }));
    host.step(ARM_GHOST_LINK, json!({}));
    host.tick();
    host.frame();
    assert!(host.ghost(partner).is_some(), "both apply on one frame");
    for _ in 1..FRAMES_PER_TICK {
        host.frame();
    }
    let risen = host.membership(partner);
    assert!(risen > 0.0);

    // Flipped off mid-ramp: falls from the presented value.
    host.step(SET_GHOST_LINK_MEMBERS, json!({ "players": false }));
    host.tick();
    host.frame();
    assert!(close(host.membership(partner), risen - ramp_step(FRAME)));
}

// AC27 / AC29: `setColorFilter` overrides the style field by field; a
// `periodMs` ramp presents its midpoint halfway through; `periodMs` 0 lands in
// one frame. P42: a pause freezes the ramp while the membership ramp runs on.
#[test]
fn color_filter_overrides_the_style_per_field_on_script_time() {
    let mut host = Host::new(false, GhostMembers::all());
    let partner = host.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
    host.frame();
    assert_eq!(
        host.ghosts.link_look(0),
        ECHO.look(),
        "the style before any filter"
    );

    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "opacity": 1.0, "periodMs": 1000 }),
    );
    host.step(SET_GHOST_LINK_COLOR_FILTER, json!({ "saturation": 1.0 }));
    host.step(ARM_GHOST_LINK, json!({}));
    let start = host.time;
    host.tick();
    host.present(false);
    let look = host.ghosts.link_look(0);
    assert_eq!(
        look.opacity, ECHO.opacity,
        "the ramp starts at the style value"
    );
    assert_eq!(look.saturation, 1.0, "periodMs 0 lands on the first frame");
    assert_eq!(look.tint, ECHO.tint, "an unset field keeps the style");

    // Paused: script time holds, so the filter ramp holds; membership rises.
    let before = host.membership(partner);
    for _ in 0..4 {
        host.present(false);
    }
    assert_eq!(
        host.ghosts.link_look(0).opacity,
        ECHO.opacity,
        "P42: frozen"
    );
    assert!(
        host.membership(partner) > before,
        "P42: the membership ramp runs on"
    );

    while host.time < start + 0.5 {
        host.frame();
    }
    let elapsed = (host.time - start) as f32;
    let midpoint = ECHO.opacity + (1.0 - ECHO.opacity) * elapsed;
    assert!(
        close(host.ghosts.link_look(0).opacity, midpoint),
        "halfway: {} vs {midpoint}",
        host.ghosts.link_look(0).opacity
    );
    while host.time < start + 1.0 {
        host.frame();
    }
    assert_eq!(host.ghosts.link_look(0).opacity, 1.0);
}

// P16 / AC8: a committed reload that changes the style keeps the fields
// `setColorFilter` set and their ramps; unset fields follow the new style.
#[test]
fn a_committed_style_reload_keeps_filter_fields_and_ramps() {
    let mut host = Host::new(false, GhostMembers::all());
    host.frame();
    host.step(
        SET_GHOST_LINK_COLOR_FILTER,
        json!({ "tint": [0.0, 0.0, 0.0], "periodMs": 1000 }),
    );
    let start = host.time;
    host.tick();
    while host.time < start + 0.25 {
        host.frame();
    }
    let retuned = GhostStyle {
        tint: [0.1, 0.1, 0.1],
        opacity: 0.9,
        saturation: 0.7,
        ..ECHO
    };
    let before = host.ghosts.link_look(0).tint;
    host.ghosts.rebind_styles(&styles(retuned));
    let look = host.ghosts.link_look(0);
    assert_eq!(
        look.tint, before,
        "the tint ramp still runs from its own from-value"
    );
    assert_eq!(
        (look.opacity, look.saturation),
        (0.9, 0.7),
        "unset fields follow the reloaded style"
    );
    while host.time < start + 1.0 {
        host.frame();
    }
    assert_eq!(
        host.ghosts.link_look(0).tint,
        [0.0; 3],
        "the ramp reaches its target"
    );
}

// AC27 / AC29 / P41 through the real sequence dispatcher: a malformed step
// warns with its reason and is dropped alone; its siblings apply.
#[test]
fn a_dispatched_malformed_ghost_link_step_warns_and_drops_only_that_step() {
    let capture = LogCapture::start();
    let mut host = Host::new(false, GhostMembers::all());
    host.frame();
    let step = |primitive: &str, args: Value| SequenceStep {
        id: SequenceTarget::Entity(host.link),
        primitive: primitive.to_string(),
        args,
    };
    let mut data = postretro_entities::DataRegistry::new();
    data.reactions.push(NamedReaction {
        name: "echo.reveal".to_string(),
        descriptor: ReactionDescriptor::Sequence(vec![
            step(ARM_GHOST_LINK, json!({})),
            step(SET_GHOST_LINK_COLOR_FILTER, json!({ "periodMs": 1500 })),
            step(SET_GHOST_LINK_COLOR_FILTER, json!({ "opacity": 1.5 })),
            step(SET_GHOST_LINK_MEMBERS, json!({ "movers": "no" })),
            step(SET_GHOST_LINK_COLOR_FILTER, json!({ "opacity": 0.8 })),
        ]),
    });
    let _chained = postretro_scripting_core::reaction_dispatch::fire_named_event_with_sequences(
        "echo.reveal",
        &data,
        &host.steps,
        &ReactionPrimitiveRegistry::new(),
        &SystemReactionRegistry::new(),
        &host.ctx,
        None,
    );
    for reason in [
        "setColorFilter names no field; expected one or more of tint, opacity or saturation",
        "setColorFilter `opacity` must be a finite number in 0..=1, got 1.5",
        "setMembers `movers` must be a boolean, got \"no\"",
    ] {
        capture.assert_logged_once(log::Level::Warn, reason);
    }
    host.tick();
    host.frame();
    let link = *host
        .ctx
        .registry
        .borrow()
        .get_component::<postretro_entities::GhostLinkComponent>(host.link)
        .unwrap();
    assert!(link.armed, "the valid sibling steps applied");
    assert_eq!(link.members, GhostMembers::all().bits());
    assert_eq!(host.ghosts.link_look(0).opacity, 0.8);
}
