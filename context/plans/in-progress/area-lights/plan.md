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
- Coplanar face dedup (`face_extract.rs:340-435`) can trim an `_e` piece → exposure and
  the polygon list come from brush sides before BSP (as the Path says), never from faces.
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

| AC | Proof | Status |
|---|---|---|
| 1 §1 table, 1e-5 | `area_light::polygon` unit tests vs §1 oracle values | achievable |
| 2 coplanar/behind/own face 0; 1 mm → π | same module | achievable |
| 3 small limit; reach 16 m; 0.972 at 4 m | same module (window, reach) | achievable |
| 4 f32 corners at 1 km | same module | achievable |
| 5 open/occluded texel exactly 1/0 | `lightmap_bake` area-arm test on a flush panel scene | achievable |
| 6 near vs far half occluder ordered | same, weighted vs unweighted | achievable |
| 7 probe L0/L1 analytic | `sh_bake` area projection test | achievable |
| 8 side 0.781 ±5%, down 0 ±0.05 | same | achievable |
| 9 pillar/flush facing 1; post > 0 (O19) | `texel_footprint` area tests | achievable |
| 10 north of pillar 1, 1% (O20) | same | achievable |
| 11 scatter + mover channel over polygon | scatter and mover-channel tests | achievable (restated) |
| 12 frame + glass: glass emits; brushes solid | parse test + compiled fixture collision/shadow check | achievable |
| 13 `_falloff_range` override; √2 reach (O22) | translation + window tests | achievable |
| 14 half reach 0.879 across six consumers | one cross-consumer test per consumer | achievable |
| 15 corner-reach wall lit (O21) | lightmap, direct-SH, patch tests | achievable |
| 16 pillar not buried/embedded; point still dropped | `buried_lights`, `embedded_emitter` tests | achievable |
| 17 flush edge texel V = 1, 1% | `texel_footprint` area test | achievable |
| 18 targets inside emitter sphere | sampler property test over three shapes | achievable |
| 19 flush/pillar SH, patch, scatter, mover 1% | per-consumer tests | achievable |
| 20 door mover gated | `mover_light_gate` area test | achievable |
| 21 exact skips byte-identical; shaft cull clears | skip on/off fixture test, cleared-tile count | achievable |
| 22 vertex move re-keys; unrelated stable | cache-key tests | achievable |
| 23 four brushes = one (O6) | exposure + term test | achievable |
| 24 overlap once; buried half (O7) | exposure test | achievable |
| 25 slivers dropped; no NaN (O8) | exposure + sampler tests | achievable |
| 26 corner-reach wall move re-keys; warm = cold (O10) | layer-key + warm/cold test | achievable |
| 27 entity-order position (O11) | parse + namespace test | achievable |
| 28 split side glows; frame no glow id (O12) | glow stamping test | achievable |
| 29 probe set order independence (O9) | `SoftProbes` same-thread test | achievable |
| 30 fail/warn naming light + key | translation tests with log capture | achievable |
| 31 inert keys re-key nothing | warm-rebuild cache test | achievable |
| 32 no promotion; never id-42 owner; no specular | `light_selection`, `eligibility`, chunk-list tests | achievable |
| 33 weight map at curve 1 = static | `animated_light_weight_maps` test | achievable |
| 34 `_bounce_fill 0` delta within 2% | `delta_sh_bake` area test | achievable |
| 35 one-sample arm averages (O23) | `soft_visibility_one_sample` area test | achievable |
| 36 `_falloff_range 3` affinity cells (O24) | `affinity_grid`/delta test | achievable |
| 37 `--soft-shadow-samples` re-keys texel paths only | warm-rebuild cache test | achievable |
| 38 AlphaLights v5 round trip; v4/unknown rejected | `level-format` tests | achievable |
| 39 no-area map byte-identical | every bake section vs `main` build on fixtures | achievable |
| 40 area fixture in `GATE_FIXTURES` | profiled fixture, determinism gate | achievable |
| 41 scripting example on `content/dev` | dev-map script test + TS/Luau checks | achievable |
| 42 `sweep` refused; spot still works | `script_primitives` test | achievable |
| 43 tagged sweep S1, A, S2 unchanged (O14) | host + client replication test | achievable |
| 44 inert-key snapshots identical (O18) | bake-time vs runtime snapshot test | achievable |
| 45 glow by strength; `_glow` scaling; 100 → 64 warns; < 0 fails | glow table + byte-identity + log tests | achievable |
| 46 `emissive_volume` vs area faces | membership test + empty warning | achievable |
| 47 capture at reference; `_start_inactive` reflected dark then lit together | capture test + share/bridge test (GPU-gated part under `POSTRETRO_REQUIRE_GPU`) | achievable |
| 48 L vs 2L glass unchanged; cycle mean 0.5 (O2) | reflection capture test | achievable |
| 49 `colorShift` moves glow and room | descriptor + glow evaluation test | achievable |
| 50 2L doubles light, glow unchanged | bake + glow entry test | achievable |
| 51 strength-4 white vs red `_e` byte-identical lighting | fixture byte compare | achievable |
| 52 `_glow` edit misses only reflection capture | warm-rebuild cache test | achievable |
| 53 fade settles at 0.25 (O1) | bridge settle test with glow evaluation | achievable |
| 54 colour settles amber (O17) | same | achievable |
| 55 `_start_inactive` reservation; pulse starts lit (O3) | membership + start-state test | achievable |
| 56 sweep-only `levelLoad` starts dark (O15) | script-compiler membership test | achievable |
| 57 static point `_start_inactive` unchanged | existing `main.rs:3578` test extended | achievable |
| 58 compose slot per area light (O4) | glow table vs namespaces test | achievable |
| 59 static before animated keeps steady glow (O16) | glow evaluation test | achievable |
| 60 no area → no entries; cap fails; loader rejects (O5) | compiler + loader tests | achievable |
| M1 hallway cold/warm bake vs 4 × 4 grid | owner run, recorded here | manual |
| M2 penumbra RMS at 32/64 vs 256 | measurement run + owner A/B | manual |
| M3 skylight vs template grid A/B | owner, in-engine | manual |
| M4 scripted flicker in step | owner, in-engine | manual |
| M5 entities under skylight | owner, in-engine | manual |

## Tasks

Sequential on the feature branch (disk-bound machine: one cargo job at a time; no
worktrees). Workers get the full brief, `research.md`, routed docs and this plan; they do
not edit `plan.md` or commit. Each task commits with its row update.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Polygon maths core: new compiler `area_light` module — clipped Lambert term and irradiance vector (f64 atan2), solid angle, derived reach and Karis window, emitter and reach spheres, Ureña/Arvo-fan solid-angle sampler with cosθ_r weights. Rows 1–4 | worker, reviewed by integrator | — | done: 22 `area_light` tests pass (`--bin prl-build area_light`); clippy clean. Note: a fixed lattice over a fan or multi-polygon emitter splits strata off-centre (1.7% high on a hexagon at 64²); exact at all-visible and none-visible, so penumbra only. The module uses `ray_traversal` (bin-only), so task 2 splits the types into a file `lib.rs` can compile beside `map_data` |
| 2 | Authoring and translation, lightmap term: `LightType::Area` (compiler, loader, `LightKind`), `light_area` parse branch, pre-BSP exposure (O6–O8), polygons + centroid + resolved reach in `MapLight`, key rules and inert-key normalisation, buried/embedded skips, FGD entry, unshadowed lightmap arm on a test map. Other bake arms read zero until tasks 4–5. Rows 12, 13, 16, 23–25, 27, 30 | integrator | 1 | in progress (WIP commit, paused for the owner). Implemented, with AlphaLights v5 pulled in from task 6; loader rejects area records until task 8. Last full `--bin prl-build` run: 2682 pass, 1 fail (`stored_bound_equals_the_runtime_union_on_the_compiled_fixture`: committed `sh-streaming-hinted-door.prl` is AlphaLights v4; a scratch rebuild differs by more than the version word, so check whether the committed fixture was already stale, then regenerate). To re-run: the drift guard after its `concat!` fix, `-p postretro-level-format alpha_lights`, level-loader tests, `cargo check --workspace --all-targets`, clippy on compiler/format/loader. Deviation to record: exposure reads post-BSP faces, so an `_e` side losing a coplanar contest to a non-`_e` face is trimmed (emission follows what renders). Transient arms are listed in the task-2 worker report: visibility 1 at `texel_footprint.rs:442`; zero SH at `sh_bake/light_eval.rs:69` |
| 3 | Lightmap shadows and culls: polygon lattice outside both memos, ratio-estimator visibility, centre probe (O19, O20), emitter/reach sphere readers (shaft cull, flood, chart cull, layer key, affinity flood), mover light gate, weight maps, shadowmask. Rows 5, 6, 9, 10, 14 (texel, weight map), 15 (lightmap), 17, 18, 20–22, 26, 29, 31, 33, 37 | integrator | 2 | |
| 4 | Probe arms: direct SH 35/41/45 (analytic L0–L2 + control variate), surface-radiance patch + `LightDigest`, billboard scatter (term facing Φ̂), mover probe channel. Rows 7, 8, 11, 14, 15, 19 | worker, reviewed by integrator | 3 | |
| 5 | Animated bounce: one-sample polygon arm, delta `light_aabb`/affinity reach sphere. Rows 34–36 | integrator | 4 | |
| 6 | Wire and refusals: AlphaLights v5 (pack, loader, engine), silent unassigned leaf, animated-promotion refusal, placeholder `SpecLight` out of id-42 owners and chunk lists. Rows 32, 38 | worker | 2 | |
| 7 | Glow: `_glow` rules, area glow section + loader + cap, compiler stamping after namespaces, widened slot-2 table, forward area branch on compose descriptors with clamp, membership skip, `_start_inactive` reservation, reflection capture glow, key and share. Rows 28, 45–60 (glow parts) | worker, reviewed by integrator | 6 | |
| 8 | Script surface: `"Area"` kind + generated types + drift fixtures, snapshots, validate-all `sweep` refusal (O14), start-state rule (O15), `content/dev` example. Rows 41–44, 55, 56 | worker | 6 | |
| 9 | Fixtures, gates, docs: profiled area fixture in `GATE_FIXTURES`, no-area byte identity vs `main`, `docs/level_design.md` and `docs/scripting-reference.md`. Rows 39, 40 | integrator | 1–8 | |
