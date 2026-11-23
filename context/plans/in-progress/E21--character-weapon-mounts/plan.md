# E21--character-weapon-mounts — plan of record

mode: compact
status: active
read at: 6fcfc4b18

## Corrections
- Bakes: "exo_red gets a bake … for every dev weapon" → the six exo_red bakes (pistol, rifle, shotgun, smg, rpg, sci_fi_weapon) landed at 83802575a with `extras.mount` and an existing folder-scan VERIFIED test (`crates/tool/src/mount/mod.rs`); planning around it by adding only the table, the table-driven test, and no new Blender work.
- Check: "the pure-Rust check in `crates/tool/src/mount/`" → `verify_mount` and `resolve_socket_frame_in_model` live in `postretro-model::mount`, but `compose_declared_axes_into_baked_frame` and the pass/fail thresholds live in the binary-only `postretro-tool`; planning around it by moving the declared-axes verdict into `postretro-model::mount`, called by the tool and by a `crates/postretro` test that reads the dev player's table (`level_load/model_tests.rs` `dev_descriptors`).
- Load-time missing-entry warnings: Rust checks loadout names only at spawn (`wieldable_inventory.rs`) → run the loadout-entry and unknown-weapon checks at level install, right after `hit_zone_store.clear()` (`lifecycle_world_cpu.rs`, `startup/lifecycle_net.rs`), using the store's warn-once set so the first-wield warning shares the key and both clear per level load.
- Client class (P1): `LocalWeaponAttachmentUpdate` carries no class → its consumer (`netcode/src/lib.rs` `client_receive_and_apply`) runs after arm, so `replication.armed_local_pawn()` already holds the arming snapshot's class; decode it there, defaulting to `player`. Remote pawns use the class `materialize_remote_entities` already decodes.
- Preload: `descriptor_mesh_models` feeds only movement descriptors → collect every descriptor's `weaponMounts` models in `weapon_presentation_models`, which both the install sweep and `predict_level_models` already merge.
- `models/ar_4/model.gltf` (the limitator fit) carries no `extras.mount`; only the moved trooper and warrior bakes do. AC 21's VERIFIED check applies to the moved bakes, as written.
- "No code, test or doc names `models/ar_4/model.gltf`" → applied to code, tests, `docs/` and `context/lib/`; `context/plans/done/` stays historical.

## Delegated answers
- Grip point and scale per bake — taken from the 2026-11-22 bakes (scale 0.62, per-weapon grips in `research.md` §exo_red bakes); scale and grip stay manual proof.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| R1 entry mounts at declared sockets on every path | netcode harness tests: SP, listen host own + slot pawn, client own + remote | achievable as stated |
| R2 two-socket entry, one not `hand_r` | same harness, two-socket fixture | achievable as stated |
| R3 two rigs, same weapon, own bake each | host + client harness, two player classes | achievable as stated |
| R4 switch swaps sockets, base returns, no-entry weapon, same model shared socket (P4) | writer unit tests + one path test | achievable as stated |
| R5 no active weapon shows base attachments (P3) | writer unit test at install and after drop | achievable as stated |
| R6 two switches same tick | host dirty-queue test | achievable as stated |
| R7 client own pawn non-default class on arming snapshot (P1) | client harness test | achievable as stated |
| R8 class change at level transition (P5) | host + client test | achievable as stated |
| R9 no wire record gains a field | wire-size/record drift guard + diff review | achievable as stated |
| R10 fire origin and hit tests identical with/without table | sim/netcode test comparing origins and hits | achievable as stated |
| M1 loadout missing entry warns at load; pickup warns at first wield; no table logs nothing | log-capture tests | achievable as stated |
| M2 loadout weapon warns at load, not again on wield | log-capture test | achievable as stated |
| M3 once per character+weapon per level load, per peer | log-capture test across respawn, switch, reload | achievable as stated |
| M4 no table keeps base attachments incl. `hand_r` | writer test through switches | achievable as stated |
| M5 missing rig socket warns once naming character/weapon/socket; no generic warning; other sockets mount | log-capture test | achievable as stated |
| M6 table models in sweep + prediction; failed model warns once; other sockets mount | preload tests + log-capture | achievable as stated |
| L1 rejecting load rules in both runtimes; misspelled mesh key; all `content/` loads | scripting-core JS + Luau tests; dev mod init test | achievable as stated |
| L2 unknown weapon name warns once, others mount | log-capture test | achievable as stated |
| L3 `thirdPersonModel` rejected in both runtimes; absent from `content/` | scripting-core tests + grep | achievable as stated |
| L4 load rules check incoming; rename commits; added loadout weapon mounts nothing, warns once | hot-reload staged-commit test | achievable as stated |
| L5 shared/spread `weaponMounts` lowers the same in both runtimes | scripting-core JS + Luau tests | achievable as stated |
| L6 table edit under hot reload deferred (P2), applied next level load | deferral test + switch test | achievable as stated |
| C1 every exo_red table model VERIFIED, beside viewmodel as `exo_red.gltf`/`.bin`, not a viewmodel | table-driven test in `crates/postretro` | achievable as stated |
| C2 exo_red table covers the ten weapons; dev player logs no missing-entry warning | test over dev descriptors + log capture | achievable as stated |
| C3 viewmodel assets byte-identical to 956a929b | `git diff --stat 956a929b -- content/dev/models/cyberpunk_weapons` shows only `exo_red.*` | achievable as stated |
| C4 `ar_4/` holds three rig bakes, no `model.gltf`; no `ar_4_*`; no code/test/doc names old path | tree check + grep + existing limitator/squad tests | achievable as stated |
| C5 moved bakes reference own buffer, still VERIFIED | tool test | achievable as stated |
| S1 Scripting surface example loads in both runtimes, same descriptor | scripting-core fixture test | achievable as stated |
| Manual 1 exo_red holds each weapon, scale/grip, through idle/walk/run/switch | owner, in-engine | manual |
| Manual 2 two-client co-op at 150 ms RTT | owner, in-engine | manual |
| Manual 3 body shadow shows seated weapon | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Descriptor: `MeshDescriptor.weapon_mounts`, both bridges, unknown-mesh-key and entry load rules, SDK lowering (copy) and typedefs, surface fixture | worker (descriptor) | — | done: `cargo test -p postretro-scripting-core` 1157 pass (11 `weapon_mount`); `-p postretro-sim typedef` 59 pass; dev-mod load twin test pass |
| 2 | Content assets: ar_4 reorg into rig-named bakes, consumer updates, model-crate check move, ar_4 tool tests | worker (content, no cargo while 1 builds) | — | done (model-crate check move deferred to 5): `cargo test -p postretro-tool mount` 22 pass incl. `every_moved_ar_4_rig_bake_declares_its_mount_and_verifies_on_its_rig` |
| 3 | Writer: compose base + entry by wielder class on all four paths, warnings, preload, install-time checks; resolution and missing-data tests | worker (netcode) | 1 | |
| 4 | Retire `thirdPersonModel`: field, validation, readers, deferral, typedefs, fixtures, content | worker | 3 | |
| 5 | exo_red table in `player.ts`, table-driven content test, dev-load no-warning test | integrating executor | 2, 3, 4 | |
| 6 | Docs: `entity_model.md`, `resource_management.md` §7, `networking.md`, `docs/weapon-mounts.md`, `docs/character-animation.md`, `docs/scripting-reference.md` | worker (docs) | 4 | |
| 7 | Preflight, review panel, fix loop | integrating executor | 1–6 | |
