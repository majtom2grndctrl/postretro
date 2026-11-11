// Replan-budget stress row: how long a wave of engaged enemies makes agents
// wait for a path replan, and what the AI and steering ticks cost. It proves
// nothing and only prints.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)
//
// Run release, single-threaded, nothing heavy beside it:
//   cargo test --release -p postretro-ai --lib replan_stress_row -- --ignored --nocapture --test-threads=1
//
// Each scenario: a wave of N enemies on rings 6–13 m around one player
// (spawned first, so they hold the low slots), then CHASERS chasers 15.5 m
// south (spawned last), inside the 18 m detection range at every phase of the
// player's walk. The player walks a 2 m circle every 4 s, so chasers'
// destinations drift past the replan threshold about every 10 ticks. An agent
// is "owed" a replan when its live destination has drifted past the threshold
// from the planned one (or it has never planned); its wait is the run of
// consecutive owed ticks that ends when steering admits it.
//
// Scenarios:
// - `strafe`: the wave strafes. The row the replan budget was measured on.
// - `chase`: the same wave on plain chase, to tell strafe's cost from the
//   engaged-wave cost that predates it.
// - `strafe+hits focus`: strafers take peer hits (friendly fire stand-in:
//   every 0.5 s, one wave enemy in eight is hit by another for 5) but carry no
//   retaliation, so they keep the player.
// - `strafe+hits infight`: the same hits, with retaliation on and a tolerance
//   the hits cross, so struck enemies turn on the peer that hit them.

use std::cell::RefCell;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::*;

const CHASERS: usize = 8;
const WARM_TICKS: usize = 60;
const SAMPLED_TICKS: usize = 600;
/// Mirrors `agent_steering`'s private `REPLAN_DEST_THRESHOLD`.
const DRIFT_THRESHOLD: f32 = 0.5;
const PLAYER_ORBIT_RADIUS: f32 = 2.0;
const PLAYER_ORBIT_PERIOD_S: f32 = 4.0;
const HIT_PERIOD_TICKS: usize = 30;
const HIT_DAMAGE: f32 = 5.0;

#[derive(Clone, Copy, PartialEq)]
enum PeerHits {
    None,
    Focus,
    Infight,
}

struct Scenario {
    label: &'static str,
    motion: MotionVerb,
    hits: PeerHits,
    counts: &'static [usize],
}

const SCENARIOS: [Scenario; 4] = [
    Scenario {
        label: "strafe",
        motion: MotionVerb::Strafe,
        hits: PeerHits::None,
        counts: &[0, 16, 32, 64, 128],
    },
    Scenario {
        label: "chase",
        motion: MotionVerb::ChaseTarget,
        hits: PeerHits::None,
        counts: &[16, 32, 64, 128],
    },
    Scenario {
        label: "strafe+hits focus",
        motion: MotionVerb::Strafe,
        hits: PeerHits::Focus,
        counts: &[32, 64, 128],
    },
    Scenario {
        label: "strafe+hits infight",
        motion: MotionVerb::Strafe,
        hits: PeerHits::Infight,
        counts: &[32, 64, 128],
    },
];

#[derive(Default)]
struct WaitStats {
    /// Completed waits, in ticks (0 = admitted on the tick it became owed).
    waits: Vec<u32>,
    /// Waits still open when the run ended.
    open: Vec<u32>,
    admissions: u32,
}

impl WaitStats {
    fn line(&mut self, label: &str, agents: usize) -> String {
        self.waits.sort_unstable();
        let pct = |p: f32| -> u32 {
            if self.waits.is_empty() {
                return 0;
            }
            let i = ((self.waits.len() - 1) as f32 * p).round() as usize;
            self.waits[i]
        };
        let mean = if self.waits.is_empty() {
            0.0
        } else {
            self.waits.iter().map(|&w| w as f64).sum::<f64>() / self.waits.len() as f64
        };
        let open_max = self.open.iter().copied().max().unwrap_or(0);
        format!(
            "{label:<17} agents {agents:>3} | admissions {:>5} | wait ticks mean {mean:>5.1} p50 {:>3} p95 {:>3} max {:>3} | open at end {:>3} (longest {open_max})",
            self.admissions,
            pct(0.5),
            pct(0.95),
            self.waits.last().copied().unwrap_or(0),
            self.open.len(),
        )
    }
}

fn owed(agent: &AgentComponent) -> bool {
    let Some(destination) = agent.destination else {
        return false;
    };
    agent
        .planned_destination
        .is_none_or(|planned| distance_xz(planned, destination) > DRIFT_THRESHOLD)
}

fn percentile(sorted: &[Duration], p: f32) -> Duration {
    sorted[((sorted.len() - 1) as f32 * p).round() as usize]
}

/// The friendly-fire stand-in: every `HIT_PERIOD_TICKS`, one wave enemy in
/// eight (at least two) takes `HIT_DAMAGE` from another, on a fixed
/// deterministic schedule. Dead enemies neither take nor deal hits.
fn land_peer_hits(reg: &mut EntityRegistry, wave: &[EntityId], round: usize) {
    let count = wave.len();
    if count < 2 {
        return;
    }
    for k in 0..(count / 8).max(2) {
        let victim = (round * 7 + k * 11) % count;
        let attacker = (victim + 1 + (round + k * 3) % (count - 1)) % count;
        let (victim, attacker) = (wave[victim], wave[attacker]);
        if !reg.exists(victim) || !reg.exists(attacker) {
            continue;
        }
        // `player_hp` reads any entity's health.
        if player_hp(reg, victim) <= HIT_DAMAGE || player_hp(reg, attacker) <= 0.0 {
            continue;
        }
        let mut context = DamageContext::new("test.replan_stress", DamageProducer::InTick);
        context.attacker = Some(attacker);
        apply_damage_with_context(
            reg,
            victim,
            &DamagePayload {
                amount: HIT_DAMAGE,
                impulse: Vec3::ZERO,
                damage_type: None,
            },
            context,
        );
    }
}

fn run_wave(scenario: &Scenario, wave_size: usize) {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let nav = floor.nav_graph();
    let mut graph = engaged_graph(
        "engage",
        vec![("engage", activity("walk", scenario.motion, None))],
    );
    if scenario.hits == PeerHits::Infight {
        graph.retaliation = Some(RetaliationDescriptor {
            window_ms: 1_500.0,
            damage_weight: 1.0,
            recency_weight: 0.001,
        });
    }
    let factions = crossfire_factions();
    let faction_sentiment = RefCell::new(FactionSentimentState::default());
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let center = target_pos();
    let player = spawn_player(&mut reg, center);

    let mut wave: Vec<EntityId> = Vec::with_capacity(wave_size);
    for index in 0..wave_size {
        let yaw = (index % 16) as f32 * std::f32::consts::TAU / 16.0 + (index / 16) as f32 * 0.2;
        let distance = 6.0 + (index / 16) as f32;
        let position = center + Vec3::new(yaw.sin(), 0.0, yaw.cos()) * distance;
        let enemy = spawn_engaged(&mut reg, position, &graph, "engage");
        if scenario.hits != PeerHits::None {
            set_crossfire_faction(&mut reg, enemy, CROSSFIRE_RAIDER_FACTION);
            let tolerance = match scenario.hits {
                PeerHits::Infight => CROSSFIRE_LOW_TOLERANCE,
                _ => f32::MAX,
            };
            set_crossfire_tolerance(&mut reg, enemy, tolerance);
        }
        wave.push(enemy);
    }
    let mut agents: Vec<(EntityId, usize)> = wave.iter().map(|&id| (id, 0)).collect();
    for index in 0..CHASERS {
        let x = center.x - 4.0 + index as f32 * 1.1;
        agents.push((spawn_chaser(&mut reg, x, center.z - 15.5), 1));
    }

    let mut open_wait: Vec<Option<u32>> = vec![None; agents.len()];
    let mut stats = [WaitStats::default(), WaitStats::default()];
    let mut late_wave = WaitStats::default();
    let mut ai_times = Vec::with_capacity(SAMPLED_TICKS);
    let mut steer_times = Vec::with_capacity(SAMPLED_TICKS);
    let mut replans_total = 0u32;
    let mut saturated_ticks = 0u32;
    let mut peak_peer_targets = 0usize;
    let budget = agent_steering::REPLAN_BUDGET_PER_TICK;

    for tick in 0..WARM_TICKS + SAMPLED_TICKS {
        let t = tick as f32 * STEER_DT;
        let angle = t / PLAYER_ORBIT_PERIOD_S * std::f32::consts::TAU;
        move_player_to(
            &mut reg,
            player,
            center.x + angle.sin() * PLAYER_ORBIT_RADIUS,
            center.z + angle.cos() * PLAYER_ORBIT_RADIUS,
        );
        if scenario.hits != PeerHits::None && tick >= WARM_TICKS && tick % HIT_PERIOD_TICKS == 0 {
            land_peer_hits(&mut reg, &wave, tick / HIT_PERIOD_TICKS);
        }

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
            },
            |_| {},
        );
        let ai_elapsed = ai_start.elapsed();

        let owed_now: Vec<bool> = agents
            .iter()
            .map(|&(id, _)| reg.exists(id) && owed(&enemy_agent(&reg, id)))
            .collect();

        let steer_start = Instant::now();
        let result = agent_steering::tick(
            &mut reg,
            &world,
            Some(&nav),
            postretro_collision::EnvironmentView::level_only(STEER_GRAVITY),
            STEER_DT,
        );
        let steer_elapsed = steer_start.elapsed();

        if tick < WARM_TICKS {
            continue;
        }
        ai_times.push(ai_elapsed);
        steer_times.push(steer_elapsed);
        replans_total += result.replans;
        if result.replans >= budget {
            saturated_ticks += 1;
        }
        let peer_targets = wave
            .iter()
            .filter(|&&id| {
                reg.exists(id)
                    && enemy_acquired_target(&reg, id).is_some_and(|target| target != player)
            })
            .count();
        peak_peer_targets = peak_peer_targets.max(peer_targets);

        for (slot, &(id, class)) in agents.iter().enumerate() {
            if !reg.exists(id) {
                open_wait[slot] = None;
                continue;
            }
            let agent = enemy_agent(&reg, id);
            // Admission sets the cooldown to the full staleness window, after
            // this tick's decrement, so it reads exactly that value now.
            let admitted = agent.replan_cooldown_ticks == agent_steering::REPLAN_STALENESS_TICKS;
            let late = class == 0 && slot >= wave_size / 2;
            if admitted {
                stats[class].admissions += 1;
                if late {
                    late_wave.admissions += 1;
                }
            }
            if owed_now[slot] {
                let wait = open_wait[slot].unwrap_or(0);
                if admitted {
                    stats[class].waits.push(wait);
                    if late {
                        late_wave.waits.push(wait);
                    }
                    open_wait[slot] = None;
                } else {
                    open_wait[slot] = Some(wait + 1);
                }
            } else {
                open_wait[slot] = None;
            }
        }
    }
    for (slot, &(_, class)) in agents.iter().enumerate() {
        if let Some(wait) = open_wait[slot] {
            stats[class].open.push(wait);
            if class == 0 && slot >= wave_size / 2 {
                late_wave.open.push(wait);
            }
        }
    }

    let live = |id: &EntityId| reg.exists(*id);
    let slotted = wave
        .iter()
        .filter(|&id| live(id))
        .filter(|&&id| {
            reg.get_component::<BrainComponent>(id)
                .unwrap()
                .combat_slot
                .is_some()
        })
        .count();
    let alive = wave.iter().filter(|&id| live(id)).count();
    let chasers_engaged = agents
        .iter()
        .filter(|&&(id, class)| {
            class == 1 && live(&id) && enemy_acquired_target(&reg, id).is_some()
        })
        .count();
    let moving = agents
        .iter()
        .filter(|&&(id, _)| live(&id) && enemy_agent(&reg, id).steer_velocity.length() > 0.1)
        .count();
    let targets: HashSet<EntityId> = wave
        .iter()
        .filter(|&id| live(id))
        .filter_map(|&id| enemy_acquired_target(&reg, id))
        .collect();
    let on_peers = wave
        .iter()
        .filter(|&id| live(id))
        .filter(|&&id| enemy_acquired_target(&reg, id).is_some_and(|target| target != player))
        .count();

    ai_times.sort_unstable();
    steer_times.sort_unstable();
    let tag = scenario.label;
    eprintln!(
        "[ReplanStress] === {tag}: {wave_size} wave + {CHASERS} chasers, {SAMPLED_TICKS} ticks ==="
    );
    eprintln!(
        "[ReplanStress] replans {replans_total} ({:.2}/tick, budget {budget}), budget-saturated ticks {saturated_ticks}/{SAMPLED_TICKS}; wave holding a slot at end {slotted}/{alive} alive; chasers holding the target {chasers_engaged}/{CHASERS}; agents moving at end {moving}/{}",
        replans_total as f64 / SAMPLED_TICKS as f64,
        agents.len(),
    );
    eprintln!(
        "[ReplanStress] targets at end: distinct {}, wave on peers {on_peers}/{alive} (peak {peak_peer_targets})",
        targets.len(),
    );
    eprintln!("[ReplanStress] {}", stats[0].line("wave", wave_size));
    eprintln!(
        "[ReplanStress] {}",
        late_wave.line("  later-slot half", wave_size - wave_size / 2)
    );
    eprintln!(
        "[ReplanStress] {}",
        stats[1].line("chasers (last)", CHASERS)
    );
    eprintln!(
        "[ReplanStress] ai tick µs p50 {:.1} p95 {:.1} max {:.1} | steering tick µs p50 {:.1} p95 {:.1} max {:.1}",
        percentile(&ai_times, 0.5).as_secs_f64() * 1e6,
        percentile(&ai_times, 0.95).as_secs_f64() * 1e6,
        ai_times.last().unwrap().as_secs_f64() * 1e6,
        percentile(&steer_times, 0.5).as_secs_f64() * 1e6,
        percentile(&steer_times, 0.95).as_secs_f64() * 1e6,
        steer_times.last().unwrap().as_secs_f64() * 1e6,
    );
}

#[test]
#[ignore = "measurement: run release with --ignored --nocapture --test-threads=1"]
fn replan_stress_row() {
    for scenario in &SCENARIOS {
        for &wave_size in scenario.counts {
            run_wave(scenario, wave_size);
        }
    }
}
