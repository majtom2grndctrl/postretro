# E25--landmark-level-change — plan of record

mode: compact
status: active
read at: e743823

## Corrections
- (none yet) Source changed since the brief's `8489a1f` only in streaming, capture and debug-UI code; no symbol the Decisions or Path cite moved. Decision reads reused.

## Delegated answers
- None: the brief has no open questions.

## AC-to-proof

Test homes: compile rows in `level-compiler` (bin `prl-build` and `tests/`), wire rows in `level-format`, arrival maths in `physics` (`movement/teleport` tests), activation, latch, occupancy and events in `sim` (trigger and seam stage tests), level-change, spawn, co-op and save rows in the `postretro` binary's CPU lifecycle tests (`test_app()`, no adapter). A row needing a full install names its test as GPU-gated in the PR.

| AC | Proof | Status |
|---|---|---|
| Arr 1 offset, yaw/pitch, velocity rotation, boost, vault, equals seamless teleport | `physics` seam-arrival tests beside `teleport_arrival_tests` | achievable as stated |
| Arr 2 facing and velocity read before unload (host and client) | `postretro` harvest test at `unload_level` | achievable as stated |
| Arr 3 no landmark → bounds centre yaw 0; authored yaw rotates | compiler test + arrival mapping test | achievable as stated |
| Arr 4 missing destination seam / blocked → start, one warning | `postretro` spawn tests with log capture | achievable as stated |
| Arr 5 arrival latched against armed teleporter | arrival spawn test reading `arrival_latches` | achievable as stated |
| Arr 6 host settles/reveals at arrival pose | install test on presented pose / camera | achievable as stated |
| Arr 7 zero-health pawn arrives landmark-relative, health per level change | harvest + spawn test | achievable as stated |
| Arr 8 disarmed destination seam behaves as armed | spawn + arrive event test | achievable as stated |
| Arr 9 client with pre-teleport stamp arrives as host-side pawn faced (P8) | harvest facing test via `HostCommandQueues` | achievable as stated |
| Carry 1 carried state crosses as `loadLevel` | lifecycle test comparing carry | achievable as stated |
| Carry 2 first spawn consumes; respawn and later non-seam level use starts silently (P2) | lifecycle test | achievable as stated |
| Carry 3 pawn outside at harvest → start (P1) | harvest test | achievable as stated |
| Carry 4 failed / cancelled seam load leaves no arrival (P12) | lifecycle test | achievable as stated |
| Carry 5 suspend at three points leaves no arrival (P6) | lifecycle test | achievable as stated |
| Ckpt 1 SP crossing saves seam name + pose; Continue twice at pose (P22) | save + Continue lifecycle test | achievable as stated |
| Ckpt 2 blocked pose / missing seam on Continue → start, one warning | lifecycle test, log capture | achievable as stated |
| Ckpt 3 failed load after save keeps seam checkpoint (P22) | lifecycle test | achievable as stated |
| Ckpt 4 host outside → seam mark, no pose; client crossing writes no pose | save test | achievable as stated |
| Ckpt 5 seam spawn set for respawns/joiners; silent fallback; later checkpoint replaces (P23) | spawn-set tests | achievable as stated |
| Ckpt 6 `LevelRecord` round-trips; old run file loads | `save_files` document tests | achievable as stated |
| Ckpt 7 Continue over a backdrop keeps pose (P24) | lifecycle test | achievable as stated |
| Ckpt 8 no run / unsaveable store still selects set silently (P25) | lifecycle test | achievable as stated |
| Ckpt 9 typo checkpoint still warns; no-pose Continue silent (P26) | spawn-set test, log capture | achievable as stated |
| Ckpt 10 later checkpoint / restart outlives pose (P27) | save test | achievable as stated |
| Ckpt 11 suspend then Continue (P28) | lifecycle test | achievable as stated |
| Ckpt 12 Continue settles at saved pose, fires `arrive` once | lifecycle + event test | achievable as stated |
| Ckpt 13 hosted Continue: clients in seam set, no reservation (P29) | resolver test | achievable as stated |
| Ckpt 14 `checkpoint(name)` on activation frame (P32) | lifecycle test | achievable as stated |
| Act 1 touch/use start; use without overlap no; disarmed no; backdrop no, no arrival | sim stage + lifecycle tests | achievable as stated |
| Act 2 popped frontend root over backdrop stays inert (P30) | lifecycle test | achievable as stated |
| Act 3 clears no pushed tree; pause → played, frontend root → backdrop (P31) | lifecycle test | achievable as stated |
| Act 4 script level verb on same frame wins, no arrival (P3) | frame-end pick test | achievable as stated |
| Act 5 unknown target warns once, no load | pick test, log capture | achievable as stated |
| Act 6 first in map order wins (P11, P15) | pick test | achievable as stated |
| Act 7 activated seam loads after disarm/empty (P21) | pick test | achievable as stated |
| Act 8 unknown-target first + valid seam (P16) | pick test | achievable as stated |
| Act 9 AABB-only overlap is outside | sim inside-test test | achievable as stated |
| Act 10 arming with player inside fires next trigger tick | sim stage test | achievable as stated |
| Act 11 zero-HP undeclared cannot activate; declared follows capabilities | sim stage test | achievable as stated |
| Act 12 seam-only tag: no absent-tag warning; counts occupants | install + occupancy test | achievable as stated |
| Act 13 bent seam occupancy, armed or not, capability rules; all-players arm moves everyone | sim occupancy test + lifecycle test | achievable as stated |
| Latch 1 D11 table incl. P4 | sim latch tests | achievable as stated |
| Latch 2 `arrive` once per arriving pawn, host-only, first tick; none for start-placed; brush and script bindings | sim binding + lifecycle test | achievable as stated |
| Latch 3 teleport into touch seam fires same tick (P9) | sim stage test | achievable as stated |
| Latch 4 respawn onto start inside seam latched (P14, P19) | sim respawn test | achievable as stated |
| Latch 5 latch in disarmed seam clears on leave (P18) | sim latch test | achievable as stated |
| Latch 6 spawn beside bend not latched | sim latch test | achievable as stated |
| Latch 7 self-target reload, occupants where they stood (P10) | lifecycle test | achievable as stated |
| Latch 8 scripting example compiles TS and Luau; disarms on arrival on dev pair | script-compiler test + dev content; in-engine part manual | achievable as stated |
| Coop 1 client inside → arrival at participation entry; outside → start | lifecycle co-op test | achievable as stated |
| Coop 2 client settles/reveals/arms at arrival; holds no start | resolver + hint test | achievable as stated |
| Coop 3 blocked at resolve / at promotion → start, warning, rehint (P13) | resolver/promotion test | achievable as stated |
| Coop 4 no resolved command → current yaw, pitch 0 | harvest test | achievable as stated |
| Coop 5 host change before client spawns clears arrival (P5, P20) | lifecycle test | achievable as stated |
| Coop 6 demote/reconnect after arrival → start; reconnect before → arrival (P7) | resolver test | achievable as stated |
| Coop 7 blocked then re-promote → start (P17) | resolver test | achievable as stated |
| Comp 1 build failures name entity; no-spawn warns; blanks default | compiler tests | achievable as stated |
| Comp 2 arrival spawns as start's class | spawn test | achievable as stated |
| Comp 3 FGD defaults equal compiler's | FGD drift test | achievable as stated |
| Comp 4 rename; grep gate | compiler test + grep | achievable as stated |
| Comp 5 `seamless` crosses as classic, no warning | compiler + arrival test, log capture | achievable as stated |
| Manual 1 dev pair cross and back, turned and moving | owner, in-engine | manual |
| Manual 2 co-op crossing, one in one out | owner, in-engine | manual |
| Manual 3 run cross, quit, Continue | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Rename `streaming_seam_volume` → `stream_split_volume` (own commit) | integrating executor | — | |
| 2 | Behavior-preserving splits of `trigger_system.rs` and `parse.rs` (own commits) | integrating executor | 1 | |
| 3 | Factor teleport arrival apply out of `resolve`; seam arrival mapping (riskiest premise: the chokepoint splits cleanly) | integrating executor | — | |
| 4 | Compile + PRL id 61 + loader + FGD + compiler tests + dev fixture pair | worker A (worktree) | 2 | |
| 5 | Script surface: component, `getMapEntities("seam")`, handles TS/Luau, arm/disarm, `seamEvents`, `@seam`, binding table | worker B (worktree) | — | |
| 6 | Runtime install, inside test, activation, latch, occupancy, `arrive` dispatch | integrating executor | 3, 4, 5 | |
| 7 | Frame-end pick, seam `LevelRequest`, harvest at unload, arrival clears | integrating executor | 6 | |
| 8 | Arrival spawn (host / SP), respawn latch, blocked fallback, Settling view, `arrive` | integrating executor | 7 | |
| 9 | Co-op client arrival through the resolver and participation entry | integrating executor | 8 | |
| 10 | Seam checkpoint: run record, level-start save, Continue, spawn-set fallback | integrating executor | 8 | |
| 11 | Docs, dev-mod seam pair, scripting example; drop "not yet built" markers at landing | integrating executor | 6–10 | |
| 12 | Preflight, review panel, fixes | integrating executor | 11 | |
