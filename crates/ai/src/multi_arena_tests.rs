// Multi-arena row: what a spread wave and a level's off-arena population cost
// the AI and steering ticks on a realistic nav graph (4,216 regions), run
// through the real NPC tiers and against today's 60 Hz NPC. It proves nothing
// and only prints.
// See: context/lib/testing_guide.md §Resource bounds · context/lib/rendering_pipeline.md §12
//      context/plans/done/E10--ai-tick-scheduling/index.md (Scale; P14)
//
// Run release, single-threaded, nothing heavy beside it:
//   cargo test --release -p postretro-ai --lib multi_arena_row -- --ignored --nocapture --test-threads=1
// `POSTRETRO_ARENA_TICKS` overrides the sampled tick count for a quick look.
//
// A wave chases the player across arena 0 from a jittered grid. Off-arena
// enemies fill arenas 1–3: half idle, a quarter patrolling, and a quarter
// chasing the player from arenas 2–3, more than 100 m away (past their
// default tactical range). The wave and the far chasers start engaged on the
// player. Each role's cost is the difference between whole-pass runs, so
// nothing enters the AI pass: the wave alone, then with idle and patrol added,
// then with the far chasers.
//
// Tiered runs go through `NpcScheduler` over a synthetic id-51 level, one 8 m
// cell column per slice along x, each arena seeing only itself, the player in
// arena 0: production cadence (15 Hz decides, dormant NPCs at 2 Hz). The 60 Hz
// baseline runs the same population with no schedule and
// `AiRuntime::deciding_every_tick()`: every NPC decides and acts every tick,
// today's NPC before the tiers.

use std::cell::RefCell;
use std::time::Duration;

use super::scale_support::{
    FIXED_STEP_MS, Lcg, Spread, TickCost, TierLevel, WaitStats, WaitTracker, World, ai_stage,
    arena_of, ms, orbit, run_tick, runtime_for, steering_stage,
};
use super::*;
use crate::candidate_scope::REFRESHES;
use crate::decide::DECIDE_INTERVAL_TICKS;
use crate::nav::arena_fixture::{ARENA, ARENAS, arena_x0, multi_arena_map};
use postretro_foundation::AwarenessDescriptor;
use postretro_sim::npc_tiers::NpcTier;

const WARM_TICKS: usize = 60;
const SAMPLED_TICKS: usize = 600;
/// (wave, off-arena) pairs: 500 and 1,000 placed.
const POPULATIONS: [(usize, usize); 2] = [(64, 436), (128, 872)];
/// Idle NPCs in the idle-scan scenario.
const SCANNERS: usize = 64;

/// Which off-arena roles a run places, cumulatively.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Roles {
    WaveAlone,
    WithIdleAndPatrol,
    WithFar,
}

impl Roles {
    const ALL: [Roles; 3] = [Self::WaveAlone, Self::WithIdleAndPatrol, Self::WithFar];

    fn label(self) -> &'static str {
        match self {
            Self::WaveAlone => "wave alone",
            Self::WithIdleAndPatrol => "+ idle, patrol",
            Self::WithFar => "+ far chasers",
        }
    }
}

fn mode(tiered: bool) -> &'static str {
    if tiered { "tiers" } else { "60 Hz" }
}

/// What one run measured over its sampled ticks.
struct Run {
    tiers: [usize; 3],
    ai: Spread,
    ai_mean_ms: f64,
    steering: Spread,
    perception: Spread,
    replans_per_tick: f64,
    mean_budget: f64,
    /// The wave's replan waits (in-arena engaged NPCs).
    wave_waits: WaitStats,
    wake: Option<Wake>,
}

/// The tick the player teleports into the most populated dormant arena.
struct Wake {
    arena: usize,
    moved_up: usize,
    cost: TickCost,
    /// The worst whole tick (perception, AI, steering) over the 60 after it.
    worst_after_ms: f64,
    /// The median whole tick before it.
    median_ms: f64,
}

fn whole_ms(cost: &TickCost) -> f64 {
    ms(cost.perception + cost.ai + cost.steering)
}

/// An NPC brain already engaged on `target`.
fn engage(reg: &mut EntityRegistry, npc: EntityId, target: EntityId) {
    let mut brain = reg.get_component::<BrainComponent>(npc).unwrap().clone();
    brain.acquired_target = Some(target);
    reg.set_component(npc, brain).unwrap();
}

/// Run one population; `wake_at` teleports the player into the most
/// populated dormant arena on that sampled tick.
fn run_arena(
    wave_size: usize,
    off_total: usize,
    roles: Roles,
    tiered: bool,
    ticks: usize,
    wake_at: Option<usize>,
) -> Run {
    let map = multi_arena_map();
    let collision = CollisionWorld::from_triangles_for_test(map.points, map.triangles);
    let nav = NavGraph::from_section(&map.section);
    let factions = crossfire_factions();
    let sentiment = RefCell::new(FactionSentimentState::default());
    let doors = super::scale_support::ArenaDoors::from_env();
    let world = World {
        nav: &nav,
        collision: &collision,
        factions: &factions,
        sentiment: &sentiment,
        doors: &doors,
    };
    let mut reg = EntityRegistry::new();
    let mut runtime = runtime_for(tiered);
    let mut tiers = tiered.then(TierLevel::multi_arena);
    let y = chaser_rest_y();
    let mut center = Vec3::new(ARENA * 0.5, y, ARENA * 0.5);
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
    let mut wave = Vec::with_capacity(wave_size);
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
        let npc = spawn_engaged(&mut reg, position, &engaged, "engage");
        engage(&mut reg, npc, player);
        wave.push(npc);
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
            let npc = spawn_engaged(&mut reg, position, &engaged, "engage");
            engage(&mut reg, npc, player);
        }
    }

    let mut ai_times = Vec::with_capacity(ticks);
    let mut steering_times = Vec::with_capacity(ticks);
    let mut perception_times = Vec::with_capacity(ticks);
    let mut whole_times = Vec::with_capacity(ticks);
    let mut replans = 0u64;
    let mut budget = 0u64;
    let mut tracker = WaitTracker::new(wave.len());
    let mut waits = [WaitStats::default()];
    let mut wake = None;
    let mut worst_after = Duration::ZERO;
    for tick in 0..WARM_TICKS + ticks {
        let sampled = tick.checked_sub(WARM_TICKS);
        let waking = sampled.is_some() && sampled == wake_at;
        let mut dormant_before = Vec::new();
        if waking && let Some(tiers) = tiers.as_ref() {
            // The most populated dormant arena.
            let mut per_arena = [0usize; ARENAS];
            for (id, _) in reg.iter_with_kind(postretro_entities::ComponentKind::Brain) {
                if tiers.tier(id) == Some(NpcTier::Dormant)
                    && let Some(arena) = reg
                        .get_component::<Transform>(id)
                        .ok()
                        .and_then(|transform| arena_of(transform.position.x))
                {
                    per_arena[arena] += 1;
                    dormant_before.push(id);
                }
            }
            let arena = (0..ARENAS).max_by_key(|&arena| per_arena[arena]).unwrap();
            center = Vec3::new(arena_x0(arena) + ARENA * 0.5, y, ARENA * 0.5);
            wake = Some(Wake {
                arena,
                moved_up: 0,
                cost: TickCost::default(),
                worst_after_ms: 0.0,
                median_ms: Spread::of(&whole_times).p50,
            });
        }
        let (x, z) = orbit(tick, center);
        move_player_to(&mut reg, player, x, z);
        let mut cost = ai_stage(
            &mut reg,
            &mut runtime,
            tiers.as_mut(),
            &[player],
            &world,
            None,
        );
        if sampled.is_some() {
            tracker.before_steering(&reg, &wave);
        }
        steering_stage(&mut reg, tiers.as_ref(), &world, &mut cost);
        if waking && let (Some(tiers), Some(wake)) = (tiers.as_ref(), wake.as_mut()) {
            wake.moved_up = dormant_before
                .iter()
                .filter(|&&id| tiers.tier(id).is_some_and(|tier| tier != NpcTier::Dormant))
                .count();
            wake.cost = cost;
        }
        let Some(sampled) = sampled else {
            continue;
        };
        tracker.after_steering(&reg, &wave, |_| Some(0), &mut waits);
        if let Some(at) = wake_at
            && sampled > at
            && sampled <= at + 60
        {
            worst_after = worst_after.max(cost.perception + cost.ai + cost.steering);
        }
        ai_times.push(cost.ai);
        steering_times.push(cost.steering);
        perception_times.push(cost.perception);
        whole_times.push(cost.perception + cost.ai + cost.steering);
        replans += u64::from(cost.steer.replans);
        budget += u64::from(cost.steer.budget);
    }
    tracker.finish(|_| Some(0), &mut waits);
    if let Some(wake) = wake.as_mut() {
        wake.worst_after_ms = ms(worst_after);
    }
    let [wave_waits] = waits;
    Run {
        tiers: tiers.as_ref().map_or([0; 3], |tiers| tiers.counts(&reg)),
        ai: Spread::of(&ai_times),
        ai_mean_ms: ms(ai_times.iter().sum::<Duration>()) / ai_times.len() as f64,
        steering: Spread::of(&steering_times),
        perception: Spread::of(&perception_times),
        replans_per_tick: replans as f64 / ticks as f64,
        mean_budget: budget as f64 / ticks as f64,
        wave_waits,
        wake,
    }
}

/// AC 54: `SCANNERS` idle NPCs in arena 0 in view of the player, with
/// `candidates` hostile players in reach and in sight and a fill so long they
/// never acquire; or the same NPCs holding the player as their target. Returns
/// (AI µs per NPC per acquisition check, candidate refreshes per check), the
/// latter from a second, untimed pass with the refresh log on: with no
/// candidacy predicate, every refreshed candidate is a line-of-sight test.
fn idle_scan(candidates: usize, with_target: bool, ticks: usize) -> (f64, f64) {
    let measure = |log: bool| -> (Duration, u64, u64) {
        let map = multi_arena_map();
        let collision = CollisionWorld::from_triangles_for_test(map.points, map.triangles);
        let nav = NavGraph::from_section(&map.section);
        let factions = crossfire_factions();
        let sentiment = RefCell::new(FactionSentimentState::default());
        let doors = super::scale_support::ArenaDoors::from_env();
        let world = World {
            nav: &nav,
            collision: &collision,
            factions: &factions,
            sentiment: &sentiment,
            doors: &doors,
        };
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        let mut tiers = TierLevel::multi_arena();
        let y = chaser_rest_y();
        let center = Vec3::new(ARENA * 0.5, y, ARENA * 0.5);
        let mut rng = Lcg(7);
        let players: Vec<EntityId> = (0..candidates)
            .map(|k| {
                let offset = if k == 0 {
                    Vec3::ZERO
                } else {
                    Vec3::new(rng.next() * 16.0 - 8.0, 0.0, rng.next() * 16.0 - 8.0)
                };
                spawn_player(&mut reg, center + offset)
            })
            .collect();
        let mut graph = if with_target {
            engaged_graph(
                "engage",
                vec![("engage", activity("walk", MotionVerb::ChaseTarget, None))],
            )
        } else {
            engaged_graph(
                "engage",
                vec![("engage", activity("idle", MotionVerb::Hold, None))],
            )
        };
        graph.ai.awareness = Some(AwarenessDescriptor {
            fill_ms: 1.0e9,
            drain_ms: 2_000.0,
        });
        let npcs: Vec<EntityId> = (0..SCANNERS)
            .map(|_| {
                let position = Vec3::new(4.0 + rng.next() * 56.0, y, 4.0 + rng.next() * 56.0);
                let npc = spawn_engaged(&mut reg, position, &graph, "engage");
                if with_target {
                    engage(&mut reg, npc, players[0]);
                }
                npc
            })
            .collect();
        let mut ai = Duration::ZERO;
        let mut refreshes = 0u64;
        let mut checks = 0u64;
        for tick in 0..WARM_TICKS + ticks {
            let sampled = tick >= WARM_TICKS;
            if log && sampled {
                REFRESHES.with(|log| *log.borrow_mut() = Some(Vec::new()));
            }
            let cost = run_tick(
                &mut reg,
                &mut runtime,
                Some(&mut tiers),
                &players[..1],
                &world,
                None,
            );
            if !sampled {
                continue;
            }
            if log {
                refreshes += REFRESHES
                    .with(|log| log.borrow_mut().take())
                    .map_or(0, |log| log.len() as u64);
            }
            ai += cost.ai;
            // The AI's tick count is this tick's index plus one.
            let ai_tick = tick as u64 + 1;
            checks += npcs
                .iter()
                .filter(|&&npc| {
                    let brain = reg.get_component::<BrainComponent>(npc).unwrap();
                    postretro_sim::npc_tiers::decides_on(
                        ai_tick,
                        postretro_sim::npc_tiers::schedule_identity(brain, npc),
                        DECIDE_INTERVAL_TICKS,
                    )
                })
                .count() as u64;
        }
        if !with_target {
            assert!(
                npcs.iter()
                    .all(|&npc| enemy_acquired_target(&reg, npc).is_none()),
                "a scanner acquired"
            );
        }
        (ai, refreshes, checks)
    };
    let (ai, _, checks) = measure(false);
    let (_, refreshes, logged_checks) = measure(true);
    (
        ai.as_secs_f64() * 1e6 / checks.max(1) as f64,
        refreshes as f64 / logged_checks.max(1) as f64,
    )
}

#[test]
#[ignore = "measurement: run release with --ignored --nocapture --test-threads=1"]
fn multi_arena_row() {
    let ticks = std::env::var("POSTRETRO_ARENA_TICKS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SAMPLED_TICKS);
    eprintln!(
        "[MultiArena] {} nav regions, {ticks} sampled ticks after {WARM_TICKS} warm; tiers: production cadence through NpcScheduler (1 player, arena 0 perceived); 60 Hz: every NPC decides and acts every tick",
        multi_arena_map().section.regions.len(),
    );
    for (wave, off) in POPULATIONS {
        let far = off / 4;
        let idle_patrol = off - far;
        let mut runs: Vec<(Roles, bool, Run)> = Vec::new();
        for roles in Roles::ALL {
            for tiered in [true, false] {
                let run = run_arena(wave, off, roles, tiered, ticks, None);
                eprintln!(
                    "[MultiArena] wave {wave:>3} off {off:>3} {:<14} {:<5} | tiers E/I/D {:>4}/{:>4}/{:>4} | ai p50 {:>7.3} worst {:>7.3} mean {:>7.3} ms | steering p50 {:>7.3} worst {:>7.3} ms | perception p50 {:>6.1} worst {:>6.1} µs | replans/tick {:>5.2} budget {:>5.2}",
                    roles.label(),
                    mode(tiered),
                    run.tiers[0],
                    run.tiers[1],
                    run.tiers[2],
                    run.ai.p50,
                    run.ai.worst,
                    run.ai_mean_ms,
                    run.steering.p50,
                    run.steering.worst,
                    run.perception.p50 * 1e3,
                    run.perception.worst * 1e3,
                    run.replans_per_tick,
                    run.mean_budget,
                );
                runs.push((roles, tiered, run));
            }
        }
        let mean = |roles: Roles, tiered: bool| {
            runs.iter()
                .find(|(r, t, _)| *r == roles && *t == tiered)
                .map(|(_, _, run)| run.ai_mean_ms)
                .unwrap()
        };
        let replans = |roles: Roles, tiered: bool| {
            runs.iter()
                .find(|(r, t, _)| *r == roles && *t == tiered)
                .map(|(_, _, run)| run.replans_per_tick)
                .unwrap()
        };
        // Per-role AI cost per NPC, µs, from mean AI ticks.
        let per_npc = |tiered: bool| {
            let wave_cost = mean(Roles::WaveAlone, tiered) * 1e3 / wave as f64;
            let idle_cost =
                (mean(Roles::WithIdleAndPatrol, tiered) - mean(Roles::WaveAlone, tiered)) * 1e3
                    / idle_patrol as f64;
            let far_cost = (mean(Roles::WithFar, tiered) - mean(Roles::WithIdleAndPatrol, tiered))
                * 1e3
                / far as f64;
            [wave_cost, idle_cost, far_cost]
        };
        let (tiers, baseline) = (per_npc(true), per_npc(false));
        let labels = [
            "engaged wave (arena 0)",
            "idle + patrol (dormant arenas)",
            "far chasers (engaged, beyond tactical range)",
        ];
        for (k, label) in labels.iter().enumerate() {
            eprintln!(
                "[MultiArena] wave {wave:>3} off {off:>3} per NPC AI {label:<44} | tiers {:>6.2} µs | 60 Hz {:>6.2} µs | fraction of 60 Hz {:>5.2}",
                tiers[k],
                baseline[k],
                tiers[k] / baseline[k],
            );
        }
        eprintln!(
            "[MultiArena] wave {wave:>3} off {off:>3} far chasers vs in-arena engaged | AI per NPC {:.2} vs {:.2} µs (x{:.2}) | far chasers' route queries/tick {:.2} (tiers) {:.2} (60 Hz)",
            tiers[2],
            tiers[0],
            tiers[2] / tiers[0],
            replans(Roles::WithFar, true) - replans(Roles::WithIdleAndPatrol, true),
            replans(Roles::WithFar, false) - replans(Roles::WithIdleAndPatrol, false),
        );
        for (roles, tiered, run) in runs.iter_mut() {
            if *roles == Roles::WithFar {
                eprintln!(
                    "[MultiArena] wave {wave:>3} off {off:>3} {:<5} replan wait, in-arena engaged: {}",
                    mode(*tiered),
                    run.wave_waits.line("wave", wave),
                );
            }
        }
        eprintln!(
            "[MultiArena] wave {wave:>3} off {off:>3} perception (step + tier choice), 1 player, {} NPCs: p50 {:.1} worst {:.1} µs",
            wave + off,
            runs.iter()
                .find(|(r, t, _)| *r == Roles::WithFar && *t)
                .map(|(_, _, run)| run.perception.p50 * 1e3)
                .unwrap(),
            runs.iter()
                .find(|(r, t, _)| *r == Roles::WithFar && *t)
                .map(|(_, _, run)| run.perception.worst * 1e3)
                .unwrap(),
        );
    }

    // AC 55 (P14): the player teleports into the most populated dormant arena.
    let (wave, off) = POPULATIONS[1];
    let wake_at = ticks / 2;
    let run = run_arena(wave, off, Roles::WithFar, true, ticks, Some(wake_at));
    let wake = run.wake.expect("the wake tick ran");
    eprintln!(
        "[MultiArena] wake tick (P14): {} placed, player teleported into arena {} at sampled tick {wake_at}: {} NPCs moved up | that tick perception {:.3} + ai {:.3} + steering {:.3} = {:.3} ms ({:.0}% of the {:.2} ms step) vs median tick {:.3} ms | worst tick over the next 60 {:.3} ms | replans that tick {} of budget {}",
        wave + off,
        wake.arena,
        wake.moved_up,
        ms(wake.cost.perception),
        ms(wake.cost.ai),
        ms(wake.cost.steering),
        whole_ms(&wake.cost),
        whole_ms(&wake.cost) / FIXED_STEP_MS * 100.0,
        FIXED_STEP_MS,
        wake.median_ms,
        wake.worst_after_ms,
        wake.cost.steer.replans,
        wake.cost.steer.budget,
    );

    // AC 54: idle-tier scan cost and line-of-sight tests per acquisition check.
    for candidates in [1, 8] {
        let (idle_us, idle_los) = idle_scan(candidates, false, ticks);
        let (target_us, target_los) = idle_scan(candidates, true, ticks);
        eprintln!(
            "[MultiArena] idle scan, {SCANNERS} NPCs in view, {candidates} hostile candidate(s) in reach and sight | without a target: AI {idle_us:>6.2} µs per NPC per check, {idle_los:.2} LOS tests per check | with a target: AI {target_us:>6.2} µs, {target_los:.2} LOS tests per check",
        );
    }
}
