# coop--networked-by-default — plan of record

mode: resumable
status: approved
read at: bd94e4e3f (source unchanged from fe1881a35 / research 221fdaf4a)

## Session state

- Owner chose resumable mode on 2026-10-10. Claim commit `fe1881a35` is on `origin/main`; branch `codex/coop-networked-by-default`.
- Source audit found one false Decision premise: held weapons do not carry Weapon only. Block recorded in `bd94e4e3f`; no implementation or tests ran.
- Owner approved canonical held ownership, shared world-targeting/replication exclusion, retained state and existing weapon resources on 2026-10-10. Applied to Decision 1, AC 2/4, new AC 22, Task 2/2a/10, orderings and M1. No unrelated Decisions changed.
- Owner approved this plan on 2026-10-10 ("Approved. Carry on!"). Full resumable source verification is complete. All proof below is **planned**, not passed; implementation is authorized.
- Durable entity/networking contracts record the approved direction as decided, not built. Original Decisions 2–12 remain in force.
- Use the existing checkout and warm `target/`. Integrator owns plan, shared contracts, commits, Cargo runs and final verification. Workers never commit or edit this file.
- Free space at planning: 37 GiB. Check after each workflow step/task; apply the build-brief cache-only cleanup rule below 15 GB.

## Corrections

1. **False premise resolved:** accepted Weapon+Touchable+Health (and behavior) descriptors retain Health/Brain/Agent on pickup and loadout composition. Inventory, rather than absence of components, determines held status. Host HIT apply must recheck target ownership before the registration sweep. Retain current Mesh/Touchable stripping/reconstruction and explicit script damage/despawn semantics.
2. **Agreement domain:** the descriptor predicate and live predicate agree for world placements. Held instances intentionally differ; prove their exclusion and drop transition separately.
3. **Sim split:** `descriptor_carries_brain` is called by sibling `descriptor_mesh_component` (`data_archetype.rs:757`), not directly by `attach_descriptor_components`. Preserve that access and existing `ai_capsule_center_from_feet_offset` re-exports.
4. Only the sim split is needed before the first vertical slice. The other four original Task 1 splits remain independent behavior-preserving commits, immediately before their consumers. Avoid delaying risk proof behind unrelated extractions.
5. Task 9 source-specific crossing/levelLoad gates also live in postretro `startup/lifecycle.rs:1135–1210`. Include staged subscriber rebuilds. The player-event walker returns one violation; extend traversal to enumerate all reached MachineLocal reactions for once-per-(binding,reaction) warnings.
6. Audience validation must reject present values of the wrong JSON type. Copying `invalid_at_value` literally would ignore non-string values through `as_str`. Preserve TS/Luau parity.
7. Task 8 must preserve push order across local and routed commands. Current separate vectors plus later host re-push can reorder them. Use one ordered representation or equivalent stable positions. Also update the app named-event wrapper and the crossing context constructor.
8. Task 11 needs an injected RTT/clock seam for deterministic proof. Production `ClientTimeSync` uses `Instant`; advancing the app packet conditioner does not advance that clock. No sleeps in clock tests.
9. Emitter update resets a changed tween and then immediately adds delta. Seed APIs must preserve the intended age through that same-frame pass, without a second frame of advancement. Light seeds cache the whole component plus start; fog seeds animation plus start.
10. The app conditioned harness currently tests combat. Extend its CPU boundary to observe real presentation bridges and command routing; a netcode-only harness cannot prove sampled phase or positioned audio intake.
11. AC 6 requires the meshless client's declaration through host HIT apply, beyond a local query test. AC 9's snapshot interval is two 60 Hz ticks, about 33.3 ms.
12. Flights need both sent sequence and sent baseline identity. An ack at/after the send sequence ends the flight window; baseline coverage determines completion or re-pending. A change while in flight must not disappear behind the older ack.
13. AC 13's pending budget is deliberately not a total snapshot cap. Measure the full encoded envelope after state/faction records as well as the budget subtotal. Renet admits unreliable messages before fragmenting; an envelope larger than remaining tick allowance is dropped whole.
14. Teardown currently caches original loadout IDs in `SlotPawns`; that cache misses pickups and can delete dropped/reacquired weapons after the pawn dies. Implement the existing current-owned auxiliary lifecycle at destruction, then remove the stale cache authority. Individual item disappearance retains existing inventory selection normalization.

## Delegated answers

- Initial join budget: retain 16 KiB pending-baseline budget. Real-payload fixture measurement may tune this delegated constant; measure transport envelope too, never weaken the workload to make it fit.
- Ownership representation: canonical ten-slot Inventory plus registry-derived item→holder/slot lookup. Validate replacement before committing either direction; O(10) changed-inventory work, allocation-free per-candidate lookup, storage proportional to owned live items. No component/wire/SDK ownership flag.
- Teardown: current auxiliaries retire with their holder, consistent with `entity_model.md` §3 and existing host/client cleanup. Centralize this at destruction so externally despawned pawns cannot leave orphan held instances. Use a private single-entity retirement operation and iterative worklist: capture current generation-valid children, retire the holder before processing them, and skip already-dead IDs. This terminates self/cyclic malformed references without recursion or a graph API. Replacing/removing Inventory releases removed membership without despawning those items; only holder destruction retires its current children. No automatic drop or new transfer gameplay.
- Mutable access: both typed/erased Inventory replacement/removal maintain ownership; unchecked mutable Inventory access cannot bypass it. Current production callers already clone and replace. Preserve stale slot and malformed selection-index degradation; reject conflicting live membership atomically.
- Mixed behavior-bearing weapons: filter world actor snapshots/apply, targeting, steering and mover capsules. Do not fold ownership into quiescence: that would delete private AI weapon state. Retain private controller resources and existing passive updates.
- Damage boundary: ownership excludes spatial targets and queued direct-target application, never the firing weapon or attacker pawn. General explicit Health mutations and deferred scripted removal remain valid. Preserve existing projectile endpoint presentation on refused target damage; endpoint policy is unchanged.
- Held presentation: existing lights/emitter behavior remains. Captured held overrides wait for drop and never acquire a bound world record. Do not invent light attachment or particle-lifetime changes.
- Model miss: reproduce with the actual model and `splash_direct` eye ray. If it passes, retain regression proof and record that no further hit-zone defect was reproduced; do not manufacture a correction.
- Manual proof: M1–M4 block landing. The brief grants no landing-first exception. Automation cannot mark the external Mac↔Windows results passed.

## Source verification

Reads completed against the unchanged source baseline. Paths below are current pre-extraction anchors; proposed test/module names below are not claims that code exists.

| Decisions / Path | Source and boundary consumers inspected | Outcome |
|---|---|---|
| D1–2; T1–2 | entities `provenance.rs`, `data_registry.rs`; scripting-core `refresh_plan.rs`, JS/Lua entity parsers; sim `builtins/data_archetype.rs`, `wieldable_inventory.rs`, `sim/touch.rs`; netcode `descriptor_class.rs`, `replication.rs`, `endpoint.rs`; app `startup/lifecycle.rs`, `lifecycle_world_cpu.rs` | Enum/ALL drift hazard and original AI/item sweep confirmed; held premise amended. Install/model suppression and DefaultWeapon provenance grounded. |
| Ownership lifecycle | entities `registry.rs` typed/erased setters/removers, mutable visits, despawn/level clear, `components/inventory.rs`; sim `weapon_stage/commands.rs`, `commands/local.rs`, `net_descriptor.rs`; netcode `host_switch.rs`, `lifecycle.rs`, client despawn and stale slot cleanup | Canonical write boundaries and stale fallback gap confirmed; no ownership wire field needed. |
| Ownership world consumers | sim `scripting_systems/hit_zones.rs`, `health.rs`, `sim/splash.rs`, `projectile_stage.rs`, `weapon_stage/impact.rs`, `resource.rs`, `agent_steering.rs`; AI `lib.rs`, `targeting.rs`, `compute.rs`, `apply.rs`, `weapon_controller.rs`; physics `kinematic_mover/blocking.rs`; netcode HIT ingest/apply | Pickup precedes ready/pending HIT apply; sweep follows. Retained Agent/Health need explicit exclusion. Quiescence and explicit damage stay separate. |
| D3–4; T3–4 | sim `builtins/net_descriptor.rs`, hit zones/Health hitbox conversion and candidate walker; netcode `remote_materialize.rs`, lib materialization loop and client retry/identity clearing; actual dummy model and splash map | Mesh-only gate/retry must become presentation-completeness aware; HitVolume remains query-only. |
| D5–6; T7/11 | sim `map_entity.rs`, spawn/classname dispatch; app level install before player-start partition/filter; light authored prefix/enrollment and fog bridge mapping; netcode mover bindings | Stamp PRL ordinals before filtering; only authored light prefix is stable. Existing failure behavior supports drop+warning. |
| D7; T10 | scripting-core tag/group/sequence dispatch and ScriptCtx; registered lighting, six fog primitives and both emitter primitives; app Input/UI, drain, serialize, clock-advance, bridge and install order | All three capture seams required. UI precedes drains; install clock must be 0; capture immutable overrides rather than live bridge samples. |
| D8; T5 | net `wire.rs` raw/typed payloads, records, header, validation/conversion; handshake/transport packed protocol gate; replication mirrors; netcode component apply/finiteness and record literals | Current wire26/snapshot17/PRLB; append layout and new binding validation feasible; invalid records reject a snapshot. |
| D9; T11 | net timesync estimator/RTT; netcode EngineClock/ClientTimeSync, sequence gate, apply/ack/demotion; app client receive ordering and light/fog/emitter caches/starts | Latest accepted batch clock, deferred-before-sync and same-frame bridge seeds are required; deterministic test seam is necessary. |
| D10; T6/12 | net replication ingest/ack/refresh/encoder; netcode host cadence/serialization; net transport config; pinned local renet unreliable admission/fragments | 60,000 bytes/tick, 5 MiB/channel, 1,200-byte slices, incomplete slices expire at 3s. Pending budget alone does not bound total envelope. |
| D11; T8–9 | entities system command queue/context/classifier; scripting-core named/sequence dispatch; sim trigger occupancy/residuals/scheduler/player-events; app all drain sites; netcode presentation routing/participation; audio anchors | Resolve audience at push; preserve interleaved order, tick occupancy, fire origins and positioned sound. |
| D11–12; T9/12 | SDK TS/Luau commands/tokens/reactions/builders; typedef templates/generated types/type-tests; compiler embedded prelude/light membership; app reaction validation/lifecycle rebuild; player-event validator/trigger binder | Scope brands and opaque tokens fit; wrong JSON type rejection and transitive warning collection need explicit coverage. |
| T12/install tooling | app `host_activations/conditioned_tests.rs`; tool `run.rs`; root project marker; docs distribution/external-projects and E16 examples | CPU harness must grow to bridge observation; checkout `run --install-root .` and SDK commands grounded. Guarded E16 milestone needs fresh progression. |

## AC-to-proof

Names marked **new** describe tests to implement. Existing coverage is extended where the scenario matches. Every automated row is achievable as amended; no results are inferred from source reads.

| AC | Proof | Status |
|---|---|---|
| 1 | Extend netcode enemy replication harness: **new** `descriptor_dummy_client_hit_credits_owner_and_despawns` through declaration, authorized host HIT, installed impact policy/per-owner XP slot and next-snapshot tombstone. Repeat over conditioned app link in P15. | planned automated |
| 2 | Extend `classifier_agrees_with_live_predicate_one_source_of_truth` with all nine world-placement shapes, expected outcomes, macro-derived ALL and exhaustive role match. Separate owned/loadout transition tests. Review `deny(wildcard_enum_match_arm)` and final clippy. | planned automated + review |
| 3 | Install-path test through `install_world_cpu`: **new** `suppressed_descriptor_models_install_hit_zones_once`; join fixture asserts exactly one copy per placement. CPU upload-ready models/hit zones; actual GPU visibility remains M1. | planned automated |
| 4 | Extend `world_item_baseline_acquire_despawn_and_drop_rebaseline_materialize_one_mesh` to retained Health, DefaultWeapon, holstered slot, fresh drop ID and actual client tombstone. P15 conditioned host-only prop animation. | planned automated |
| 5 | Extend remote materialization tests to descriptor Mesh/Light/Emitter/HitVolume completeness, absent authority components, idempotence and meshless retry/cache cleanup. | planned automated |
| 6 | **new** `meshless_hit_volume_client_declaration_applies_on_host`; no-hitbox miss, unzoned mesh+hitbox hit, outside miss; negative client damage/splash/impact-policy/perception assertions. | planned automated |
| 7 | Real-model locally spawned and display-copy ray tests, including splash_direct map origin and client eye height; assert entity hit and contact. Fix only a reproduced defect. | planned automated |
| 8 | Capture tests exercise every listed primitive via real tag/group/sequence dispatch, all six target shapes, networked overrides and KVP payloads; P15 host+2 clients/late join with actual bridge samples. | planned automated |
| 9 | App CPU conditioned fixture, fixed delay/no jitter/loss on shared virtual clock and injected RTT: finite light/fog completion, loop phase within 33.3ms, host/client stalls, emitter same-frame seed, unchanged writes and late join. | planned automated |
| 10 | **new** `presentation_capture_matches_all_drain_origins`: drive every drain row, UI frame mark ordering, install progress clock0, wait/follow-up subject clearing and any-origin networked light/emitter writes. | planned automated |
| 11 | Resolver round-trip + out-of-range/kind/>1cm rejection; shared log capture asserts one warning and no other entity write. Acknowledge rejection; re-promotion revalidates. | planned automated |
| 12 | No-write/no-record baseline, last write wins, acked bound silence, nooverride descriptor silence, valid initial KVP payload; nonfinite/empty/>64 curve refusal. Running fog/emitter sampled values never mutate retained payload; held MapOrdinal overrides never bind. | planned automated |
| 13 | Encoder ordering/flight/refresh tests plus **new** `bound_presentation_1324_records_respect_budget_and_converge`, real-shaped light/fog/emitter payloads, formula+RTT, tombstone next snapshot and full-envelope admission metrics. Ignored baked-map measurement records actual totals. | planned automated + explicit ignored measurement |
| 14 | Queue/routing matrix across five kinds and trigger/no-subject origins; app conditioned observer proves one local/remote presentation and mover sound position. | planned automated |
| 15 | All five kinds × token/everyone recipients, occupancy captured at edge tick, disconnected seats and interleaved queue order. TS negative scoped-fire case and both-runtime install rejection. | planned automated |
| 16 | Existing player-event presentation regression plus everyone fanout, unseated behavior and wrong trigger tokens rejected by TS and both-runtime installs. | planned automated |
| 17 | SDK type tests and TS/Luau install parity matrix for every named source; unknown/wrong-type audience/nonpresentation misuse; shared named-event Skip warning once, every-machine everyone local once. | planned automated |
| 18 | Extend `a_demoted_client_and_a_level_transition_present_nothing`; held participation gate and single-player matrix, including illegal shared-site token warning. | planned automated |
| 19 | Hosted trigger transitive reachability fixtures, direct/post-wait/fire/onComplete, warn once per binding/reaction; absence single-player/client; commands still execute host-local. | planned automated |
| 20 | Wire/snapshot live pins 27/18; transport packed-ID refusal for26 and app gate mismatch. Preserve historical version fixture meaning. | planned automated |
| 21 | Review author docs/defaults/scope/ownership and SDK-runnable examples; TS+Luau player-event screenShake build/type/IR parity. | planned review + automated |
| 22 | P1/P2 tests below: canonical atomic mutations/lifecycle, spatial/direct/queued HIT exclusion, mixed behavior suspension, resources/refresh/state retention, dropped-item-safe current teardown, generation reuse, client reconstruction/demotion. | planned automated |
| M1 | Owner Mac↔Windows dummy/XP/death/impact/invisible-target and health-bearing pickup/holster/drop runbook below. | outstanding manual; blocks landing |
| M2 | Owner both screens + late join: all presentation targets, prop animation, visibly aligned loop. | outstanding manual; blocks landing |
| M3 | Owner pressure plate in both activation directions; activator-only flash, everyone alarm. | outstanding manual; blocks landing |
| M4 | Owner affected-player E16 XP/scald/local vignette, then forwarded flash/shake with client accommodations. | outstanding manual; blocks landing |

## Tasks

Task names refer to the original brief; P numbers are execution/checkpoint order. Each completed P task includes its plan/proof update in its commit. P1's prerequisite extraction remains a separate behavior-preserving commit as the brief requires. Other original Task 1 extractions also commit separately. No broad parallel editing of shared files.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| P1 | Thin highest-risk proof: sim predicate extraction, canonical ownership seam, generic classifier/sweep and dummy + health-bearing pickup/drop/HIT-before-sweep slice (T1 sim + T2) | integrator | owner plan approval | complete |
| P2 | Complete world participation and ownership lifecycle (T2a), resource/refresh/reconstruction/teardown proofs | integrator; bounded AI/physics worker after seam fixed | P1 | in progress |
| P3 | Real dummy glTF/aim regression (T3) | bounded combat worker | P1 | pending |
| P4 | Extract remote materialization loop to snapshot_apply (T1) | integrator | P2 | pending |
| P5 | Extract wire component mirrors/validation (T1) | bounded worker | P1 | pending |
| P6 | Wire presentation state/bindings/clock/position, versions and all call sites (T5) | integrator | P4,P5 | pending |
| P7 | Descriptor display set + HitVolume22, completeness retry/cache (T4) | integrator | P6 | pending |
| P8 | Extract client mover bindings (T1) | bounded worker | P7 | pending |
| P9 | Stable identity, MapOrdinal23, authored-prefix resolver (T7) | integrator | P7,P8 | pending |
| P10 | Extract replication encoder/state (T1) | bounded worker | P6 | pending |
| P11 | Budget/order/flights/ack repair + byte telemetry (T6) | bounded net worker | P10 | pending |
| P12 | Fire origin, ordered recipients, all drains, tick occupants and positioned sound (T8) | integrator | P6,P9 | pending |
| P13 | TS/Luau audience SDK/types/compiler/install/source gates and UI warning (T9) | bounded SDK worker; integrator shared validation seams | P12 | pending |
| P14 | Host immutable capture/registration and held overrides (T10), then client binding/clock apply/seeding (T11) | integrator | P2,P9,P11,P12 | pending |
| P15 | Dev fixture/docs/E16 examples, app conditioned end-to-end, 1324-record proof + ignored real-map measurement (T12) | integrator; bounded content/docs worker | P3,P7,P11,P13,P14 | pending |
| P16 | Review-readiness, review-panel/fix/focused-retest loop, final preflight once; populate every AC result | integrator + skill-directed reviewers/workers | P15 | pending |
| P17 | External M1–M4 run; record owner evidence and wait for land-the-plane instruction | owner; integrator records | P16 | pending |

### Execution evidence

- P1 prerequisite sim extraction complete. Six production bodies and nine existing test bodies preserved; old capsule-helper path and sibling brain access retained. `cargo test -p postretro-sim --lib scripting::builtins::host_replicated::tests`: 9 passed, 0 failed. Ownership/classification changes remain in progress.


- P1/P2 integration focused checks: `cargo test -p postretro-entities --lib inventory_ownership` (4 passed); `cargo test -p postretro-sim -p postretro-ai -p postretro-physics --lib held_world_participation` (sim 6, AI 3, physics 2 passed); `cargo test -p postretro-netcode --lib enemy_replication_harness_test` (17 passed, including actual client HIT declaration, installed source-addressed XP policy and next-snapshot dummy tombstone, retained-health pickup/drop/fresh identity and held DefaultWeapon).
- Additional focused checks: netcode `descriptor_class::tests` (5 passed), `replication::tests` (11 passed), `held_descriptor` (2 passed: ready/post-FIRE pending HIT before sweep and current-slot cleanup), `lifecycle::tests` (11 passed); sim `scripting::builtins::host_replicated::tests` (10 passed), `authored_loadout_strips_world_item_components_from_held_wieldable` (1 passed), `held_descriptor_lifecycle` (1 passed). Application `cargo test -p postretro --features dev-tools --bin postretro suppressed_descriptor_models_install_hit_zones_once` passed (1 test): the real model parses/uploads once through the injected CPU install seam and retains precise hit zones on both host and suppressed client. P1 complete. P2 production consumers passed their first focused checks; real-touch/refresh/reconstruction extensions remain in progress.

### P1 and P2 boundaries

First prove the false premise's replacement, not just the old happy path. P1 uses a real accepted Weapon+Touchable+Health/hitbox descriptor, canonical Inventory mutation and a deliberately still-mapped former NetworkId. Assert a ray ignores it and host HIT application refuses it immediately; pickup tombstones the client copy, drop re-registers at its pose with retained state, and DefaultWeapon starts excluded. The same slice proves a dummy declaration/XP/host despawn through the replication harness. Extract only the sim predicate code before this slice. A failure changes neither owner Decisions nor proof metrics silently.

P2 owns registry setter/removal/despawn invariants and all remaining consumers:

- `entities/registry.rs`, Inventory access: typed/erased replacement/removal; unchecked mutation protection; duplicate live ownership atomic refusal; same-holder move; item and holder retirement; Inventory removal versus holder destruction; terminating malformed cyclic membership; level-clear and generation reuse. Reuse existing selection normalization; no new selection rules.
- `sim/builtins/wieldable_inventory.rs`, `net_descriptor.rs`, touch/drop and weapon commands: composition failure rollback, acquisition conflicts with unchanged item/components, successful release before drop; connected-client Control/baseline order, retuning/replacement/removal; held descriptor refresh.
- `sim/scripting_systems/hit_zones.rs`, `health.rs`, `sim/splash.rs`, `projectile_stage.rs`, `weapon_stage/impact.rs`: shared world query exclusion and target rechecks on earlier contacts; no damage, knockback or target policy after pickup; general explicit script mutation stays valid. Firing weapon ownership is expected, not a refusal.
- `netcode/lib.rs` ready-HIT apply and post-FIRE pending flush: ownership check before sweep, hitscan accepted contacts/credit absent on refusal; new drop ID succeeds and old ID stays invalid. Preserve existing remote projectile endpoint presentation.
- `ai/lib.rs`, `targeting.rs`, `apply.rs`, `compute.rs`; `sim/agent_steering.rs`; `physics/kinematic_mover/blocking.rs`: mixed behavior item cannot act/move/separate/target/block/crush while held, retained targets and stale computed work recheck; dropping restores world participation without deleting controller/Brain/Agent state.
- `netcode/lifecycle.rs`, stale slot cleanup, client demotion/despawn: retire current inventory, including later pickups, and remove stale initial-ID authority. Raw holder despawn before close/replacement leaves no orphan; former dropped/reacquired items survive. Central destruction terminates generation-safely without a general ownership graph API.
- Extend loadout, touch, passive heat/cell, inactive recovery, reload/equip and held deferred-despawn tests. Preserve authoritative-only resource advancement and existing activation cancellation on drop. Test retained nondefault Health/resources, not only default values.

Proposed focused ownership groups: `inventory_ownership` (entities), `held_world_participation` (sim/AI/physics), and `held_descriptor_lifecycle` (netcode). Final names/commands must be recorded after implementation and filters must match nonzero tests.

### Integration and delegation

At most three workers plus integrator. Disjoint useful concurrency: P3 real-model test while integrator completes P2; bounded AI/physics consumer edits only after registry API is fixed; P11 encoder while integrator routes origins; P13 SDK private files after P12 shared context/gate call sites stabilize. Integrator retains `registry.rs` component discriminants, netcode shared lib/client, app main/frame/install, resolver trait, cross-crate call-site migrations and plan. No worker independently changes a Decision.

Wire variants land before their consumers. HitVolume22 lands before MapOrdinal23 and shared exhaustive matches. Identity/install changes land before fire-origin install edits. Routing context lands before SDK install-gate edits. Capture lands before client apply extends the resolver's seed methods and main receive call. P14 executes host then client sequentially at those seams; the brief's conceptual concurrent halves do not authorize overlapping edits.

Workers read full brief, research, AC list, this plan and routed docs. Give explicit owned files/upstream API/downstream consumers, no commit/plan permission. Integrator runs Cargo only after concurrent mutations stop, using default dev profile and dev-tools for app targets. No second checkout/target unless an actual independent need and disk budget justify it.

## Verification commands and gates

Existing test seams to extend; commands below have **not** run. Select focused filters per task, then record actual matched count and result. Do not treat a zero-match filter as proof.

```sh
cargo test -p postretro-entities --lib inventory
cargo test -p postretro-sim --lib authored_loadout_strips_world_item_components_from_held_wieldable
cargo test -p postretro-netcode --lib classifier_agrees_with_live_predicate_one_source_of_truth
cargo test -p postretro-netcode --lib world_item_baseline_acquire_despawn_and_drop_rebaseline_materialize_one_mesh
cargo test -p postretro-netcode --lib materialize_armed_remote_enemy
cargo test -p postretro-netcode --lib presentation_commands
cargo test -p postretro-net --lib wire::tests
cargo test -p postretro-net --lib handshake
cargo test -p postretro-net --lib replication::tests
cargo test -p postretro-net --lib timesync::tests
cargo test -p postretro-scripting-core --lib reaction_dispatch
cargo test -p postretro-sim --lib player_events
cargo test -p postretro-sim --lib trigger_bindings
cargo test -p postretro-sim --lib player_event_sdk_tests
cargo test -p postretro --features dev-tools --bin postretro reaction_validation
cargo test -p postretro --features dev-tools --bin postretro scripting_systems::light_bridge::tests
cargo test -p postretro --features dev-tools --bin postretro scripting_systems::fog_volume_bridge::tests
cargo test -p postretro --features dev-tools --bin postretro scripting_systems::emitter_bridge::tests
cargo test -p postretro --features dev-tools --bin postretro host_activations::conditioned_tests
cargo test -p postretro-sim --lib committed_sdk_types_match_current_registry
cargo test -p postretro-script-compiler --lib luau_build_prelude_exports_match_runtime_prelude
tsc --noEmit -p sdk/type-tests/tsconfig.json
```

When typedef templates change, regenerate with `cargo run -p postretro-sim --bin gen-script-types`, then type/IR parity tests. No `sdk/package.json` runner exists. Tests use real dispatch/install and shared log capture; no private process-global logger.

Before panel: fmt check, cargo check touched crates, focused behavior/integration tests. Apply `/review-panel` then `/fix-review-findings`, focused retest; repeat only for concrete new findings. After that loop, `/preflight` once (fmt, isolated clippy target with warnings denied, full workspace tests); include dev-tools checks for touched gated app paths. Do not run the full workspace suite earlier. Review the changed crate layering and exhaustive component matches.

Resource measurement pins 1,324 real-shaped records, budget subtotal, whole-envelope bytes after state/faction splice, convergence snapshots/RTT, no-flight-resend and tombstone timing. Run the ignored stress-map measurement after baking; record actual counts/bytes and command, not invented estimates. If full envelopes cannot be admitted with the stated workload/transport config, stop for a grounded policy decision rather than changing exempt traffic or silently increasing transport allowance.

## External runbook (blocking)

P15 finalizes fixture target labels and launch/build commands in this section before `test-ready`. The following observations remain external even if all automated tests pass.

Preparation: same branch/build/content on Mac and Windows; compile helpers/engine first. Checkout authoring uses `postretro-tool run --install-root .` from workspace root, which discovers root `postretro.toml` (`mod=dev`, package `postretro-dev`). Example host after fixture exists: `cargo run -p postretro-tool -- run --install-root . maps/coop-presentation-test.prl --host 27015`; peer uses `--connect <host-address>:27015`. Repeat on `maps/splash-damage-demo.prl`. Human docs give bundle `bin/postretro-tool` commands, never xtask. Capture platform/build SHA, map, host/client roles, join timing, accommodation values and result per M row. Swap host/client roles for M1/M3.

- **M1:** Client pistol hits real dummy torso, XP increases only for that player; either player kills a dummy, both see death/linger/despawn and impacts. Shoot invisible hitbox target. Pick up fixture health-bearing weapon, switch away/back and drop; both see ground copy disappear/return at the drop pose, retained Health/resources and normal use. P15 labels the fixture and gives an observable retained-state setup.
- **M2:** Fire atmosphere/prop trigger with both peers present; compare PRL and descriptor light, fog, emitter stop/spin and prop animation. A third process or reconnecting peer joins after the changes; it sees current values and finite completion rather than a restart. Compare loop pulses at the same moment on both screens.
- **M3:** Client enters pressure plate: only client flashes, both hear alarm; reverse with host as activator. With both inside, on.trigger shake reaches both; activator click remains local. No double playback.
- **M4:** Start with fresh or backed-up dev progression so the guarded XP>=100 and level<2 milestone can fire (never delete personal saves implicitly). Kill to threshold; only affected player gets fanfare/flash/shake. P15 must provide a reachable, named heat-resource weapon in fixture because current dev content only declares the scald event; fire to overheat, observe 5 damage/hiss only there, then cool. Lower one player's health through 25 and observe local no-delay vignette. On client enable flash limiter and reduce motion, trigger forwarded flash/shake, verify accommodations apply; record configured values. Repeat TS/Luau fixture install parity in automation; external visual proof remains owner-run.

After P16 add AC result column and exact external commands. If M1–M4 outstanding, set `test-ready`, commit evidence and wait. After owner results resolve all blocking rows, report branch/table/review and wait for **land the plane**. Only then update remaining durable contracts, move to done/roadmap, clean session-owned artifacts, and push feature branch. No PR or merge is implied by plan approval.
