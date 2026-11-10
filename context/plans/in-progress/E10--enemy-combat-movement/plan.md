# E10--enemy-combat-movement — plan of record

mode: resumable
status: approved
read at: d29ac3a5f

Source re-read at `d29ac3a5f` (106 commits past the brief's `5932287f`). Only the Mixamo brute and shadow-hunter swaps (`22d29d530`, `2bfc3afa2`, `a9e4dcd6d`) touch the cited paths, and they change content and one doc comment, not logic. Every Decision premise holds. The corrections below are Path claims and clarifications that keep each Decision's meaning.

## Corrections
- **Hot reload of `draws`.** Path: "draws resolve from the live descriptor … as `resolved_projectile_attacks` does". → `resolve_projectile_attacks` (`crates/ai/src/brain_programs.rs`) reads the brain's spawn-time `brain.graph` and looks up only the *weapon* descriptor by name. No reload path replaces a live brain's `graph` Arc, so its `AttackParams` are stale after a reload. **Approach:** on a `descriptor_generation` change, `BrainPrograms::sync` resolves each attack's `draws` from the live `EntityTypeDescriptor`, found by the entity's `DescriptorProvenance.canonical_name`, falling back to `brain.graph` when absent. Turns compare against that resolved set. AC rows B26 and B27 keep their meaning.
- **Target switch cancels an execution.** No per-actor cancel exists. `AiWeaponControllers` cancels only on `aim == None`, a different weapon, `cancel_activations()` (global) or `remove_actor`. A target switch today re-aims the remaining shots. **Approach:** add `AiWeaponControllers::cancel_actor(id)`, called when a turn holder's target changes (pin P6).
- **Turn target.** `brain.acquired_target` clears whenever the path stops engaging, so it cannot name a held turn's target. **Approach:** each turn stores its own target `EntityId`, as the Shape already implies.
- **Level change does not reset `AiRuntime`.** It lives on `App` across levels. Level unload calls only `cancel_weapon_activations()` (`crates/postretro/src/startup/lifecycle_net.rs`, `clear_surface_lifetime_level_state`). **Approach:** refreshing counts live in one budget ledger on `AiRuntime`, keyed by (pool, target, token), where the pool is `Default` or an attack-pool `EntityId`. Level unload clears the ledger through a new `AiRuntime::clear_level_state`, which also covers pin P30. This departs from the Shape's "refreshing counts live with the pool's tally": the default pool has no entity, and one ledger avoids two homes. Held counts are recounted each tick from turns on brains, as the Shape says. Pool *state* (running, start rank) stays on the serializable pool component.
- **"Serialize-and-restore".** No save/load system exists (`entity_model.md` §9 non-goal). The mechanism is component serde: `ComponentValue` derives `Serialize`/`Deserialize`, and transient fields use `#[serde(skip)]`. **Proof shape** for rows M8, B38 and pin P33: serialize every component of the involved entities, deserialize into a fresh registry with a fresh `AiRuntime`, then tick. Turns, wait clocks, strafe direction and reposition entry state are `#[serde(skip)]` on `BrainComponent`. Pool running state, start rank and each enemy's pool assignment are serialized fields.
- **Combat ring.** `RING_DIRECTIONS` is a fixed world-axis constant with `RADIAL_MULTIPLIERS = [1.0, 0.75, 1.25]`, and `generated_positions` takes no bearing. The brief already plans a rotated ring for `reposition`. `strafe` also uses one, rotated to its current bearing. Both generate radius multiplier 1.0 only ("at standoff"). "One slot spacing" is new: the ring-neighbor chord at standoff, `2·R·sin(22.5°)`. The existing `min_spacing` is the claimant's capsule diameter, used for batch exclusion, and is unrelated.
- **`scan_challengers` and `apply_hysteresis`.** `scan_challengers` is a `bool` field on `CombatQuery`, set to `retained_slot.is_none()` in `combat_slots.rs`, not a function. `apply_hysteresis` lives in `crates/sim/src/combat_positioning.rs`. Same intent: `reposition` and `strafe` set `scan_challengers` and drop the incumbent themselves.
- **`engages_path` hard-codes `ChaseTarget`** (`graph_eval.rs`) as the only engaged motion. It gains `Strafe` and `Reposition`. `SteeringIntent` (`engine_floor.rs`) keeps its four variants: both new verbs resolve to `MoveTo` toward their claimed slot, or `Hold` when they hold.
- **Directional variants and the animation switch.** `should_switch_animation` (apply) fires only on a state change or a moving-flag edge. A `walk_left` ↔ `walk_right` change has neither. **Approach:** `BrainComponent` keeps the resolved clip name it last played as a transient field, and apply switches when the resolved name differs.
- **Launch facing input.** `AgentPathState` carries only combined `velocity`. **Approach:** add `knockback_velocity` and an airborne flag to `AgentPathState` (`path_state`, `crates/sim/src/agent_steering.rs`). Facing, `locomotion_intent` and the variant choice read steering velocity (`velocity − knockback_velocity`). The impact yaw is latched on the brain as a transient field on the first launched tick. Compute's post-slew yaw and apply's slew both read the same rule, so the attack gate cannot diverge.
- **Launch state persists (task 1).** `BrainComponent::launch` (`LaunchState`: impact yaw, air ticks, airborne-presented latch) is serialized with `#[serde(default)]`, following `locomotion_moving`. With `#[serde(skip)]`, a brain restored mid-flight could strand `jump_loop` on landing. Turn and reposition state stay transient as planned. The walk-cycle playback rate (`update_brain_animation_playback_rates`) also reads steering velocity, so a shove never quickens the stride. The fall-delay count runs until landing, even when drag spends the knockback mid-air, so `jump_loop` never flickers off in flight.
- **Facing seam for task 3.** `crates/ai/src/facing.rs`: `verb_facing_policy(steering, engaged, committed_aim, steering_moving) -> Option<FacingPolicy>` reproduces today's rules, and `facing_direction(policy, steering_velocity, aim, impact_yaw)` applies them. `MoveTo` maps to `Travel` unconditionally, so task 3 resolves authored or verb-default facing *before* falling back to `verb_facing_policy`.
- **Where `facing` and `reposition` resolve.** `motion_for_path` searches the active path deepest-first, and a selector row may supply the motion (`BehaviorSelectorRow { when, motion, action }`, with no facing or params). **Clarification:** `facing` resolves deepest-first, and an activity declaring it covers the motion its own selector layer supplies. A selector-supplied `reposition` uses the default `arc` and `minTravel`. `reposition: {…}` is allowed only on an activity whose own `motion` is `reposition`, matching the brief's rejection row.
- **The reference enemy's location.** Decisions say the slam/jab reference enemy is in `sdk/behaviors/reference`. → It is `content/dev/scripts/reference-enemy.ts` with a `.luau` twin. `sdk/behaviors/reference/entities.{ts,luau}` holds `pose_fixture_enemy` (a `jab`) and two driver stubs. The shadow hunter (`content/dev/scripts/shadow-hunter.ts`), limitator (`limitator.ts`) and rifle squads (`rifle-squads.ts`, which spread `...limitator`) are TS only. **Approach:** the reference enemy's slam and jab draws land in `content/dev` in both twins. `sdk/behaviors/reference` adopts the vocabulary on `pose_fixture_enemy`'s jab, so every SDK bundle shows `draws`. `enemy_rifle_burst` and the `walk_left`/`walk_right`/`walk_backward` bindings are new content.
- **Level content digest.** `level_content_digest` (`crates/postretro/src/runtime_movers.rs`) hashes only prediction inputs and never id-29 key/values, so `budget_profile` needs no exclusion code. Row A13 is a test next to `level_content_digest_*`.
- **Compatibility digest.** `mod_compatibility_digest` takes explicit inputs and never sees descriptors or vocabularies. The tokens stay out by omission. Add a "Named skip" line on `mod_compatibility_digest_from_registry`.
- **Pool request timing (P15).** Traps fold into `ProjectileTrapComponent.pending` and act in the trap stage *after* `run_ai` (`crates/sim/src/sim/mod.rs`). **Approach:** a pool keeps an ordered pending-request list, not a bounded set, because a re-`start` moves its rank. Requests drain immediately before `run_ai`, after the trigger fires. Trigger-bound steps therefore apply on their own tick, and every other source latches to the next.
- **Binding-name sugar has a second list.** `crates/script-compiler/src/light_membership.rs` (`DATA_SCRIPT_FIELDS`) duplicates the definer inventory. `defineAttackToken` joins both lists and the Luau lists in `luau_prelude.rs` and `luau_require.rs`.
- **Map members need no PRL change.** The id-29 record is a generic `{classname, origin, angles, key_values, tags}` bag. The runtime dispatches by classname (`register_builtins`, `crates/sim/src/scripting/builtins/mod.rs`). `TriggerCommand::Start`/`Stop` already exist. `ai_attack_pool` adds a `MapMemberKind` variant, a `ComponentKind` (discriminant 28), a builtin handler and `COMMAND_KINDS` routing.

## Delegated answers
- **`strafe` direction on first entry.** Score both neighbor slots on the rotated ring with the same `score_candidate` the batch uses. Take the better-scored one. An exact tie, or both unusable, takes counter-clockwise seen from above (+yaw). Fully deterministic, with no RNG, and the same rule re-picks after a restore (pin P17).

## Approach notes (non-binding, for resumption)
- **Turn state** on `BrainComponent`, all `#[serde(skip)]`: `held_turns: SmallVec<[HeldTurn; 2]>` (`attack`, `target`, `pool`, `draws` resolved at grant, `release_rule` flags: marked / cooldown / execution), `pending_request: Option<TurnRequest>` (`attack`, `draws`), and `wait_since_tick: Option<u64>` with its target and pool.
- **Arbitration** is a new batch pass, `arbitrate_attack_turns`, between `resolve_combat_slots` and `apply_outcomes` in `run_ai_tick_with_host`. It runs per (pool, target): recount held counts from brains, add refreshing counts from the ledger, sort waiters by `(wait_since_tick, slot index)`, assign head-of-line reservations per token (P32), then grant in order against running counts (P31). A refused `PendingAttack` is cleared before apply, so P2 holds by construction. It is a new module, `crates/ai/src/attack_turns/` (requests, arbitration, ledger, facts). `compute.rs` is at 641 lines and takes only call sites.
- **Facts** refresh in compute from the previous tick's state (P7). `attackTurnFree` runs the same fit check against last tick's counts and reservations, leaving out the reader's own request.
- **Turn-fact scripting path:** `BrainInputRef::AttackTurnHeld` / `AttackTurnFree` projecting `IrType::Bool` in both `BrainValidationScope` and the runtime `BrainScope`, SDK helpers in `sdk/lib/brain.{ts,luau}`, `BRAIN_LUAU_FIELDS` and typedef templates, and an attack-name check beside `check_guard_layer_reads` (`crates/scripting-core/src/runtime/types.rs`). `hasAttackTurn` appends at `BRAIN_INPUTS` slot 20 with a slot-pin test.
- **Tokens:** a `HandleRef::AttackToken` variant (`crates/foundation/src/defense.rs`) and an `AttackBudgetVocabulary` beside `DefenseVocabulary` (`from_manifest`, drains in both runtimes, `ModManifestResult`/`StagedManifest`/transfer, commit hook mirroring `commit_defense_vocabulary`). `AttackParams` gains `draws: Option<Vec<TokenDraw>>`.

## AC-to-proof

Test homes: `crates/ai/src/attack_turns/tests/` (budget, arbitration and facts), `crates/ai/src/ai_tests/` (motion, facing and knockback through the tick), `crates/scripting-core` (twin-runtime validation), `crates/level-compiler` (`--bin prl-build`, FGD validation), and `crates/sim` (pool commands and spawner). Test names are proposals.

**Motion and facing**

| AC | Proof | Status |
|---|---|---|
| M1 strafe orbits at standoff, faces, fires mid-move | `strafe_orbits_at_standoff_facing_target_and_fires_when_gates_pass` | achievable as stated |
| M2 strafe reverses; holds with no neighbor, no flip; batch loser reverses once (P9) | `strafe_reverses_at_blocked_occupied_or_blind_slot`, `strafe_holds_without_flipping_when_both_neighbors_unusable`, `strafe_batch_loser_reverses_once` | achievable as stated |
| M3 reposition ≥ one spacing from entry position and slot, after `hold`, with no slot; re-entry picks again; holds with none | `reposition_arrives_one_spacing_from_entry_position_and_slot` (variants: after hold, no held slot, re-entry), `reposition_holds_without_oscillating_when_no_slot_qualifies` | achievable as stated |
| M4 arc bounds 45–135 / 90–90 / 180–180 / empty arc holds | `reposition_arc_bounds_select_ring_slots_on_both_sides` (tests generated ring index angles) | achievable as stated |
| M5 arc and entry position fixed while target moves; re-entry re-measures (P16) | `reposition_entry_bearing_and_position_stay_fixed_while_target_moves` | achievable as stated |
| M6 `minTravel` 8: refuses 7 m, accepts 9 m; unreachable holds without flipping | `reposition_min_travel_refuses_near_and_accepts_far_slot`, `reposition_min_travel_beyond_every_candidate_holds` | achievable as stated |
| M7 two repositioners, same tick, bearing and arc → distinct slots, replay identical | `reposition_twins_claim_distinct_slots_deterministically` | achievable as stated |
| M8 serialize-and-restore mid-reposition and mid-strafe (P17) | `restore_mid_reposition_and_strafe_records_fresh_entry` (component serde round-trip; see Corrections) | achievable as stated |
| M9 validation rejects bad `arc` / `minTravel` / misplaced `reposition`, pathed, both runtimes | `reposition_params_reject_with_pathed_error_{js,luau}` (scripting-core) + foundation descriptor validation unit tests | achievable as stated |
| M10 `facing: travel` on reposition; `facing: target` on chase; unchanged default on every existing verb | `activity_facing_policy_overrides_verb_default`, `existing_verbs_face_as_before_without_facing` (one case per `MotionVerb::ALL` entry) | achievable as stated |
| M11 directional variants under target facing; `walk` under travel facing; missing variants → `walk` | `target_facing_travel_selects_directional_variant`, `travel_facing_plays_base_clip`, `missing_variant_falls_back_to_base_clip` | achievable as stated |
| M12 strafe after reposition steps 45° from current bearing, no snap | `strafe_after_reposition_steps_from_current_bearing` | achievable as stated |
| M13 reposition from above target uses held slot or ignores arc; inward/outward slots never qualify | `reposition_above_target_measures_from_held_slot_or_ignores_arc`, `reposition_never_picks_radial_slots` | achievable as stated |

**Knockback**

| AC | Proof | Status |
|---|---|---|
| K1 launched under non-target facing keeps impact yaw, steering still runs | `launched_enemy_holds_impact_yaw_while_steering_runs` | achievable as stated |
| K2 under target facing, slews at the facing rate | `launched_target_facing_enemy_slews_at_facing_rate` | achievable as stated |
| K3 airborne launch → `jump_loop` after fall delay, ground clip on landing; horizontal shove keeps ground clip (P10) | `airborne_launch_plays_jump_loop_after_fall_delay_then_lands`, `horizontal_shove_keeps_ground_clip` | achievable as stated |

**Budget** (B1–B39, in brief order)

| AC | Proof | Status |
|---|---|---|
| B1 no budgets or draws: `defaultToken` at 3, fourth waits | `default_token_admits_three_and_queues_fourth` | achievable as stated |
| B2 limit 2, third never starts until release (P1) | `third_requester_waits_until_a_holder_releases` | achievable as stated |
| B3 three requesters, one free → exactly one, replay same | `one_free_turn_grants_exactly_one_deterministically` | achievable as stated |
| B4 cost 2 alone / refused beside cost 1 / cost 3 when empty; `draws: []` never counts (P8) | `cost_fits_against_limit_and_oversized_draw_admits_when_empty`, `undrawn_attack_never_counts_or_blocks` | achievable as stated |
| B5 cost-3 waiter among rotating cost-1 granted in bounded wait, none pass it (P12) | `oversized_head_of_line_waiter_is_granted_within_bounded_wait` | achievable as stated |
| B6 melee 4 / heavy 1: second slam reserves, jabs pass, granted on release (P14) | `waiting_slam_reserves_melee_and_heavy_while_jabs_pass` | achievable as stated |
| B7 slam and rocket sharing `heavy` never overlap on one target | `heavy_draws_from_two_archetypes_never_overlap` | achievable as stated |
| B8 head of line per token (P18, P32) | `head_of_line_reserves_per_token_in_wait_order` | achievable as stated |
| B9 multi-token waiter among single-token traffic (P27) | `multi_token_waiter_is_granted_among_cycling_single_token_traffic` | achievable as stated |
| B10 fit judged against this tick's earlier grants (P31) | `grants_within_a_tick_see_earlier_grants_and_reservations` | achievable as stated |
| B11 one enemy's turns overlap on separate tokens; second melee waits, wait starts then | `one_enemy_holds_melee_and_ranged_turns_at_once` | achievable as stated |
| B12 refresh 4 s on heavy while jabs flow (P26) | `heavy_refresh_spaces_slams_while_jabs_draw_melee` | achievable as stated |
| B13 refreshing count counts for admission and head of line (P28) | `refreshing_count_holds_admission_and_head_of_line` | achievable as stated |
| B14 interrupted turn still refreshes (P29: no start, target switch, killed, reload) | `interrupted_turn_spends_its_refresh` (four cases) | achievable as stated |
| B15 refresh lifetime (P30) | `refresh_lands_in_pool_at_turn_end_and_drops_on_stop_level_and_target_loss` | achievable as stated |
| B16 `attackTurnFree` false during refresh, true the tick after lapse (P7) | `attack_turn_free_reads_refresh_with_one_tick_lag` | achievable as stated |
| B17 profile override changes only named fields | `profile_override_keeps_unnamed_fields_and_tokens` | achievable as stated |
| B18 every target its own allowance; ally and hostile on one monster share | `each_target_has_its_own_allowance` | achievable as stated |
| B19 `defaultToken` unlisted, overridable; listing it rejects | `default_token_needs_no_registration_and_rejects_when_listed_{js,luau}` + arbitration case | achievable as stated |
| B20 grenade/claw switch while waiting (P19) | `waiter_switching_attack_keeps_clock_and_frees_dropped_token` | achievable as stated |
| B21 waiter switching off a blocking draw (P24) | `waiter_switching_to_a_fitting_draw_unblocks_token_same_tick` | achievable as stated |
| B22 marked sibling switch queues behind existing waiters (P21) | `marked_sibling_switch_requests_behind_older_waiters` | achievable as stated |
| B23 marked switch mid-execution holds until clear (P20) | `marked_switch_mid_execution_holds_turn_until_clear` | achievable as stated |
| B24 request names the attack the fire step resolves (P25) | `request_names_fire_step_attack_not_cooldown_seed_attack` | achievable as stated |
| B25 waiter clock across pause, target switch (P22) | `wait_clock_survives_gaps_and_restarts_on_target_switch` (with pool-change, AI-set exit and reseat variants) | achievable as stated |
| B26 reload lowering limit revokes nothing; removed profile → default; lowered `refreshMs` applies | `reload_lowering_budgets_revokes_no_turn_and_applies_next_tick` | achievable as stated |
| B27 reload changing resolved draws releases holders; resolved comparison | `reload_changing_resolved_draws_releases_holders` (via the live descriptor; see Corrections) | achievable as stated |
| B28 refused attack commits nothing, fires on first granted tick (P2) | `refused_attack_commits_nothing_and_fires_when_granted` | achievable as stated |
| B29 limit 1, three contact attackers: ≤1 holds, cooldown holds after leaving, bounded window | `contact_attackers_take_bounded_turns_under_limit_one` | achievable as stated |
| B30 two unmarked attacks on one token never overlap | `unmarked_attacks_sharing_a_token_never_overlap` | achievable as stated |
| B31 three-shot weapon holds through execution and cooldown (P4) | `unmarked_burst_holds_turn_through_execution_and_cooldown` (first enemy multi-shot test) | achievable as stated |
| B32 granted unmarked attack that does not start releases (P3) | `granted_unmarked_attack_that_cannot_start_releases_same_tick` | achievable as stated |
| B33 out of range or line of sight requests nothing | `unmarked_attack_requests_only_when_fire_gates_pass` | achievable as stated |
| B34 marked siblings hold one turn without a gap; leaving mid-burst holds until clear | `marked_chain_holds_one_turn_without_regrant_gap` | achievable as stated |
| B35 target switch mid-burst cancels and releases, no shot on new target (P6) | `target_switch_mid_burst_cancels_execution_and_releases` (new `cancel_actor`) | achievable as stated |
| B36 no leak across every P5 release case; returner re-requests | `turn_releases_on_every_ai_set_exit` (quiescence, despawn, inventory, killed in pass, aggro close, reseat) | achievable as stated |
| B37 the three facts per P7/P23 | `turn_facts_follow_pins` | achievable as stated |
| B38 serialize-and-restore (P11) | `restore_drops_turns_waits_and_refresh_but_keeps_pools` (component serde round-trip) | achievable as stated |
| B39 compatibility digest ignores tokens, budgets, draws | `compatibility_digest_ignores_attack_budget_fields` (`mod_digest.rs`) | achievable as stated |

**Attack pools** (A1–A13; pin ids stay P1–P34)

| AC | Proof | Status |
|---|---|---|
| A1 trigger `start` and script `start` both apply profile; empty → default; undeclared warns and uses default | `attack_pool_start_by_trigger_and_script_applies_profile`, `undeclared_budget_profile_warns_at_load` (log capture) | achievable as stated |
| A2 separate pools grant independently; head of line never crosses pools | `attack_pool_and_default_pool_arbitrate_independently` | achievable as stated |
| A3 restart re-assigns; held state stands; client start inert | `restarting_a_running_pool_reassigns_tag_holders`, `attack_pool_start_on_client_changes_nothing` | achievable as stated |
| A4 two pools run; stop returns to default same tick; stop on stopped is no-op | `two_pools_run_side_by_side_and_stop_independently` | achievable as stated |
| A5 membership by start and tagged spawn; later tag edits move no one; shared `member_tag` compiles | `pool_membership_is_assigned_by_start_and_spawn_only` (sim), compile case in `prl-build` | achievable as stated |
| A6 latest assignment wins; issue order (P15); spawn on two pools (P33); spawn on stop tick (P34) | `latest_pool_assignment_wins`, `spawn_matching_two_pools_joins_latest_started`, `spawn_on_pool_stop_tick_follows_issue_order` | achievable as stated |
| A7 leaving a pool keeps the held turn, counted in the new pool (P13) | `enemy_changing_pool_keeps_turn_counted_in_new_pool` | achievable as stated |
| A8 validation set (compile errors and pathed rejects, both runtimes) | `prl-build`: `ai_attack_pool_empty_member_tag_is_compile_error`, `ai_attack_pools_sharing_tags_is_compile_error`; scripting-core: `attack_budget_declarations_reject_with_pathed_errors_{js,luau}` (table of cases); TS reference typo: typedef pins `draws` to `AttackToken \| AttackTokenRef` (no `string`), plus the undeclared binding being a TS unresolved name, the same standard as `getMapEntities`' unknown kind. No tsc gate runs in CI | achievable as stated |
| A9 no `ai_attack_pool` → default pool alone | covered by B1–B39 (no pool) + `level_without_attack_pool_uses_default_pool` | achievable as stated |
| A10 start/stop timing (P15) | `trigger_bound_pool_start_applies_before_arbitration`, `pool_start_and_stop_same_tick_apply_in_issue_order` | achievable as stated |
| A11 level restart or change: no pool running, default from first tick | `level_change_stops_attack_pools_and_clears_ledger` | achievable as stated |
| A12 reload changing `default` reaches a pool using default | `reload_changing_default_profile_reaches_pool_next_tick` | achievable as stated |
| A13 level digest ignores `budget_profile` | `level_content_digest_ignores_attack_pool_profile` (`runtime_movers.rs`) | achievable as stated |

**Surface**

| AC | Proof | Status |
|---|---|---|
| S1 surface example parses identically in TS and Luau, runs as `content/dev`; arena guard in pool; reference slams within 10 s, ≥4 s apart, jab granted during slam; ≤2 ranged turns on three troopers | `scripting_surface_example_parses_identically_in_both_runtimes` (scripting-core) + sim integration `dev_mod_budget_paces_reference_enemies_and_troopers` driving the shipped `content/dev` descriptors headlessly | achievable as stated |

**Manual**

| AC | Proof | Status |
|---|---|---|
| MAN1 `movement-feel` rifle squad relocate/strafe/take over; `closet-reveal` payload without a pile-on; rocket-launched reference enemy keeps facing | owner, in-engine | manual |
| MAN2 co-op loopback shows the same strafing, directional clips and launch facing | owner, two instances on loopback | manual |

**Not an AC: perf row** (Path: "Add a wave-scale stress row for arbitration"). An `#[ignore]`-gated measurement times `arbitrate_attack_turns` at 64 and 128 engaged agents on one target with three tokens, plus a zero-allocation probe on the steady-state pass (`alloc_probe`). It records its numbers in this plan beside the `perf-ai-tick-cost` draft's `sim_ai` baseline, which has no engaged-agent row yet.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Knockback facing split** (riskiest seam: separating steering velocity from knockback velocity, which strafe also needs): extend `AgentPathState`; latch impact yaw; facing, `locomotion_intent` and the attack gate read steering velocity; `jump_loop` after `FALL_DELAY_TICKS`. Rows K1–K3 | integrating executor | — | done: `launch.rs`, `facing.rs` seam; `-p postretro-ai` 257 passed (4 `launch` + `horizontal_shove_keeps_ground_clip`); sim agent_steering/playback/locomotion 160 passed |
| 2 | **Descriptor surface for motion**: `MotionVerb::{Strafe, Reposition}`, `facing`, `reposition { arc, minTravel }`, `attackTurn` field (parsed only), validation in foundation and both runtimes, SDK types, typedefs and generated `sdk/types`. Row M9 | integrating executor | 1 | |
| 3 | **Rotated ring and per-query constraints**: `CombatQuery` gains a ring bearing and per-query constraints (arc, min travel, exclusions, standoff multiplier only). Strafe and reposition steering with transient entry state; `engages_path`; facing policy resolution; directional variants plus the resolved-clip switch. Rows M1–M8, M10–M13 | integrating executor | 2 | |
| 4 | **Turns and arbitration on `defaultToken`**: the `attack_turns` module (requests, marked and unmarked rules, release cases, wait clocks, single-token arbitration with cost, hard gate before apply), `cancel_actor`, the three facts and the scripting path. Rows B1–B5, B10, B11, B22–B25, B28–B38 | integrating executor | 2 | |
| 5 | **Tokens**: `defineAttackToken`/`defaultToken`/`with`, the `attackToken` handle kind, `draws`, `attackTokens` registration, binding-name sugar (all four lists), twin-runtime validation, multi-token head of line and cost, digest skip. Rows B6–B9, B18–B21, B39, A8 (token half) | integrating executor; the SDK twin (TS and Luau `data_script`, typedefs) may go to one worker once 4's contracts are fixed | 4 | |
| 6 | **Profiles, refresh and reload**: `attackBudgets`, the refresh ledger on `AiRuntime`, `clear_level_state`, live-descriptor `draws` on reload. Rows B12–B17, B26, B27 | integrating executor | 5 | |
| 7 | **`ai_attack_pool`**: FGD, compiler validation, `MapMemberKind`, component plus builtin, `COMMAND_KINDS`, script `start`/`stop` step primitives and validators, `getMapEntities("attackPool")` twins, the pre-AI request drain, spawn joining, undeclared-profile warning. Rows A1–A13 | worker (sim, compiler and SDK plumbing, against task 6's pool-id contract), integrated and tested by the executor | 6 | |
| 8 | **Shipped content**: dev mod `melee`/`ranged`/`heavy`; reference enemy (both twins), shadow hunter, limitator and troopers with strafe-while-waiting, fire-then-reposition, `enemy_rifle_burst`; Mixamo `walk_left`/`walk_right`/`walk_backward` bindings; `sdk/behaviors/reference` adoption; surface-example and pacing tests. Row S1; enables MAN1–MAN2 | integrating executor | 3, 7 | |
| 9 | **Arbitration stress row**: the `#[ignore]` measurement and alloc probe above; record numbers | integrating executor | 6 | |
| 10 | Preflight, review panel and fix loop; full gate; results column | integrating executor | 1–9 | |

Hot paths touched: the per-tick AI pass (arbitration adds one batch pass; no per-tick allocation in steady state, so turn and waiter vectors are reused scratch on `AiRuntime`), combat-slot scoring (the rotated ring replaces, never adds to, the generated set for the two verbs), and apply (one clip-name compare per brain).
