# Glossy world reflections, Task 12b: parity A/B (G1, rows 1, 2, 47, 51)

Every comparison here is HEAD against the build before the branch, each binary baking its own maps fresh (`plan.md`, AC-to-proof: "A/B means against binaries or bakes from `8dda8a34a`, baked fresh").

- **Before:** release build of `8dda8a34a` (`postretro` with `capture,dev-tools`, and `prl-build`), the pre-branch binaries every earlier check used. Its compiled-in checkout path is gone, so a `scripts-build` sits beside it (copied from this checkout's release build; `crates/script-compiler` is unchanged since `8dda8a34a`, and it reads only `light_membership` and `environment_volumes` from level-format, both unchanged).
- **After:** release build of `ca71993bf` (`capture,dev-tools`). HEAD (`f5b479b44`) differs from it in production source by nothing: `git diff --stat ca71993bf HEAD -- crates/ ':!*test*'` lists only `crates/renderer/src/render/mod.rs`, whose two added lines register a `#[cfg(test)]` module.
- **Bakes:** `prl-build <map> --no-cache --no-tui -j 6 -v --baked-root <own root>/baked`, each binary into its own copy of `content/dev` (maps copied, every other entry symlinked), so each engine reads only its own compiler's `.prl` and `.prm`. HEAD baked water maps and the glossy test map twice, without and with `--glossy-reflections`.

## G1: without the flag, id 54 as before and no world binding

Compare script (scratch `t12/g1.py`): every section other than 54 byte-identical by SHA-256 of its payload, section version and size equal; id 54 present in both bakes or neither; where present, the pre-branch version-2 section and HEAD's version-3 section equal in header (tile size, border, atlas and distance formats, K), every probe record (source, box, capture point, layer), every light's roster index and reference, the five water lobes' shares (−Y zero in HEAD's), fluid bindings, mip-0 colour tiles and distance tiles; HEAD's section binds no world face and holds one colour mip.

| HEAD bake | maps compared | id 54 in both | problems |
|---|---|---|---|
| without `--glossy-reflections` | 69 of 80 `content/dev` maps | 11 (the water maps) | none |
| with `--glossy-reflections`, water maps | 11 | 11 | none |

- **Without the flag, 69 maps:** every PRL is identical to the pre-branch bake in every section but id 54, and id 54 matches as above on all 11 water maps. Every other map, the glossy test map and `specular-texel-sharpness` (glossy materials) included, has no id 54 in either bake, so it binds no world face.
- **With the flag, the 11 water maps** (campaign-test, e24, reflection-lake, reflection-probes, vault, the five mover-closet maps, stress-env-volumes): the same, identical to the flagless bake. Their probes: e24 13, reflection-probes 3, stress-env-volumes 256 (382 fluid bindings, 68 stored light shares), the rest 1 each.
- **The glossy test map with the flag** is the one map that differs, in id 54 alone (10 probes, 50 world bindings), as intended; every other section matches the pre-branch bake.
- **Not baked: the 11 other `stress-*` maps** (`stress-env-volumes-baseline`, `stress-street`, `stress-warren` and its eight variants). One stress bake runs 77–125 minutes alone on this machine (earlier tasks' `stress-env-volumes` logs; 2 h 25 min each with three in parallel here), so 22 more were out of this session's reach. None holds a `fluid_volume` or `reflection_volume`, and without the flag the probe stage's entry gate (`reflection_probes/mod.rs`, `bake_or_load_with`) returns no section when there are no reflection volumes, no up-facing fluid faces and no glossy world faces, the last empty whenever the flag is off (`glossy_world_faces`); the pre-branch gate is the first two terms. So neither compiler emits id 54 for them, by source, not by bake.
- **Runtime half** (no world-face probe resource, no overlay): HEAD's install log on e24 reads `Reflection probes installed: 13 probes, BC6H 128² tiles × 1 mip(s) … 18 fluid and 0 world bound faces`; the GPU gates are Tasks 4, 5 and 10's (`plan.md`).

## Row 1: water placement, tiles, shares and exclusion

Proof: the ignored compiler test `every_water_probe_places_and_captures_as_the_build_before_glossy_reflections` in `crates/level-compiler/tests/reflection_probe_water_parity_ab.rs` (added in Task 12b). It reads `POSTRETRO_WATER_PARITY_BEFORE_DIR` (pre-branch bakes) and `POSTRETRO_WATER_PARITY_AFTER_DIR` (HEAD's `--glossy-reflections` bakes), one `<map>.prl` and its `-v` log per map, over every `content/dev` map whose source holds a `fluid_volume` (11), failing on any missing input. It decodes the pre-branch version-2 section by hand and HEAD's through `ReflectionProbesSection::from_bytes`, matches each HEAD probe binding no world face to the pre-branch probe binding the same fluid faces (bindings up to renumbering), and asserts equal source entity, box and capture point (bit-exact), light rosters, references and +Y, ±X, ±Z shares (bit-exact) with −Y zero, mip-0 colour tile, distance tile and the log's excluded-brush count per probe (the exclusion classes; the excluded set itself enters the capture key and shapes the tiles compared), and that every pre-branch probe is matched once.

```
POSTRETRO_WATER_PARITY_BEFORE_DIR=<pre-branch bakes> POSTRETRO_WATER_PARITY_AFTER_DIR=<HEAD flag bakes> \
  cargo test -p postretro-level-compiler --test reflection_probe_water_parity_ab -- --ignored
```

Result: passed. 280 probes over the 11 maps matched (stress-env-volumes 256, e24 13, reflection-probes 3, eight maps 1 each); none binds a world face. Negative controls, run once and discarded: one map's excluded-brush count edited fails on `excluded brushes`; another map's PRL substituted fails on the fluid-face match.

## Row 2: water renders as before at every tier and style

Plain PNG capture (`--capture <scene>`, no measurement block), so every frame renders the authored instant at time zero and the water clock is identical across binaries. 1920×1080, `POSTRETRO_SH_STREAMING=sync-proof`, default light-term mask. Each binary renders its own bake (before: its own; after: HEAD's `--glossy-reflections` bake, byte-identical to its flagless bake on these maps, G1).

Views: the reflection-probes landing parity views of `metal/parity.py` (`near`, `test`, `mid`, `under`; `run.py` poses, FOV 100) and Task 10's six probes and lake poses (`results-glossy-task10-water-identity.md`, FOV 90). Tiers off, low, medium, high, very_high, ultra; reflection styles pixel and smooth.

| view | map | off | low | medium | high | very_high | ultra |
|---|---|---|---|---|---|---|---|
| parity-near | e24 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| parity-test | probes | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| parity-mid | e24 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| parity-under | e24 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| probes-down | probes | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| probes-across | probes | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| probes-canal | probes | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| lake-spawn | lake | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| lake-centre | lake | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| lake-lamp | lake | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |

Max per-channel difference in 8-bit sRGB over the whole frame, pixel / smooth. All 120 before/after pairs are byte-identical PNGs: 0 pixels differ, so water is within 0/255, inside row 2's 1/255. Water's mip-0-only read stays pinned by the source scan `water_samples_colour_mip_zero_only` (Task 7).

The probe term, screen-space layer, planar layer and smooth search are live in these frames (HEAD, pixels of 2 073 600 differing, max channel difference): lake-spawn low vs off 736 593 (103), high vs low 128 842 (126), very_high vs high 269 667 (133), smooth vs pixel at high 45 992 (125); parity-mid 997 339 (91), 167 337 (68), 764 433 (72), 211 977 (65); probes-across 473 622 (73), 53 187 (73), 449 295 (192), 247 026 (63).

## Row 47: at reflection quality off the glossy map draws as before

`content/dev/maps/glossy-reflections-test.map`: the before binary renders its own bake (no glossy probes; the pre-branch compiler has no flag), HEAD renders its `--glossy-reflections` bake (10 probes, 50 world-bound faces, 6 colour mips installed). Both at reflection quality off, reflection styles pixel and smooth, at 1920×1080 and 3840×2160, four views: `glossy-across` and `glossy-down` (the kit's measurement poses, FOV 100), `glossy-level` (the spawn, level, toward the monument, FOV 90) and `glossy-back` (engine (−11.4, 1.6, −11.4), yaw 225, pitch −10, back toward the spawn, FOV 90).

All 16 before/after pairs are byte-identical PNGs. Not vacuous: HEAD's own `glossy-across` 1080p frame at high differs from its off frame in 992 570 pixels (max 70), and at low in 993 126 (max 70).

## Row 51: forward's colour as before on maps without glossy faces

The row 2 captures at high and off on e24 (parity-near, -mid, -under), the probes map (parity-test, probes-down, -across, -canal) and the lake map (lake-spawn, -centre, -lamp), both reflection styles: 40 before/after pairs, every one byte-identical over the whole frame, so forward's colour output is byte-identical at every pixel there. On the glossy map at off, row 47's 16 pairs. The A/B is whole-frame; it holds forward's output because nothing drawn after forward differs either.

## Artefacts

Scratch, not kept: scenes, PNGs and logs under the session scratchpad's `t12/caps/`; bakes, section digests and compare scripts under `t12/`.
