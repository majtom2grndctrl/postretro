# E10--npc-weapon-mounts — plan of record

mode: resumable
status: approved
read at: 0773e4bdb

Source unchanged in `crates/`, `sdk/` since the brief's `dcd60bfb3`; only `content/dev/models/mutant_arm_cannon/` was added. Every symbol cited by Decisions and Path was re-read at `0773e4bdb`.

## Corrections
- Research §Fire decision: `slot_sees_target` lives in `scoring.rs` → it is `CombatQuery::slot_sees_target` at `crates/sim/src/combat_positioning.rs:89`, called from `combat_positioning/scoring.rs` and `uncached_reference.rs`. No effect here: the cover brief owns the scorer.
- Path, muzzle pose eligibility: the airborne launch clip is not a clock field → it is `brain.launch.airborne_presented`, read through `launch::airborne_clip` in `drawn_state` (`crates/ai/src/clip_clock.rs`). Planning around it by reading that field at the launch, beside the new interim record on `ClipClock`.
- Path, launch order: in `apply_outcomes` (`crates/ai/src/apply.rs`) `advance_actor` runs before the own-switch block (`should_switch_animation`, travel variant, launch clip, redraw, gated by `flinch_shows`). Planning around it by hoisting that predicate into a pure function evaluated before `advance_actor`, then reusing its result for the switch itself, so the two cannot disagree.
- Path, files past ~800 lines (`compute.rs` 954, `hit_zones.rs` 4092, `data_archetype.rs` 3865; `ai_tests.rs` 14235 is a test file): new code goes in new modules instead of extending them — the muzzle pose beside `hit_zones.rs`, the NPC fire origin and near-path gate beside `compute.rs`, NPC mount composition beside `data_archetype.rs`, new AI tests in their own test file. No behavior-preserving split commit is then needed; if a seam forces new code into one of them, the split lands first in its own commit.

## Delegated answers
- Fixture attack clip — `punch`, the shot's clip moment at the strike, provided `socket_dump` shows `cannon_tip`'s +Y pointing along the model's front within the aim cone at that time; otherwise the first `mixamo_brute` clip that does, recorded here. The brief's recommendation; it keeps the fixture on existing clips.
- `muzzleSocket` on a player pawn's entry — validates, mounts nothing, and is ignored for fire origin. Pawns launch from placement plus `muzzleOffset`, which client prediction reproduces.
- An entry mounting several models with more than one `muzzle` socket — warn once per level load naming the archetype and weapon, and launch from the first in socket-name order. Deterministic and loud, with no new key.
- Near-path test and the eye verdict's loss grace — no grace: a raw test on the fire tick. A grace would launch a shot into cover.

## AC-to-proof

Proof names are the tests the task adds; "both runtimes" means a JS and a Luau case.

| AC | Proof | Status |
|---|---|---|
| R1 `weaponMounts` rejects as unknown `mesh` key, entry `weapon` rejects, `mounts`/`item` load | `scripting-core` data-descriptor tests, both runtimes (`mounts_*` in `tests/weapon_mounts.rs` renamed) | achievable as stated |
| R2 pawn mounts from `mounts` as from `weaponMounts` | existing E21 suites renamed (`netcode` `weapon_mounts_*_tests`, `sim` `weapon_mounts_tests`, `postretro` `weapon_mount_content_tests`) pass unchanged in meaning | achievable as stated |
| R3 grep gate on `weaponMounts` / `WeaponMountDescriptor` | source-scanning test (`/`-joined paths) over `content/`, `sdk/`, `docs/`, `context/lib/`, `crates/` with the rejection-test allowlist | achievable as stated |
| M1 limitator and squads mount own AR_4 once on `hand_r`, host and client; no content NPC base-attaches an `ar_4` bake | `postretro` content test over loaded dev descriptors + `netcode` replication harness client case | achievable as stated |
| M2 NPC mounts attack weapons' entries over base, later wins; unused entry mounts nothing, logs nothing | `sim` composition unit tests + log capture | achievable as stated |
| M3 different models on one socket reject, naming archetype and socket; same model or different sockets load | descriptor validation tests, both runtimes | achievable as stated |
| M4 missing entry warns once per level load at install, mounts nothing, eye launch; no table → silent | `sim` install-check test with log capture (exactly-once, placed and unplaced) + AI launch test | achievable as stated |
| M5 entry socket missing from rig or model unloaded warns as a pawn's, eye launch (NM-3) | `sim` install + launch test, log capture | achievable as stated |
| M6 no wire field; client mounts match host's from own descriptors | `git diff main -- crates/net crates/netcode/src/**/wire*` empty at preflight + `netcode` replication harness test | achievable as stated |
| S1 shot from posed bake `muzzle`, moves with pose; limitator and squads below head top; sight reads eye | T1 pose test on real dev models + AI launch test; perception tests unchanged | achievable as stated |
| S2 burst and long rifles launch from the one shared bake's `muzzle` | AI launch test on the limitator descriptor | achievable as stated |
| S3 `muzzleSocket` entry loads both runtimes, mounts nothing, launches from body socket and moves with pose | descriptor tests both runtimes + pose test on `mutant_arm_cannon` | achievable as stated |
| S4 launch point a function of the fire tick across frame splits and time scales; matches bake `muzzle` at clip time | `sim` tick-harness test running one fire tick under two frame splits and dev time scales | achievable as stated |
| S5 turn + new clip on fire tick: one point for tests and launch (NM-1) | AI ordering test | achievable as stated |
| S6 pawn fire origin unchanged with/without `mounts`; obstructed pawn muzzle still eye | existing `sim/src/weapon` muzzle tests + one with-table case | achievable as stated |
| S7 hit tests identical with/without `mounts` | `sim` hit-zone test comparing hits on one NPC with and without its table | achievable as stated |
| N1 lip within radius of the line whiffs; raised line launches | near-path gate tests on built collision worlds | achievable as stated |
| N2 tip through wall or closed door whiffs; moved back or door open launches | near-path tests, world and mover cases | achievable as stated |
| N3 obstacle just inside 2 m whiffs, just past launches and strikes; near target tests whole path | near-path gate tests | achievable as stated |
| N4 column past near distance mid-burst: later shots launch and strike, none skipped, through eye-grace loss | AI burst test | achievable as stated |
| N5 no-clip-moment attack held then fires on first clear tick | AI test | achievable as stated |
| N6 blocked burst shot skipped, spends one shot, re-tests; clear bursts fire every shot | AI burst test reading weapon resources | achievable as stated |
| N7 skipped last shot restarts cooldown, adds nothing to fired count (NM-11) | AI burst timing test | achievable as stated |
| N8 low-cover test keeps contact proof, gains meshed projectile twin | `enemy_eye_height_clears_low_cover_but_full_cover_blocks_fire` kept + twin in the new AI test file | achievable as stated |
| B1 near cover holds every shot: `targetVisible` true, fired 0 | AI test | achievable as stated |
| B2 held count per whiff and per skipped shot, next-tick visible, scope-relative reset | AI + `brain_scope` tests | achievable as stated |
| B3 held span counts once; re-count rules; tick-rate independent (NM-7, NM-29) | AI tests at two AI tick rates | achievable as stated |
| B4 eye failure, missing muzzle, far obstacle, not aiming, flinch leave held unchanged (NM-5, NM-8) | AI tests | achievable as stated |
| B5 burst outlasting activity counts skipped shot in activity entered (NM-9) | AI test | achievable as stated |
| B6 dev behavior row on fired-or-held leaves after held and after fired | AI test on a dev-shaped graph | achievable as stated |
| B7 `targetVisible` doc and `attacksHeldInActivity` typed and documented both runtimes | typedef fixture (`expected.d.ts`, `.d.luau`) + drift guard | achievable as stated |
| X1 bake without `muzzle` or missing body socket launches from eye; warns once after sweep, loaded models only (NM-4) | `postretro` install test with log capture + AI launch test | achievable as stated |
| X2 unresolved clip or no-animation activity launches from eye, no warning | AI test with log capture | achievable as stated |
| X3 low cover that whiffs a muzzle shot: same NPC without bake `muzzle` fires from eye | AI test (twin of N8) | achievable as stated |
| A1 reaction, no flinch route, attack ready: no launch while drawn; own draw fires from muzzle (NM-2, NM-5, NM-16); flinch route as today (NM-8) | AI tests | achievable as stated |
| A2 dropped enemy before redraw: nothing launches, held unchanged, even on default state (NM-25) | AI test | achievable as stated |
| A3 dropped enemy given reaction before redraw (NM-31) | AI test | achievable as stated |
| A4 earlier NPC's contact-hit reaction after fire decision: not aiming, skip and spend (NM-6) | AI multi-NPC apply test | achievable as stated |
| A5 same, on a tick own draw lands: aiming; flinch-route stride change: not aiming (NM-28) | AI multi-NPC apply tests | achievable as stated |
| A6 burst decision before an earlier reaction does not start; fires whole burst later (NM-13) | AI multi-NPC apply test | achievable as stated |
| A7 reaction drawn on a tick own draw lands: aiming, clip moment on entry fires (NM-16) | AI test | achievable as stated |
| A8 entry onto shared state under reaction poses at the switch's start time (NM-26) | AI pose-time test | achievable as stated |
| A9 airborne clip: not aiming, skip and spend; fires after landing (NM-17) | AI knockback test | achievable as stated |
| A10 airborne before fall delay or no `jump_loop`: aiming on tracked state (NM-30) | AI knockback test | achievable as stated |
| A11 own switch fails unknown state: no pose, eye, no extra warning | AI test with log capture | achievable as stated |
| A12 own switch fails under reaction: not aiming until a later own draw (NM-32) | AI test | achievable as stated |
| A13 interim record clears only on own draw; survives save/restore; no reaction → bursts unchanged | AI test + `ClipClock` serde round trip (`PartialEq` includes the field) | achievable as stated |
| A14 strafing stride/variant change on due shot fires from newly tracked state (NM-10) | AI test | achievable as stated |
| L1 entry with both or neither of `attachments`/`muzzleSocket` rejects both runtimes; one loads | descriptor tests both runtimes | achievable as stated |
| L2 each AR_4 bake carries rigid `muzzle`; `solve-weapon-mount --check` still VERIFIED | `postretro` content test reading each bake's sockets + executor runs `cargo run -p postretro-tool -- solve-weapon-mount … --check` per rig, output recorded here | achievable as stated |
| U1 Scripting surface example runs as `content/dev` fixture both runtimes, same descriptor | `scripting-core`/`postretro` fixture test loading the TS and Luau files and comparing descriptors | achievable as stated |
| G1 limitator and troopers' shots leave the barrel through every clip | owner, in-engine | manual |
| G2 trooper behind chest-high cover never fires into lip or from above head | owner, in-engine | manual |
| G3 co-op 150 ms RTT: client sees each rifle seated in hand | owner, two-client run | manual |
| G4 same session: client shots appear to leave drawn barrel; record largest offset | owner, two-client run | manual |

## Tasks

Workers run sequentially on the feature branch (disk-bound machine: one cargo job at a time). Each worker gets the full brief, `research.md`, this plan, the routed context docs and the full Acceptance list, owns only its named files, and neither edits this plan nor commits. The integrating executor reviews, runs focused tests, normalizes line endings, and commits each task with its row here.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Riskiest slice: a host query posing a body model's tracked state at a given clip time (drawn clip sampled directly, no crossfade, no presentation modifiers) and composing holder joint × mounted bake × bake `muzzle` into a world point, in a new module beside `hit_zones.rs`; a rigid `muzzle` node on `models/ar_4/limitator.gltf` at the barrel tip; tests on the real limitator and its bake: point below head top at the barrel, moves with clip time, equals the bake socket at the posed hand. Confirms the host holds the bake's socket data after the model sweep. A false premise here blocks the plan. | integrating executor | — | done: `posed_muzzle_world` (`crates/sim/src/scripting_systems/muzzle_pose.rs`); bake muzzle placed on the principal-axis end matching the trooper/warrior `extras.mount` barrel; `cargo test -p postretro-sim --lib muzzle_pose` 6 passed. Premise holds: the store keeps every bake's sockets after the sweep, and clips resolve by name when `clip_index` is unset. |
| 2 | Rename, behavior-preserving: `mesh.weaponMounts` → `mesh.mounts`, entry `weapon` → `item`, `WeaponMountDescriptor` → `MountDescriptor`, every consumer in `research.md` §Rename consumers (Rust, both runtime readers, SDK lib and types, typedef templates and fixtures, content, human docs, `context/lib`); the grep gate test (R3). | worker (Sonnet), reviewed by executor | 1 | done: `cargo check --workspace --all-targets` clean; `mount` filters: entities 5, scripting-core 17, sim 18, netcode `weapon_mount` 26, postretro --bins 9; sim `typedef` 59, `surface` 38 (drift guard). R1 `retired_mount_table_keys_reject_while_the_renamed_table_loads_in_both_runtimes`; R3 `retired_mount_names_appear_only_in_the_rejection_proof` (`crates/postretro/src/level_load/mount_rename_gate_tests.rs`). |
| 3 | Entry shape: `attachments` optional, `muzzleSocket` added; both/neither rejects (L1); pawn `muzzleSocket` ignored (delegated); SDK types, lowering and drift guard in both runtimes. | worker, reviewed by executor | 2 | |
| 4 | NPC mounts: one composition (base `mesh.attachments`, then each attack weapon's entry, later wins per socket) used by host attach and client materialize; different-model same-socket rejection in both runtimes; NPC missing-entry warning; missing-muzzle and multi-muzzle warnings after the model sweep (NM-4); limitator and rifle squads move to `mesh.mounts`; `muzzle` nodes on the trooper and warrior bakes and `--check` runs (L2). | integrating executor, content slice delegated | 3 | |
| 5 | Shot origin and near-path gate: the fire proposal and `advance_actor` launch from the posed muzzle; body→muzzle and muzzle→target legs at the projectile radius out to 2 m, world and movers; each burst shot re-tests; a skipped shot spends and restarts cooldown; the missing-muzzle eye path; `attacksHeldInActivity` on `Brain`, `ScopeRelativeValues`, graph guards, SDK types and docs in both runtimes, `targetVisible` comment. | integrating executor | 4 | |
| 6 | Fire only while aiming: the interim record on `ClipClock` (serde, `PartialEq`), set by a foreign switch seen at `begin_tick` or by apply before the launch, cleared by the brain's next own draw; the hoisted own-draw predicate; airborne clip and pending redraw; failed own switch as no pose; not-aiming skip-and-spend and NM-13's not-start; pin tests A1–A14. | integrating executor | 5 | |
| 7 | Built-in fixture: arm-cannon weapon and attack, `cannon_brute` on `models/mutant_arm_cannon/` with `{ item, muzzleSocket: "cannon_tip" }`; the Scripting surface as a `content/dev` fixture in TS and Luau lowering to one descriptor (U1); fixture clip per Delegated answers. | worker for content, executor for tests | 5 | |
| 8 | Human docs and SDK comments: `docs/weapon-mounts.md`, `docs/scripting-reference.md`, `docs/character-animation.md` per Decisions §Docs amended. `context/lib` already records the contract as decided; flip it to built at landing. | worker, reviewed by executor | 6, 7 | |
