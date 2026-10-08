# sdk-addressing-model — plan of record

mode: compact
status: active
read at: c855e3e18

## Corrections
- No scripting, sim, entities, SDK or content source changed between the brief's `read at` 2bc52d471 and c855e3e18, so the grounded Decision reads hold.
- AC M2 "(TS and Luau)" → only trigger-fanout-fixture has a `.luau` twin; arena-lights, switch-demo and coop-two-button-puzzles are TS only. Read as "each runtime twin that exists"; same meaning.
- AC M2 → coop-two-button-puzzles has no map and arena-lights runs on campaign-test, so manifest data is captured by evaluating each script's `setupLevel` through `ScriptRuntime::run_data_script` over one fixed synthetic registry, emitting the raw returned JSON (keys sorted).
- Path "split `data_script.luau`" → Luau's prelude has no `require`, so the split publishes part chunks through a temporary `__postretroDataScriptParts` global (`luau_prelude::evaluate_data_script_sdk`, mirrored in `script-compiler/src/light_membership.rs`). Part files are `--!nocheck` because their type names live in `data_script.luau`.
- Path "sequence validation" → `sequence_primitives_are_valid` (install) rejected every `@activators` grant step because the id-step registry holds no grant/damage primitive, dropping them before V4a could name the reaction. It now skips subject-token and group steps; V4a (`reaction_validation.rs` Pass A) owns post-`wait` token rejection, and an unknown primitive on those steps warns at partition/dispatch.
- Group steps get no install-time unknown-primitive check (validation sites hold only the sequence registry, as primitive bodies already do); an unknown group primitive warns `is not registered` at fire. Threading the reaction registry into validation is a possible follow-up.
- Path "`bind_sequence_step` maps it to `BoundTarget::Tag`" → maps to a new `BoundTarget::Group(GroupTarget)` resolved through `resolve_group`, since `Tag` cannot carry a kind or an absent tag (A1, A2). A non-consequential pre-`wait` group step goes to the residual and drains through `dispatch_group`.
- R1/R2 tick path → trigger ticks never run on a connected client (`main.rs` connected-client branch `continue`s before `simulate_tick_with_presentation_aim`), so the tick-path test proves the gate only; the client-reachable proof is Task 3's named-dispatch tests.
- Spawned NPCs arrive with aggro armed (`spawner.rs`, `enabled_on_spawn=true`), so Q2/A4 tests observe the group step with `update({ aggro: false })`; a control spawn confirms the default.
- M8 predicate ("map placement or no provenance") applies to every `worldQuery` kind, so `transform` queries also drop runtime spawns and player pawns. Baselines unchanged.
- `progress` recompose now keeps each tag's install-time set, kill tally and fired latch (it used to reset and recount, which would have pulled in tagged spawns).
- Owner amendment (2026-10-07), resolving Task 5's `progress` trap: spawner-tag inheritance is replaced by an `entity_spawner` FGD key `spawned_tags`; the spawner's own `_tags` never pass to spawns. Brief Decision, S1, U1, example and Boundary inventory amended. `progress` membership is unchanged (every entity carrying the tag at install). Task 5b swaps the implementation.
- Trigger-event dedupe covers every resolved manifest binding (two mod rules with different tags reaching one volume/edge/reaction also bind once and warn), not only the mod-global + level-member pair. Brush KVP bindings are not deduped against manifest events.
- Level-script trigger-event rejections name the source as "level script `setupLevel`", not its file path (`LevelManifest::from_js_value` has no path).
- `getMapEntities` data-context check: thread-local `level_data_context` guard held by `ScriptRuntime::run_data_script`; `worldQuery` raises outside it. `scripting.md` §3 ("scope never enforced at call time") needs updating in Task 8.
- Transitional breakage until Task 8: closet-reveal, spawner-test, trigger-event-presser-fixture (ts/luau), combat-demo-reaction and a11y-strobe-test return level tag-keyed trigger events, now rejected at load. Tests weakened to assert rejection, to restore in Task 8: `trigger_event_presser_fixtures_produce_identical_wire_in_both_runtimes`, `the_strobe_fixture_evaluates`.
- AC M1 "light-membership output byte-identical" → the reserved `lights` records are byte-identical on all 14 maps; the sidecar's diagnostic `stubbedPrimitives` inventory gains `worldQuery:<component>` entries on 8 maps because migrated scripts now query trigger/spawner/mover members (a direct consequence of the volume-keyed trigger-event and spawner-member Decisions). Read as: membership identical, stub inventory may name the newly queried member kinds. Same meaning.
- AC U2 regex `(damage|grantHealth|grantAmmo|addSlot)\(\s*["'@]` also matches mandated method calls (`on.activators.grantAmmo("…")`, pre-existing `impact.source.grantAmmo(…)`). Gate applies it to free calls only: no preceding `.`, `:` or identifier character. Same meaning (retire free verbs with string targets). `sdk/type-tests/addressing.ts` reaches retired names by element access to prove they are gone.
- `on.activators` verbs returned primitive bodies only (as the old free verbs did), so a pre-`wait` subject-token sequence step was reachable only as raw wire. Task 7b makes token verbs dual-use like group verbs, so the brief's "subject-token command legal before any `wait`" is authorable.
- `entities/transforms.{ts,luau}` deleted (no consumer; transform is not a member kind). spawner-test's raw tag-keyed `moverStart` became a mover member step. `tools/gen_stress_map.py` and both generated stress scripts migrated in Task 7 (M1 needed them).
- Client role check reuses the `owner_slot_writes_enabled` flag unchanged, decided in one predicate `group_resolution::group_commands_apply_here`; it now gates group commands as well as owner-slot writes.

## Delegated answers
- Which component marks the `npc` kind — `BrainComponent`, excluding any entity bound to a seat (`EntityRegistry::seat_for_pawn`). Brain is the existing "brain-driven character" marker `updateEnemyState` already filters on; the seat exclusion keeps a future brained pawn out of `npcs`.
- Whether raw `worldQuery` stays visible in generated typedefs — hidden from the author-facing typedefs; the SDK lib declares it privately. A visible raw query reintroduces the kindless snapshot spelling the brief retires. If the SDK lib's compile cannot declare it privately, keep it visible but undocumented and record that here.

## Baseline (AC 1, AC 2)
Baseline commit: c855e3e18 (`main` after the claim). Before-data is captured from that commit: manifest JSON for arena-lights, switch-demo, coop-two-button-puzzles, trigger-fanout-fixture (TS and Luau), and build-time light-membership output for every content map. A resumed session without the session scratchpad regenerates it from c855e3e18 in a throwaway worktree. Capture: `bash <scratchpad>/baseline/capture.sh <checkout> <out-dir>` then `diff -r baseline/before <out-dir>`; build the driver with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0` (own target dir, deleted after). M1 = the light-membership sidecar `scripts-build` hands `prl-build` for each of the 14 content maps with a data script (22 without are listed in `_no-data-script.txt`). M2 = `setupLevel` return JSON for arena-lights.ts, switch-demo.ts, coop-two-button-puzzles.ts, trigger-fanout-fixture.{ts,luau}. Deterministic across two runs; identical after Task 2's splits.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| M1 light-membership byte-identical, every content map | baseline diff script over every content map vs c855e3e18 | achievable as stated |
| M2 four scripts' manifest data byte-identical (TS+Luau) | baseline diff of manifest JSON vs c855e3e18 | achievable as stated |
| M3 `getMapEntities("npc"/"transform")` fails TS compile, raises in Luau; no match → `[]` | sdk type-test (`@ts-expect-error`) + Luau/JS runtime tests | achievable as stated |
| M4 `getMapEntities` raises in mod start script; works in `setupLevel` | scripting-core tests, both runtimes | achievable as stated |
| M5 spawner member `fire()` spawns from that spawner only (name-fired, after wait, trigger tick); two steps → two batches | sim tests (A3, A5) | achievable as stated |
| M6 trigger member `on("enter")` fires for that volume only | sim trigger-binding test | achievable as stated |
| M7 light member + group step reserves membership as without (A12) | script-compiler `light_membership` test | achievable as stated |
| M8 `getMapEntities("light"/"emitter")` excludes NPC-carried | world-query test on provenance predicate | achievable as stated |
| G1 `npcs({tag}).damage` skips tagged pawn (3 paths); tagless reaches every NPC (A1) | sim tests | achievable as stated |
| G2 `players()`/tagless `npcs()` bind to trigger edge, apply in tick, no missing-tag warning (A2) | sim test + log capture | achievable as stated |
| G3 `players().grantHealth` credits each pawn once; `on.activators` only activators | sim test | achievable as stated |
| G4 deterministic match order incl. slot reuse | registry/dispatch test | achievable as stated |
| G5 disconnect hold skips pawn for every verb; reclaim restores; SP reaches local pawn | dispatch tests | achievable as stated |
| S1 spawned NPC carries `spawned_tags`; spawner tagged `x` without `spawned_tags` → untagged | `spawner.rs` test | achievable as stated |
| S2 `progress` counts install-time members only | progress tracker test | achievable as stated |
| S3 group misuse fails TS compile / raises in Luau (`.length`, `.map`, `damage("boss",10)`, `npcs().fire()`, `#g`, `g[1]`, `g.length`) | sdk type-test + Luau runtime test | achievable as stated |
| Q1 wait then NPC/player group step tracks membership changes; zero match debug no-op (A6) | scheduler/dispatch tests | achievable as stated |
| Q2 `[s.fire(), npcs().update]` aggroes the just-spawned, 3 paths (A4) | sim tests | achievable as stated |
| Q3 subject token after wait dropped with error; group after wait installs (A7) | install-validation test + log capture | achievable as stated |
| Q4 mover member step then group step in authored order; Exit cancel runs neither | scheduler test | achievable as stated |
| Q5 trigger-fired group / `on.activators` step before wait applies in tick | sim test | achievable as stated |
| T1 `defineTriggerEvent` binds tagged volumes per `levels`, fires as `onTriggerEvent` did | sim/startup test | achievable as stated |
| T2 tag-keyed event from `setupLevel` rejected w/ warning naming script; volume-keyed siblings install | startup test + log capture | achievable as stated |
| T3 excluded level binds nothing; volume-keyed in `ModManifest` rejected w/ warning naming manifest | startup test + log capture | achievable as stated |
| T4 interruptible wait via member `on` installs; exit derived per volume; sibling inert; `once` drops (A8) | validation + binding tests | achievable as stated |
| T5 brush → mod-global → level member order on one edge (A9) | binding test | achievable as stated |
| T6 member `on` survives mod hot reload recompose (A10) | recompose test | achievable as stated |
| T7 same reaction via mod-global + level `t.on` runs once, one warning naming both | binding test + log capture | achievable as stated |
| T8 `on.trigger.disarm()/arm()` target the fired volume | sim test | achievable as stated |
| T9 `getGravity`/`setGravity` match old behavior incl. non-finite warn/no-op | scripting test | achievable as stated |
| R1 client: group commands apply nothing, log ≤ debug; host applies all; post-wait never on client | dispatch tests + log capture | achievable as stated |
| R2 client: member step beside skipped group step applies (A11) | dispatch test | achievable as stated |
| W1 parse rejects `id`+`kind` / bad `kind`, naming reaction (both runtimes); raw kindless descriptors unchanged; `updateEnemyState` rejected unknown | parse tests + raw-descriptor tests | achievable as stated |
| W2 `WIRE_VERSION` unchanged; no `NetworkId` in scripting crates / `sdk/` / typedefs | grep gate script | achievable as stated |
| U1 Scripting surface example replaces closet-reveal; after wait: NPCs (placed+spawned) aggroed+damaged, spawner count, players ammo; TS/Luau byte-identical | integration test driving the compiled manifest + twin diff | achievable as stated |
| U2 regression grep gate over `content/ sdk/ docs/ context/lib/`; no hand-written player `applyDamage` | grep gate script | achievable as stated |
| Manual: host+client playtest of closet-reveal and coop-two-button-puzzles | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Capture baselines (M1, M2) from c855e3e18; record commands here | worker | — | done |
| 2 | Behavior-preserving splits of `reaction_dispatch.rs`, `trigger_bindings.rs`, `data_script.ts`, `data_script.luau` (one commit each) | worker | 1 | done — 11d23900c, b18ad72a9, 647960755, c58e35ced; scripting-core 749+2, sim 1246, script-compiler 37 pass; baselines identical |
| 3 | First slice: wire `kind` on primitive descriptor + sequence entry (JS+Luau parse, rejections), `SequenceTarget` group arm, kind resolution (npc = Brain minus seat-bound, player = seat-bound), client role check, `updateEnemyState`→`updateNpcState`, light pass/`collect_membership` skip; test NPC spawned during wait | worker | 2 | done — `GroupKind`/`GroupTarget` (entities), `group_resolution::resolve_group`, `reaction_dispatch/group_dispatch.rs`; W1 G1(named/wait) G3(players) G4 G5 Q1 Q3 Q4 R1 R2 M7 pass; scripting-core 754, sim 1258, script-compiler 38, postretro reaction_validation 30; baselines identical. Trigger-tick group binding currently warns-and-skips (Task 4) |
| 4 | Trigger tick path: group `BoundTarget` (kind, optional tag), `bind_sequence_step`/partition group arm; A1, A2, A4-tick, Q5, R1/R2 in tick | worker | 3 | done — `BoundTarget::Group`, shared role predicate; G1(tick) G2 G3(activators) Q5 A4(tick half) R1/R2(gate) pass in `trigger_bindings::group_tick_tests`; sim 1264, scripting-core 756; baselines identical |
| 5 | Spawner member: `spawner` query kind + snapshot, sequenced/bound id `spawnFromSpawner`, spawned NPCs inherit tags, `progress` install-time membership (M5, S1, S2, A3–A5) | worker | 4 | done — `worldQuery` `spawner` arm `{id, position, tags}`, `is_map_placed` on every kind, `Spawn{Entity}` bound arm + sequenced `spawnFromSpawner`, tag inheritance, `ProgressTracker` id membership; M5 M8 S1 S2 Q2 pass; sim 1276, postretro bin 1184, netcode 524, ai 241; baselines identical |
| 5b | Replace spawner-tag inheritance with `spawned_tags` (FGD key, parse, spawn path, spawner snapshot field, S1 tests) | worker | 5, 6 | done — `SpawnerComponent.spawned_tags` via generic KVP bag (no PRL change), snapshot `spawnedTags`; S1 S2 M8 retested; sim 1287, postretro bin 1187, netcode 524; baselines identical. Task 8: `spawner-test.map` needs `spawned_tags "closet"` and `closet`-tagged NPCs for U1 |
| 6 | Trigger events: volume-keyed level form, manifest-only `defineTriggerEvent`, rejections, resolved-binding dedupe, A8–A10 ordering/validation/hot reload; `getMapEntities` data-context raise | worker | 4 | done — `VolumeTriggerEventDescriptor`, `resolve_manifest_trigger_events` + `dedupe_resolved`, `level_data_context` guard; M4 M6 T1–T8 pass; sim 1286, scripting-core 757, entities 284, postretro bin 1187; baselines identical |
| 7 | SDK surface TS+Luau: `getMapEntities`, `npcs`/`players` groups, methods on targets and tokens, gravity, `t.on`, `defineTriggerEvent`; retire free verbs/`world`/`enemies`/`spawner`/`onTriggerEvent`; typedef templates, prelude, generated typedefs, type-tests, parity tests; migrate every `content/dev/scripts` script and restore the two weakened fixture tests | worker | 5b, 6 | done — `map_entities`, `gravity`, group/token commands, member handles in both runtimes; Luau opacity via frozen metatables; raw `worldQuery` hidden from typedefs; M3 S3 T9 + SDK halves of G3 T8 W1 pass; M2 identical, M1 lights identical (stub inventory note); scripting-core 764, sim 1297, script-compiler 38, postretro bin 1187; type tests `npx -y -p typescript@6 tsc -p sdk/type-tests` (4 pre-existing errors) |
| 7b | Subject-token verbs dual-use (body and pre-`wait` sequence entry) in both runtimes | worker | 7 | |
| 8 | Consumers: docs, context/lib, FGD and crate comments; closet-reveal surface example + Luau twin + map tags; U1 integration test; grep gates (W2, U2); baseline diffs (M1, M2) | worker | 7 | |
| 9 | Preflight, review panel, fix loop, full gate | integrating executor | 8 | |
