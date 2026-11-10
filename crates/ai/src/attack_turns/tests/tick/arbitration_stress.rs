// Arbitration stress row: wall time of one `arbitrate_attack_turns` call for a
// wave of engaged enemies on one target with three tokens. It proves nothing
// and only prints; the steady-state zero-allocation proof for the same shapes
// is `steady_state_turn_passes_allocate_nothing`.
// See: context/lib/entity_model.md §7c (Attack budget)
//
// Run release, single-threaded, nothing heavy beside it:
//   caffeinate -dims cargo test --release -p postretro-ai --lib \
//     arbitration_stress_row -- --ignored --nocapture --test-threads=1
//
// Each wave is 64 or 128 enemies on a marked path toward one player, every
// one with a ready contact attack, drawing melee, melee+heavy or ranged+heavy
// in turn. Limits are melee 3, heavy 1 (4 s refresh), ranged 2, as in the
// steady-state probe. Two phases per wave, each sampled per call after warmup:
// - `held`: two holders fill melee and heavy, the head waiter reserves ranged,
//   and every other enemy waits; no turn ends and nothing is granted. The
//   common tick of a wave held at the gate.
// - `turnover`: before each call one holder leaves its marked activity, so its
//   turn ends that call and the longest waiter can be granted; the enemy that
//   left the call before re-enters and requests behind every waiter.

use std::time::{Duration, Instant};

use super::*;
use crate::attack_turns::arbitrate_attack_turns;
use crate::{AttackOutcome, EnemyOutcome, PendingAttack};

const WAVES: [usize; 2] = [64, 128];
const WARMUP_CALLS: usize = 200;
const SAMPLED_CALLS: usize = 5000;
const TICK_MS: f32 = 16.0;
const ATTACK: &str = "attack";
const TOKEN_MELEE: u16 = 0;
const TOKEN_HEAVY: u16 = 1;
const TOKEN_RANGED: u16 = 2;

struct Wave {
    runtime: AiRuntime,
    outcomes: Vec<EnemyOutcome>,
    marked: usize,
    idle: usize,
}

impl Wave {
    fn new(enemy_count: usize) -> Self {
        let graph = marked_graph(vec![(ATTACK, contact(1.0, 100_000.0))]);
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        runtime.turns.budgets = AttackBudgets::new(BudgetTable::new(vec![
            TokenBudget {
                limit: 3,
                refresh_ms: 0.0,
            },
            TokenBudget {
                limit: 1,
                refresh_ms: 4000.0,
            },
            TokenBudget {
                limit: 2,
                refresh_ms: 0.0,
            },
        ]));
        let pawn = spawn_player(&mut reg, player_spot());
        let target = TargetPawn {
            entity: pawn,
            position: player_spot(),
        };
        let enemies: Vec<EntityId> = (0..enemy_count)
            .map(|index| spawn_at(&mut reg, index, &graph, "attack_act"))
            .collect();
        let mut warned = HashSet::new();
        runtime.programs.sync(&reg, &[], 0, &mut warned);
        let kinds = [
            draws(&[(TOKEN_MELEE, 1)]),
            draws(&[(TOKEN_MELEE, 2), (TOKEN_HEAVY, 1)]),
            draws(&[(TOKEN_RANGED, 1), (TOKEN_HEAVY, 1)]),
        ];
        let attack = attack_index(&graph, ATTACK).expect("the graph declares the attack");
        for (slot, &enemy) in enemies.iter().enumerate() {
            runtime
                .programs
                .set_attack_draws_for_test(enemy, attack, kinds[slot % kinds.len()]);
        }
        let outcomes = enemies
            .iter()
            .map(|&enemy| {
                let transform = *reg.get_component::<Transform>(enemy).unwrap();
                let mut outcome = engaged_outcome(
                    enemy,
                    transform.position,
                    reg.get_component::<BrainComponent>(enemy).unwrap().clone(),
                    target,
                );
                outcome.attack = Some(ready_attack());
                outcome
            })
            .collect();
        Self {
            runtime,
            outcomes,
            marked: graph_activity_index(&graph, "attack_act").unwrap(),
            idle: graph_activity_index(&graph, "idle").unwrap(),
        }
    }

    /// Everything a tick does around the arbitration call: the clocks
    /// advance, every enemy stays in the AI set, and every enemy's attack is
    /// ready again after last call's hard gate.
    fn prepare(&mut self, registry: &EntityRegistry) {
        self.runtime.turns.begin_tick(TICK_MS, registry);
        let tick = self.runtime.turns.tick;
        for outcome in &mut self.outcomes {
            crate::attack_turns::admit(&mut outcome.brain.attack_turns, tick);
            if outcome.attack.is_none() {
                outcome.attack = Some(ready_attack());
            }
        }
    }

    /// One timed arbitration; returns its wall time and how many enemies
    /// hold a turn after it.
    fn arbitrate(&mut self) -> (Duration, usize) {
        let AiRuntime {
            programs,
            turns,
            weapons,
            ..
        } = &mut self.runtime;
        let started = Instant::now();
        arbitrate_attack_turns(&mut self.outcomes, programs, turns, weapons);
        let elapsed = started.elapsed();
        let holders = self
            .outcomes
            .iter()
            .filter(|outcome| outcome.brain.attack_turns.holds_any())
            .count();
        (elapsed, holders)
    }

    /// Turnover: the next holder after `cursor` leaves its marked activity
    /// for `idle`, where it has no attack ready, and `returning`, if any,
    /// re-enters it. Returns the leaver.
    fn rotate(&mut self, cursor: usize, returning: Option<usize>) -> usize {
        if let Some(slot) = returning {
            assert!(self.outcomes[slot].brain.enter_activity_at(0, self.marked));
        }
        let count = self.outcomes.len();
        let leaver = (1..=count)
            .map(|step| (cursor + step) % count)
            .find(|&slot| self.outcomes[slot].brain.attack_turns.holds_any())
            .expect("fixture: someone holds a turn");
        let outcome = &mut self.outcomes[leaver];
        assert!(outcome.brain.enter_activity_at(0, self.idle));
        outcome.attack = None;
        leaver
    }
}

fn ready_attack() -> PendingAttack {
    PendingAttack {
        attack_name: ATTACK.to_string(),
        cooldown_ms: 100_000.0,
        effect: AttackOutcome::Contact {
            damage: 1.0,
            damage_type: None,
        },
    }
}

/// Median, p95, p99 and max of `samples`, in microseconds.
fn summary(samples: &mut [Duration]) -> String {
    samples.sort_unstable();
    let us = |at: f64| {
        let index = ((samples.len() - 1) as f64 * at).round() as usize;
        samples[index].as_secs_f64() * 1e6
    };
    let mean = samples.iter().sum::<Duration>().as_secs_f64() * 1e6 / samples.len() as f64;
    format!(
        "median {:.2} us  p95 {:.2} us  p99 {:.2} us  max {:.2} us  mean {mean:.2} us",
        us(0.5),
        us(0.95),
        us(0.99),
        us(1.0),
    )
}

#[test]
#[ignore = "manual measurement: times arbitrate_attack_turns; run release with --nocapture"]
fn arbitration_stress_row() {
    let registry = EntityRegistry::new();
    eprintln!(
        "[ArbitrationStress] debug_assertions={} warmup={WARMUP_CALLS} samples={SAMPLED_CALLS} \
         tokens=3 (melee 3, heavy 1 refreshing 4 s, ranged 2)",
        cfg!(debug_assertions)
    );
    for enemy_count in WAVES {
        let mut wave = Wave::new(enemy_count);

        let mut held = Vec::with_capacity(SAMPLED_CALLS);
        let mut holders = 0;
        for call in 0..WARMUP_CALLS + SAMPLED_CALLS {
            wave.prepare(&registry);
            let (elapsed, now_holding) = wave.arbitrate();
            if call >= WARMUP_CALLS {
                held.push(elapsed);
            }
            holders = now_holding;
        }
        assert!(holders > 0, "fixture: the budget admits someone");
        assert!(holders < enemy_count, "fixture: the rest wait");

        let mut turnover = Vec::with_capacity(SAMPLED_CALLS);
        let mut granted = 0;
        let mut cursor = 0;
        let mut returning = None;
        for call in 0..WARMUP_CALLS + SAMPLED_CALLS {
            wave.prepare(&registry);
            cursor = wave.rotate(cursor, returning);
            returning = Some(cursor);
            let before = wave
                .outcomes
                .iter()
                .filter(|outcome| outcome.brain.attack_turns.holds_any())
                .count();
            let (elapsed, after) = wave.arbitrate();
            if call >= WARMUP_CALLS {
                turnover.push(elapsed);
                // The leaver ended one turn; anything past that was granted.
                granted += after + 1 - before;
            }
        }
        assert!(granted > 0, "fixture: turnover grants waiters");

        eprintln!(
            "[ArbitrationStress] enemies={enemy_count} target=1 held   holders={holders}: {}",
            summary(&mut held)
        );
        eprintln!(
            "[ArbitrationStress] enemies={enemy_count} target=1 turnover grants/call={:.2}: {}",
            granted as f64 / SAMPLED_CALLS as f64,
            summary(&mut turnover)
        );
    }
}
