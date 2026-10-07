# sdk-addressing-model — plan of record

mode: compact
status: active
read at: c855e3e18

## Corrections
- None yet. No scripting, sim, entities, SDK or content source changed between the brief's `read at` 2bc52d471 and c855e3e18, so the grounded Decision reads hold.

## Delegated answers
- Which component marks the `npc` kind — `BrainComponent`, excluding any entity bound to a seat (`EntityRegistry::seat_for_pawn`). Brain is the existing "brain-driven character" marker `updateEnemyState` already filters on; the seat exclusion keeps a future brained pawn out of `npcs`.
- Whether raw `worldQuery` stays visible in generated typedefs — hidden from the author-facing typedefs; the SDK lib declares it privately. A visible raw query reintroduces the kindless snapshot spelling the brief retires. If the SDK lib's compile cannot declare it privately, keep it visible but undocumented and record that here.

## Baseline (AC 1, AC 2)
Baseline commit: c855e3e18 (`main` after the claim). Before-data is captured from that commit: manifest JSON for arena-lights, switch-demo, coop-two-button-puzzles, trigger-fanout-fixture (TS and Luau), and build-time light-membership output for every content map. A resumed session without the session scratchpad regenerates it from c855e3e18 in a throwaway worktree. Capture commands are recorded in Task 1's row once written.

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
| S1 spawned NPC carries spawner tags; untagged spawner → untagged | `spawner.rs` test | achievable as stated |
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
| 1 | Capture baselines (M1, M2) from c855e3e18; record commands here | worker | — | |
| 2 | Behavior-preserving splits of `reaction_dispatch.rs`, `trigger_bindings.rs`, `data_script.ts`, `data_script.luau` (one commit each) | worker | 1 | |
| 3 | First slice: wire `kind` on primitive descriptor + sequence entry (JS+Luau parse, rejections), `SequenceTarget` group arm, kind resolution (npc = Brain minus seat-bound, player = seat-bound), client role check, `updateEnemyState`→`updateNpcState`, light pass/`collect_membership` skip; test NPC spawned during wait | worker | 2 | |
| 4 | Trigger tick path: group `BoundTarget` (kind, optional tag), `bind_sequence_step`/partition group arm; A1, A2, A4-tick, Q5, R1/R2 in tick | worker | 3 | |
| 5 | Spawner member: `spawner` query kind + snapshot, sequenced/bound id `spawnFromSpawner`, spawned NPCs inherit tags, `progress` install-time membership (M5, S1, S2, A3–A5) | worker | 4 | |
| 6 | Trigger events: volume-keyed level form, manifest-only `defineTriggerEvent`, rejections, resolved-binding dedupe, A8–A10 ordering/validation/hot reload; `getMapEntities` data-context raise | worker | 4 | |
| 7 | SDK surface TS+Luau: `getMapEntities`, `npcs`/`players` groups, methods on targets and tokens, gravity, `t.on`, `defineTriggerEvent`; retire free verbs/`world`/`enemies`/`spawner`/`onTriggerEvent`; typedef templates, prelude, generated typedefs, type-tests, parity tests | worker | 5, 6 | |
| 8 | Consumers: content, generated maps, `gen_stress_map.py`, docs, context/lib; closet-reveal surface example + Luau twin + map tags; U1 integration test; grep gates (W2, U2); baseline diffs (M1, M2) | worker | 7 | |
| 9 | Preflight, review panel, fix loop, full gate | integrating executor | 8 | |
