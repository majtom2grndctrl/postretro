# E18 deferred review dispatch manifest

**STOP: metadata only; review has not started.** Owner requested a pause after workers finish. Do not dispatch until the owner resumes and coordinator confirms readiness (fmt, touched-crate check and focused seam tests). No source diff was read and no findings were produced to prepare this file. No builds, source edits, plan edits, commits or agent spawns were performed.

Target: entire `codex/player-life-states` feature against base `a953f386f`, including committed Tasks 1–6 and uncommitted/untracked Task 7. Snapshot inventory: **216 tracked files / 14,437 changed lines**, plus **10 untracked files / 2,065 lines** = **226 files / 16,502 lines**. Counts are additions + deletions (untracked counts are full line counts). Refresh status/numstat before dispatch; working tree may change. File names and brief/research/plan supplied boundaries; entry points below are hypotheses for later triage, not source-verified implementation conclusions.

Read: `context/lib/index.md`, `.agents/skills/review-panel/SKILL.md`, `.agents/skills/model-guidance.md`, and full E18 `index.md`, `research.md`, `plan.md`. Scope follows review-panel partition rules. Most logic slices are about 1,000–1,500 lines; 01/04/07 are near 1,000 to retain coherent boundaries, 06 is 1,503. Keep tests with their relevant production slice; 12 intentionally owns cross-system conditioned proof with adjacent production reads.

Dispatch policy after resume/readiness: `reviewers:1` = **exactly one depth worker plus one separate hygiene/drift worker per logic slice** (01–13). Depth: GPT-6.1 Sol xhigh; hygiene: GPT-6.1 Sol medium. Mechanical-only 14/15: GPT-6 Luna medium hygiene alone. Never combine depth and hygiene in one worker. Use fresh isolated context with full shared preamble and verbatim lens prompt from the skill, plus this target and slice brief. Hygiene receives full code-review skill as prescribed. Each worker reads routed development/testing context and full E18 requirements. With four concurrency slots, dispatch bounded batches; no worker builds. Any eventual build requires the owner’s rustc/prl-build process precheck and root coordination.

All paths below are repository-relative. Changed-line ownership is exhaustive and disjoint; adjacent reads are allowed for named flows. Modules and mixed entry files may serve multiple flows: do not pretend path partitioning removes their cross-slice dependencies.

## 01 Descriptor and state foundations — 924 lines / 19 files

**Lens:** Contract verifier; separate hygiene + drift.
**Likely entry:** components.lifeStates → neutral raw validator → lowered EntityTypeDescriptor / DataRegistry name table.
**Scope:** AC1, AC3; O26/O36/O42 table consumers exit to 07/08. Validate both VM readers, defaults, full error paths and global 255-name cap.

- `crates/entities/src/data_descriptors/mod.rs` (2)
- `crates/entities/src/data_descriptors/types/entity.rs` (1)
- `crates/entities/src/data_descriptors/types/life_states.rs` (36)
- `crates/entities/src/data_registry.rs` (5)
- `crates/entities/src/lib.rs` (4)
- `crates/entities/src/life_state_table.rs` (193)
- `crates/foundation/src/data_descriptors/validate/consequential.rs` (31)
- `crates/foundation/src/data_descriptors/validate/consequential_life_state_tests.rs` (34)
- `crates/foundation/src/seat.rs` (2)
- `crates/scripting-core/src/data_descriptors/js/entity.rs` (8)
- `crates/scripting-core/src/data_descriptors/js/reactions.rs` (8)
- `crates/scripting-core/src/data_descriptors/life_states.rs` (285)
- `crates/scripting-core/src/data_descriptors/lua/entity.rs` (11)
- `crates/scripting-core/src/data_descriptors/lua/reactions.rs` (8)
- `crates/scripting-core/src/data_descriptors/mod.rs` (6)
- `crates/scripting-core/src/data_descriptors/tests/life_state_commands.rs` (69)
- `crates/scripting-core/src/data_descriptors/tests/life_states.rs` (210)
- `crates/scripting-core/src/data_descriptors/tests/mod.rs` (2)
- `crates/scripting-core/src/data_descriptors/validate/group_target.rs` (9)

## 02 SDK and command binding — 1,321 lines / 30 files

**Lens:** Contract verifier; separate hygiene + drift.
**Likely entry:** SDK playerVerbs / inLifeState → consequential validation → bound trigger and runtime handlers.
**Scope:** AC7–AC9; O5/O15/O39. Check TS/Luau twins and post-wait runtime execution; compare generated mirrors in 15 as adjacent contract inputs.

- `crates/scripting-core/src/ir/addressing_sdk_tests.rs` (47)
- `crates/scripting-core/src/ir/life_state_inputs.rs` (27)
- `crates/scripting-core/src/ir/life_state_inputs_tests.rs` (128)
- `crates/scripting-core/src/ir/player_scope.rs` (35)
- `crates/scripting-core/src/ir/scopes.rs` (26)
- `crates/scripting-core/src/lib.rs` (4)
- `crates/scripting-core/src/luau_prelude.rs` (2)
- `crates/scripting-core/src/player_slots.rs` (25)
- `crates/scripting-core/src/reaction_dispatch/mod.rs` (6)
- `crates/scripting-core/src/reaction_dispatch/tests.rs` (8)
- `crates/scripting-core/src/typedef/common.rs` (12)
- `crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` (16)
- `crates/scripting-core/src/typedef/templates/sdk_lib.luau` (11)
- `crates/scripting-core/src/typedef/templates/virtual_module.luau` (2)
- `crates/sim/src/life_state_commands.rs` (96)
- `crates/sim/src/life_state_commands/tests.rs` (521)
- `crates/sim/src/scripting/primitives/life_states.rs` (43)
- `crates/sim/src/scripting/primitives/mod.rs` (3)
- `crates/sim/src/scripting/typedef/tests/committed.rs` (3)
- `crates/sim/src/scripting/typedef/tests/life_state_surface.rs` (123)
- `crates/sim/src/scripting/typedef/tests/mod.rs` (1)
- `crates/sim/src/trigger_bindings/command_binding.rs` (21)
- `crates/sim/src/trigger_bindings/group_tick_tests.rs` (49)
- `crates/sim/src/trigger_bindings/partition.rs` (2)
- `crates/sim/src/trigger_commands.rs` (53)
- `sdk/lib/data_script.luau` (14)
- `sdk/lib/data_script.ts` (6)
- `sdk/lib/data_script/commands.luau` (11)
- `sdk/lib/data_script/commands.ts` (24)
- `sdk/lib/index.ts` (2)

## 03 Reload remap and install lifecycle — 1,377 lines / 21 files

**Lens:** Correctness tracer; separate hygiene + drift.
**Likely entry:** commit_staged_manifest_result → refresh planner → replace_entity_types → authority-only remap → spawn/restore.
**Scope:** AC3/AC16; O8/O14/O34/O37/O39/O41. Trace kept/removed/new/undeclared names through all planner outcomes and descriptor restoration; rejoin lifecycle exits to 09.

- `crates/postretro/src/mod_digest.rs` (1)
- `crates/postretro/src/session/mod.rs` (17)
- `crates/postretro/src/startup/life_state_validation.rs` (90)
- `crates/postretro/src/startup/loading_screen.rs` (1)
- `crates/postretro/src/startup/mod.rs` (3)
- `crates/postretro/src/startup/reaction_validation.rs` (1)
- `crates/postretro/src/startup/session.rs` (2)
- `crates/scripting-core/src/life_state_remap.rs` (68)
- `crates/scripting-core/src/life_state_remap/tests.rs` (432)
- `crates/scripting-core/src/refresh_plan.rs` (10)
- `crates/scripting-core/src/runtime/core.rs` (3)
- `crates/scripting-core/src/runtime/life_state_manifest_tests.rs` (127)
- `crates/scripting-core/src/runtime/mod_init_exec.rs` (17)
- `crates/scripting-core/src/runtime/types.rs` (1)
- `crates/scripting-core/src/staged_manifest.rs` (12)
- `crates/sim/src/scripting/builtins/data_archetype.rs` (30)
- `crates/sim/src/scripting/builtins/data_archetype_test_fixtures.rs` (1)
- `crates/sim/src/scripting/builtins/host_replicated.rs` (2)
- `crates/sim/src/scripting/builtins/net_descriptor.rs` (6)
- `crates/sim/src/scripting/builtins/wieldable_inventory.rs` (2)
- `crates/sim/src/sim/life_state_restore_tests.rs` (551)

## 04 Physics capability and arrival — 993 lines / 18 files

**Lens:** Correctness tracer; separate hygiene + drift.
**Likely entry:** enter_life_state / restamp_life_state → movement intent and teleport::resolve / shared arrival tail.
**Scope:** AC2/AC4–AC6/AC17; O13/O29/O40/O43. Follow stamps through vault cancellation, forced posture, speed channels, latch pruning and arrival ground classification; replay enters from 08.

- `crates/foundation/src/lib.rs` (9)
- `crates/foundation/src/movement/life_state.rs` (161)
- `crates/foundation/src/movement/mod.rs` (2)
- `crates/foundation/src/movement/player_movement.rs` (44)
- `crates/physics/src/movement/dispatch.rs` (3)
- `crates/physics/src/movement/intents/crouching.rs` (10)
- `crates/physics/src/movement/intents/dash.rs` (9)
- `crates/physics/src/movement/intents/normal.rs` (6)
- `crates/physics/src/movement/intents/slide.rs` (13)
- `crates/physics/src/movement/intents/swim.rs` (2)
- `crates/physics/src/movement/life_capabilities.rs` (132)
- `crates/physics/src/movement/mod.rs` (12)
- `crates/physics/src/movement/swim_tests.rs` (37)
- `crates/physics/src/movement/swim_transition.rs` (22)
- `crates/physics/src/movement/teleport.rs` (59)
- `crates/physics/src/movement/teleport_arrival_tests.rs` (211)
- `crates/physics/src/movement/teleport_tests.rs` (58)
- `crates/physics/src/movement/tests/life_states.rs` (203)

## 05 Simulation and weapon capability gates — 1,422 lines / 30 files

**Lens:** Correctness tracer; separate hygiene + drift.
**Likely entry:** simulate_tick_with_presentation_aim → Triggers-stage capability capture → touch/pickup → Weapons-stage gates / death sweep.
**Scope:** AC2/AC4/AC6; O11/O12/O16/O35. Trace one transition during trigger dispatch into immutable stage capture and subsequent weapon admission, including switch/reload and damage eligibility.

- `crates/ai/src/ai_tests.rs` (1)
- `crates/ai/src/brain_programs.rs` (1)
- `crates/postretro/src/client_weapon/frame.rs` (59)
- `crates/postretro/src/client_weapon/life_state_tests.rs` (91)
- `crates/postretro/src/client_weapon/tests.rs` (2)
- `crates/postretro/src/fluid_presentation/tests.rs` (6)
- `crates/postretro/src/ghost_links/opt_ins_test_fixtures.rs` (1)
- `crates/postretro/src/ghost_links/replication_test_fixtures.rs` (3)
- `crates/postretro/src/host_activations.rs` (9)
- `crates/postretro/src/runtime_teleporters/client_tick.rs` (4)
- `crates/postretro/src/scripting/frame_systems/mesh_render.rs` (1)
- `crates/postretro/src/sound_events/descriptors.rs` (1)
- `crates/sim/src/ai_host.rs` (2)
- `crates/sim/src/lib.rs` (2)
- `crates/sim/src/scripting_systems/health.rs` (20)
- `crates/sim/src/sim/frame_timing.rs` (2)
- `crates/sim/src/sim/host_movement.rs` (12)
- `crates/sim/src/sim/mod.rs` (237)
- `crates/sim/src/sim/player_life.rs` (199)
- `crates/sim/src/sim/player_life_tests.rs` (623)
- `crates/sim/src/sim/teleport_tick_tests.rs` (1)
- `crates/sim/src/sim/touch.rs` (33)
- `crates/sim/src/sim/weapon_stage.rs` (1)
- `crates/sim/src/sim/weapon_stage/commands/local.rs` (18)
- `crates/sim/src/sim/weapon_stage/commands/remote.rs` (7)
- `crates/sim/src/sim/weapon_stage/machine.rs` (9)
- `crates/sim/src/traps/trap_test_fixtures.rs` (1)
- `crates/sim/src/trigger_system.rs` (33)
- `crates/sim/src/weapon/activation_prediction.rs` (22)
- `crates/sim/src/weapon/pawn_gate.rs` (21)

## 06 Simulation respawn and tick start — 1,503 lines / 5 files

**Lens:** Correctness tracer; separate hygiene + drift.
**Likely entry:** respawn enqueue → coalesced pending queue → sim respawn drain and deterministic placement selection.
**Scope:** AC8/AC11/AC13; O3/O4/O17–O19/O21/O32. Follow state-by-name reload handling, foot anchor, geometry/velocity/latch resets and seat-order overlap selection. Tick-start integration is 10.

- `crates/sim/src/respawn_commands.rs` (101)
- `crates/sim/src/respawn_commands/tests.rs` (385)
- `crates/sim/src/sim/respawn.rs` (185)
- `crates/sim/src/sim/respawn_boundary_tests.rs` (88)
- `crates/sim/src/sim/respawn_tests.rs` (744)

## 07 Network tables and tuning — 912 lines / 12 files

**Lens:** Contract verifier; separate hygiene + drift.
**Likely entry:** tuning_payload_for_pawn / encode_tuning_payload → install_tuning_payload → table history → materialize_armed_local_pawn.
**Scope:** AC4/AC16; O14/O26/O36/O42. Verify payload schema/version gates, name/profile/revision agreement, history pinning and same-revision replacement. Reconcile consumption is 08; backlog proof is included.

- `crates/net/src/handshake.rs` (11)
- `crates/net/src/replication.rs` (3)
- `crates/net/src/wire.rs` (32)
- `crates/net/src/wire/components.rs` (3)
- `crates/netcode/src/client_tuning_install_tests.rs` (153)
- `crates/netcode/src/client_tuning_install_tests/life_state_backlog.rs` (325)
- `crates/netcode/src/endpoint.rs` (24)
- `crates/netcode/src/life_state_tables.rs` (263)
- `crates/netcode/src/lifecycle.rs` (3)
- `crates/netcode/src/remote_materialize.rs` (15)
- `crates/netcode/src/tests/fixtures/tuning_payload.expected.json` (2)
- `crates/netcode/src/tuning_payload.rs` (78)

## 08 Reconcile and network gates — 1,214 lines / 29 files

**Lens:** Correctness tracer; separate hygiene + drift.
**Likely entry:** client_receive_and_apply → reconcile_local_pawn_with_mover_history → merge/restamp/replay → correction class.
**Scope:** AC4/AC17/AC18; O9/O13/O14/O24/O26/O36/O42/O43. Trace missing-table preservation and elapsed rebasing before replay, then smoothing; owner-private reset evidence exits to 11.

- `crates/netcode/src/client.rs` (3)
- `crates/netcode/src/client/ghost_link_tests.rs` (3)
- `crates/netcode/src/client/late_baseline_tests.rs` (1)
- `crates/netcode/src/client/teleporter_tests.rs` (3)
- `crates/netcode/src/client_switch_life_state_tests.rs` (69)
- `crates/netcode/src/client_switch_relay_test.rs` (5)
- `crates/netcode/src/e18_timed_reaction_replication_harness_test.rs` (1)
- `crates/netcode/src/enemy_replication_harness_test.rs` (2)
- `crates/netcode/src/host_switch_timing.rs` (7)
- `crates/netcode/src/ingest_hit_harness_test.rs` (1)
- `crates/netcode/src/lib.rs` (124)
- `crates/netcode/src/life_state_elapsed.rs` (25)
- `crates/netcode/src/movement_state.rs` (22)
- `crates/netcode/src/predict_reconcile_harness_test_fixtures.rs` (1)
- `crates/netcode/src/predict_reconcile_loopback_test_fixtures.rs` (13)
- `crates/netcode/src/predict_reconcile_relay_harness.rs` (2)
- `crates/netcode/src/predict_reconcile_relay_harness/life_states.rs` (402)
- `crates/netcode/src/predict_reconcile_relay_test_fixtures.rs` (22)
- `crates/netcode/src/prediction.rs` (32)
- `crates/netcode/src/presentation/overlay_tests.rs` (1)
- `crates/netcode/src/projectile_presentation.rs` (3)
- `crates/netcode/src/reconcile.rs` (147)
- `crates/netcode/src/reconcile/tests/life_states.rs` (128)
- `crates/netcode/src/state_slots.rs` (21)
- `crates/netcode/src/state_slots/player_slot_lookup_test.rs` (18)
- `crates/netcode/src/tests/life_state_projectile_tests.rs` (48)
- `crates/netcode/src/timed_slot_receive_crossing_test.rs` (1)
- `crates/netcode/src/timed_slot_receive_test.rs` (108)
- `crates/netcode/src/trigger_state_channel_harness_test.rs` (1)

## 09 Rejoin and carried state — 1,168 lines / 7 files

**Lens:** Correctness tracer; separate hygiene + drift.
**Likely entry:** host_handle_lifecycle → harvest_pawn / harvest_bound_pawns → held seat → demotion/re-promotion restore.
**Scope:** AC14; O6/O7/O21–O23/O30/O31/O41. Trace disconnect, suspend and level-unload clearing with held records; actual descriptor spawn restore lives in 03.

- `crates/combat-model/src/carried_loadout.rs` (43)
- `crates/netcode/src/host.rs` (22)
- `crates/netcode/src/seat.rs` (82)
- `crates/netcode/src/seat/life_state_harvest_test.rs` (195)
- `crates/postretro/src/startup/lifecycle.rs` (16)
- `crates/postretro/src/startup/lifecycle_life_state_tests.rs` (759)
- `crates/postretro/src/startup/lifecycle_net.rs` (51)

## 10 App respawn integration — 1,015 lines / 6 files

**Lens:** Correctness tracer; separate hygiene + drift.
**Likely entry:** App placement publication → begin_tick → finish_tick_start → resolved command yaw/admission → started simulation.
**Scope:** AC11–AC13; O10/O17/O18/O20/O28/O29/O32/O33/O44. Trace physical respawn before command admission and arrival yaw after resolution, including no-command pawn and owner/observer presentation.

- `crates/entities/src/ctx.rs` (6)
- `crates/entities/src/respawn.rs` (30)
- `crates/postretro/src/host_activations/conditioned_tests/life_states/respawn.rs` (466)
- `crates/postretro/src/respawn_placements.rs` (75)
- `crates/postretro/src/respawn_placements/tests.rs` (250)
- `crates/sim/src/sim/tick_start.rs` (188)

## 11 Presentation and frame integration — 1,112 lines / 14 files

**Lens:** Correctness tracer; separate hygiene + drift.
**Likely entry:** accepted owner sample reset witness → frame_loop UI snapshot inputs → life_state_display / life_state_clock / life_state_eye.
**Scope:** AC7/AC10/AC17/AC18; O15/O24/O25/O37/O38. Trace every-positive-frame advancement, real reset evidence, matched revision name identity and render-only eye easing without gameplay feedback.

- `crates/entities/src/engine_state_catalog.rs` (27)
- `crates/postretro/src/app/bindings_tests.rs` (1)
- `crates/postretro/src/app/timed_slot_ui_tests.rs` (1)
- `crates/postretro/src/app/window_modes/menu_tests.rs` (1)
- `crates/postretro/src/frame_loop/mod.rs` (86)
- `crates/postretro/src/life_state_clock.rs` (184)
- `crates/postretro/src/life_state_display.rs` (229)
- `crates/postretro/src/life_state_eye.rs` (309)
- `crates/postretro/src/main.rs` (67)
- `crates/script-compiler/src/light_membership.rs` (2)
- `crates/sim/src/scripting_systems/ui_proxy.rs` (58)
- `crates/sim/src/scripting_systems/ui_proxy_life_state_tests.rs` (107)
- `crates/ui/src/tree/tests/life_state.rs` (38)
- `crates/ui/src/tree/tests/mod.rs` (2)

## 12 Conditioned capability integration — 1,323 lines / 11 files

**Lens:** Adversarial tester; separate hygiene + drift.
**Likely entry:** conditioned host activation fixture → transition or late baseline → client command/receipt path.
**Scope:** AC4/AC15/AC17/AC18; O11/O12/O24/O38. Test adequacy and concrete edge sequences across delayed capability arrival, backlog trim, late baseline, held/burst/charge and look gating. Read relevant production paths in 05/07/08/11 as needed.

- `crates/postretro/src/host_activations/conditioned_tests.rs` (44)
- `crates/postretro/src/host_activations/conditioned_tests/life_states.rs` (498)
- `crates/postretro/src/host_activations/conditioned_tests/life_states/late_baseline_tests.rs` (301)
- `crates/postretro/src/host_activations/conditioned_tests/life_states/life_state_clock_trim_tests.rs` (219)
- `crates/postretro/src/host_activations/conditioned_tests/life_states/look_receipt_tests.rs` (139)
- `crates/postretro/src/host_activations/conditioned_tests/receive_alternate.rs` (22)
- `crates/postretro/src/host_activations/conditioned_tests/switch_order.rs` (1)
- `crates/postretro/src/host_activations/conditioned_tests/switch_timing_fixture.rs` (93)
- `crates/postretro/src/host_activations/conditioned_tests/switch_timing_lifecycle.rs` (2)
- `crates/postretro/src/host_activations/conditioned_tests/switch_timing_teardown.rs` (2)
- `crates/postretro/src/host_activations/presentation_conditioned_fixture.rs` (2)

## 13 Dev policy and author contract — 1,322 lines / 15 files

**Lens:** Contract verifier; separate hygiene + drift.
**Likely entry:** dev player descriptor + player-death twins → registered events → 900-tick sim/player-event consumer → HUD and author recipes.
**Scope:** AC3/AC15/AC19–AC21; O1/O2/O5. Verify real consumer parity, zero-HP authored policy, initial/restart behavior, docs and SDK-bundle commands. Manual feel proof stays outstanding until observed.

- `content/dev/maps/coop-presentation-test.README.md` (26)
- `content/dev/maps/coop-presentation-test.map` (60)
- `content/dev/scripts/coop-presentation-test.luau` (14)
- `content/dev/scripts/coop-presentation-test.ts` (9)
- `content/dev/scripts/hud.ts` (31)
- `content/dev/scripts/player-death.luau` (31)
- `content/dev/scripts/player-death.ts` (31)
- `content/dev/scripts/player.ts` (13)
- `content/dev/start-script.ts` (18)
- `crates/postretro/src/startup/player_death_surface_tests.rs` (643)
- `crates/postretro/src/startup/player_spawn_surface_tests.rs` (6)
- `crates/sim/src/player_events/life_state_ordering_tests.rs` (245)
- `crates/sim/src/player_events/mod.rs` (2)
- `docs/scripting-reference.md` (177)
- `docs/teleporters.md` (16)

## 14 Evidence and plan records — 626 lines / 5 files

**Lens:** Hygiene + drift only.
**Likely entry:** Plan checkpoint, Ordering ledger and manual runbook.
**Scope:** Evidence records are not production code. Review current claims against executed results after readiness; do not infer completed manual proof. This is a small documentation-only slice.

- `context/plans/in-progress/E18--player-life-states/index.md` (15)
- `context/plans/in-progress/E18--player-life-states/manual-playtest.md` (78)
- `context/plans/in-progress/E18--player-life-states/ordering-proof.md` (53)
- `context/plans/in-progress/E18--player-life-states/plan.md` (472)
- `context/plans/in-progress/E18--player-life-states/research.md` (8)

## 15 Generated mirrors — 270 lines / 4 files

**Lens:** Hygiene + drift only.
**Likely entry:** Generated SDK type mirrors and full-registry expected declaration fixtures.
**Scope:** Provisional generated/mechanical classification based on paths only; reviewer must confirm. Compare against authoritative declarations in 02. Any semantic discrepancy belongs to the 02 contract verifier too.

- `crates/sim/src/scripting/typedef/tests/fixtures/expected.d.luau` (66)
- `crates/sim/src/scripting/typedef/tests/fixtures/expected.d.ts` (69)
- `sdk/types/postretro.d.luau` (66)
- `sdk/types/postretro.d.ts` (69)

## Seam pass — three correctness tracers maximum

Each is a separate GPT-6.1 Sol xhigh worker after readiness. Brief the complete producer-to-consumer path and both sides, and allow adjacent reads; do not assign either endpoint alone.

1. **State identity and capabilities across reload and unreliable delivery:** descriptor readers/global table (01) → authority remap (03) → payload construction/history (07) → record revision gate/restamp/replay (08) → physics/weapon gates (04/05) → displayed state name (11). Entry: `replace_entity_types` or `commit_staged_manifest_result`; exit: accepted owner movement/HUD. Focus O14/O26/O36/O37/O42/O43, including same-revision profile replacement and missing-profile records.

2. **Queued respawn across tick start, command admission and lifecycle:** SDK/bound/runtime command (02) → queue/drain (06) → App two-phase tick start, placements, remote start aim and owner/observer generation cut (10/08) → disconnect/demotion harvest or actual unload (09/03). Entry: `respawn()`; exit: T+1 placed pawn or rejoin. Focus O3/O4/O10/O17–O23/O28–O33/O41/O44; ensure queued state remains a name across reload.

3. **Exact gameplay elapsed to smooth client HUD and authored bleed-out:** state entry/event ordering (02/05) → host tick elapsed (04/05) → wire reconcile/replay + accepted owner reset evidence (08) → frame clock and UI projection (11) → real dev down/15-second respawn and parity (13), with conditioned trim/look/baseline proof (12). Entry: player-event `setLifeState("down")`; exit: continuously advancing HUD and next-tick respawn. Focus O5/O15/O24/O25/O27/O31/O38/O39 and AC7/AC17/AC18/AC20.

## Aggregation and handoff

Deduplicate by live file and concern, preserve lens attribution, take higher severity on disagreement, keep comment drift separate, and report scope/efforts/target/verdict. Reviewers must verify current on-disk text before reporting drift. No findings or verdict are implied by this manifest. Manual AC17/AC18 results remain pending independently of automated readiness.
