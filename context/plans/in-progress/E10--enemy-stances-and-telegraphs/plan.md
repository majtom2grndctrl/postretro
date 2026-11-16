# E10--enemy-stances-and-telegraphs — plan of record

mode: resumable
status: approved
read at: f5bb5d5
feature branch: `claude/enemy-stances-telegraphs-837003`

## Approval
Owner approved the plan as proposed (2026-11-15). The proposals stand: stances build now with C1's client half and M5 outstanding until `E10--coop-enemy-telegraph-sounds` lands; the working names `releaseAfter` and `staggerTo` are accepted; the source-FBX location is still open and is asked at Task 8.

## Corrections
Source verified at f5bb5d5. No commit since the brief's 9580828 touches `crates/`, `content/`, `sdk/` or `docs/` beyond unrelated mover work. Every Decision premise holds; these are Path and inventory corrections.

- **Missing lowering-only fields.** The hidden release draw and the stagger route both need runtime data that the Boundary inventory does not list. A hold needs its draw range, and a release or recover needs the rest it enters after a flinch. Neither is derivable from authored fields. Lowering will write two more lowering-only fields: the hold's release range (working name `releaseAfter: [min, max]`, on the hold) and the flinch destination (working name `staggerTo: "<rest>"`, on the release and recover). An authored copy of either rejects load like `pose`, `queue`, `afterMoment` and `atDefault`. The AC row "An authored `pose`, `queue`, `afterMoment` or `atDefault` field rejects load" is proved as stated and extended to the two new names. Names accepted with the plan.
- **Prerequisite not built.** `E10--coop-enemy-telegraph-sounds` ("builds first") is still in `drafts/`. Stance sounds lower to the rest's activity `sound` either way, and the host already plays it. Only AC C1's client half depends on the route. Task 10 waits on it. **Owner to choose:** promote and build the sounds brief first, or build stances now and land with C1 outstanding.
- `GuardedRow` is `{ when: IrNode, to: String }` with `deny_unknown_fields` (`crates/foundation/src/data_descriptors/types/behavior/recursive.rs:31`). `BehaviorGraphDescriptor` deserializes through a hand-written `RawBehaviorGraphDescriptor`. New fields land in both.
- The JSON-before-serde step is `crates/scripting-core/src/data_descriptors/{js,lua}/entity.rs`, not `crates/scripting-core/{js,lua}/`. Both readers see every `defineMod` entity, wrapped or bare. `defineEntity` lowers loadout, assistable and defense-layer references only; nothing touches `components.behavior`. Lowering goes in after `js_to_json` / `normalize_behavior_selectors` and before `serde_json::from_value`.
- Load-time attack checks for turn reads live in `check_guard_attack_turn_reads` (`crates/scripting-core/src/runtime/types.rs:441`), not in foundation. Under an `attacks` list, lowering runs before it, so it sees only kept rows (P41).
- The `attacks` map type `BehaviorAttacks` is hard-coded in `crates/scripting-core/src/typedef/common.rs:150` (TS) and `:360` (Luau), not in the `crates/sim` registry. It moves to handle types there. `ActionVerb` and `attackTurn` are in the registry (`crates/sim/src/scripting/primitives/mod.rs:662`, `:745`).
- `attach_agent` is `crates/entities/src/components/agent.rs:341`, called from `data_archetype.rs:595`. `locomotion_rate_ratio` is `crates/entities/src/components/animation/playback.rs:41`. The host scales in `update_brain_animation_playback_rates` (`crates/sim/src/sim/mod.rs:1370`), and the client in `update_remote_enemy_walk_playback_rate` (`crates/netcode/src/client.rs`). Both scale only the graph's locomotion state. Rates clamp to [0.5, 1.5].
  - A moving release's state needs one shared rule host and client both derive from the lowered graph: every state played by an activity whose motion steers.
  - The fallback `move_speed` stays the archetype's base value on both sides, so `speed` never splits them (research, round 1).
- `retain_held` (`crates/ai/src/attack_turns/lifecycle.rs:97`) keeps a turn only while the target is unswitched and draws are unchanged, and one of these holds: a targeted mark names it, its execution runs, or an until-cooldown turn is cooling. The hold's re-request after a switch goes through `next_request`'s mark path, as the brief assumes.
- The hard gate (`lifecycle.rs:236`) drops any pending drawn attack without a turn, marked or not, and runs after grants, so a grant can fire on its own tick. An unmarked grant is released on its grant tick only if its attack did not start (`apply.rs:443`). A rest's queued grant must skip that release.
- The moment latch is two bools on `ClipClock` (`moment_open`, `moment_timed`, `crates/entities/src/components/brain/clip_clock.rs`), not a fired/whiffed/cancelled enum. `moment_open` closes on reach (gates pass or not), launch and foreign switch. The release's moment gate reads "closed" as resolved. The stagger route needs a distinct "cancelled by a foreign switch" bit, which this build adds.
- `ModelHitZones::state_clip` is `crates/sim/src/scripting_systems/hit_zones.rs:119`, reached from `begin_tick` through `clip_facts` → `read_clip`.
- `apply_damage_with_context` is `crates/entities/src/components/health.rs:199`. It calls `record_damage_perception` (`damage_perception.rs:12`, live targets with a Brain) for every producer: hitscan, projectile, splash, melee, `applyDamage`, AppDrain and mover crush. `damageInActivity` accumulates `points_dealt` there.
- `AiCue::Entered` (`crates/postretro/src/sound_events/descriptors.rs:237`) is skipped on the event-silent spawn seating, so a stance that is a level's `initial` at spawn plays no sound until it is re-entered. No AC requires one.
- Test names: the descriptor parity test is `the_shipped_reference_enemy_descriptor_is_identical_in_both_authorings` (`crates/scripting-core/src/data_descriptors/tests/behavior.rs:521`). The name in research.md does not exist. `dev_mod_budget_paces_reference_enemies_and_troopers` is in `crates/ai/src/attack_turns/tests/tick/shipped_content.rs:201`.
- The limitator has no Luau twin. Each shipped TS script with a twin (`reference-enemy`, `positional-sound`, `sdk/behaviors/reference/entities`) migrates in both.
- `PROTOCOL_ID` is now "PRLE" (`crates/net/src/handshake.rs:19`); `WIRE_VERSION` 39, `SNAPSHOT_VERSION` 25. The grep gate pins these three as unchanged by this branch.
- A third committed copy of the generated types exists: `crates/sim/src/scripting/typedef/tests/fixtures/expected.d.{ts,luau}`. Every typedef change updates it with `sdk/types`.
- `applyDamage` is handled in `crates/sim/src/health/reactions.rs` (`dispatch`, `DamageProducer::AppDrain`, attacker `None`); the brief's `scripting_systems/health.rs:383` is a test.
- **Pose and the attack count (clarification).** A pose is one activity to the engine, so moving between pose members carries `attacksFiredInActivity` with the timer, roll and damage sum. No attack fires in a rest or hold, so the choice is unobservable today; it keeps the pose rule uniform.
- The no-layers drain path (`drain_hit`, `defense_layers.rs`) reports `points_dealt` without the finite cap `drain_layers` applies. The brain sum caps it; the contributor ledger and impact dispatch do not. Outside this brief; noted for follow-up.
- **Shared rate-scale rule (clarification).** A state played by any activity whose own motion steers now rate-scales, attack or not, on host and client from the same descriptor graph. The limitator's and rifle squads' `run` (under `reposition`) newly scales; at cruise its rate is ~1.0. Pin escape and mover-ride walks keep the base speed.
- Turn facts `attackTurnFree`/`attackTurnHeld` are top-level SDK exports, not `brain` members; the brief's `brain.attackTurnFree(a)` reads them there.
- Strict globals apply to mod-rooted Luau VMs (mod init, staged reload, level data, required modules); probes for absent globals use `rawget(_G, name)`.
- **Release-draw range on the row (clarification).** `releaseAfter: [min, max]` sits on the hold's release row (`GuardedRow`), next to `afterMoment`, not on the hold activity. Validation allows one per activity and none on `"*"` rows. A hold must be entered by a transition, never seated (P21).
- Runtime-spawn counts live in session-owned `SpawnContextState` and are not serialized, so spawns after a mid-level restore restart counts at 0 and can reuse a salt. Known limit; no AC covers restore.
- The determinism gate's enemy is built with `BrainComponent::from_graph` directly, with no descriptor attach or Agent (`determinism_tests.rs:1162`). The extended gate spawns two role-labelled enemies through the same path, plus a descriptor-attached pair for the release-draw rows.

## Delegated answers
- **Seed salt** — proposed, confirmed at Task 1: the per-instance salt is the enemy's placement identity, not its `EntityId`. A map placement uses its authored entity index; a runtime spawn uses its spawner's placement index plus that spawner's spawn count. SplitMix64 mixes the salt with the descriptor name. The roll and the release draw draw from separate streams of that seed. Two same-descriptor enemies differ by placement, and spawn order changes neither. The choice and its test go in the Task 1 row.
- **Default hold range, recovery cap and reaction budget** — tuned on `movement-feel` under the lag profile at Task 8. Starting points: reaction budget 150 ms (floor 400 ms), default `holdMs` [floor, floor + 600], default recovery cap 600 ms.
- **Brute's new attacks, shared stance, speeds and graph nesting** — tuned at Task 8 and reported there.
- **Brute cuts and event times** — recut `punch_windup`/`punch_strike` near 167 ms and `smash_windup` at 433–467 ms at Task 8, with Blender 5.2 (`C:\Program Files\Blender Foundation\Blender 5.2\blender.exe`, not on PATH). Every `--trim`/`--event` goes in `license.txt`, and every moved cut is listed for playtest. **Owner to point to the source FBXs** (outside the repo).
- **Misspelled Luau handle** — strict globals in Luau data scripts: reading an undeclared global raises at load, naming the global and the script line. An agent's normal loop is `cargo check` plus focused `cargo test`, and shipped-content load tests already run every Luau data script. `luau-analyze` is wired into nothing (one hand-run script needs a binary path). This catches a misspelled handle, a misspelled `when` guard and a nil table field by the same rule.

## AC-to-proof

Proposed test names are working names. Every row names its crate and module; the builder records the final name in Status.

| AC | Proof | Status |
|---|---|---|
| T1 binding-name sugar for `defineAttack`/`defineStance`, hold keys, diagnostic, Luau parity | `script-compiler` sugar tests; `scripting-core` TS/Luau descriptor parity test | achievable as stated |
| T2 handle crosses with declaration; lowering builds name-keyed `attacks`; once per load and staged reload | `scripting-core` lowering unit test; staged-reload counter test | achievable as stated |
| T3 inferred vs exhaustive `attacks`; drops, logs, P37/P40/P41 | `scripting-core` lowering tests per pin, with log capture | achievable as stated |
| T4 string where a handle is expected: TS type error and pathed load rejection in both runtimes, both routes; equal/unequal same-name handles | `sdk/type-tests/e10-typed-attacks.ts` `@ts-expect-error` cases; `scripting-core` rejection tests ×2 runtimes ×2 routes | runtime half proved; TS half written, unchecked (no `tsc` on this machine) |
| T5 `attackTurnFree`/`attackTurnHeld` throw on string/nil; misspelled Luau global reported | SDK runtime tests both runtimes; strict-globals load test | achievable as stated |
| T6 authored lowering-only field rejects | `scripting-core` rejection test over `pose`, `queue`, `afterMoment`, `atDefault` (+ `releaseAfter`, `staggerTo`) | achievable as stated |
| T7 one handle in two archetypes | `scripting-core` two-archetype load test | achievable as stated |
| T8 grep gate: no string attack in shipped scripts, docs, typedef comments | surface gate test (xtask `surface_gates` or Rust test) | achievable as stated |
| S1 identical descriptors TS/Luau; Scripting surface example runs as `content/dev` | parity test; `content/dev` example script load + tick test | achievable as stated |
| S2 lowering matches checked-in hand-written fixture | `scripting-core` fixture test (`fixtures/stance_overhead_slam.json`) | achievable as stated |
| S3 only reached rests/strikes generated; name collision rejects | lowering tests | achievable as stated |
| S4 `{ to: overhead }` enters rest, no turn, sound once; `initial: overhead` | `ai` tick test with AI cue capture | achievable as stated |
| S5 `when` optional on every row | foundation serde + `ai` eval test | achievable as stated |
| S6 two attacks sharing a stance: separate holds, shared fields, own `attackTurn` | lowering test | achievable as stated |
| S7 `stance: "crouch"` after `E10--enemy-cover`; rejects before | rejection test now; crouch wiring falls to whichever brief lands second | achievable as stated (crouch half deferred by the Decision) |
| S8 rest with no way out rejects, with or without `breakOn` | lowering test | achievable as stated |
| S9 `endingStance` rest, turn ends, refresh lapses, re-strike | `ai` attack-turn tick test | achievable as stated |
| S10 `motion: "hold"` everywhere; `release.motion`/`speed` replace on release only | lowering test + `ai` steering test | achievable as stated |
| S11 minimal stance/attack load silently; defaults | lowering test with log capture | achievable as stated |
| S12 omitted `recover`: clip played through or default cap; continues release clip | `ai` clip-clock tick tests (looping and not) | achievable as stated |
| S13 authored `recover` without/with `ms` | `ai` tick tests | achievable as stated |
| S14 `at: "hit"` fires at moment; lowered `action`/mark agree | `ai` tick test; foundation validate test | achievable as stated |
| S15 release with `at` vs `ms`; looping at every phase; `"*"` on moment tick (P42, P43, P47) | `ai` tick tests per pin | achievable as stated |
| S16 default `hit`; clip without `hit`; `at: false`; no moment no `ms` (P53) | `ai` tick tests + bind-warning log capture | achievable as stated |
| S17 unreached moment whiffs; untimed named moment (P48, P51, P57) | `ai` tick tests | achievable as stated |
| S18 summed clip time across rate and variant restarts (P54) | `ai` tick test | achievable as stated |
| S19 `at: false`/defaulted-missing fires on first passing tick (P57) | `ai` tick test | achievable as stated |
| S20 release on its stance's state rejects | lowering test | achievable as stated |
| S21 impact switch cancels before moment; entry-tick switch does not (P46); reaction plays, enters ending rest, skips recover (P52) | `ai` tick tests with an impact switch fixture | achievable as stated |
| S22 second switch never extends cap; `"*"` leaves at once (P55) | `ai` tick test | achievable as stated |
| S23 switch after moment / `at: false` / in recover; `ms: 2000` case; moving release stops steering (P52) | `ai` tick tests | achievable as stated |
| S24 cooldown at the queue (P44) | `ai` attack-turn tick test | achievable as stated |
| S25 first attack cooling vs refreshing; cooldown elapses mid-tick (P49, P50) | `ai` attack-turn tick tests | achievable as stated |
| S26 rejection list (11 cases) | `scripting-core` lowering rejection table test | achievable as stated |
| W1 queued row gates and request conditions | `ai` attack-turn tick test | achievable as stated |
| W2 rest grant survives into N+1 (P26) | `ai` tick test | achievable as stated |
| W3 re-rank inside margin keeps target; past margin switches (P27, P39) | `ai` co-op target tick test | achievable as stated |
| W4 starvation (P28) | `ai` arbitration test | achievable as stated |
| W5 one request per enemy; place kept (P29) | `ai` tick test | achievable as stated |
| W6 first row refreshing keeps requesting it (P31) | `ai` tick test | achievable as stated |
| W7 request precedence mark > rest > pending unmarked | `ai` `next_request` test | achievable as stated |
| W8 resting enemy keeps target and aim | `ai` tick test | achievable as stated |
| W9 two enemies in one shared stance (P24) | `ai` tick test with cue capture | achievable as stated |
| W10 grant then no row ends turn (P36) | `ai` tick test | achievable as stated |
| W11 break beats granted strike (P35) | `ai` tick test | achievable as stated |
| P1 pose carry: no restart, no second sound, facts carried, 20+20 trips 25 (P30) | `ai` tick test | achievable as stated |
| P2 target switch mid-tell (P33) | `ai` tick test | achievable as stated |
| P3 fresh entry into a pose (P34) | `ai` tick test | achievable as stated |
| P4 release times span range independent of roll; no exposure (P32) | `ai` statistical tick test; grep gate test | achievable as stated |
| P5 `"*"`/ancestor row into current pose skipped; release/recover abandoned | `ai` graph_eval test | achievable as stated |
| P6 root-level strike beside a chase keeps target and aim (P23) | `ai` tick test | achievable as stated |
| P7 break vs missing turn (P22) | `ai` tick test | achievable as stated |
| P8 timer vs missing turn (P16) | `ai` tick test | achievable as stated |
| P9 zero `ms` release/recover (P45, P56, P57) | `ai` tick tests | achievable as stated |
| F1 floor ≥ interpolation ceiling test | `netcode` test beside `spawn_windup_covers_interpolation_ceiling_without_equality_pin` | achievable as stated |
| F2 default above floor; one warning per mod load below it | lowering test with log capture | achievable as stated |
| R1 hold threshold varies in [min, max]; fixed never varies | `ai` statistical test | achievable as stated |
| R2 `holdMs: 0` and `[x, x]` (P12) | `ai` tick test | achievable as stated |
| R3 roll fixed per entry; redraw on re-entry; reseat-and-leave (P3) | `ai` tick test | achievable as stated |
| R4 independent rolls per level; nested redraw (P4) | `ai` tick test | achievable as stated |
| R5 selector guards read own level (P5) | `ai` graph_eval test | achievable as stated |
| R6 both branches of `approach` read same roll | `ai` tick test | achievable as stated |
| R7 determinism gate: two enemies by role, reversed order | `sim` `determinism_tests` extension | achievable as stated |
| R8 same-tick spawns differ (P13) | `sim` spawn test | achievable as stated |
| R9 grep gate: no wire record carries roll, draw or seed | surface gate test | achievable as stated |
| R10 release draws compared by role; same-tick holds differ (P38) | `sim` determinism extension | achievable as stated |
| D1 `damageInActivity` sums all producers, two hits one tick | `sim` damage-producer test per source | achievable as stated |
| D2 hit before/after AI pass on entry tick (P2) | `sim` stage-order test | achievable as stated |
| D3 `breakOn` at threshold; break beats timer (P1) | `ai` tick test | achievable as stated |
| D4 release does not break; `"*"` leaves release/recover; flinch damage not carried | `ai` tick test | achievable as stated |
| D5 limitator aim breaks into `hit_reaction`; both roll branches in one engagement | `ai` shipped-content test | achievable as stated |
| Sp1 `speed: 2` on two archetypes; inner overrides outer | `sim` steering test | achievable as stated |
| Sp2 speed from entry tick to tick before leave (P17) | `sim` steering test | achievable as stated |
| Sp3 charge `run` rate vs `travelSpeed`, host and loopback client | `sim` playback test + `netcode` loopback test | achievable as stated |
| Sp4 knockback unaffected by `speed` | `sim` steering test | achievable as stated |
| Sp5 locomotion clip rate equal host/client, calibrated or not | `netcode` loopback test | achievable as stated |
| Sp6 charge release reach edges (P18) | `ai`/`sim` tick test | achievable as stated |
| Sp7 old brain deserializes; sums 0, next entry draws | `entities` serde test | achievable as stated |
| C1 stance sound once per pose entry, host and loopback client | `netcode` loopback test | achievable once `E10--coop-enemy-telegraph-sounds` lands (host half now) |
| C2 entering an activity whose clip is playing restarts nothing, host and client | `ai` apply test + `netcode` loopback test | achievable as stated |
| C3 grep gate: no server message, version constants unchanged | surface gate test | achievable as stated |
| K1 brute slam/lunge/jab fire at `*_strike` `hit`; `punch_strike` carries `hit`; windup-pose test on recut library | `ai` `clip_timed_shipped_tests`; `model` `reference_enemy_strikes_start_on_their_windup_pose` | achievable as stated (needs the recut, Task 8) |
| K2 limitator `shoot` release fires at `kick` from drawn phase, leaves at every phase | `ai` `clip_timed_shipped_tests` | achievable as stated |
| M1 re-export reproduces library; moved cuts listed | owner, Windows + Blender | manual |
| M2 jab and slam tells read and land on `movement-feel` | owner, in-engine | manual |
| M3 brute stances read, mix-up unpredictable, heavy fire breaks, bellow reads as charge | owner, in-engine | manual |
| M4 limitator branches and aim flinch | owner, in-engine | manual |
| M5 co-op loopback hears stance sounds, sees stance before hit | owner, in-engine | manual (after the sounds brief) |
| Doc1 `docs/` and `sdk/behaviors/reference` | review + `sdk/behaviors/reference` load test | achievable as stated |

## Tasks

Builds are disk-bound (46 GB free) and serialized, so no concurrent worktree builds. Workers run one at a time on the feature branch, and the integrating executor runs Cargo and commits each task.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **First slice.** `activityRoll` fact (`BRAIN_INPUTS` slot 22, `ScopeRelativeValues`, per-depth array, redraw on entry), per-instance seed and streams, determinism gate with two role-labelled enemies and reversed spawn order. Proves R1–R8 roll rows (release-draw half in Task 5), R9 gate. | integrating executor | — | done: R3–R8, Sp7 (roll half). Seed = descriptor name × placement salt (`Placement(map ordinal)`, `Spawned{spawner ordinal, count}`, `Entity` fallback for synthetic spawns), counter-based SplitMix64 in `foundation/src/behavior_draws.rs`, streams 0 roll / 1 hold release. Tests: `graph_eval` roll tests, `determinism_tests` per-role rolls (fails under an `EntityId` salt), `same_descriptor_placements_draw_their_own_rolls_by_authored_index`, `same_tick_spawns_draw_rolls_salted_by_spawner_placement_and_count`, `brain_serialized_before_activity_rolls_restores_and_draws_on_next_entry`. R1, R2 move to Task 5; R9 gate to Task 9. |
| 2 | `damageInActivity` (slot 23) fed at `apply_damage_with_context`; serde defaults for all new brain fields. D1, D2, Sp7. | worker (sequential) | 1 | done: D1 (`sim/damage_in_activity_tests.rs` hitscan, projectile, splash, applyDamage; melee in `ai_tests.rs` `contact_attack_counts_toward_the_struck_enemys_damage_in_activity`), D2/P2 (`a_hit_before_the_ai_pass_counts_toward_the_activity_left_and_one_after_toward_the_one_entered`), Sp7 complete (`brain_serialized_before_activity_rolls_restores_and_draws_on_next_entry`). sim 1995 passed, ai 423, entities 320. |
| 3 | Per-activity `speed` field: steering multiplier on the innermost setter; shared moving-state rate rule host and client. Sp1–Sp5. | worker (sequential) | — | done: Sp1, Sp2/P17, Sp4 (`sim/activity_speed_tests.rs`), Sp5 and Sp3 engine half (`netcode/enemy_walk_rate_loopback_tests.rs`), speed validation (foundation + both-runtimes scripting-core test). Multiplier on `AgentComponent::activity_speed`, written by AI apply from the post-transition path; `move_speed` stays base. Shared rate rule `any_rate_scaled_animation`: locomotion state plus states of activities whose own motion steers. Changes one shipped state: limitator/rifle-squad `run` now rate-scales (~1.0 at cruise). sim 2000, netcode 1026, ai 423 passed; `postretro` checks with dev-tools. |
| 4 | Typed attacks and lowering skeleton: `defineAttack`/`defineStance` SDK (TS+Luau), binding-name sugar, handle types in typedefs and registry, `scripting-core` lowering (handle resolution, inferred/exhaustive `attacks`, drop fixed point, string and nil rejection), turn facts take handles, optional `when`, Luau strict globals. Migrate shipped scripts and `sdk/behaviors/reference` to handles. T1–T8, S5. | worker (sequential) | — | done: T1, T2, T4, T5, T6, T7, S5, T3 partial (P37, P40, P41c with stances in Task 7), T8 migration half (grep gate in Task 9). Lowering in `scripting-core/data_descriptors/behavior_lowering/`, called from both entity readers before serde; Task 7 seams `prune_to_fixed_point` and `lower_row_targets`. Migration proof: ai oracle (now reads through the Luau entity reader, so it sees the lowered descriptor), 6 `clip_timed::shipped` tests, `dev_mod_budget_paces_reference_enemies_and_troopers`, both scripting-core parity tests. foundation 204, script-compiler 54, scripting-core 1069, sim 2000, ai 424 passed; `postretro` checks. **Unverified:** T4 TS half (`sdk/type-tests/e10-typed-attacks.ts`): `tsc` is not installed. |
| 5a | Runtime pose, hidden release draw, target retention under poses and marks. P1–P8, R1, R2, R10, W8, C2. | integrating executor | 1, 2 | done: `ai/stance_pose_tests.rs` (P1/P30, P2/P33, P3/P34, P4/P32+R1, P5, P6/P23, W8+P39b, P7/P22, P8/P16, R1/R2/P12), C2 host `clip_timed::entering_an_activity_on_the_playing_clip_restarts_nothing` and client `moving_within_a_pose_restarts_no_clip_on_host_or_loopback_client`, R10/P38 in the determinism gate. Pose carry in `enter_activity_at_inner` (transitions only; reseats start fresh); release draw per seat on `HoldRelease`; `"*"` rows skip pose-mates; retention when the path has a `pose` or `attackTurn`. Mutation checks fail 5–6 tests. foundation 206, entities 322, ai 440, sim 2000, netcode 1026 (+ fixed C2 test) passed; `postretro` checks. No shipped behavior change. |
| 5b | Queued rows: `queue` field, request source from a rest, grant lifetime, cooldown gate, precedence, place in line. W1–W7, W9–W11, S24, S25. | integrating executor | 5a | done: W1–W7, W9–W11, S24/P44, S25/P49/P50 in `ai/attack_turns/tests/tick/queued_rows.rs` (17 tests). Gate and candidate come from one row walk in `graph_eval` (`QueueGate`, `TransitionPass.queued`); `next_request` precedence mark > queued > pending unmarked; a queued request counts as marked, so apply never releases its grant and `retain_held` keeps it only once the hold's mark names it. The row reads `attackTurnHeld` (held for any target), which P39a requires. Mutation checks fail 2–16 tests. ai 457, foundation 207, entities 322, sim 2000 passed; netcode compiles; `postretro` checks. |
| 6 | Release exit: `afterMoment`/`atDefault`/`staggerTo`, moment gate closed until resolved or one summed pass, untimed resolution, cancel bit, flinch play-out to ending rest, recover defaults. S12–S19, S21–S25, P9, D3, D4. | integrating executor | 5 | |
| 7 | Stance lowering: expand stances and attacks into rest, holds, release and recover with fields, rows and row order; validation table; co-op floor constant, netcode guard and warning; hand-written fixture; Scripting surface example as `content/dev` content. S1–S4, S6–S11, S20, S26, F1, F2, T6. | worker (sequential) | 4, 5, 6 | |
| 8 | Shipped content: brute recut (Blender, `license.txt`), brute states, stances, lunge and charge, walk/run speeds; limitator roll branch, aim stance and `hit_reaction` break; update pinned tests; tune defaults. K1, K2, D5, Sp6, P4. | integrating executor | 3, 7 | |
| 9 | `docs/` and `sdk/behaviors/reference` stance coverage; grep gates (T8, R9, C3, P4). Doc1. | worker (sequential) | 8 | |
| 10 | Co-op sound proof once `E10--coop-enemy-telegraph-sounds` lands: loopback test. C1. | integrating executor | 7, sounds brief | blocked on prerequisite |
