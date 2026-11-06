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

- Parse a shared library once per load? — pending measurement (task 11).
- Parent mismatch and unbound joints — pending; intended policy: bind anyway, one warning per model naming the joints. A non-leaf mesh joint with no library joint warns; a leaf stays silent.

## Contracts (integrating executor)

- `ModelSources` carries `library: Option<ClipLibrarySources>` (document, buffers, document digest), read after the skin check. `model_content_key` hashes a present/absent marker, then digest and length-prefixed buffers. Key version 2.
- `gltf_resolve` gains one relative-URI resolver shared by materials, the loader and `dist`.
- `build_skeleton` also returns joint names in topo order; `Joint` is unchanged.
- Binding works on parsed library clips, before root-motion derivation. Each clip derives once.
- Post-export pass lives in `tools/mixamo_to_gltf.py`, which ships in bundles. No new shipped script.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| B1 different joint order and count | `clip_library` fixture test | achievable as stated |
| B2 rest-rotation correction | fixture test | achievable as stated |
| B3 missing joints either side | fixture test | achievable as stated |
| B4 bone lengths, root scale | fixture test | achievable as stated |
| B5 root translation ratio and facing | fixture test | achievable as stated |
| B6 travel speed scaled, neutralized | fixture test | achievable as stated |
| B7 bounds and reach (P4) | fixture test | achievable as stated |
| B8 skinless library under non-joint parent | fixture test | achievable as stated |
| B9 key times, interpolation, duration (P5) | fixture test | achievable as stated |
| C1 embedded first, override silent | fixture test + log capture | achievable as stated |
| C2 embedded clips unchanged (P6) | fixture test | achievable as stated |
| C3 clip 0 is library's first | fixture test | achievable as stated |
| C4 no `clipLibrary` regression | existing loader tests + fixture | achievable as stated |
| R1 load failures; forest with declared root binds | fixture tests | achievable as stated |
| R2 empty library | fixture test | achievable as stated |
| R3 malformed reference warns, `tags` survive, `..` accepted | fixture test + log capture | achievable as stated |
| R4 static model (P3) | fixture + key test | achievable as stated |
| K1 one byte changes key; move keeps it (P1) | `model_cache_tests` | achievable as stated |
| K2 edit after read (P2) | fixture test | achievable as stated |
| K3 hit equals fresh load, by track | GPU cache test under `POSTRETRO_REQUIRE_GPU` | achievable as stated |
| K4 library edit between levels (P8) | GPU cache test | achievable as stated |
| K5 `dist` payload | `dist` test | achievable as stated |
| K6 `dist` refusals | `dist` test | achievable as stated |
| K7 post-export pass | `tools/tests` | achievable as stated |
| K8 numbered prefix (P7) | `tools/tests` | achievable as stated |
| K9 unittest discovery count | `python -m unittest discover -s tools/tests` | achievable as stated |
| L1 grep gates | grep gate test or preflight grep | achievable as stated |
| M1 mesh-only, clip names resolve | dev-model test | achievable as stated |
| M2 size limits | size check in test | achievable as stated |
| M3 parity report (P9) | one-time report below | achievable as stated |
| M4 mesh report | one-time report below | achievable as stated |
| M5 keys at zero, recipe recorded | check + recipe below | achievable as stated |
| M6 floor contact | one-time check via engine load | achievable as stated |
| M7 AR_4 `--check` metrics | `solve-weapon-mount --check` before and after | achievable as stated |
| N1 new model files | dev-model test | achievable as stated |
| N2 scenery idle, map unchanged | descriptor check + `git diff` | achievable as stated |
| N3 new enemy classnames | descriptor check + enemy tag guard | achievable as stated |
| N4 more enemies and factions | map count check | achievable as stated |
| N5 rifle `--check` per enemy (P10) | `solve-weapon-mount --check` | achievable as stated |
| N6 docs page | `docs/` page + bundle-path check | achievable as stated |
| Man1 player and `limitator` | owner, in-engine | manual |
| Man2 new enemies on `movement-feel` | owner, in-engine | manual |
| Man3 faction separation | owner, in-engine | manual |
| Man4 `campaign-test` props | owner, in-engine | manual |
| Man5 real Blender `--library` | owner; executor runs it too | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Split clip parsing into its own module, behavior-preserving | engine worker | — | |
| 2 | Library read, bind, failure policy, key, fixtures | engine worker | 1 | |
| 3 | `--library`, `--clip-library`, post-export pass, tests | tool worker | — | |
| 4 | First slice: real library, bind staged trooper, look at it | integrating executor | 2, 3 | |
| 5 | `dist` reference check and payload test | integrating executor | 2 | |
| 6 | Cache hit and P8 GPU tests | integrating executor | 2 | |
| 7 | Migrate `exo_red` and `limitator`; one-time reports | integrating executor | 4 | |
| 8 | New characters, descriptors, classnames, rifle mounts | integrating executor | 7 | |
| 9 | `movement-feel` and `stress-warren-mini` factions, READMEs | integrating executor | 8 | |
| 10 | Docs page and context | integrating executor | 8 | |
| 11 | Parse-once measurement and delegated answers | integrating executor | 8 | |
