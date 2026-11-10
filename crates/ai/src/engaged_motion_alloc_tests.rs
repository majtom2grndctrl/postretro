// Allocation cost of the rotated combat ring: heap allocations per AI tick for
// a wave of engaged strafers on the ring against the same wave on chase.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use super::*;
use crate::alloc_probe::AllocSnapshot;

const WAVE: usize = 64;
const WARM_TICKS: usize = 4;
/// Two chase hold windows (`COMBAT_SLOT_HOLD_TICKS`), so challenger rescans
/// are counted at their real rate.
const COUNTED_TICKS: usize = 16;

/// Mean heap allocations per AI tick for `WAVE` enemies in `motion`, engaged
/// on one target, standing still on four rings 6 m to 13.5 m out.
fn allocs_per_tick(motion: MotionVerb) -> f64 {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let nav = floor.nav_graph();
    let graph = engaged_graph("engage", vec![("engage", activity("walk", motion, None))]);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, target_pos());
    let mut enemies = Vec::with_capacity(WAVE);
    for index in 0..WAVE {
        let yaw = index as f32 * std::f32::consts::TAU / 16.0;
        let distance = 6.0 + (index / 16) as f32 * 2.5;
        let position = target_pos() + Vec3::new(yaw.sin(), 0.0, yaw.cos()) * distance;
        enemies.push(spawn_engaged(&mut reg, position, &graph, "engage"));
    }
    for _ in 0..WARM_TICKS {
        ai_tick(&mut reg, &mut runtime, &world, &nav);
    }
    let snapshot = AllocSnapshot::arm();
    for _ in 0..COUNTED_TICKS {
        ai_tick(&mut reg, &mut runtime, &world, &nav);
    }
    let allocs = snapshot.allocs_since();
    // Fixture: the whole wave holds the target, so every enemy is engaged
    // and queries for a slot each tick; strafers query the ring.
    for &enemy in &enemies {
        assert!(enemy_acquired_target(&reg, enemy).is_some(), "{motion:?}");
        if motion == MotionVerb::Strafe {
            assert!(matches!(
                memory(&reg, enemy),
                EngagedMotionMemory::Strafe(_)
            ));
        }
    }
    let claiming = enemies
        .iter()
        .filter(|&&enemy| {
            reg.get_component::<BrainComponent>(enemy)
                .unwrap()
                .combat_slot
                .is_some()
        })
        .count();
    eprintln!("[RingAllocs] {motion:?}: {claiming} of {WAVE} hold a slot");
    allocs as f64 / COUNTED_TICKS as f64
}

// The plan's allocation note: a ring query allocates one candidate list of at
// most 8 entries in place of the chase query's generated set, so a strafing
// wave allocates no more per tick than the same wave on chase.
#[test]
fn ring_wave_allocates_no_more_per_tick_than_chase() {
    let chase = allocs_per_tick(MotionVerb::ChaseTarget);
    let strafe = allocs_per_tick(MotionVerb::Strafe);
    eprintln!(
        "[RingAllocs] {WAVE} engaged enemies, allocations per AI tick: chase {chase:.1}, strafe {strafe:.1}"
    );
    assert!(
        strafe <= chase,
        "the ring adds per-tick allocations: strafe {strafe:.1} vs chase {chase:.1}"
    );
}
