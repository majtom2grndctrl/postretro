# teleporters — plan of record

mode: resumable
status: approved
read at: 08231a583

## Checkpoint

- Claim commit `08231a583` moved the brief alone and was pushed to `origin/main` with explicit owner approval after automatic approval review initially rejected that push.
- Feature branch: `codex/teleporters`.
- Active checkout: `/Users/dhiester/.codex/worktrees/teleporters/postretro`. The primary checkout contains unrelated owner edits; leave them untouched.
- Source verification completed against the claim commit; no engine implementation changes exist yet. No Cargo builds or tests have run in this planning checkpoint.
- Owner approved the traveller-sound amendment in this chat on 2026-10-15: retain the speculative cue for a same-generation authoritative replacement; no second playback even if the teleporter/cue differs. Ahead adoptions play the authoritative last cue once; several unseen teleports coalesce. Applied to the brief and research P32. No other Decision or Acceptance meaning changed.
- Owner approved this plan in chat on 2026-10-15. Approval recorded before implementation; execute tasks in dependency order.

## Corrections

- Observer arrival audio uses the authoritative pose of the accepted generation-change sample, independently of delayed rendering. The specified pawn record has no frozen original arrival point; later catch-up movement may precede that sample. No additional wire field or cue lane is introduced.

- Generation and accumulated yaw alone can agree when client and host take different teleporters. For AC39/P19 and AC41/P27, also compare the already-specified `last_teleporter` after replay. A mismatch takes teleport class even when yaw/generation agree and arrivals are less than 3 m apart. This clarifies the sufficient mismatch rule without adding wire data or changing Acceptance.
- `sdk/lib/map_entities.{ts,luau}` routes wrappers; trigger `.on`, arm and disarm live in `sdk/lib/entities/triggers.{ts,luau}`. Implement corresponding teleporter wrappers and all exports/converters, including the script compiler's independent light-membership wrapper inventory.
- The suggested semantic brush parser exits before generic map records are appended. Preserve teleporter name/tags through a `MapEntityRecord` companion in existing id29, keyed by source entity, while keeping id55's specified record layout. Install one entity per teleporter; never duplicate it from the companion. Destinations remain compile-time only.
- Local-content bindings already exist for lights, fog and emitters, as well as movers. Teleporters need their own stable movement-substrate binding to installed teleporter entities; do not spawn a second region on receipt or despawn the installed region on unbinding.
- Environment overlap ranking quantizes sizes to eight mantissa bits. Teleporters will follow their own explicit contract: compare the finite computed sum of convex brush volumes, then map entity order. Do not silently inherit that quantization. Summing overlapping brushes is intentional; the brief specifies a sum, while containment uses union.
- Id51 origins contribute themselves plus one-hop-dilated sampled PVS. Merge ordinary and arrival origins together nearest-first before first-stamping any reached cell, preserving that existing visibility superset.
- Hash canonical teleporter section data in the id51 cache key, as Path requests. Research's resolved-cell-only alternative misses destination/landmark moves within unchanged cells. Current id51 stage epoch is 2; bump it, leave `PORTAL_WALK_EPOCH` unchanged. Cue/initial-arm edits may conservatively invalidate this bake; the gameplay digest must still exclude them.
- Research's all-resident pool-gate risk is stale: `gate_runtime_pool_with_floor` errors only without id51, warns with it, and arrival origins do not change packed blocks. Mandatory streamed demand and runtime growth remain the resource exposure to measure.
- `capsule_static_placement_center` grounds/adjusts an arrival, so it cannot prove an exact authored teleport placement. Add a non-relocating capsule clearance query against static geometry and current mover poses. Refuse the selected teleport atomically, without trying a different winner.
- Existing `settle_broken_vault` zeros base path velocity. Teleport must abort the vault state while retaining and rotating the teleport contract's current velocity channels; do not reuse that velocity-reset helper unchanged.
- Clear both cached mover carry ground and its render-only yaw residual on relocation, not only the next input-seam carry. Publish yaw before pawn rotation, pre-captured local/remote pose aims, camera follow and fire consume it.
- Multiplayer payloads use bitcode. Integrate the specified sorted latch indices/count into the existing codec with explicit cardinality and index validation; preserve the brief's u16 count/index contract rather than assuming a derived Vec wire layout is identical. Movement payloads that currently derive `Copy` must be adjusted for owned latch storage.
- "Blocked by world geometry" includes an arrival capsule wholly inside solid or exterior (outside-the-seal) BSP cells, and wholly inside a closed mover hull at its current pose, not only triangle contact. Known residual: detail brushes absent from the BSP are seen only through triangle contact.
- Mover yaw render residual is derived each frame from `mover_yaw_carry_ground`; setting that ground Airborne on relocation clears both carry and residual (proved by `teleport_detaches_turntable_carry_in_view_and_motion`). No separate residual clear is needed.
- Progress counts successful removals of map-placed monsters, not their initial zero-HP writes. The closet fixture needs an authored despawn policy and must wait for the existing progress drain before asserting rearming. Add a concrete source for the example's otherwise-unfired `era.shift` reaction.

## Delegated answers

- Network audit — source confirms input stamps must stay paired with original facing on real, held and every neutral path; first-record silence keys on prediction identity change, not baseline kind. The source has the required epoch boundary and point sound anchor already.
- Compiler audit — use exact compiler-side convex intersections after AABB rejection for source and seamless image cell classification. Runtime cell AABBs alone would inflate mandatory demand. Intersect against final BSP leaf half-spaces, including boundary contact for the source region; use the cell locator for a classic point arrival. Transform convex vertices and planes, not just two AABB endpoints.
- Script audit — teleport events need an authoritative in-tick dispatch after all pawns move, with mutations queued in listed order for the following tick. Joining the normal frame-end movement emission bucket would break the one-way gate under catch-up ticks.
- Sound policy — resolved by owner: predicted feedback stays immediate; same-generation replacement adds no cue; authoritative unseen arrivals coalesce to their last cue. Observer cues remain host-derived, at the actual arrival point.

## Verified source map

- Movement: `crates/physics/src/movement/mod.rs::{tick,tick_with_immersion,MovementEvents}`, `movement/mover_carry.rs`, `collision/moving.rs::{CombinedCollisionWorld,deepest_mover_penetration}`, `collision/mod.rs::capsule_static_placement_center`; `crates/collision/src/{leaf,volumes}.rs`; `crates/foundation/src/movement/player_movement.rs::{PlayerMovementComponent,MovementState,settle_broken_vault}`.
- Integration/presentation: `crates/sim/src/sim/host_movement.rs::{run_host_movement_tick,drop_vault_if_dead}`, `sim/mod.rs::{simulate_tick_with_presentation_aim,run_movement_tick}`; `crates/entities/src/registry.rs::set_presentation_transform`; `crates/sim/src/sim/frame_timing.rs::{hold_state,push_state}`; `crates/postretro/src/main.rs::{apply_mover_yaw_carry,mover_yaw_render_residual}`, `frame_loop/mod.rs` client and host camera pushes and post-movement closure.
- Network: `crates/net/src/handshake.rs::WIRE_VERSION` = 31; `wire.rs::SNAPSHOT_VERSION` = 20; last component kind Immersion = 23, teleporter 24 free. `crates/netcode/src/{movement_state,wire_convert,prediction,reconcile,command_queue,client,interpolation,host_presentation}.rs`, `client/bindings.rs`, `lib.rs` fixed/zero-tick send and own-record adoption; `crates/net/src/transport.rs` participation epoch framing/drop. Keep one wire and one snapshot version bump for this feature.
- Audio: `crates/postretro/src/sound_events/descriptors.rs::movement_sound`, `crates/sim/src/emission.rs::append_movement_emissions`, `crates/audio/src/spatial.rs` existing `SoundAnchor::Point`. Observer arrival anchors must be fixed points, since entity anchors resolve delayed presented transforms.
- Scripting: `sdk/lib/entities/triggers.{ts,luau}`, map entity routers, data scripts and reaction targets; `crates/scripting-core/src/{group_resolution,luau_prelude}.rs`, `data_descriptors/runtime_manifest.rs`; both reaction converters and subject-scope validation; `crates/sim/src/{trigger_system,trigger_commands}.rs`, `scripting/entity_world_primitives.rs`; startup install/recompose/unload and SDK type generation. Reuse `group_commands_apply_here` for teleporter mutation role checks.
- Compiler: `crates/level-compiler/src/parse/{environment_volumes,trigger_volumes}.rs`, `parse.rs` semantic dispatch and generic record emission, `format/quake_map.rs` angle conversion, `partition/region_polytope.rs`; `crates/level-format/src/environment_volumes.rs` and `section_registry.rs` (55 free, 54 reserved in context).
- Residency/digest: `cell_residency_bake.rs`, `cell_residency_bake/lead_map.rs::build_lead_map`, `Neighbors::within_distances`, `cell_residency_set_bake_cached`, `pipeline/lightmap_pool_gate.rs`; `crates/postretro/src/runtime_movers.rs::level_content_digest`, fingerprint epoch 3.
- Test precedents: `startup/closet_reveal_surface_tests.rs::closet_reveal_twins_emit_byte_identical_wire_data`; production-path prediction/link harness, `sim/determinism_tests`, existing reconcile/interpolation/command queue tests; `sh_streaming/reach_measurement.rs::sh_reach_mandatory_bytes_from_prl`.

## AC-to-proof

AC numbers count the brief's checkboxes in order, automated first (1–48), then manual (49–52). Proof names below are planned scenario names, not claims that tests already exist or have passed. No row is satisfied yet. New tests use a `teleport` filter and live beside the behavior or in focused sibling suites. Each filter must match its intended tests.

| AC | Contract | Proof | Status |
|---|---|---|---|
| 1 | Classic exact arrival, view and all velocity channels | physics relocation + sim camera/aim `teleport_classic_exact_arrival` | achievable as stated |
| 2 | Seamless four yaw deltas, vertical offset and speeds | physics `teleport_seamless_landmark_transforms` | achievable as stated |
| 3 | Two-way pair round trip and exit latch | physics multi-tick `teleport_pair_round_trip` | achievable as stated |
| 4 | No carry yaw/motion after turntable departure | sim + app `teleport_detaches_turntable_carry` | achievable as stated |
| 5 | Post-step body point, capsule exclusion, brush union | collision + physics `teleport_body_point_union` | achievable as stated |
| 6 | Disarmed/spawn-disabled never fires | load + physics `teleport_initial_armed_state` | achievable as stated |
| 7 | Arm/disarm takes effect next tick | sim `teleport_next_tick_mutation` | achievable as stated |
| 8 | Exactly one smallest-sum/map-order winner | collision + physics `teleport_overlap_winner` | achievable as stated |
| 9 | Arrival latch, exit/re-entry, same-tick arm | physics + sim `teleport_arrival_latch` | achievable as stated |
| 10 | All arrival overlaps latched; third still eligible | physics `teleport_latches_all_arrival_regions` | achievable as stated |
| 11 | Arming pair once, remote shift without ping-pong | production-path netcode harness `teleport_arm_pair_latched_remote` | achievable as stated |
| 12 | Ordered arm/disarm batches and idempotent arm | sim `teleport_ordered_mutation_batch` | achievable as stated |
| 13 | Two pawns arrive, one authoritative event each | sim `teleport_two_pawn_entry_and_arm` | achievable as stated |
| 14 | Atomic world/mover refusal, once warning, pose ordering | physics + sim/prediction `teleport_blocked_arrival`; test-log-capture | achievable as stated |
| 15 | Vault abort, dash/slide survive, airborne | physics `teleport_movement_state_carry` | achievable as stated |
| 16 | Players only, deferred/late kill ordering | real sim `teleport_liveness_and_body_kinds` | achievable as stated |
| 17 | Arrival trigger enter and source exit this tick | sim `teleport_trigger_arrival_edges` | achievable as stated |
| 18 | Real dev level, TS/Luau wire parity and gameplay loop | startup `teleport_level_twins_emit_byte_identical_wire_data` + installed sim flow; AC51 covers human playtest | achievable as stated |
| 19 | One-way gate under one/three catch-up ticks | sim/app `teleport_one_way_catchup` | achievable as stated |
| 20 | Frame-end progress/crossing command effective next tick | app drain integration `teleport_frame_end_mutation` | achievable as stated |
| 21 | Authoritative-only event, activators/token, no occupancy | script install + sim `teleport_event_scope_and_authority` | achievable as stated |
| 22 | Predicted cue once; replay silent | app sound requests + reconcile `teleport_prediction_sound_once` | achievable as stated |
| 23 | Observer/host cue at fixed arrival; coalesced last cue | observer/app sound `teleport_observer_arrival_cue` | achievable as stated |
| 24 | Refused prediction cue once, no cut-back/observer cue | harness + sound `teleport_refused_prediction_cues` | achievable as stated |
| 25 | Ahead adoption cue; skew/replacement dedup; coalescing | harness + sound `teleport_adoption_cue_policy` | achievable under owner-approved amendment |
| 26 | Absent cue silent everywhere | app sound `teleport_no_sound_key` | achievable as stated |
| 27 | Strict compiler errors and yaw-only warning | prl-build bin `teleport_invalid_authoring`; test-log-capture | achievable as stated |
| 28 | Lead by region distance, source/image cells, cache edits | compiler `teleport_residency_routes_and_cache` with same-cell pose edits | achievable as stated |
| 29 | Minimum lead across all origins; disabled parity | compiler `teleport_residency_nearest_origin` | achievable as stated |
| 30 | Gameplay digest inclusions and presentation/state skips | app `teleport_content_digest_fields` | achievable as stated |
| 31 | Malformed section rejected, absent section empty | level-format + app load `teleport_section_validation` | achievable as stated |
| 32 | Same-tick yaw at every alpha and arrival-eye shot | sim/app and prediction `teleport_same_tick_aim_and_fire` | achievable as stated |
| 33 | No source/arrival blend in any catch-up position or zero tick | frame timing + registry/app `teleport_catchup_camera_and_mesh_cut` | achievable as stated |
| 34 | Reconcile cut holds camera this frame and following tick | app + reconcile `teleport_reconcile_camera_hold` | achievable as stated |
| 35 | Pending correction offset cannot pull back to source frame | prediction/app `teleport_clears_prior_correction_offset` | achievable as stated |
| 36 | 100 ms clean and lossy prediction/yaw/ordinary-band proof | production-path harness `teleport_walkin_conditioned_link` | achievable as stated |
| 37 | 100 ms scripted occupied-room shift, one cut, no follow-up | harness `teleport_scripted_shift_inflight_input` | achievable as stated |
| 38 | Host refusal restores yaw/cuts even at 1 m | harness `teleport_refused_entry_short_distance` | achievable as stated |
| 39 | Agreed/refused/multiple/divergent teleports | harness `teleport_reconcile_identity_sequences`, including equal generation/yaw different last teleporter | achievable as stated |
| 40 | Lost/held/neutral/late input stays in actual frame | command queue + lossy harness `teleport_gap_command_stamp` | achievable as stated |
| 41 | Ahead/behind/divergent prediction stamps and no repeat correction | harness `teleport_divergent_command_frames` | achievable as stated |
| 42 | Multi-tick and zero-tick send samples facing/stamp together | app/netcode `teleport_input_sampling_seams` | achievable as stated |
| 43 | Held classic command adoption turns by missed turn | queue + harness `teleport_held_classic_view_turn` | achievable as stated |
| 44 | Observer/host cuts, wrap, starvation, safe speed/heading, first sample | interpolation + host presentation `teleport_observer_cut_and_starvation` | achievable as stated |
| 45 | Join armed convergence, silent first pawn record, zero pre-record stamp | client/harness `teleport_join_first_record_and_armed_binding` | achievable as stated |
| 46 | Same-pawn periodic baseline adopts audibly and turns view | client/reconcile `teleport_periodic_rebaseline_adoption` | achievable as stated |
| 47 | Old level/restart stamped commands rejected | real transport epoch `teleport_stale_epoch_input` | achievable as stated |
| 48 | Client member/token mutation changes nothing | script/sim `teleport_client_mutation_role_guard` across all entry paths | achievable as stated |
| 49 | Metal seamless swap has no pop/lighting change | owner in-engine, distant room copies at fixed landmark offsets | manual-visual |
| 50 | Co-op traveller timing, observer cut, scripted agreement | owner two-peer playtest at approximately 100 ms RTT | manual-playtest |
| 51 | Closet loop end to end | owner playtest dev fixture: reveal → disarm → map-monster clear → rearm → use/disarm | manual-playtest |
| 52 | Runtime lightmap/SH comparison | matched baseline/teleporter dev and eight-source stress diagnostics run below | manual-measurement |

## Tasks

Integrating executor owns shared types, integration, plan, commits and all Cargo commands. Implementation delegation is optional only after a stable contract exists; any worker must read the entire amended brief, research and routed context, must own named non-overlapping files, and must not edit this plan or commit.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin host-seat classic path: minimal id55 record/load/install and indexed region; pawn relocation result/state; exact arrival clearance; host same-tick yaw/aim/presentation write and camera hold. Prove AC1, basic AC5/14/32/33 and short-distance cut identity premise before widening. | integrating executor | — | implemented (WIP `86bec6c83` + milestone commit); enclosed-capsule clearance fixed; pawn-level AC5/14 proofs → task 9 |
| 2 | Complete strict FGD/parser/format and tag companion; seamless transforms, union/ranking, all arrival latches, state/mover detachment; digest; hot reload/tuning state carry. Focused placement/compiler/codec proofs. | integrating executor | 1 | implemented; render yaw residual clear (AC4) → task 10; hot-reload/tuning carry unverified → task 9 |
| 3 | Full TS/Luau member/event/token/manifests/SDK surface; authoritative ordered mutation queue at next tick start; in-tick teleport dispatch after all pawn movement; role/lifecycle/death/trigger/progress proofs. | integrating executor | 2 | implemented; proofs AC21/48 → task 12, sim proofs → task 9 |
| 4 | Teleporter armed replication/binding; pawn record codec/merge and protocol bumps; sampled yaw stamp through every input path; prediction/replay/adoption/identity cut retaining unacked ring; first-record and epoch handling. Focused net tests. | integrating executor | 2, 3 | implemented, unit-tested (netcode 15, net 3); production-path/lossy proofs → task 11 |
| 5 | Observer and host-seat cut/hold/speed/heading behavior; forward/adoption/observer sound dedup under approved policy, fixed arrival anchors; app camera, view-feel offset and carry residual integration. | integrating executor | 4 | partial: observer/host-seat arrival sound unconsumed in app → task 10 |
| 6 | Source and transformed-region cell couplings; nearest-first id51 origin merge/cache epoch; mandatory-demand measurement on matched dev/stress variants. Report resource results for the owner's existing open question; do not change mandatory-vs-band policy without direction. | integrating executor | 2, 5 | compiler side implemented (stage version 2→3); measurement pending → task 13 |
| 7 | Runnable content/dev teleporter map and TS/Luau twin, closet loop and scripted shift source; production-path clean/lossy harness for all remaining ordering pins and client/host agreement; SDK-bundle-runnable author docs. | integrating executor | 3–6 | maps/scripts exist; twin-wire + loop tests and docs → task 13; harness → task 11 |
| 8 | Review-readiness focused gates, review-panel → fix-review-findings → focused retest, final preflight once, AC results/runbooks and manual checks. | integrating executor | 7, 9–13 | pending |
| 9 | Physics/sim proof slice: AC5, 7, 9, 12, 13, 14 (mover in/out, warn once), 15, 16 (incl. deferred health write, P25), 17, 19, 20; verify hot-reload/tuning carry. | delegated worker | 1–3 | done: physics `teleport.rs` AC5 pawn-level, AC15; `sim/teleport_tick_tests.rs` (real `simulate_tick`) AC7, 9, 12, 13, 14 world+mover+warn-once, 16/P25 (mutation-checked), 17, 19 sim side (1/3 ticks × first/middle/last), 20 sim side, descriptor hot-reload carry. App frame placement of drains (AC19/20) not provable without event loop |
| 10 | App slice (`crates/postretro`): consume observed/host-seat teleports for arrival cue at fixed anchor (AC23); clear mover yaw render residual (AC4); app sound/camera proofs AC22–26, 33, 34. | delegated worker | 5 | done: `sound_events::observed_arrival_sounds` fixed-point observer/host-seat cue; `runtime_teleporters/app_tests.rs` AC4, 23, 24 (observer), 26, app half of 22/24/25; AC33/34 host-seat + adoption via existing tests. Predicting-client catch-up camera and AC19/20 app frame driver need a live client/event loop — carried by netcode/sim proofs |
| 11 | Netcode slice: production-path clean/lossy harness proofs AC11, 36–43, 45–47, P16; verify no production caller predicts from `enabled_on_spawn` defaults instead of replicated armed state. | delegated worker | 4 | done: harness now runs production prediction/host resolution; `predict_reconcile_harness_teleport.rs` + rig (clean 100 ms, lossy) AC11, 22/25 netcode half, 35–43, 45, 46; AC44 existing; AC47 real transport epoch in `teleport_tests.rs`; 5 mutation checks bite. Owner question on AC37 wording (below) |
| 12 | Scripting/compiler proof slice: AC21, 48 across member step/reaction/token paths; AC27 prl-build log capture incl. pitch/roll warning; AC28 pipeline cache re-bake. | delegated worker | 2, 3 | done: sim `teleporter_commands/entry_path_tests.rs` (AC21, AC48 member/primitive/token), level-compiler `tests/teleporters_compile.rs` real `prl-build` (AC27 seven cases + pitch/roll warn; AC28 cache miss/hit on pose/shape/mode edits), script-compiler `teleport_members_evaluate_at_build_in_both_hosts`; AC29/31 existing tests verified |
| 13 | Fixtures, docs, measurement: AC18 twin byte-identical + closet loop + era shift startup test; `docs/` author page; extend `reach_measurement` and run the runbook for AC52. | delegated worker | 9–12 | pending |

Each completed task commits its implementation and this plan's matching task/proof update together. Check disk after each numbered task/step. Worktree builds use their own target and `CARGO_INCREMENTAL=0`; free space must remain at least 15 GB, with the expected cold build budget considered before starting.

## Runtime and resource constraints

- Build region/query indexes at install; query candidate convex brushes through a spatial index. No scan of every teleporter in ordinary movement, no per-query allocation. Arrival containment visits only candidate regions; reuse per-pawn latch storage and avoid rebuilding unrelated state each tick. Latch capacity follows installed teleporter count and the specified u16 wire envelope, with no arbitrary gameplay cap.
- Arm/disarm is change-driven. Drain a reusable ordered queue once at tick start on host; clients only install replicated armed state and authoritative latches. An arm edge clears affected pawn latches on host only. All pawn movement reads the same tick's armed snapshot.
- Replay resolves mover poses once per mover per replayed tick using the existing reusable history workspace. Stamp rotation must add no per-command history map. Retain the existing bounded input ring and transport envelopes; validate finite wrapped yaw and sorted valid latch indices at the boundary.
- Observer sound responds once to accepted sample generation changes, independently of delayed render sampling. First samples are silent. Do not allocate a new network cue lane or rescan sound assets per frame.
- Residency geometry/visibility work stays offline, bounded to playable compiler cells. Runtime consumes id51 unchanged; no per-frame teleporter visibility walk and no GPU work outside renderer. Measure mandatory demand and growth transients against matched baselines rather than widening caps to make fixtures pass.

## Verification and landing policy

During tasks run only `cargo check` for touched crates and focused tests, confirming nonzero intended counts. Compiler internals use `--bin prl-build`, not `--lib`. Use `dev-tools` for affected feature-gated app paths. Do not run the full workspace suite or preflight during implementation.

After integration: `cargo fmt --check`, touched-crate checks and focused behavior proofs; invoke review-panel and fix-review-findings for concrete findings, retest affected paths, then preflight once as the final gate. No GPU context in automated tests. Manual results never become inferred passes.

The brief does not authorize landing before manual proof. After automated checks pass, record every AC result and set `status: test-ready` while any blocking owner/manual result or resource-policy decision remains outstanding. Once all required results arrive, report them and wait for "land the plane" before moving the brief to done, updating durable contracts and pushing the feature branch.

## External runbook to complete after fixture paths exist

- Build helper binaries first, then compile the dev and eight-source stress maps with identical geometry, lights, hints, bake flags and worker count. For each, produce with/without-teleporter PRLs by removing only teleporter entities. Keep source/cell IDs comparable.
- Eight stress sources fit inside one 32 m lead radius; destinations occupy distinct distant lighting regions. Include classic and 90-degree seamless routes, disabled sources and occupied arming.
- Extend the existing ignored `sh_reach_mandatory_bytes_from_prl` measurement to emit every camera cell's mandatory lightmap blocks/pairs/bytes and SH clusters/bytes, including pins and transitive owners. Run on all four PRLs with `POSTRETRO_SH_REACH_PRL` and `POSTRETRO_SH_REACH_LEADS=0,16,32`; retain per-cell rows, median/p95/max and baseline deltas. These are bake measurements, not a substitute for runtime VRAM.
- On Mac/Metal, run the engine from this worktree with ordinary async SH and streamed lightmaps. Approach/dwell/cross/return and arm an occupied room on identical routes in paired variants. Record machine, renderer lead/cap settings, frame timing, visible misses, mandatory/resident SH, lightmap peak pool layers/active bytes, requested GPU bytes and growth transient peak bytes. Preserve the 2 GB VRAM target and report overruns plainly.
- Owner then decides the brief's resource question: keep full mandatory arrival origins or amend the distant-origin policy. The existing all-resident gate does not resolve this decision.
- Run the two-peer loop at approximately 100 ms RTT, using the established clean/lossy harness profiles; verify walk-in immediacy, scripted shift agreement and the closet loop. Record visual/sound outcomes separately from automated sample/anchor proofs.
- Replace runbook placeholders with actual map/script paths and exact runnable commands before requesting external testing. Keep human-facing `docs/` commands runnable from an SDK bundle alone.

## Implementation checkpoint (active)

- Session 2 (2026-10-15) took over after the first executor hit a usage limit; its uncommitted tree became WIP commit `86bec6c83`.
- All touched crates pass `cargo check --tests` (postretro with and without `dev-tools`). Teleport-filtered tests: collision 7, physics 9, entities 1, level-format 5, level-loader 3, net 3, sim 4, scripting-core 2, netcode 15, level-compiler (`--bin prl-build`) 9, postretro `dev-tools` 5 — all pass. Full physics lib 348 pass.
- Enclosed-capsule clearance gap fixed: `physics/src/collision/solid_cells.rs` point-in-solid over the cell locator; `moving.rs::point_inside_mover` winding-number test at current pose; regression tests with open controls.
- Task 13b (bake half of AC52) done: `reach_measurement.rs` per-cell mandatory lightmap/SH rows + median/p95/max (`POSTRETRO_SH_REACH_CSV`). Fixtures: dev +0.35 MiB SH / +0.12 MiB lightmap worst cell, no pool layer added; stress at L≥16 every source-room cell carries all 8 arrival rooms (9×). Dense-map extrapolation from campaign-test: 8 distinct arrivals ≈ +360–830 MiB non-evictable pool (upper bound, unmeasured). Owner decision on the open question pending; runtime Mac/Metal runbook drafted outside the repo, to be placed with the AC results.
- **Owner question (AC37):** the client applies only deltas matching its held baseline, so the teleporter's armed record can arrive one snapshot before the pawn's record; the client then predicts the scripted seamless shift itself (exact isometry: no correction, one cue, view turned). Across 6 arm timings every outcome claim holds, but "one teleport-class cut" occurs only for some timings. Proposed restatement: "…reaches a remote client as exactly one landing — one teleport-class cut, or an agreeing prediction with no correction —" (matches P29's phrasing). Test currently asserts the outcome claims for all timings and the cut path for at least one.
- Open follow-up: teleporter `arrival_sound` keys may bypass the install-time unknown-sound-key warning (`warn_unknown_sound_keys`, audio.md §4) → task 13.
- Known clippy debt: collapsible ifs at `frame_loop/mod.rs`, `runtime_teleporters.rs` (also items after test module), `.err().expect()` in `parse/teleporters/tests.rs`.
- Remaining work is tasks 9–13 then 8. Gap audit detail: concerns include the dead-pawn bar riding `vault_barred` (prove P25) and default `enabled_on_spawn` prediction variants (prove unused in production).
