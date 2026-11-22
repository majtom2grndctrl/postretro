# area-lights — plan of record

mode: resumable
status: approved
read at: 1bfec9f49

Approved by the owner on 2026-11-21, with row 11 restated as below.

## Corrections
No source under `crates/` changed between the brief's `c56f894e` and `1bfec9f49`. Every
cited symbol exists. No Decision premise is false. The Path is stale or incomplete here:

- Receiver-floor centre probe is in `texel_footprint.rs::footprint_visibility` (probe
  `k == 0` aims at `hard_ray_target`, `:564-574`), paired in `texel_footprint/pairing.rs`,
  not in `SoftProbes::new` → the O19 change lands there. `ProbeEarlyOut::Blocked`'s
  justification (centre probe aims at the emitter centre in open air) is restated for
  the area arm.
- A second count-keyed lattice memo exists: `FOOTPRINT_MEMO` / `TexelFootprint::build`,
  keyed by `EmitterKey {Sphere, Cone}` (`texel_footprint.rs:197-240`). `pairing_frame`
  builds its axis from `light.origin` → the area arm bypasses both memos (O9) and pairs
  by the sample's projected offset; `footprint_visibility`'s inline hard/soft match gets
  an Area arm.
- `ShaftCull::new` takes the light, not a (centre, radius) pair, and derives
  `origin, light_size + EMITTER_SLACK` itself (`shaft_cull.rs:134-140`) → one
  `emitter_sphere(light)` accessor feeds it. Further emitter-sphere readers the Path
  omits: `lightmap_reach/flood.rs::Emitter::of` (own `EMITTER_SLACK`),
  `chart_cull.rs` `behind` test against the origin, `mover_light_gate/texels.rs`
  origin-in-mover tests.
- `PosedOccluder::may_meet` is an AABB test (probe ∪ `origin ± emitter_radius` box), not a
  sphere test; `posed_scene::{may_shadow, light_may_reach}`, `direct.rs::light_movers` and
  `mover_probe_channel/cost.rs:360` also read origin balls → all take the emitter or reach
  sphere.
- Layer key `light_prefix` hashes the whole `MapLight` through postcard
  (`cache_keys.rs:293`), as do the id-35, 41/45, 27, scatter and mover memos → polygons
  live in `MapLight`, so those keys fold them automatically; only
  `surface_radiance::LightDigest::of` needs them by hand. Inert keys (`delay`,
  `_light_size`, spot keys) are normalised out of `MapLight` at translation, or the
  "re-keys nothing" rows fail.
- Billboard scatter (ids 47/48) is not an SH projection: `visible_radiance_sum` stores,
  per light, the radiance arriving at the probe with no surface cosine. For a point light
  that is I/d², what a card facing the light receives, and runtime dynamic lights on
  scatter maps follow the same rule (`billboard.wgsl` `color × attenuation`). The polygon
  analogue keeps that invariant: the polygon term evaluated with the probe treated as a
  receiver facing the light's net direction Φ̂ (normalised irradiance vector of the
  polygons whose front half-space holds the probe), × the texel path's weighted
  visibility with Φ̂ as the receiver normal. It equals I/d² in the far limit and never
  exceeds what a card facing the light receives. L·Ω was considered and rejected: it
  reads up to 2× a surface facing the panel (1.42× at 1 m under a 4 × 4 m panel), a
  brightness no point light gives scatter today. Row 11 needs restating (below).
- `delta_drop_policy` reads `falloff_range` only in a test; no change there.
- `affinity_grid::light_portal_reach` floods portals from the leaf holding `light.origin`
  and bypasses the filter when that leaf is solid → an area light floods from the leaves
  of its lifted polygon centroids, else bypasses (conservative, so exact).
- `parse.rs` light branch requires `origin`, always `continue`s, and so drops a light
  entity's brushes; unknown brush entities are silently dropped (`:1716`) → `light_area`
  gets its own branch outside `LIGHT_CLASSNAMES`, joins `world_brush_ids` and pushes its
  light at its entity position. Its diagnostics label uses the centroid, not an authored
  `origin`.
- `_falloff_range` is required for Point/Spot (`quake_map.rs:159-173`); it is optional on
  Area, where the reach is derived.
- Coplanar face dedup (`face_extract.rs:340-435`) can trim an `_e` piece. As built (task
  2), exposure reads the post-BSP render faces grouped by brush and side, emitting the
  unsplit side winding whenever its pieces sum to the side's area. An `_e` side that loses
  a coplanar contest to a non-`_e` face is trimmed, so emission follows what renders;
  O6–O8 and row 12 keep their meaning.
- As built (task 3), Path clarifications that keep every row's meaning:
  - **Shaft cull, single-plane exemption.** With the emitter sphere alone no tile of a
    real panel clears: its own glass and a flush ceiling enter every hull, which would
    make row 21's cleared-tile count unmeetable. When every polygon shares one plane and
    a tile's slab corners lie at least τ in front of it, a triangle lying in that plane
    (within 0.1 mm, tilt under 1 mrad, sphere within 1 km of the origin) is skipped, since
    no traced segment can cross it. Multi-plane lights still test it. This is the simplest
    form of research §4's front-half-space tightening (`lightmap_layer/shaft_cull.rs`).
  - **Clear tiles** store `TexelSampler::clear_visibility`: 1 for every light, except an
    area light with no weighted lattice sample (O8), which stores 0. Skip-on stays byte
    equal to skip-off.
  - **Reach cull** admits area lights. Each polygon centroid lifted by τ must sit in an
    open leaf, and the flood seeds from all of them. `Emitter::of` takes the emitter
    sphere. The affinity flood seeds the same way.
  - **`hard_ray_target(Area)`** is the solid-angle median sample of the faced polygons,
    lifted by τ. It falls back to the centroid when no polygon faces the point. The mover
    gate's texel and probe segments use it and never set `inside` for area. Its hull uses
    the emitter-sphere box.
  - **Chart cull** drops a pair when every texel centre is outside the reach sphere, or
    when every polygon is behind or wholly below the horizon of every centre.
  - **Lattice and probes.** The area lattice is rank-1: x is stratified, y steps by the
    golden ratio, and the whole lattice is shifted on the torus by the seed. The probe
    subset (`area_probe_indices`) is one sample per quadrant and depends on the count
    alone. Pairing uses each sample's offset from the lattice mean. An all-clear probe set
    ends early only when the probes carry weight; otherwise the trace escalates.
- As built (task 4):
  - **Probe-point visibility.** Direct SH and the mover channel's q take unweighted
    solid-angle samples, because a probe has no normal. Each sample is lifted along Φ̂, and
    q is the clear share of the samples.
  - **Shadowed SH.** Shadowed SH applies a control variate: the analytic coefficients
    minus (Ω/N)·Σ Y(ωₖ) over the blocked samples.
  - **Analytic L2.** L2 comes from the polygon second moment (Arvo's axial-moment
    recurrence, n = 2).
  - **Scatter.** The Φ̂-facing term lives in `incident_radiance_at_point(Area)`;
    `billboard_direct_scatter_bake` is unchanged.
  - **A reach reader the Path missed.** `surface_radiance/reach.rs::for_each_in_reach`
    sized area lights by origin ± `falloff_range`. It now uses the reach sphere (O21).
- As built (task 5):
  - **Separate one-sample helper.** The O23 one-sample arm is
    `lightmap_bake::area_visibility_one_sample`, reached from
    `soft_visibility_one_sample`. Its estimate w·V·Ω/∫w dω can exceed 1. The bounce caller
    multiplies by it and never clamps it.
  - **Strata over lattice indices.** The seed's torus shift differs per gather ray, so
    there is one sample per stratum over lattice indices, not over shifted positions. The
    estimate stays unbiased.
  - **No reach edits needed (task 5).** `delta_sh_bake` and `affinity_grid` already read the reach
    sphere, so they needed no edit. The merged `pipeline/delta_plan.rs` reaches the same
    affinity decomposition.
- As built (task 6):
  - **Promotion refusal.** The animated-promotion refusal lives in the renderer's
    `light_selection.rs`. There is no compiler promotion path for animated lights. The
    refusal leaves a hole so that later roster indices hold their place.
  - **Runtime placeholder.** At runtime an area light is an inert placeholder `SpecLight`:
    zero colour, point type, no cone, no SDF flag. It keeps its spec index, so stray
    records add nothing.
  - **Loader refusals.** The loader refuses an area record marked dynamic, SDF or
    entity-shadow-casting, and `validate_entity_shadow_light_selection` refuses area lights.
  - **Bridge stub until task 8.** The bridge maps Area to `LightKind::Point` until task 8,
    so `lightType` reads "Point" and `sweep` is not refused yet.
  - **Permanent arms.** The two chunk-list arms behind `overlaps_chunk` are now
    `unreachable!`. The `reflection_probes/sdf_lights.rs` arm is permanent, mirroring the
    runtime placeholder.
- As built (task 7):
  - **Section.** The area glow section is PRL id 62, `AreaGlow`. It stores the authored
    facts per light (`_glow`, linear colour, intensity, compose slot or `u32::MAX`) and the
    light's emitting faces in ascending order. The GPU table still holds one entry per
    face, and the cap still counts faces (3839).
  - **GPU entry encoding.** Taken literally, the Delegated answer's "`_glow × _color / L`"
    would apply colour twice, because the descriptor's `base_color` already carries
    colour × L. The entry is therefore one of two forms:
    - static: `(glow·r, glow·g, glow·b, −1)`;
    - slotted: `(glow, 1/L, 0, slot)`.

    The slot is stored as a float, so no NaN bit patterns reach a uniform under FXC.
  - **Clamps.** Besides the `_glow × brightness` clamp at 64, the shader caps each channel
    at 64. This also bounds brightness that a settled one-shot has folded into the
    descriptor.
  - **Table sizes.** `EMISSIVE_TABLE_ENTRIES` stays 257 for the volume packer. The
    4096-entry table has its own constants, and area entries are written once at install.
  - **Docs (task 7).** Context docs were updated in this task: `build_pipeline.md` (id 62, the
    membership skip), `rendering_pipeline.md` §7.3, and `resource_management.md` §4.5
    and §5.2.
- As built (task 8):
  - **Row 44 in two halves.** The bake-time snapshot can't be read back from a compiled
    PRL, so row 44 is proved in two exact halves through the AlphaLights record: bake-time
    snapshot = record (compiler), and runtime snapshot = record (engine).
  - **O15: a refused step is skipped entirely.** Membership skips a refused step whole, so
    it also reserves no slot, matching the runtime, which never applies it. A
    `_start_inactive` light still reserves its slot.
  - **Row 43 harness.** Row 43's host/client test uses the conditioned harness with
    synthetic spot/area/spot lights, not a compiled map.
  - **Example.** The dev example is `content/dev/maps/area-lights-demo.map` with
    `content/dev/scripts/area-lights-demo.{ts,luau}`. `context/lib/scripting.md` §2 and
    §12 are updated.
  Faces carry `brush_index` only (no side index): glow ids stamp on every render face
  whose brush belongs to the light and whose texture has an `_e` map, which is the
  Decision's definition of an emitting face.
- id-42 owner eligibility admits any light type (`shadowmask_bake/eligibility.rs:41-80`),
  and chunk light lists (id 23) admit every non-dynamic spec light → explicit Area
  refusals in both, so the placeholder `SpecLight` keeps its index and reaches no
  specular loop.
- `pack::encode_alpha_lights` warns and marks unassigned any origin in a solid leaf → an
  area light whose centroid sits in a pillar is marked unassigned silently (it never
  promotes, the only runtime reader of that leaf).
- O14: the tagged reaction handler (`script_primitives.rs:221-242`) writes targets in turn
  and returns on the first error → validate every target's kind before any write.
- O15: `script-compiler/src/light_membership.rs::collect_membership` treats any `levelLoad`
  `setLightAnimation` without `startActive` as start-active → a step carrying a direction
  channel contributes no start state to an area light.
- Compose descriptor `base_color` is colour × intensity, so it includes `light`
  (`sh_bake/animation.rs:36-51`, `light_bridge.rs:1698-1726`) → a glow entry carries
  `_glow / L`, as O2 already states. `light 0` takes the static glow term (no divide).
  `fluid_reflection.wgsl::fluid_reflection_light_value` is the evaluation to reuse.
- `emissive_membership` has no mover/fluid skip code: those faces are simply absent from
  static geometry. Area faces are present → an explicit per-face skip via `face_brushes`;
  the empty-volume warning text is reworded for a volume holding only area faces.
- Reflection capture zeroes `material.emissive` at strength 0
  (`surface_materials.rs:150`) → area faces sample `_e` on their own path. Each light's
  reference already includes L (`reference.rs`), so the unit glow is `_e × _glow / L`.
  The capture key gains an area-glow section only when one exists.
- Brightness curves are unbounded above 1, so `_glow` 64 × a curve above 1 can pass the
  half-float bound the ceiling exists for. Clarification, same meaning: the shader clamps
  the live glow factor (`_glow` × brightness multiplier) at the ceiling 64.

## Delegated answers
- **Where area glow entries live** — their own optional section, written only when the
  map has an area light; id 60 stays v1 and byte-identical. One entry per emitting render
  face: `_glow × _color / L`, its light's compose slot or none (O16). Faces reach entries
  through the existing slot-2 `u32`, widened: ids above 256 are area entries. The
  group-4 binding-8 uniform table grows from 257 to 4096 `vec4` (64 KiB, the uniform
  binding limit; `animated_block_table` already uses it), so the cap is 3839 emitting
  faces (O5). No new binding, vertex attribute or pass bind. A map with no area light
  writes the same empty, zero-filled stream. Reason: no format bump touches maps without
  area lights, and the binding budget pins stay at their counts.
- **IP follow-ups and counsel** — run beside the build with the owner and counsel; a
  filing whose elements all appear present stops it.

## Restatement (owner-approved 2026-11-21)
Row 11 now reads, in the brief and here:

> Billboard scatter and the mover probe channel each integrate a 4 × 4 m panel over the
> polygon. At a probe 1 m below its centre, the mover probe channel's side-facing
> reconstruction is within 5% of 0.781, and billboard scatter stores the polygon term
> facing the panel within 1% of 2.611, the floor's value there, not the centroid point
> value L·A/d² (16).

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 §1 table, 1e-5 | `area_light::polygon` unit tests vs §1 oracle values | achievable | pass |
| 2 coplanar/behind/own face 0; 1 mm → π | same module | achievable | pass |
| 3 small limit; reach 16 m; 0.972 at 4 m | same module (window, reach) | achievable | pass |
| 4 f32 corners at 1 km | same module | achievable | pass |
| 5 open/occluded texel exactly 1/0 | `lightmap_bake` area-arm test on a flush panel scene | achievable | pass |
| 6 near vs far half occluder ordered | same, weighted vs unweighted | achievable | pass |
| 7 probe L0/L1 analytic | `sh_bake` area projection test | achievable | pass |
| 8 side 0.781 ±5%, down 0 ±0.05 | same | achievable | pass |
| 9 pillar/flush facing 1; post > 0 (O19) | `texel_footprint` area tests | achievable | pass (review fix: zero-weight probes no longer end a cut-horizon texel at 0) |
| 10 north of pillar 1, 1% (O20) | same | achievable | pass |
| 11 scatter + mover channel over polygon | scatter and mover-channel tests | achievable (restated) | pass |
| 12 frame + glass: glass emits; brushes solid | parse test + compiled fixture collision/shadow check | achievable | pass |
| 13 `_falloff_range` override; √2 reach (O22) | translation + window tests | achievable | pass |
| 14 half reach 0.879 across six consumers | one cross-consumer test per consumer | achievable | pass |
| 15 corner-reach wall lit (O21) | lightmap, direct-SH, patch tests | achievable | pass |
| 16 pillar not buried/embedded; point still dropped | `buried_lights`, `embedded_emitter` tests | achievable | pass |
| 17 flush edge texel V = 1, 1% | `texel_footprint` area test | achievable | pass |
| 18 targets inside emitter sphere | sampler property test over three shapes | achievable | pass |
| 19 flush/pillar SH, patch, scatter, mover 1% | per-consumer tests | achievable | pass (review fix: zero-Φ probes keep the light via `probe_direction`) |
| 20 door mover gated | `mover_light_gate` area test | achievable | pass |
| 21 exact skips byte-identical; shaft cull clears | skip on/off fixture test, cleared-tile count | achievable | pass |
| 22 vertex move re-keys; unrelated stable | cache-key tests | achievable | pass |
| 23 four brushes = one (O6) | exposure + term test | achievable | pass |
| 24 overlap once; buried half (O7) | exposure test | achievable | pass (review fix: only drawn faces emit; void-facing sides excluded) |
| 25 slivers dropped; no NaN (O8) | exposure + sampler tests | achievable | pass |
| 26 corner-reach wall move re-keys; warm = cold (O10) | layer-key + warm/cold test | achievable | pass |
| 27 entity-order position (O11) | parse + namespace test | achievable | pass |
| 28 split side glows; frame no glow id (O12) | glow stamping test | achievable | pass |
| 29 probe set order independence (O9) | `SoftProbes` same-thread test | achievable | pass |
| 30 fail/warn naming light + key | translation tests with log capture | achievable | pass (review: negative `light` and `_bake_only`+`_start_inactive` fail; more inert keys warn; light named once) |
| 31 inert keys re-key nothing | warm-rebuild cache test | achievable | pass (`area_light_warm_rebuild`) |
| 32 no promotion; never id-42 owner; no specular | `light_selection`, `eligibility`, chunk-list tests | achievable | pass |
| 33 weight map at curve 1 = static | `animated_light_weight_maps` test | achievable | pass |
| 34 `_bounce_fill 0` delta within 2% | `delta_sh_bake` area test | achievable | pass |
| 35 one-sample arm averages (O23) | `soft_visibility_one_sample` area test | achievable | pass |
| 36 `_falloff_range 3` affinity cells (O24) | `affinity_grid`/delta test | achievable | pass |
| 37 `--soft-shadow-samples` re-keys texel paths only | warm-rebuild cache test | achievable | pass (`area_light_warm_rebuild`, now also vs a cold 16-sample build) |
| 38 AlphaLights v5 round trip; v4/unknown rejected | `level-format` tests | achievable | pass |
| 39 no-area map byte-identical | every bake section vs `main` build on fixtures | achievable | pass (opt-in gate vs `11f27aaa9` binaries, rerun after review fixes: only id 18 version word differs) |
| 40 area fixture in `GATE_FIXTURES` | profiled fixture, determinism gate | achievable | pass (determinism gate; budget gate stale on `main` itself, not this branch). After the `player_spawn` fix (2026-11-22), the budget gate run on `area-light-panel` alone matches both its id 22 and id 42 goldens |
| 41 scripting example on `content/dev` | dev-map script test + TS/Luau checks | achievable | pass, except TS and Luau type checks outstanding (`tsc`, `luau-analyze` not installed) |
| 42 `sweep` refused; spot still works | `script_primitives` test | achievable | pass |
| 43 tagged sweep S1, A, S2 unchanged (O14) | host + client replication test | achievable | pass |
| 44 inert-key snapshots identical (O18) | bake-time vs runtime snapshot test | achievable | pass |
| 45 glow by strength; `_glow` scaling; 100 → 64 warns; < 0 fails | glow table + byte-identity + log tests | achievable | pass |
| 46 `emissive_volume` vs area faces | membership test + empty warning | achievable | pass |
| 47 capture at reference; `_start_inactive` reflected dark then lit together | capture test + share/bridge test (GPU-gated part under `POSTRETRO_REQUIRE_GPU`) | achievable | pass for lit lights; a `light 0` light's reflected glass stays at its reference glow (as-built limitation, owner call) |
| 48 L vs 2L glass unchanged; cycle mean 0.5 (O2) | reflection capture test | achievable | pass |
| 49 `colorShift` moves glow and room | descriptor + glow evaluation test | achievable | pass |
| 50 2L doubles light, glow unchanged | bake + glow entry test | achievable | pass |
| 51 strength-4 white vs red `_e` byte-identical lighting | fixture byte compare | achievable | pass |
| 52 `_glow` edit misses only reflection capture | warm-rebuild cache test | achievable | pass |
| 53 fade settles at 0.25 (O1) | bridge settle test with glow evaluation | achievable | pass (plus `light 0` variants: settles held) |
| 54 colour settles amber (O17) | same | achievable | pass (plus `light 0` variant) |
| 55 `_start_inactive` reservation; pulse starts lit (O3) | membership + start-state test | achievable | pass |
| 56 sweep-only `levelLoad` starts dark (O15) | script-compiler membership test | achievable | pass |
| 57 static point `_start_inactive` unchanged | existing `main.rs:3578` test extended | achievable | pass |
| 58 compose slot per area light (O4) | glow table vs namespaces test | achievable | pass |
| 59 static before animated keeps steady glow (O16) | glow evaluation test | achievable | pass |
| 60 no area → no entries; cap fails; loader rejects (O5) | compiler + loader tests | achievable | pass |
| M1 hallway cold/warm bake vs 4 × 4 grid | owner run, recorded here | manual | pass, restated by the owner (2026-11-22): measured on the `area-lights-ab-panel`/`-grid` room pair instead of hallway copies, debug `prl-build --no-cache`: panel 4 s total (lightmap 3.7 s), 16-point grid 9–10 s, 31 s with the points 2 units under the glass. A low-quality hallway preview (`-j 2`, `--lightmap-density 0.1`, `--sh-probe-spacing 2`) showed the skylight in place; its 20.6 min was 17 min SH bake from the map's 678 spot lights, so hallway totals would mostly measure those. Owner judged hallway copies overkill |
| M2 penumbra RMS at 32/64 vs 256 | measurement run + owner A/B | manual | waived by the owner (2026-11-22): needs full-density hallway bakes; no visible penumbra noise in the M3 A/B |
| M3 skylight vs template grid A/B | owner, in-engine | manual | pass (owner, 2026-11-22, `area-lights-ab-panel` vs `-grid`): the 16-point grid floods the ceiling around the panel and reads over-exposed; the panel reads as light leaving the glass, away from the ceiling. The corridor panel fades out with no visible edge at its reach |
| M4 scripted flicker in step | owner, in-engine | manual | pass (owner, 2026-11-22, `area-lights-demo`): panes and room light pulse together |
| M5 entities under skylight | owner, in-engine | manual | pass (owner, 2026-11-22, `area-lights-ab-panel`): dummies and enemy lit and shaded as expected; no entity shadow, as the v1 refusal intends |

Gate (`a5fadf4e9`, workspace): fmt, clippy `-D warnings`, `cargo check --release`, crate-graph and both reducers pass; `cargo test --workspace --no-fail-fast` passes 16260 tests. Failures, none from this branch: `smooth_specular_matches_the_frozen_pre_change_function` (6 ulp, identical shaders on `main`); xtask `mover_shot_gates` ×2 (Windows path separators in files this branch never touches); `telemetry_run` ×2 (windowed run stalled on a sleeping display; passed the run before).

Review loop: three passes. Pass 1 (11 reviewers) found 3 🔴, 6 🟡 and ~20 🟢; pass 2 (3 reviewers) 1 🔴, 3 🟡; pass 3 (1 reviewer) 1 🟡 needing an owner call. All acted on except the deliberately left items below.

Left deliberately or for the owner:
- `light 0` area light: a second animation starts from `_glow × _color`, not from the first one-shot's held value (a lit light keeps its settled value). Documented in `rendering_pipeline.md` §7.3, `docs/level_design.md`, `docs/scripting-reference.md`.
- `light 0` reflections hold the reference glow, even while the light is dark (row 47 above).
- No stage cache versions were bumped: `main` holds no area-light entries, so only caches built on this branch can be stale. Bake area-light maps with `--no-cache` for the manual checks.
- `research.md O#` and acceptance-row pointers in comments; they resolve once the brief moves to `done/`.
- Trimmed sides emit BSP-leaf pieces, so an unrelated split can re-key a light (follow-on); hand-made-file-only gaps (falloff model on type 3, client-side direction); capture's 64 cap with curves above 1; f32 lift beyond 8 km; the mover-gate estimate ray.


## Tasks

Sequential on the feature branch (disk-bound machine: one cargo job at a time; no
worktrees). Workers get the full brief, `research.md`, routed docs and this plan; they do
not edit `plan.md` or commit. Each task commits with its row update.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Polygon maths core: new compiler `area_light` module — clipped Lambert term and irradiance vector (f64 atan2), solid angle, derived reach and Karis window, emitter and reach spheres, Ureña/Arvo-fan solid-angle sampler with cosθ_r weights. Rows 1–4 | worker, reviewed by integrator | — | done: 22 `area_light` tests pass (`--bin prl-build area_light`); clippy clean. Note: a fixed lattice over a fan or multi-polygon emitter splits strata off-centre (1.7% high on a hexagon at 64²); exact at all-visible and none-visible, so penumbra only. The module uses `ray_traversal` (bin-only), so task 2 splits the types into a file `lib.rs` can compile beside `map_data` |
| 2 | Authoring and translation, lightmap term: `LightType::Area` (compiler, loader, `LightKind`), `light_area` parse branch, pre-BSP exposure (O6–O8), polygons + centroid + resolved reach in `MapLight`, key rules and inert-key normalisation, buried/embedded skips, FGD entry, unshadowed lightmap arm on a test map. Other bake arms read zero until tasks 4–5. Rows 12, 13, 16, 23–25, 27, 30 | integrator | 1 | done. AlphaLights v5 pulled in from task 6; loader rejects area records until task 8. `--bin prl-build`: 2682 pass before the fixture fix; drift guard `light_wire_drift_tests` passes; `level-format` 834 pass; `level-loader` 423 pass; `cargo check --workspace --all-targets` clean; clippy `-D warnings` clean on compiler/format/loader. `sh-streaming-hinted-door.prl` regenerated per its README (two cold builds identical): id 18 v4→v5 word, and id 52 v1→v2, which was already stale on `main` (E10 perception step). Transient arms until tasks 3–8: visibility 1 at `texel_footprint.rs:442`; zero SH/scatter/patch/bounce at `sh_bake/light_eval.rs:69`; the rest are named in task 3–8 scope |
| 3 | Lightmap shadows and culls: polygon lattice outside both memos, ratio-estimator visibility, centre probe (O19, O20), emitter/reach sphere readers (shaft cull, flood, chart cull, layer key, affinity flood), mover light gate, weight maps, shadowmask. Rows 5, 6, 9, 10, 14 (texel, weight map), 15 (lightmap), 17, 18, 20–22, 26, 29, 31, 33, 37 | integrator | 2 | done. `--bin prl-build` 2703 pass, 0 fail; workspace check and compiler clippy clean. Proofs: `texel_footprint::area_tests` (rows 5, 6, 9, 10, 14, 17, 18, 29, 33), `mover_light_gate::tests::a_mover_between_an_area_light_and_what_it_lights_is_gated` (20), `pipeline::lightmap_stage::tests::area_light_tests` (15 via chart planning, 21, 22, 26, 37), `area_light::exposure::tests::inert_keys_leave_every_lighting_key_unchanged` (31). Follow-ups: rerun row 22 after task 4 folds polygons into `LightDigest::of`. Rows 31 and 37 are key-level proofs; add a warm-rebuild pipeline check at task 9, once probe arms are live. The lightmap cache was not version-bumped, so warm entries from task-2 builds (visibility 1) can be served stale, on this branch only. `build_pipeline.md` needs wording for the shaft-cull exemption, reach-cull eligibility and mover gate (task 9). Still transient: the one-sample arm traces `hard_ray_target` (task 5), and `chunk_light_list_bake` arms are unreachable behind `overlaps_chunk` (task 6) |
| 4 | Probe arms: direct SH 35/41/45 (analytic L0–L2 + control variate), surface-radiance patch + `LightDigest`, billboard scatter (term facing Φ̂), mover probe channel. Rows 7, 8, 11, 14, 15, 19 | worker, reviewed by integrator | 3 | done. `--bin prl-build` 2712 pass, 0 fail; workspace check and compiler clippy clean. Proofs in `area_light_probe_tests`: row 7 (L0/L1 within 1e-5), row 8 (side 0.761, 2.6% under 0.781; down −0.045 before the clamp), row 11 (scatter 2.611 within 1%; mover side 0.761), row 14 (each probe consumer at 0.8789 within 1e-3, 0 at reach), row 15 (corner patch and probe), row 19 (flush and pillar, all four consumers). Also `mover_probe_channel::direct_tests::an_area_lights_q_and_tile_integrate_over_its_polygons` (row 11, production bake). Row 22 re-run passes now that `LightDigest::of` folds the polygons. No stage version bumps (same branch-only stale-cache caveat as task 3). Still transient: the animated per-hit bounce skips Area explicitly (task 5) |
| 5 | Animated bounce: one-sample polygon arm, delta `light_aabb`/affinity reach sphere. Rows 34–36 | integrator | 4 | done. `cargo test -p postretro-level-compiler --no-fail-fast`: bin 2740 pass, every integration binary passes, including `mover_bounded_bake_goldens`. Workspace check and compiler clippy clean. Proofs in `area_light_bounce_tests`: row 34 (delta 1302.20 vs static 1303.83, −0.12%, all 196 probes), row 35 (open mean 1.00039; far half occluded mean 0.87183 vs quadrature 0.86959), row 36 (every cell within 3 m holds an entry and a non-zero tile). No stage version bump (branch-only stale-cache caveat, as in task 3) |
| 6 | Wire and refusals: AlphaLights v5 (pack, loader, engine), silent unassigned leaf, animated-promotion refusal, placeholder `SpecLight` out of id-42 owners and chunk lists. Rows 32, 38 | worker | 2 | done. An area-light map loads and renders (`area-light-panel.map`, `--capture` under `POSTRETRO_SH_STREAMING=sync-proof`, exit 0). Row 32: `light_filter_tests::promotion_selects_no_area_light_on_either_path`, `shadowmask_bake::tests::eligibility_skips_an_area_light_and_keeps_later_spec_indices`, `area_light_tests::an_area_light_owns_no_shadowmask_channel_even_where_it_alone_lights`, `chunk_light_list_bake::an_area_light_keeps_its_slot_but_enters_no_chunk_list`, `spec_buffer::area_light_packs_an_inert_placeholder_at_its_index`. Row 38: `alpha_lights::writes_v5_and_round_trips_every_light_type`, loader `load_prl_loads_an_area_light_at_its_authored_index` and `load_prl_rejects_a_v4_alpha_lights_section`. Suites: format 835, loader 427, `prl-build` 2744, lighting 75, postretro 3030+7, all pass. Renderer: 1438 pass, 1 fail, `world_specular_test::smooth_specular_matches_the_frozen_pre_change_function` (6 ulps on this GPU). Its test and shaders are byte-identical to `main`, so it is not this branch. Workspace check and clippy on six crates clean. Already broken before this task: `cargo test -p postretro-lighting` without `postretro-level-loader/load-prl` fails to compile |
| 7 | Glow: `_glow` rules, area glow section + loader + cap, compiler stamping after namespaces, widened slot-2 table, forward area branch on compose descriptors with clamp, membership skip, `_start_inactive` reservation, reflection capture glow, key and share. Rows 28, 45–60 (glow parts) | worker, reviewed by integrator | 6 | done (worker-verified; integrator focused re-run pending, since builds paused for the owner). All owned glow rows pass:
<ul>
<li>row 28: `area_light::glow::tests::a_split_side_glows_on_every_piece_and_the_frame_carries_no_glow`</li>
<li>rows 45, 51, 52: `tests/area_glow_compile.rs`</li>
<li>row 46: `emissive_membership` tests</li>
<li>rows 47, 48: `reflection_probes::area_glow_capture_tests`, plus the GPU `area_glow_gpu_test`</li>
<li>rows 49, 50: render-cpu and the lightmap. For row 50 the probe-output doubling is not measured; the probe arms are linear in L.</li>
<li>rows 53–55 (glow part), 58, 59: `light_bridge` tests. Row 55 uses a synthetic manifest record; task 8 owns the real start state and row 56.</li>
<li>row 57: extended `fixture_keeps_script_and_kvp_animated_lights_distinct`</li>
<li>row 60: section, cap and loader tests</li>
</ul>
Suites:
<ul>
<li>pass: format 840, render-cpu 845, loader 429, compiler 3140 (goldens unchanged), postretro 3039, `dx12_fxc`</li>
<li>renderer with GPU: 1439 pass and 1 fail, the known `main` failure</li>
<li>check, fmt and clippy on six crates clean</li>
</ul>
The capture shows the panel glowing over the lit room |
| 8 | Script surface: `"Area"` kind + generated types + drift fixtures, snapshots, validate-all `sweep` refusal (O14), start-state rule (O15), `content/dev` example. Rows 41–44, 55, 56 | worker | 6 | done. Proofs:
<ul>
<li>row 41: `startup::area_lights_surface_tests` (real compile of the demo; TS and Luau twins emit identical wire) and the `LightKind` typedef drift guard</li>
<li>rows 42, 43: lighting `script_primitives::tests` (sweep refused; tagged S1/A/S2 writes nothing), plus the host/client coop test</li>
<li>row 44: `inert_keyed_area_light_snapshot_matches_its_alpha_lights_record` and the runtime twin</li>
<li>rows 55, 56: prl-build `a_level_load_pulse_lights_a_start_inactive_area_light_and_a_sweep_does_not`, plus script-compiler `a_sweep_step_contributes_no_start_state_to_an_area_light_in_both_hosts`</li>
</ul>
Suites (all pass, `--no-fail-fast`): entities 347, lighting 100, script-compiler 63, scripting-core 1148, sim 2234, compiler bin 2758 plus all integration tests (goldens unchanged), postretro 3044. Check, fmt and clippy clean. Outstanding: `tsc` and `luau-analyze` are not installed, so `sdk/type-tests/area-lights.ts`, the demo `.ts` and the generated Luau alias are not type-checked |
| 9 | Fixtures, gates, docs: profiled area fixture in `GATE_FIXTURES`, no-area byte identity vs `main`, `docs/level_design.md` and `docs/scripting-reference.md`. Rows 39, 40 | integrator | 1–8 | done. Proofs:<ul>
<li>row 39: `tests/no_area_light_byte_identity.rs` (`#[ignore]`, opt-in via `POSTRETRO_AREA_LIGHTS_BASELINE_DIR`) compares 18 no-area maps against `prl-build` built at merge base `11f27aaa9`: section set, table versions and payloads equal, except AlphaLights' leading `u32` 4 → 5; no id 62. Passed; a Python container diff agreed on two maps, and penumbra-floor-off bakes of two maps agreed too. Rerun note: `scripts-build` reads `sdk/lib` from its own checkout path, so keep that tree when comparing a copied binary</li>
<li>row 40: `area-light-panel` in `GATE_FIXTURES` (about 1 s cold plus warm); the determinism gate passes with it, and its budget digests are recorded (id 42 equals the animated fixtures', an area light owning no channel)</li>
<li>row 31: `tests/area_light_warm_rebuild.rs::inert_keys_on_area_lights_rebuild_every_stage_from_the_cache` (every stage hits, sections identical)</li>
<li>row 37: `soft_shadow_samples_rekey_only_the_texel_paths_of_area_lights` (4 → 16 misses only the lightmap section, weight maps and the two mover-light texel stages; every probe stage hits; only ids 22, 25, 42 move)</li>
<li>row 50 probe half: `doubling_light_doubles_the_lightmap_and_leaves_the_glow` now also decodes ids 35 (exact) and 34 (within 0.41%; the patch visibility seed hashes intensity)</li>
<li>docs: `build_pipeline.md` "decided, not yet built" markers removed and the area section rewritten as built; `rendering_pipeline.md` §4; `resource_management.md`; `docs/level_design.md` (`light_area` section); `docs/scripting-reference.md` (Area kind, sweep refusal, all-or-nothing tagged writes, refused `levelLoad` step)</li>
</ul>
Suites: compiler `--no-fail-fast` 3144 passed, 87 ignored; workspace check, fmt and compiler clippy clean. Pre-existing, not this branch: `layer_budget_sections_match_pre_change_cold_and_warm` fails on `main` at `11f27aaa9` too (goldens stale since `9fc95406a`; `main` and this branch print identical digests for every old fixture). Left for the owner |
