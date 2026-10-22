// Trap stage: requests, repeat schedules, the shared weapon gate, shot launch and attribution.
// See: context/lib/entity_model.md §Components (ProjectileTrap)

use std::cell::RefCell;
use std::rc::Rc;

use glam::{Quat, Vec3};
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::projectile_trap::{ProjectileTrapComponent, TrapRequest};
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::{
    ActivationTrigger, AmmoResource, CellResource, HeatResource, ResolutionMode, SplashDescriptor,
    WeaponResource, activation_duration_ticks,
};
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;
use postretro_test_log_capture::LogCapture;

use super::install::resolve_traps_for_level;
use super::trap_test_fixtures::{action, bolt, entity, weapon};
use super::{issue_trap_request, run_trap_stage};
use crate::collision::CollisionWorld;
use crate::scripting::map_entity::MapEntity;
use crate::scripting_systems::hit_zones::HitZoneStore;

const DT: f32 = 1.0 / 60.0;

fn ms_ticks(ms: f64) -> u32 {
    crate::scripting_systems::reaction_scheduler::ms_to_ticks(ms)
}

/// One host's registry of traps, stepped one trap stage per tick.
struct Rig {
    registry: EntityRegistry,
    descriptors: Vec<EntityTypeDescriptor>,
    tick: u32,
    /// `(tick, trap, projectile)` for every launched shot, in launch order.
    shots: Vec<(u32, EntityId, EntityId)>,
}

impl Rig {
    fn new(descriptors: Vec<EntityTypeDescriptor>) -> Self {
        Self {
            registry: EntityRegistry::new(),
            descriptors,
            tick: 0,
            shots: Vec::new(),
        }
    }

    fn trap_at(
        &mut self,
        weapon: &str,
        interval_ms: f64,
        phase_ms: f64,
        start_on_spawn: bool,
        transform: Transform,
    ) -> EntityId {
        let id = self.registry.spawn(transform);
        self.registry
            .set_component(
                id,
                ProjectileTrapComponent::new(
                    weapon.to_string(),
                    interval_ms,
                    phase_ms,
                    start_on_spawn,
                ),
            )
            .unwrap();
        id
    }

    fn trap(&mut self, weapon: &str, interval_ms: f64, phase_ms: f64) -> EntityId {
        self.trap_at(weapon, interval_ms, phase_ms, false, Transform::default())
    }

    fn install(&mut self) {
        resolve_traps_for_level(&mut self.registry, &self.descriptors, true);
    }

    fn issue(&mut self, trap: EntityId, request: TrapRequest) {
        issue_trap_request(&mut self.registry, trap, request);
    }

    fn fire(&mut self, trap: EntityId) {
        self.issue(trap, TrapRequest::Fire);
    }

    fn start(&mut self, trap: EntityId) {
        self.start_with(trap, None, None);
    }

    fn start_with(&mut self, trap: EntityId, interval_ms: Option<f64>, phase_ms: Option<f64>) {
        self.issue(
            trap,
            TrapRequest::Start {
                interval_ms,
                phase_ms,
            },
        );
    }

    fn stop(&mut self, trap: EntityId) {
        self.issue(trap, TrapRequest::Stop);
    }

    /// One trap stage. Returns the number of shots it launched.
    fn step(&mut self) -> usize {
        let output = run_trap_stage(&mut self.registry, DT);
        for spawn in &output.projectile_spawns {
            let owner = self
                .registry
                .get_component::<ProjectileComponent>(spawn.projectile)
                .unwrap()
                .owner_pawn;
            self.shots.push((self.tick, owner, spawn.projectile));
        }
        self.tick += 1;
        output.projectile_spawns.len()
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step();
        }
    }

    fn shot_ticks(&self, trap: EntityId) -> Vec<u32> {
        self.shots
            .iter()
            .filter(|(_, owner, _)| *owner == trap)
            .map(|(tick, _, _)| *tick)
            .collect()
    }

    /// Ticks on which an execution started: each run of shots begins one.
    fn trap_component(&self, trap: EntityId) -> ProjectileTrapComponent {
        self.registry
            .get_component::<ProjectileTrapComponent>(trap)
            .unwrap()
            .clone()
    }

    fn running(&self, trap: EntityId) -> bool {
        self.trap_component(trap)
            .runtime
            .is_some_and(|runtime| runtime.schedule.is_some())
    }
}

fn single_bolt_rig(recovery_ms: f32) -> (Rig, EntityId) {
    let mut rig = Rig::new(vec![entity("bolt", bolt(recovery_ms))]);
    let trap = rig.trap("bolt", 1000.0, 0.0);
    rig.install();
    (rig, trap)
}

/// The tick offset after a shot at which a `recovery_ms` recovery first admits
/// a start, read from the shared gate rather than restated.
fn recovery_ticks(recovery_ms: f32) -> u32 {
    let (mut rig, trap) = single_bolt_rig(recovery_ms);
    rig.fire(trap);
    rig.step();
    for offset in 1..1000 {
        rig.fire(trap);
        if rig.step() == 1 {
            return offset;
        }
    }
    panic!("recovery never ended");
}

#[test]
fn fire_launches_from_origin_along_map_facing() {
    let mut rig = Rig::new(vec![entity("bolt", bolt(100.0))]);
    let rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    let trap = rig.trap_at(
        "bolt",
        1000.0,
        0.0,
        false,
        Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation,
            scale: Vec3::ONE,
        },
    );
    rig.install();
    rig.fire(trap);
    assert_eq!(rig.step(), 1);
    let (_, _, projectile) = rig.shots[0];
    let transform = rig.registry.get_component::<Transform>(projectile).unwrap();
    assert!(
        transform
            .position
            .abs_diff_eq(Vec3::new(1.0, 2.0, 3.0), 1e-6)
    );
    let component = rig
        .registry
        .get_component::<ProjectileComponent>(projectile)
        .unwrap();
    assert!(Vec3::from(component.direction).abs_diff_eq(rotation * Vec3::NEG_Z, 1e-6));
    assert_eq!(component.speed, 12.0);
    assert_eq!(component.damage, 7.0);
    assert_eq!(component.credit_source, "trap.test-bolt");
    assert_eq!(component.owner_pawn, trap);
    assert!(component.attackerless, "a trap's shot credits no attacker");
    assert!(component.spawned, "the shot keeps its launch-tick grace");
    assert_eq!(component.source_weapon.as_deref(), Some("bolt"));
}

// The compiler hands the classname handler engine angles; each map spelling
// lands here as yaw, pitch or the `angle` sentinels' pure pitch.
#[test]
fn trap_facing_follows_angles_mangle_and_angle_sentinels() {
    let cases = [
        (Vec3::new(0.0, std::f32::consts::PI, 0.0), Vec3::Z),
        (Vec3::new(0.0, -std::f32::consts::FRAC_PI_2, 0.0), Vec3::X),
        (Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0), Vec3::Y),
        (
            Vec3::new(-std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            Vec3::NEG_Y,
        ),
    ];
    for (angles, expected) in cases {
        let mut rig = Rig::new(vec![entity("bolt", bolt(100.0))]);
        let map_entity = MapEntity {
            map_ordinal: None,
            classname: "projectile_trap".into(),
            origin: Vec3::ZERO,
            angles,
            key_values: [("weapon".to_string(), "bolt".to_string())].into(),
            tags: Vec::new(),
        };
        let trap =
            crate::scripting::builtins::projectile_trap::handle(&map_entity, &mut rig.registry)
                .unwrap();
        rig.install();
        rig.fire(trap);
        rig.step();
        let direction = Vec3::from(
            rig.registry
                .get_component::<ProjectileComponent>(rig.shots[0].2)
                .unwrap()
                .direction,
        );
        assert!(
            direction.abs_diff_eq(expected, 1e-5),
            "angles {angles} fire along {direction}, expected {expected}"
        );
    }
}

#[test]
fn fire_runs_the_authored_shot_sequence() {
    let mut rig = Rig::new(vec![entity(
        "burst",
        weapon(action(ActivationTrigger::Press, 100.0, 3, 50.0)),
    )]);
    let trap = rig.trap("burst", 1000.0, 0.0);
    rig.install();
    rig.fire(trap);
    rig.run(40);
    let gap = activation_duration_ticks(50.0);
    assert_eq!(rig.shot_ticks(trap), vec![0, gap, 2 * gap]);
}

#[test]
fn fire_during_execution_or_recovery_is_dropped() {
    let mut rig = Rig::new(vec![entity(
        "burst",
        weapon(action(ActivationTrigger::Press, 100.0, 3, 50.0)),
    )]);
    let trap = rig.trap("burst", 1000.0, 0.0);
    rig.install();
    rig.fire(trap);
    rig.step();
    // Every later tick of the execution and its recovery asks again.
    let last = 2 * activation_duration_ticks(50.0);
    while rig.tick < last + recovery_ticks(100.0) {
        rig.fire(trap);
        rig.step();
    }
    assert_eq!(
        rig.shot_ticks(trap).len(),
        3,
        "no shot joins the running sequence"
    );
    rig.fire(trap);
    rig.step();
    assert_eq!(
        rig.shot_ticks(trap).len(),
        4,
        "a fire after recovery starts the next execution"
    );
}

// Duplicate requests on one tick are one execution.
#[test]
fn two_fires_on_one_tick_give_one_execution() {
    let (mut rig, trap) = single_bolt_rig(100.0);
    rig.fire(trap);
    rig.fire(trap);
    rig.run(3);
    assert_eq!(rig.shot_ticks(trap), vec![0]);
}

#[test]
fn resource_weapons_never_run_dry_overheat_or_reload() {
    let mut ammo = bolt(20.0);
    ammo.resource = Some(WeaponResource::Ammo(AmmoResource {
        ammo_type: "cells".into(),
        magazine: 2,
        cost_per_shot: 1,
        reserve: 0,
        reload_ms: 500,
        reload_style: postretro_foundation::ReloadStyle::Magazine,
    }));
    let mut heat = bolt(20.0);
    heat.resource = Some(WeaponResource::Heat(HeatResource {
        heat_per_shot: 50.0,
        overheat_at: 100.0,
        cool_per_second: 1.0,
        cool_delay_ms: 10_000.0,
        overheat_behavior: Default::default(),
    }));
    let mut cell = bolt(20.0);
    cell.resource = Some(WeaponResource::Cell(CellResource {
        capacity: 10.0,
        cost_per_shot: 10.0,
        regen_per_second: 0.001,
        regen_delay_ms: 10_000.0,
    }));
    let mut rig = Rig::new(vec![
        entity("ammo", ammo),
        entity("heat", heat),
        entity("cell", cell),
    ]);
    let traps = ["ammo", "heat", "cell"].map(|name| rig.trap(name, 1000.0, 0.0));
    rig.install();
    let spacing = recovery_ticks(20.0);
    for _ in 0..10 {
        for trap in traps {
            rig.fire(trap);
        }
        let output = run_trap_stage(&mut rig.registry, DT);
        assert_eq!(output.projectile_spawns.len(), 3, "every trap fires");
        assert!(
            output
                .emissions
                .iter()
                .all(|emission| emission.address == "activate"),
            "no dry fire, overheat or reload event"
        );
        rig.run(spacing);
    }
    for trap in traps {
        let weapon = rig.trap_component(trap).runtime.unwrap().weapon;
        assert!(weapon.ammo.is_none() && weapon.heat.is_none() && weapon.cell.is_none());
    }
}

#[test]
fn inert_weapons_warn_once_and_valid_projectile_arms() {
    let mut hitscan = bolt(100.0);
    hitscan.resolution = ResolutionMode::Hitscan;
    hitscan.projectile = None;
    let mut charged = bolt(100.0);
    charged.primary.charge = Some(
        serde_json::from_value(serde_json::json!({ "minMs": 100, "fullMs": 500 }))
            .expect("charge parses"),
    );
    let mut rig = Rig::new(vec![
        entity("bolt", bolt(100.0)),
        entity("hitscan", hitscan),
        entity("charged", charged),
    ]);
    let valid = rig.trap("bolt", 1000.0, 0.0);
    let inert = ["missing", "hitscan", "charged"].map(|name| rig.trap(name, 1000.0, 0.0));
    let capture = LogCapture::start();
    let report = resolve_traps_for_level(&mut rig.registry, &rig.descriptors, true);
    assert_eq!((report.armed, report.inert), (1, 3));
    for (name, reason) in [
        ("missing", "names no weapon descriptor"),
        ("hitscan", "is not a projectile weapon"),
        ("charged", "has a charged primary action"),
    ] {
        capture.assert_logged_once(log::Level::Warn, &format!("weapon `{name}` {reason}"));
    }
    assert!(rig.trap_component(valid).is_armed());
    for trap in inert {
        assert!(!rig.trap_component(trap).is_armed());
    }
}

// An inert trap takes no request from any source.
#[test]
fn inert_trap_ignores_every_request() {
    let mut rig = Rig::new(Vec::new());
    let trap = rig.trap_at("missing", 1000.0, 0.0, true, Transform::default());
    let timing_inert = rig.trap("missing", 1000.0, 0.0);
    rig.registry
        .set_component(
            timing_inert,
            ProjectileTrapComponent {
                timing_valid: false,
                ..rig.trap_component(timing_inert)
            },
        )
        .unwrap();
    rig.install();
    for target in [trap, timing_inert] {
        rig.fire(target);
        rig.start(target);
    }
    rig.run(120);
    assert!(rig.shots.is_empty());
    assert!(!rig.running(trap) && !rig.running(timing_inert));
}

#[test]
fn start_on_spawn_starts_only_on_an_authoritative_install() {
    for authoritative in [true, false] {
        let mut rig = Rig::new(vec![entity("bolt", bolt(20.0))]);
        let trap = rig.trap_at("bolt", 100.0, 50.0, true, Transform::default());
        let silent = rig.trap("bolt", 100.0, 0.0);
        resolve_traps_for_level(&mut rig.registry, &rig.descriptors, authoritative);
        rig.run(60);
        if authoritative {
            let first = ms_ticks(50.0);
            let interval = ms_ticks(100.0);
            assert_eq!(
                rig.shot_ticks(trap)[..3],
                [first, first + interval, first + 2 * interval]
            );
        } else {
            assert!(rig.shot_ticks(trap).is_empty(), "a client never starts it");
        }
        assert!(rig.shot_ticks(silent).is_empty(), "false stays silent");
    }
}

// `start_on_spawn` is the install's first request, so a `levelLoad`
// `stop` wins and a `levelLoad` `start(opts)` finds it running.
#[test]
fn start_on_spawn_precedes_level_load_requests() {
    let mut rig = Rig::new(vec![entity("bolt", bolt(20.0))]);
    let stopped = rig.trap_at("bolt", 100.0, 0.0, true, Transform::default());
    let restarted = rig.trap_at("bolt", 100.0, 0.0, true, Transform::default());
    rig.install();
    rig.stop(stopped);
    rig.start_with(restarted, Some(250.0), Some(30.0));
    rig.run(30);
    assert!(rig.shot_ticks(stopped).is_empty());
    assert!(!rig.running(stopped));
    let interval = ms_ticks(100.0);
    assert_eq!(rig.shot_ticks(restarted)[..2], [0, interval]);
}

#[test]
fn schedule_fires_n_executions_over_n_intervals_after_its_phase() {
    for (interval_ms, phase_ms) in [(100.0, 0.0), (100.0, 40.0), (100.0, 250.0), (5.0, 0.0)] {
        let (mut rig, trap) = single_bolt_rig(0.0);
        rig.start_with(trap, Some(interval_ms), Some(phase_ms));
        let interval = ms_ticks(interval_ms);
        let phase = if phase_ms > 0.0 {
            ms_ticks(phase_ms)
        } else {
            0
        };
        let n = 5;
        rig.run(phase + n * interval);
        let expected: Vec<u32> = (0..n).map(|k| phase + k * interval).collect();
        assert_eq!(
            rig.shot_ticks(trap),
            expected,
            "interval {interval_ms} ms, phase {phase_ms} ms"
        );
    }
    assert_eq!(ms_ticks(5.0), 1, "a sub-tick interval repeats every tick");
}

#[test]
fn authored_defaults_repeat_every_rounded_second() {
    let (mut rig, trap) = single_bolt_rig(100.0);
    rig.run(200);
    assert!(rig.shots.is_empty(), "silent until started");
    rig.start(trap);
    rig.run(3 * ms_ticks(1000.0));
    let start = 200;
    let second = ms_ticks(1000.0);
    assert_eq!(
        rig.shot_ticks(trap),
        vec![start, start + second, start + 2 * second]
    );
}

// `start` and `stop` apply in issue order, and options apply to one run only.
#[test]
fn start_stop_rules_follow_issue_order_and_per_run_options() {
    let interval = ms_ticks(100.0);
    // `start` on a running trap changes neither rate nor phase.
    let (mut rig, trap) = single_bolt_rig(0.0);
    rig.start_with(trap, Some(100.0), Some(0.0));
    rig.run(3);
    rig.start_with(trap, Some(30.0), Some(0.0));
    rig.run(3 * interval);
    assert_eq!(rig.shot_ticks(trap)[..3], [0, interval, 2 * interval]);

    // stop → start(opts) on one tick re-anchors with the new values.
    let (mut rig, trap) = single_bolt_rig(0.0);
    rig.start_with(trap, Some(100.0), Some(0.0));
    rig.run(4);
    rig.stop(trap);
    rig.start_with(trap, Some(50.0), Some(0.0));
    rig.run(2 * ms_ticks(50.0) + 1);
    assert_eq!(
        rig.shot_ticks(trap),
        vec![0, 4, 4 + ms_ticks(50.0), 4 + 2 * ms_ticks(50.0)]
    );

    // start → stop on one tick fires nothing and leaves it stopped.
    let (mut rig, trap) = single_bolt_rig(0.0);
    rig.start(trap);
    rig.stop(trap);
    rig.run(100);
    assert!(rig.shots.is_empty() && !rig.running(trap));

    // After an options run stops, a bare start runs at the authored values,
    // and each omitted field takes its authored value.
    let mut rig = Rig::new(vec![entity("bolt", bolt(0.0))]);
    let trap = rig.trap("bolt", 100.0, 50.0);
    rig.install();
    rig.start_with(trap, Some(30.0), Some(0.0));
    rig.run(2);
    rig.stop(trap);
    rig.step();
    let base = rig.tick;
    rig.start(trap);
    rig.run(ms_ticks(50.0) + interval + 1);
    assert_eq!(
        rig.shot_ticks(trap)[1..],
        [base + ms_ticks(50.0), base + ms_ticks(50.0) + interval]
    );
    for (options, first, gap) in [
        ((None, Some(0.0)), 0, interval),
        ((Some(30.0), None), ms_ticks(50.0), ms_ticks(30.0)),
    ] {
        let mut rig = Rig::new(vec![entity("bolt", bolt(0.0))]);
        let trap = rig.trap("bolt", 100.0, 50.0);
        rig.install();
        rig.start_with(trap, options.0, options.1);
        rig.run(first + gap + 1);
        assert_eq!(
            rig.shot_ticks(trap)[..2],
            [first, first + gap],
            "{options:?}"
        );
    }
}

#[test]
fn stop_halts_repeats_and_lets_the_inflight_execution_finish() {
    let mut rig = Rig::new(vec![entity(
        "burst",
        weapon(action(ActivationTrigger::Press, 20.0, 3, 50.0)),
    )]);
    let trap = rig.trap("burst", 2000.0, 0.0);
    rig.install();
    rig.start(trap);
    rig.step();
    rig.stop(trap);
    rig.run(400);
    assert_eq!(rig.shot_ticks(trap).len(), 3, "the running burst completes");
    assert!(!rig.running(trap));
}

// A `fire` and a phase-0 `start` on one tick, either order.
#[test]
fn fire_and_phase_zero_start_share_one_execution() {
    for fire_first in [true, false] {
        let (mut rig, trap) = single_bolt_rig(20.0);
        if fire_first {
            rig.fire(trap);
        }
        rig.start_with(trap, Some(100.0), Some(0.0));
        if !fire_first {
            rig.fire(trap);
        }
        rig.run(ms_ticks(100.0) + 1);
        assert_eq!(rig.shot_ticks(trap), vec![0, ms_ticks(100.0)]);
    }
}

// `stop` cancels repeats, never a `fire`.
#[test]
fn fire_and_stop_on_one_tick_fire_once_and_stay_stopped() {
    for fire_first in [true, false] {
        let (mut rig, trap) = single_bolt_rig(20.0);
        rig.start_with(trap, Some(100.0), Some(500.0));
        rig.step();
        if fire_first {
            rig.fire(trap);
        }
        rig.stop(trap);
        if !fire_first {
            rig.fire(trap);
        }
        rig.run(100);
        assert_eq!(rig.shot_ticks(trap), vec![1]);
        assert!(!rig.running(trap));
    }
}

// An off-schedule `fire` never moves the schedule.
#[test]
fn off_schedule_fire_never_moves_the_schedule() {
    // Recovery spans six ticks; the repeat is due every nine.
    let recovery = recovery_ticks(90.0);
    assert!(recovery > 3 && recovery < 9 - 1);
    let (mut rig, trap) = single_bolt_rig(90.0);
    let interval_ms = 9.0 * 1000.0 / 60.0 - 1.0;
    assert_eq!(ms_ticks(interval_ms), 9);
    rig.start_with(trap, Some(interval_ms), Some(0.0));
    rig.run(6);
    // A fire at 6 is clear of the 0 shot's recovery; the repeat due at 9
    // meets the 6 shot's recovery and drops; 18 fires on its original tick.
    rig.fire(trap);
    rig.run(18 - 6 + 1);
    assert_eq!(rig.shot_ticks(trap), vec![0, 6, 18]);
    // A fire on a due tick is one execution; the next repeat stays put.
    rig.run(27 - rig.tick);
    rig.fire(trap);
    rig.run(9 + 1);
    assert_eq!(rig.shot_ticks(trap), vec![0, 6, 18, 27, 36]);
}

// Overlapping repeats drop; the schedule stays anchored.
#[test]
fn interval_shorter_than_execution_drops_overlapping_repeats() {
    let recovery = recovery_ticks(90.0);
    let (mut rig, trap) = single_bolt_rig(90.0);
    // Due every 4 ticks against a 6-tick recovery: 0 fires, 4 drops, 8 fires.
    rig.start_with(trap, Some(4.0 * 1000.0 / 60.0 - 1.0), Some(0.0));
    rig.run(25);
    assert_eq!(recovery, 6);
    assert_eq!(rig.shot_ticks(trap), vec![0, 8, 16, 24]);
}

#[test]
fn hold_primary_runs_one_execution_per_request() {
    let mut rig = Rig::new(vec![entity(
        "held",
        weapon(action(ActivationTrigger::Hold, 20.0, 1, 0.0)),
    )]);
    let trap = rig.trap("held", 1000.0, 0.0);
    rig.install();
    rig.fire(trap);
    rig.run(60);
    assert_eq!(
        rig.shot_ticks(trap),
        vec![0],
        "a hold action never restarts itself"
    );
    rig.start_with(trap, Some(250.0), Some(0.0));
    let start = rig.tick;
    rig.run(2 * ms_ticks(250.0) + 1);
    assert_eq!(
        rig.shot_ticks(trap)[1..],
        [start, start + ms_ticks(250.0), start + 2 * ms_ticks(250.0)],
        "it repeats at the authored interval, not at execution plus recovery"
    );
}

#[test]
fn a_command_on_a_non_trap_warns_and_skips() {
    let mut registry = EntityRegistry::new();
    let other = registry.spawn(Transform::default());
    let capture = LogCapture::start();
    issue_trap_request(&mut registry, other, TrapRequest::Fire);
    capture.assert_logged_once(log::Level::Warn, "is not a projectile trap");
}

fn damageable(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let target = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry
        .set_component(
            target,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .unwrap();
    target
}

fn enemy(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = damageable(registry, position);
    let graph = postretro_foundation::BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: postretro_foundation::BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: std::collections::BTreeMap::from([(
                "idle".to_string(),
                postretro_foundation::BehaviorActivityDescriptor {
                    sound: None,
                    animation: None,
                    motion: Some(postretro_foundation::MotionVerb::Hold),
                    action: None,
                    on_enter: None,
                    layers: Default::default(),
                },
            )]),
            transitions: Default::default(),
        },
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: Default::default(),
        engagement_radius: None,
        move_speed: 3.0,
    };
    registry
        .set_component(id, BrainComponent::from_graph(&graph))
        .unwrap();
    id
}

fn assert_attackerless(registry: &EntityRegistry, target: EntityId, credit: &str) {
    let health = registry.get_component::<HealthComponent>(target).unwrap();
    assert!(
        health.current < health.max,
        "the trap shot damaged {target:?}"
    );
    let entries = health.contributor_ledger.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].source_id, credit);
    assert_eq!(entries[0].last_attacker, None, "no attacker is credited");
    if let Ok(brain) = registry.get_component::<BrainComponent>(target) {
        assert_eq!(
            brain.time_since_damage_ms, 0.0,
            "the enemy registers damage"
        );
        assert!(!brain.damage_source_known, "from an unknown source");
        assert!(brain.recent_attackers.iter().all(Option::is_none));
        assert_eq!(
            brain.last_known_target_pos, None,
            "it never turns toward the trap"
        );
    }
}

/// Fly every live projectile until none remain, as the authoritative tick does.
fn fly(rig: &mut Rig, world: &CollisionWorld) {
    let registry = Rc::new(RefCell::new(std::mem::take(&mut rig.registry)));
    let zones = HitZoneStore::new();
    for _ in 0..240 {
        crate::sim::advance(&registry, world, &zones, 0.0, DT, &mut |_| {});
        let remaining = registry
            .borrow()
            .iter_with_kind(ComponentKind::Projectile)
            .count();
        if remaining == 0 {
            break;
        }
    }
    rig.registry = Rc::try_unwrap(registry)
        .ok()
        .expect("the flight pass released the registry")
        .into_inner();
}

#[test]
fn trap_projectile_damages_without_an_attacker() {
    let mut rig = Rig::new(vec![entity("bolt", bolt(100.0))]);
    let trap = rig.trap("bolt", 1000.0, 0.0);
    let player = damageable(&mut rig.registry, Vec3::new(0.0, 0.0, -4.0));
    rig.install();
    rig.fire(trap);
    rig.step();
    let world = CollisionWorld::new();
    fly(&mut rig, &world);
    assert_attackerless(&rig.registry, player, "trap.test-bolt");

    let mut rig = Rig::new(vec![entity("bolt", bolt(100.0))]);
    let trap = rig.trap("bolt", 1000.0, 0.0);
    let npc = enemy(&mut rig.registry, Vec3::new(0.0, 0.0, -4.0));
    rig.install();
    rig.fire(trap);
    rig.step();
    fly(&mut rig, &world);
    assert_attackerless(&rig.registry, npc, "trap.test-bolt");
}

fn wall_at_x(x: f32) -> CollisionWorld {
    let points = vec![
        Vec3::new(x, -3.0, -8.0),
        Vec3::new(x, 3.0, -8.0),
        Vec3::new(x, 3.0, 2.0),
        Vec3::new(x, -3.0, 2.0),
    ];
    CollisionWorld::from_triangles_for_test(points, vec![[0, 1, 2], [0, 2, 3]])
}

// Regression guard for the splash path's separately derived attacker.
#[test]
fn trap_splash_damages_without_an_attacker_and_respects_walls() {
    let mut rocket = bolt(100.0);
    rocket.splash = Some(SplashDescriptor {
        radius: 3.0,
        min_fraction: 0.5,
        self_damage: true,
        knockback: None,
    });
    let mut rig = Rig::new(vec![entity("rocket", rocket)]);
    let trap = rig.trap("rocket", 1000.0, 0.0);
    // The bolt strikes the floor target; its neighbours sit in the blast.
    let struck = damageable(&mut rig.registry, Vec3::new(0.0, 0.0, -5.0));
    let player = damageable(&mut rig.registry, Vec3::new(-1.5, 0.0, -5.0));
    let npc = enemy(&mut rig.registry, Vec3::new(0.0, 0.0, -6.5));
    let sheltered = damageable(&mut rig.registry, Vec3::new(2.0, 0.0, -5.0));
    rig.install();
    rig.fire(trap);
    rig.step();
    fly(&mut rig, &wall_at_x(1.0));
    for target in [struck, player, npc] {
        assert_attackerless(&rig.registry, target, "trap.test-bolt");
    }
    let sheltered = rig
        .registry
        .get_component::<HealthComponent>(sheltered)
        .unwrap();
    assert_eq!(sheltered.current, sheltered.max, "the wall shelters it");
}
