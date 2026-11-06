# character-clip-library — plan of record

mode: compact
status: active
read at: deb9ec20f
pre-migration commit: deb9ec20f

## Corrections

- Brief read at a82073138. `git diff a82073138 deb9ec20f` is empty for `crates/model`, `crates/tool`, `crates/level-format`, `crates/postretro/src/level_load` and `tools/mixamo_to_gltf.py`, so the grounded Decision reads stand.
- Brief Path says `gltf_loader.rs` is "past 800 lines". It is 2,200 source lines plus tests. Same action: split clip parsing out first.
- Rig facts re-read from the committed glTFs: joint counts, Hips rest heights, `Armature` parent, and staged rigs' joints all being in `exo_red` match `research.md`.

## Delegated answers

- Parse a shared library once per load? — no, keep one read per bound model. Measured in release on four bound models: 29-30 ms read each (`exo_red` 52 ms), about 140 ms in total, on the load pool, against 12.6 ms for the library's own read and JSON parse. A shared parse saves about 85 ms of pool CPU per load here, and P2 stays trivially true without it. The cost grows with bound characters, so a roster of 20 would be about 600 ms of pool time; revisit then.
- Parent mismatch and unbound joints — bind anyway, with one warning per model naming the joints. A library parent with no same-named mesh joint reads as no parent, so an armature above the root stays silent. A non-leaf mesh joint with no library joint warns; leaves and library-only joints stay silent.

## Contracts (integrating executor)

- `ModelSources` carries `library: Option<ClipLibrarySources>` (document, buffers, document digest), read after the skin check. `model_content_key` hashes a present/absent marker, then digest and length-prefixed buffers. Key version 2.
- `gltf_resolve` gains one relative-URI resolver shared by materials, the loader and `dist`.
- `build_skeleton` also returns joint names in topo order; `Joint` is unchanged.
- Binding works on parsed library clips, before root-motion derivation. Each clip derives once.
- Post-export pass lives in `tools/mixamo_to_gltf.py`, which ships in bundles. No new shipped script.
- `--strip-face` is added to the migrated library's `--library` run. It is an existing flag; the brief's command omits it, but the 47 face bones reach no mesh. Same clips, 1.1 MB smaller.
- Each new enemy carries its own AR_4 bake (`ar_4_trooper`, `ar_4_warrior`), which the brief allows: the shared AR_4 fails `--check` on both rigs.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| B1 different joint order and count | `clip_library` fixture test | achievable as stated | pass |
| B2 rest-rotation correction | fixture test | achievable as stated | pass |
| B3 missing joints either side | fixture test | achievable as stated | pass |
| B4 bone lengths, root scale | fixture test | achievable as stated | pass |
| B5 root translation ratio and facing | fixture test | achievable as stated | pass |
| B6 travel speed scaled, neutralized | fixture test | achievable as stated | pass |
| B7 bounds and reach (P4) | fixture test | achievable as stated | pass |
| B8 skinless library under non-joint parent | fixture test | achievable as stated | pass |
| B9 key times, interpolation, duration (P5) | fixture test | achievable as stated | pass |
| C1 embedded first, override silent | fixture test + log capture | achievable as stated | pass |
| C2 embedded clips unchanged (P6) | fixture test | achievable as stated | pass |
| C3 clip 0 is library's first | fixture test | achievable as stated | pass |
| C4 no `clipLibrary` regression | existing loader tests + fixture | achievable as stated | pass |
| R1 load failures; forest with declared root binds | fixture tests | achievable as stated | pass |
| R2 empty library | fixture test | achievable as stated | pass |
| R3 malformed reference warns, `tags` survive, `..` accepted | fixture test + log capture | achievable as stated | pass |
| R4 static model (P3) | fixture + key test | achievable as stated | pass |
| K1 one byte changes key; move keeps it (P1) | `model_cache_tests` | achievable as stated | pass |
| K2 edit after read (P2) | fixture test | achievable as stated | pass |
| K3 hit equals fresh load, by track | GPU cache test under `POSTRETRO_REQUIRE_GPU` | achievable as stated | pass; fixture clip is a LINEAR translation, so Step and rotation tracks are not exercised on this path |
| K4 library edit between levels (P8) | GPU cache test | achievable as stated | pass |
| K5 `dist` payload | `dist` test | achievable as stated | pass; drives stage 4 and `copy_filtered_tree`, as full `dist` needs release binaries |
| K6 `dist` refusals | `dist` test | achievable as stated | pass |
| K7 post-export pass | `tools/tests` | achievable as stated | pass |
| K8 numbered prefix (P7) | `tools/tests` | achievable as stated | pass |
| K9 unittest discovery count | `python -m unittest discover -s tools/tests` | achievable as stated | pass |
| L1 grep gates | grep gate test or preflight grep | achievable as stated | pass |
| M1 mesh-only, clip names resolve | dev-model test | achievable as stated | pass |
| M2 size limits | size check in test | achievable as stated | pass |
| M3 parity report (P9) | one-time report below | achievable as stated | pass; 0.105° on both, see One-time reports |
| M4 mesh report | one-time report below | achievable as stated | pass; all equal |
| M5 keys at zero, recipe recorded | check + recipe below | achievable as stated | pass |
| M6 floor contact | one-time check via engine load | achievable as stated | pass; −0.64 cm and −0.77 cm |
| M7 AR_4 `--check` metrics | `solve-weapon-mount --check` before and after | achievable as stated | pass; metrics identical |
| N1 new model files | dev-model test | achievable as stated | pass |
| N2 scenery idle, map unchanged | descriptor check + `git diff` | achievable as stated | pass |
| N3 new enemy classnames | descriptor check + enemy tag guard | achievable as stated | pass |
| N4 more enemies and factions | map count check | achievable as stated | pass |
| N5 rifle `--check` per enemy (P10) | `solve-weapon-mount --check` | achievable as stated | pass; both VERIFIED with their own bakes |
| N6 docs page | `docs/` page + bundle-path check | achievable as stated | pass |
| Man1 player and `limitator` | owner, in-engine | manual | outstanding; owner, in-engine |
| Man2 new enemies on `movement-feel` | owner, in-engine | manual | outstanding; owner, in-engine |
| Man3 faction separation | owner, in-engine | manual | outstanding; owner, in-engine |
| Man4 `campaign-test` props | owner, in-engine | manual | outstanding; owner, in-engine |
| Man5 real Blender `--library` | owner; executor runs it too | manual | outstanding; executor ran `--library` in real Blender 5.2, owner's in-game check and rerun remain |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Split clip parsing into its own module, behavior-preserving | engine worker | — | `d7372ae74` |
| 2 | Library read, bind, failure policy, key, fixtures | engine worker | 1 | `be5634631`, `caca2cb76`, `dddd797a6` |
| 3 | `--library`, `--clip-library`, post-export pass, tests | tool worker | — | `159f1adf0` |
| 4 | First slice: real library, bind staged trooper, look at it | integrating executor | 2, 3 | passed; poses looked right, see First slice |
| 5 | `dist` reference check and payload test | integrating executor | 2 | `50fb690b9` |
| 6 | Cache hit and P8 GPU tests | integrating executor | 2 | `e98c63e1a` |
| 7 | Migrate `exo_red` and `limitator`; one-time reports | integrating executor | 4 | `8411dfa55` |
| 8 | New characters, descriptors, classnames, rifle mounts | integrating executor | 7 | `8411dfa55` |
| 9 | `movement-feel` and `stress-warren-mini` factions, READMEs | integrating executor | 8 | `8411dfa55` |
| 10 | Docs page and context | integrating executor | 8 | docs in `8411dfa55`; `context/lib` at landing |
| 11 | Parse-once measurement and delegated answers | integrating executor | 8 | done, see Delegated answers |

## First slice

Real library bound onto the staged `rodin_sci-fi_trooper_m` through `load_model`, CPU-skinned and rendered front and side. `idle`, `idle_aiming`, `run_forward` and `death_from_front_headshot` all read correctly: feet on the floor, arms toward +Z, corpse on the floor. 25 joints, 52 clips.

## Library recipe

Run from the workspace root, Blender 5.2. `library-src` stages the sources listed below: the Pro Rifle Pack (`fbx-bin`) and three Basic Shooter Pack clips.

```bash
blender --background --python tools/mixamo_to_gltf.py -- --input-dir library-src --base exo_red.fbx --library --strip-face --output content/dev/models/clip_libraries/mixamo_rifle/library.gltf
```

The committed file came from a scratch output path, then copied in, built by the script at `159f1adf0`. A later review fix resets pose bones between clips, so a rebuild with today's script differs on `mixamorig:Throat` alone: no clip keys it, and it moves from a stale 0.91° to rest. That joint is a leaf with no vertices weighted to it, so nothing visible changes, and keeping the committed bake keeps the parity report below exact. Mesh-only models were produced by `bind_gltf_to_clip_library` on the committed bakes, so no mount needed re-solving for `limitator`.

Source folder:

- crouching turn 90 left.fbx
- crouching turn 90 right.fbx
- death crouching headshot front.fbx
- death from back headshot.fbx
- death from front headshot.fbx
- death from right.fbx
- death from the back.fbx
- death from the front.fbx
- exo_red.fbx
- firing rifle.fbx
- hit reaction.fbx
- idle aiming.fbx
- idle crouching aiming.fbx
- idle crouching.fbx
- idle.fbx
- jump down.fbx
- jump loop.fbx
- jump up.fbx
- reloading.fbx
- run backward left.fbx
- run backward right.fbx
- run backward.fbx
- run forward left.fbx
- run forward right.fbx
- run forward.fbx
- run left.fbx
- run right.fbx
- sprint backward left.fbx
- sprint backward right.fbx
- sprint backward.fbx
- sprint forward left.fbx
- sprint forward right.fbx
- sprint forward.fbx
- sprint left.fbx
- sprint right.fbx
- turn 90 left.fbx
- turn 90 right.fbx
- walk backward left.fbx
- walk backward right.fbx
- walk backward.fbx
- walk crouching backward left.fbx
- walk crouching backward right.fbx
- walk crouching backward.fbx
- walk crouching forward left.fbx
- walk crouching forward right.fbx
- walk crouching forward.fbx
- walk crouching left.fbx
- walk crouching right.fbx
- walk forward left.fbx
- walk forward right.fbx
- walk forward.fbx
- walk left.fbx
- walk right.fbx

## One-time reports

Baseline: bakes at `deb9ec20f`, loaded through the engine loader. Rotations sampled at 60 Hz on every joint of every shared clip.

| Check | `limitator` | `exo_red` |
|---|---|---|
| Clips compared | 52 of 52 library clips | 49 shared; `firing_rifle`, `hit_reaction`, `reloading` have no baseline |
| Samples | 289,848 | 229,218 |
| Baseline offset | same time | one frame later |
| Worst rotation (limit 0.25°) | 0.105° | 0.105° |
| Worst root translation | not measured: `k` changes it on purpose | 0.002 mm (limit 1 mm) |
| Mesh, submeshes | equal | equal |
| Skeleton, inverse binds | equal | equal |
| Sockets, hit zones, pose masks, legs, tags | equal | equal |
| `.gltf` + `.bin` | 230,497 B (limit 0.3 MB) | 1,352,044 B (limit 1.5 MB) |

`limitator` lowest joint at clip end: `death_from_front_headshot` −0.64 cm, `idle_crouching` −0.77 cm (limit 2 cm).

`solve-weapon-mount --check`, AR_4 on `limitator`, before and after migration: `barrel·+Z` 1.000000, `barrel·+Y` 0.000721, `up·+Y` 0.999999. Status UNVERIFIED, identical.

New enemies, `solve-weapon-mount --check`, status VERIFIED, `barrel·+Z` 1.000000, `barrel·+Y` 0.000000, `up·+Y` 1.000000:

- `ar_4_trooper` on `rodin_sci-fi_trooper_m`: `--grip 0.075 -0.05 0 --scale 0.34 --rotate-euler 9.368709 -69.441841 162.890732`, axes barrel `1 0 0`, up `0 1 0`.
- `ar_4_warrior` on `cyberpunk_warrior_f`: same grip and scale, `--rotate-euler 24.637445 -61.257420 163.871979`.

Shared AR_4 on those rigs fails `--check` (`barrel·+Y` −0.132 and −0.038), hence the separate bakes.

Dev launch of `movement-feel`: all 16 models install, none missed by prediction; bound models report 52 clips; both squads spawn. No model errors.

## Review

Panel: two correctness tracers, a contract verifier and two adversarial passes (opus), plus hygiene and drift (sonnet). Pins P1, P2, P4, P5 and P6 traced clean.

Fixed:

- Sampler accessors `gltf` cannot read safely now warn and skip the channel: zero count, short stride, overrun, sparse with no buffer view. This was the panel's one must-fix. Sparse accessors with a buffer view still read, so a model with no library loads as before.
- Bind: one warning for duplicate library node names; a near-zero rotation key drops its track; root heights must be finite with a ratio above zero. The last refines the Failure policy's "zero or below" without changing its meaning.
- `clipLibrary` values with a backslash or non-UTF-8 escape read as malformed. The material path is unchanged.
- Tool: `--library` and `--clip-library` runs keep neighbouring files; a bind tags a renamed or untagged rig; pose bones reset between clips; `OSError` exits 1; unsupported extensions and misaligned views handled; non-UTF-8 references refused.
- Docs: mixed-character route, root failure wording, `dist` scope under `models/`, a library's own folder.

Left, with reasons:

- A library whose clips bind no usable tracks for a mesh loads and stays in rest. The Failure policy does not list this; owner's call.
- A degenerate root rest rotation warns and holds rest rather than failing. Same reason.
- Unnamed or duplicate library clip names stay silent. No invariant breaks.
- A failed post-export pass leaves its artifact: exit 1 signals it.
- `stale_outputs` guesses `<stem>.bin`, can drop an image two documents in one folder share, and `has_engine_tags` is any-tag. Each needs a layout the repo does not have.
- Mesh and skin accessor reads share the sampler panics. Pre-existing and outside this brief; proposed as its own task.

## Verification

`cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo check --release` and `crate-graph --check` pass. `cargo test --workspace --no-fail-fast` under `POSTRETRO_REQUIRE_GPU=1`: 14,375 passed, 0 failed, with three ghost tests skipped. After the last sparse fix, the model crate (203 passed), `model_` filter (39) and tool (161) reran clean.

Pre-existing failures, unrelated to this branch:

- `ghost_draws_record_through_blend_and_dither_pipelines`, `a_nearer_ghost_blends_over_a_farther_one_and_each_hides_its_own_inner_surface` and `mover_and_billboard_ghosts_record_through_their_pipelines_in_both_modes` lose the GPU device on the 1660 Super, and in a full run crash the renderer test binary. Same three at `deb9ec20f`; this branch changes nothing in the render crates.
- `test_gen_stress_map` `test_gravity_is_straight_down_or_zero` and `test_volume_kvps_are_present_and_well_formed` also failed before this branch's tool work. `tools/tests` otherwise passes: 130 run.

`movement-feel` and `stress-warren-mini` both recompile. `movement-feel` launches in the dev engine. Portal and cell baselines in `stress-warren.README.md` stand because no geometry changed; they were not re-measured.
