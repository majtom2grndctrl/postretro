// Multi-arena row: what a spread wave and a level's off-arena population cost
// the AI and steering ticks on a realistic nav graph (4,216 regions). It proves
// nothing and only prints.
// See: context/lib/testing_guide.md §Resource bounds · context/lib/rendering_pipeline.md §12
//
// Run release, single-threaded, nothing heavy beside it:
//   cargo test --release -p postretro-ai --lib multi_arena_row -- --ignored --nocapture --test-threads=1
// `POSTRETRO_ARENA_TICKS` overrides the sampled tick count for a quick look.
//
// A wave chases the player across arena 0 from a jittered grid. Off-arena
// enemies fill arenas 1–3: half idle, a quarter patrolling, and a quarter
// chasing the player from arenas 2–3, more than 100 m away. Each role's cost
// is the difference between whole-pass runs, so nothing enters the AI pass:
// the wave alone, then with idle and patrol added, then with the far chasers.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use super::*;
use crate::nav::arena_fixture::{ARENA, ARENAS, arena_x0, multi_arena_map};
use crate::sim::TimingGate;
use crate::sim::cpu_stages::SimStage;

const WARM_TICKS: usize = 60;
const SAMPLED_TICKS: usize = 600;
const PLAYER_ORBIT_RADIUS: f32 = 2.0;
const PLAYER_ORBIT_PERIOD_S: f32 = 4.0;
/// (wave, off-arena) pairs: 500 and 1,000 placed.
const POPULATIONS: [(usize, usize); 2] = [(64, 436), (128, 872)];

/// Which off-arena roles a run places, cumulatively.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Roles {
    WaveAlone,
    WithIdleAndPatrol,
    WithFar,
}

impl Roles {
    fn label(self) -> &'static str {
        match self {
            Self::WaveAlone => "wave alone",
            Self::WithIdleAndPatrol => "+ idle, patrol",
            Self::WithFar => "+ far chasers",
        }
    }
}

/// Deterministic LCG in [0, 1).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }
}

struct Medians {
    ai_ms: f64,
    steering_ms: f64,
    positioning_ms: f64,
}

fn median_ms(times: &mut [Duration]) -> f64 {
    times.sort_unstable();
    times[(times.len() - 1) / 2].as_secs_f64() * 1e3
}

fn run_arena(
    wave_size: usize,
    off_total: usize,
    roles: Roles,
    timing: TimingGate,
    ticks: usize,
) -> Medians {
    let map = multi_arena_map();
    let world = CollisionWorld::from_triangles_for_test(map.points, map.triangles);
    let nav = NavGraph::from_section(&map.section);
    let factions = crossfire_factions();
    let faction_sentiment = RefCell::new(FactionSentimentState::default());
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let y = chaser_rest_y();
    let center = Vec3::new(ARENA * 0.5, y, ARENA * 0.5);
    let player = spawn_player(&mut reg, center);

    let engaged = engaged_graph(
        "engage",
        vec![("engage", activity("walk", MotionVerb::ChaseTarget, None))],
    );
    let idle_graph = test_graph_with(18.0, 18.0);
    let mut patrol_graph = test_graph_with(18.0, 18.0);
    patrol_graph.patrol = Some(PatrolDescriptor {
        points: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
        mode: PatrolMode::Loop,
    });
    patrol_graph.envelope.activities.insert(
        TEST_IDLE_STATE.to_string(),
        authored_state("locomotion", MotionVerb::Patrol, None),
    );

    let mut rng = Lcg(42);
    // The wave: a jittered grid over arena 0, clear of the player.
    let side = (wave_size as f32).sqrt().ceil().max(1.0) as usize;
    let step = (ARENA - 8.0) / side as f32;
    for k in 0..wave_size {
        let (gx, gz) = (k % side, k / side);
        let mut position = Vec3::new(
            4.0 + (gx as f32 + 0.2 + 0.6 * rng.next()) * step,
            y,
            4.0 + (gz as f32 + 0.2 + 0.6 * rng.next()) * step,
        );
        if distance_xz(position, center) < 6.0 {
            position.x += 8.0;
        }
        spawn_engaged(&mut reg, position, &engaged, "engage");
    }
    let far = off_total / 4;
    let patrol = off_total / 4;
    let idle = off_total - far - patrol;
    let place = |rng: &mut Lcg, first_arena: usize| -> Vec3 {
        let arena =
            (first_arena + (rng.next() * (ARENAS - first_arena) as f32) as usize).min(ARENAS - 1);
        Vec3::new(
            arena_x0(arena) + 3.0 + rng.next() * (ARENA - 18.0),
            y,
            3.0 + rng.next() * (ARENA - 18.0),
        )
    };
    if roles >= Roles::WithIdleAndPatrol {
        for _ in 0..idle {
            let position = place(&mut rng, 1);
            spawn_enemy(
                &mut reg,
                position,
                brain_with(idle_graph.clone(), TEST_IDLE_STATE),
                50.0,
            );
        }
        for _ in 0..patrol {
            let position = place(&mut rng, 1);
            spawn_enemy(
                &mut reg,
                position,
                brain_with(patrol_graph.clone(), TEST_IDLE_STATE),
                50.0,
            );
        }
    }
    if roles >= Roles::WithFar {
        for _ in 0..far {
            let position = place(&mut rng, 2);
            spawn_engaged(&mut reg, position, &engaged, "engage");
        }
    }

    let mut ai_times = Vec::with_capacity(ticks);
    let mut steering_times = Vec::with_capacity(ticks);
    let mut positioning_times = Vec::with_capacity(ticks);
    for tick in 0..WARM_TICKS + ticks {
        let t = tick as f32 * STEER_DT;
        let angle = t / PLAYER_ORBIT_PERIOD_S * std::f32::consts::TAU;
        move_player_to(
            &mut reg,
            player,
            center.x + angle.sin() * PLAYER_ORBIT_RADIUS,
            center.z + angle.cos() * PLAYER_ORBIT_RADIUS,
        );
        let ai_cpu = crate::ai_host::AiStageFrame::new(timing);
        let ai_start = Instant::now();
        run_ai_tick_with_navigation_and_impact(
            &mut reg,
            &mut runtime,
            STEER_DT,
            AiTickInputs {
                nav_graph: Some(&nav),
                mover_nav: crate::nav::MoverNav::none(),
                collision_world: Some(&world),
                descriptors: &[],
                descriptor_generation: 0,
                factions: &factions,
                faction_sentiment: &faction_sentiment,
                clips: None,
                schedule: Default::default(),
                cpu: Some(&ai_cpu),
            },
            |_| {},
        );
        let ai_elapsed = ai_start.elapsed();
        let steering_start = Instant::now();
        agent_steering::tick(
            &mut reg,
            &world,
            Some(&nav),
            postretro_collision::EnvironmentView::level_only(STEER_GRAVITY),
            STEER_DT,
        );
        let steering_elapsed = steering_start.elapsed();
        if tick < WARM_TICKS {
            continue;
        }
        ai_times.push(ai_elapsed);
        steering_times.push(steering_elapsed);
        positioning_times.push(Duration::from_nanos(
            ai_cpu.value(SimStage::AiPositioning).unwrap_or(0),
        ));
    }
    Medians {
        ai_ms: median_ms(&mut ai_times),
        steering_ms: median_ms(&mut steering_times),
        positioning_ms: median_ms(&mut positioning_times),
    }
}

#[test]
#[ignore = "measurement: run release with --ignored --nocapture --test-threads=1"]
fn multi_arena_row() {
    let ticks = std::env::var("POSTRETRO_ARENA_TICKS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SAMPLED_TICKS);
    eprintln!(
        "[MultiArena] {} nav regions, {ticks} sampled ticks; AI and steering medians timing off, positioning median timing on",
        multi_arena_map().section.regions.len(),
    );
    for (wave, off) in POPULATIONS {
        for roles in [Roles::WaveAlone, Roles::WithIdleAndPatrol, Roles::WithFar] {
            let untimed = run_arena(wave, off, roles, TimingGate::OFF, ticks);
            let timed = run_arena(wave, off, roles, TimingGate::ON, ticks);
            eprintln!(
                "[MultiArena] wave {wave:>3} off {off:>3} {:<14} | ai p50 {:>8.3} ms | steering p50 {:>7.3} ms | sim_ai_positioning p50 {:>8.3} ms",
                roles.label(),
                untimed.ai_ms,
                untimed.steering_ms,
                timed.positioning_ms,
            );
        }
    }
}
