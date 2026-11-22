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
// - `strafe+hits …`: strafers take peer hits (friendly fire stand-in: every
//   0.5 s, one wave enemy in eight is hit by another for 5). The wave and the
//   chasers share one faction, since different factions are hostile by
//   default and the wave would otherwise turn on the chasers.
//   - `infight`: tolerance 4 and a 1.5 s retaliation window, so a struck
//     enemy turns on the peer that hit it.
//   - `window 0`: tolerance 4, but `retaliation.windowMs` 0, which disables
//     retaliation outright.
//   - `tolerance 100`: the default 1.5 s window, but a tolerance no damage
//     inside one window reaches.

use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Duration;

use super::scale_support::{
    TierLevel, WaitStats, World, ai_stage, orbit, owed, percentile, runtime_for, steering_stage,
};
use super::*;
use crate::sim::TimingGate;
use crate::sim::cpu_stages::SimStage;
use postretro_stage_timing::StageSet;

const CHASERS: usize = 8;
const WARM_TICKS: usize = 60;
const SAMPLED_TICKS: usize = 600;
const HIT_PERIOD_TICKS: usize = 30;
const HIT_DAMAGE: f32 = 5.0;

#[derive(Clone, Copy, PartialEq)]
enum PeerHits {
    None,
    Infight,
    WindowZero,
    Tolerance100,
}

struct Scenario {
    label: &'static str,
    motion: MotionVerb,
    hits: PeerHits,
    counts: &'static [usize],
}

const SCENARIOS: [Scenario; 5] = [
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
        label: "strafe+hits infight",
        motion: MotionVerb::Strafe,
        hits: PeerHits::Infight,
        counts: &[32, 64, 128],
    },
    Scenario {
        label: "strafe+hits window 0",
        motion: MotionVerb::Strafe,
        hits: PeerHits::WindowZero,
        counts: &[32, 64, 128],
    },
    Scenario {
        label: "strafe+hits tolerance 100",
        motion: MotionVerb::Strafe,
        hits: PeerHits::Tolerance100,
        counts: &[32, 64, 128],
    },
];

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

/// Ticks per digest checkpoint. Every tick folds into the chain, so equal
/// checkpoints mean equal ticks; the interval only localizes a divergence.
const DIGEST_INTERVAL: usize = 30;
/// The scenarios and sizes the P2 digest pins (`perf-ai-tick-cost`).
const DIGEST_SCENARIOS: [&str; 3] = ["chase", "strafe", "strafe+hits infight"];
const DIGEST_SIZES: [usize; 3] = [32, 64, 128];

/// FNV-1a over the bytes of one tick's observable AI outcome.
struct Digest(u64);

impl Digest {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn id(&mut self, id: Option<EntityId>) {
        match id {
            Some(id) => {
                self.bytes(&[1]);
                self.bytes(&id.to_raw().to_le_bytes());
            }
            None => self.bytes(&[0]),
        }
    }

    fn vec3(&mut self, v: Option<Vec3>) {
        match v {
            Some(v) => {
                self.bytes(&[1]);
                for c in [v.x, v.y, v.z] {
                    self.bytes(&c.to_bits().to_le_bytes());
                }
            }
            None => self.bytes(&[0]),
        }
    }
}

/// Fold one tick into the run's chain: per agent, in spawn order, its id, its
/// acquired target, its retaliation latch, its combat-slot bits and its
/// position bits. A despawned agent folds a marker only.
fn fold_tick(chain: &mut Digest, reg: &EntityRegistry, agents: &[(EntityId, usize)]) {
    for &(id, _) in agents {
        chain.id(Some(id));
        let Ok(brain) = reg.get_component::<BrainComponent>(id) else {
            chain.bytes(&[0xff]);
            continue;
        };
        chain.id(brain.acquired_target);
        chain.id(brain.retaliation_acquired_target);
        chain.vec3(brain.combat_slot);
        chain.vec3(reg.get_component::<Transform>(id).ok().map(|t| t.position));
    }
}

/// What one wave run prints.
#[derive(Clone, Copy, PartialEq)]
enum Report {
    /// Nothing: the digest run.
    Silent,
    /// Nothing, as today's 60 Hz NPC (no tiers, every NPC decides and acts
    /// every tick): the baseline the row's per-NPC cost is read against.
    Baseline,
    /// The row: waits, replans and the timing-off AI and steering medians.
    Row,
    /// AI sub-stage medians from a timing-on run. Its scopes would inflate the
    /// row's medians, so they come from this second run.
    SubStages,
}

const AI_SUB_STAGES: [SimStage; 5] = [
    SimStage::AiTargeting,
    SimStage::AiPositioning,
    SimStage::AiPerception,
    SimStage::AiGraph,
    SimStage::AiApply,
];

fn run_wave(scenario: &Scenario, wave_size: usize) {
    let tiers = run_wave_with(scenario, wave_size, Report::Row);
    run_wave_with(scenario, wave_size, Report::SubStages);
    let baseline = run_wave_with(scenario, wave_size, Report::Baseline);
    let agents = (wave_size + CHASERS) as f64;
    eprintln!(
        "[ReplanStress] {} {wave_size}: per NPC, tiers vs 60 Hz | ai mean {:.2} vs {:.2} µs (fraction {:.2}) | steering mean {:.2} vs {:.2} µs (fraction {:.2}) | route queries/tick {:.2} vs {:.2}, mean budget {:.1} vs {:.1}",
        scenario.label,
        tiers.ai_mean_us / agents,
        baseline.ai_mean_us / agents,
        tiers.ai_mean_us / baseline.ai_mean_us,
        tiers.steer_mean_us / agents,
        baseline.steer_mean_us / agents,
        tiers.steer_mean_us / baseline.steer_mean_us,
        tiers.replans_per_tick,
        baseline.replans_per_tick,
        tiers.mean_budget,
        baseline.mean_budget,
    );
}

/// What one wave run returns.
struct WaveRun {
    /// The digest chain's checkpoints, one every [`DIGEST_INTERVAL`] ticks
    /// from tick 0.
    checkpoints: Vec<u64>,
    ai_mean_us: f64,
    steer_mean_us: f64,
    replans_per_tick: f64,
    mean_budget: f64,
}

/// Run one wave and print per `report`: through the NPC tiers over one
/// perceived cell, except the 60 Hz baseline.
fn run_wave_with(scenario: &Scenario, wave_size: usize, report: Report) -> WaveRun {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let nav = floor.nav_graph();
    let mut graph = engaged_graph(
        "engage",
        vec![("engage", activity("walk", scenario.motion, None))],
    );
    // `Tolerance100` keeps the graph's omitted block: the default window.
    let window_ms = match scenario.hits {
        PeerHits::Infight => Some(1_500.0),
        PeerHits::WindowZero => Some(0.0),
        PeerHits::None | PeerHits::Tolerance100 => None,
    };
    if let Some(window_ms) = window_ms {
        graph.retaliation = Some(RetaliationDescriptor {
            window_ms,
            damage_weight: 1.0,
            recency_weight: 0.001,
        });
    }
    let factions = crossfire_factions();
    let faction_sentiment = RefCell::new(FactionSentimentState::default());
    let mut reg = EntityRegistry::new();
    let tiered = report != Report::Baseline;
    let mut runtime = runtime_for(tiered);
    let mut tiers = tiered.then(TierLevel::single_cell);
    let center = target_pos();
    let player = spawn_player(&mut reg, center);

    let mut wave: Vec<EntityId> = Vec::with_capacity(wave_size);
    for index in 0..wave_size {
        let yaw = (index % 16) as f32 * std::f32::consts::TAU / 16.0 + (index / 16) as f32 * 0.2;
        let distance = 6.0 + (index / 16) as f32;
        let position = center + Vec3::new(yaw.sin(), 0.0, yaw.cos()) * distance;
        let enemy = spawn_engaged(&mut reg, position, &graph, "engage");
        // The per-enemy (archetype) tolerance outranks the faction pair's.
        let tolerance = match scenario.hits {
            PeerHits::None => None,
            PeerHits::Infight | PeerHits::WindowZero => Some(CROSSFIRE_LOW_TOLERANCE),
            PeerHits::Tolerance100 => Some(100.0),
        };
        if let Some(tolerance) = tolerance {
            set_crossfire_faction(&mut reg, enemy, CROSSFIRE_RAIDER_FACTION);
            set_crossfire_tolerance(&mut reg, enemy, tolerance);
        }
        wave.push(enemy);
    }
    let mut agents: Vec<(EntityId, usize)> = wave.iter().map(|&id| (id, 0)).collect();
    for index in 0..CHASERS {
        let x = center.x - 4.0 + index as f32 * 1.1;
        let chaser = spawn_chaser(&mut reg, x, center.z - 15.5);
        if scenario.hits != PeerHits::None {
            set_crossfire_faction(&mut reg, chaser, CROSSFIRE_RAIDER_FACTION);
        }
        agents.push((chaser, 1));
    }

    let doors = super::scale_support::ArenaDoors::from_env();

    let stage_world = World {
        nav: &nav,
        collision: &world,
        factions: &factions,
        sentiment: &faction_sentiment,
        doors: &doors,
    };
    let mut open_wait: Vec<Option<u32>> = vec![None; agents.len()];
    let mut stats = [WaitStats::default(), WaitStats::default()];
    let mut late_wave = WaitStats::default();
    let mut ai_times = Vec::with_capacity(SAMPLED_TICKS);
    let mut steer_times = Vec::with_capacity(SAMPLED_TICKS);
    let mut replans_total = 0u32;
    let mut saturated_ticks = 0u32;
    let mut peak_peer_targets = 0usize;
    let mut budget_total = 0u32;
    let mut chain = Digest::new();
    let mut checkpoints = Vec::new();
    let timing = TimingGate::new(report == Report::SubStages);
    let mut sub_stage_nanos: [Vec<u64>; AI_SUB_STAGES.len()] = Default::default();

    for tick in 0..WARM_TICKS + SAMPLED_TICKS {
        let (x, z) = orbit(tick, center);
        move_player_to(&mut reg, player, x, z);
        if scenario.hits != PeerHits::None && tick >= WARM_TICKS && tick % HIT_PERIOD_TICKS == 0 {
            land_peer_hits(&mut reg, &wave, tick / HIT_PERIOD_TICKS);
        }

        let ai_cpu = crate::ai_host::AiStageFrame::new(timing);
        let mut cost = ai_stage(
            &mut reg,
            &mut runtime,
            tiers.as_mut(),
            &[player],
            &stage_world,
            Some(&ai_cpu),
        );
        // The perception step and tier choice count with the AI tick.
        let ai_elapsed = cost.perception + cost.ai;
        if tick >= WARM_TICKS && timing.is_enabled() {
            for (nanos, stage) in sub_stage_nanos.iter_mut().zip(AI_SUB_STAGES) {
                // Absent is a tick the stage did not run: zero time.
                nanos.push(ai_cpu.value(stage).unwrap_or(0));
            }
        }

        let owed_now: Vec<bool> = agents
            .iter()
            .map(|&(id, _)| {
                // Steering skips the dead, so a corpse's last request is not a wait.
                reg.exists(id) && player_hp(&reg, id) > 0.0 && owed(&enemy_agent(&reg, id))
            })
            .collect();

        steering_stage(&mut reg, tiers.as_ref(), &stage_world, &mut cost);
        let (result, steer_elapsed) = (cost.steer, cost.steering);

        fold_tick(&mut chain, &reg, &agents);
        if (tick + 1) % DIGEST_INTERVAL == 0 {
            checkpoints.push(chain.0);
        }

        if tick < WARM_TICKS {
            continue;
        }
        ai_times.push(ai_elapsed);
        steer_times.push(steer_elapsed);
        replans_total += result.replans;
        budget_total += result.budget;
        if result.replans >= result.budget {
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
    let run = WaveRun {
        checkpoints,
        ai_mean_us: ai_times.iter().sum::<Duration>().as_secs_f64() * 1e6 / ai_times.len() as f64,
        steer_mean_us: steer_times.iter().sum::<Duration>().as_secs_f64() * 1e6
            / steer_times.len() as f64,
        replans_per_tick: replans_total as f64 / SAMPLED_TICKS as f64,
        mean_budget: budget_total as f64 / SAMPLED_TICKS as f64,
    };
    match report {
        Report::Silent | Report::Baseline => return run,
        Report::SubStages => {
            let medians: Vec<String> = sub_stage_nanos
                .iter_mut()
                .zip(AI_SUB_STAGES)
                .map(|(nanos, stage)| {
                    nanos.sort_unstable();
                    let p50 = nanos[(nanos.len() - 1) / 2] as f64 / 1e3;
                    format!("{} {p50:.1}", stage.label())
                })
                .collect();
            eprintln!(
                "[ReplanStress] {} {wave_size}: timing-on ai sub-stage µs p50: {}",
                scenario.label,
                medians.join(" | ")
            );
            return run;
        }
        Report::Row => {}
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
        "[ReplanStress] replans {replans_total} ({:.2}/tick, mean budget {:.1}), budget-saturated ticks {saturated_ticks}/{SAMPLED_TICKS}; wave holding a slot at end {slotted}/{alive} alive; chasers holding the target {chasers_engaged}/{CHASERS}; agents moving at end {moving}/{}",
        replans_total as f64 / SAMPLED_TICKS as f64,
        budget_total as f64 / SAMPLED_TICKS as f64,
        agents.len(),
    );
    eprintln!(
        "[ReplanStress] targets at end: distinct {}, wave on peers {on_peers}/{alive} (peak {peak_peer_targets})",
        targets.len(),
    );
    if let Some(tiers) = tiers.as_ref() {
        let [engaged, idle, dormant] = tiers.counts(&reg);
        eprintln!("[ReplanStress] tiers at end: engaged {engaged} idle {idle} dormant {dormant}");
    }
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
    // The longest open wait's agent state, to tell a steering exemption or a
    // measurement artifact (steering sees no drift: `drift_wait_ticks` 0) from
    // a drift waiter the fair queue never reached (`drift_wait_ticks` large).
    if let Some((slot, wait)) = open_wait
        .iter()
        .enumerate()
        .filter_map(|(slot, wait)| wait.map(|wait| (slot, wait)))
        .max_by_key(|&(_, wait)| wait)
        .filter(|&(_, wait)| wait > 60)
    {
        let (id, class) = agents[slot];
        let agent = enemy_agent(&reg, id);
        let brain = reg.get_component::<BrainComponent>(id).unwrap();
        eprintln!(
            "[ReplanStress] longest open wait {wait} ticks: slot {slot} class {class} | drift_wait_ticks {} cooldown {} blocked {} arrived {} pin_escape {} ride {} path {} cursor {} stuck {} unstick {} | dest {:?} planned {:?} | pos {:?} | target {:?} slot {:?} hp {}",
            agent.drift_wait_ticks,
            agent.replan_cooldown_ticks,
            agent.blocked,
            agent.arrived,
            agent.pin_escape.is_some(),
            agent.ride.is_some(),
            agent.path.len(),
            agent.waypoint_cursor,
            agent.stuck_ticks,
            agent.unstick_window_remaining,
            agent.destination,
            agent.planned_destination,
            reg.get_component::<Transform>(id).unwrap().position,
            enemy_acquired_target(&reg, id),
            brain.combat_slot,
            player_hp(&reg, id),
        );
    }
    run
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

fn digest_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(format!("replan_stress_digest.{}.txt", std::env::consts::OS))
}

/// One line per pinned run: `<scenario>|<size>|<hex checkpoints, space-separated>`.
fn digest_lines() -> Vec<String> {
    let mut lines = Vec::new();
    for label in DIGEST_SCENARIOS {
        let scenario = SCENARIOS
            .iter()
            .find(|scenario| scenario.label == label)
            .expect("pinned scenario exists");
        for size in DIGEST_SIZES {
            let checkpoints = run_wave_with(scenario, size, Report::Silent).checkpoints;
            let hex: Vec<String> = checkpoints.iter().map(|c| format!("{c:016x}")).collect();
            lines.push(format!("{label}|{size}|{}", hex.join(" ")));
        }
    }
    lines
}

/// P2 (`perf-ai-tick-cost`): per-tick targets, retaliation latches, slot
/// claims and positions over the pinned waves equal the digest recorded at
/// the pre-change commit, on the OS that recorded it (platform `sin`/`cos`
/// may differ in the last bit). `POSTRETRO_RECORD_STRESS_DIGEST=1` rewrites
/// this OS's file instead of comparing.
///
/// Run release, single-threaded:
///   cargo test --release -p postretro-ai --lib replan_stress_digest -- --ignored --test-threads=1
#[test]
#[ignore = "identical-results gate: run release with --ignored --test-threads=1"]
fn replan_stress_digest() {
    let path = digest_path();
    let lines = digest_lines();
    if std::env::var_os("POSTRETRO_RECORD_STRESS_DIGEST").is_some_and(|v| v == "1") {
        let mut body = lines.join("\n");
        body.push('\n');
        std::fs::write(&path, body).expect("write digest");
        eprintln!("[ReplanStress] recorded {}", path.display());
        return;
    }
    let recorded = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no stress digest for this OS at {}; record one at the pre-change commit with \
             POSTRETRO_RECORD_STRESS_DIGEST=1. Computed:\n{}",
            path.display(),
            lines.join("\n")
        )
    });
    let recorded: Vec<&str> = recorded.lines().collect();
    assert_eq!(recorded.len(), lines.len(), "pinned run count");
    for (want, got) in recorded.iter().zip(&lines) {
        if *want == got.as_str() {
            continue;
        }
        let (label, want_hex) = want.rsplit_once('|').expect("digest line");
        let (_, got_hex) = got.rsplit_once('|').expect("digest line");
        let tick = want_hex
            .split(' ')
            .zip(got_hex.split(' '))
            .position(|(a, b)| a != b)
            .map_or(0, |checkpoint| checkpoint * DIGEST_INTERVAL);
        panic!(
            "{label}: AI outcome diverged within ticks {tick}..{}",
            tick + DIGEST_INTERVAL
        );
    }
}
