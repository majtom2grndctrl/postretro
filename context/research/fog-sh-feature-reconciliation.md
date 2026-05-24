# Fog + SH Feature Reconciliation

**Date investigated:** 2026-05-23

Audit prompted by a recurring question: has the fog-volume / SH-irradiance code
accreted more capability than the engine actually needs, and is the accretion
brittle or just the normal cost of graphics work? This compares the *want list*
(roadmap M9 + `rendering_pipeline.md` §4/§7.5) against the *actual feature
surface* and *real content usage*.

## The want list

- **Product definition** (`context/lib/index.md` §1): "baked volumetric indirect
  lighting (SH irradiance volumes)" and "billboard sprite volumetrics that react
  to light." Fog is cyberpunk atmosphere, not a simulation.
- **M9 scope** (`roadmap.md` lines 162–179): kill light-leak through walls
  (depth-aware probes), then add one *directional* fog term on top of the
  existing volumetric pass. Back-scatter (negative `g`) is explicitly **optional**.
- **Stated SH intent** (roadmap line 174): the depth-aware interpolant
  *"replaces the trilinear SH sample entirely — one runtime path."*

## Actual surface vs. real content usage

Content surveyed: `content/dev/maps/campaign-test.map`, `occlusion-test.map`.
Authoring contract: `sdk/TrenchBroom/postretro.fgd` (fog entities ~lines 159–237).
Resolvers: `crates/level-compiler/src/parse.rs` (`resolve_fog_volume` 634–773,
`resolve_fog_ellipsoid` 778–897, `resolve_fog_lamp` 900–993, `resolve_fog_tube`
999–1138).

| Per-volume knob | Plumbed | Authored in any map | Verdict |
|---|---|---|---|
| density, glow, saturation | yes | yes — everywhere | **core** |
| edge_softness | yes | yes (4×) | keep |
| tint | yes | once | marginal |
| min_brightness | yes | once (−0.05, clamps to 0) | marginal |
| light_range | yes | once (0.5) | marginal |
| scatter_bias → anisotropy | yes | 4×, but values 0.8/1.0/100; only **100** is non-flat (1.0 → g≈0.009) | **one real use** |
| radial_falloff / falloff | yes | **never** | stale |
| ambient_scatter | yes | **never** (always 1.0) | new in M9; unproven |
| _tags | yes | **never** | stale |

Six of twelve knobs are core/used; three are touched exactly once; three are
never authored. The directional term we spent a full session debanding is
exercised by **one map** at essentially **one** meaningful value.

## Where things are genuinely "bolted on"

Two distinct phenomena, only one of which is brittle.

### 1. Knob accretion = a long synchronized plumbing chain (coupling, not nesting)

Each fog knob must be added in lockstep across ~7 sites:

1. FGD entity param (`postretro.fgd`)
2. resolver + clamp (`parse.rs`, e.g. `scatter_bias_to_anisotropy` ~600,
   `clamp_ambient_scatter` ~616)
3. `FogVolumeRecord` PRL serialization (`crates/level-format`)
4. CPU `FogVolume` struct + `pack_fog_params`/pack order (`crates/postretro/src/fx/fog_volume.rs`)
5. WGSL `FogVolume` struct — **byte-layout must match the CPU struct exactly**
   (`fog_volume.wgsl`; the struct carries a "112 bytes; layout must match"
   warning and hand-packed vec3+scalar padding)
6. an accumulator in the per-step inner loop (`vs_*_accum`) + a normalize line
7. application logic (saturation/tint/min_brightness/etc.)

The fragility here is **not** deep nesting — it's the manual cross-crate chain
with a byte-exact GPU layout coupling in the middle. A field added in the wrong
order silently corrupts every downstream read; nothing fails loudly. This is the
real cost of "cheap to add a knob": cheap functionally, expensive in coupling
surface. The inner-loop accumulator block grows one parallel `vs_*_accum`
variable per knob — wide, repetitive, but flat and readable.

### 2. SH helper family is fragmenting into near-duplicates (this *is* brittle)

`sh_sample.wgsl` now exposes three corner-sampling entry points over the same
8-corner trilinear math:

- `sample_sh_indirect_corners_impl` — the source-of-truth loop, parameterized by
  `reject_backface` + `use_depth_visibility` bools.
- `sample_sh_indirect_corners_depth_aware` / `_without_depth` — thin wrappers
  setting those bools. Fine.
- `sample_sh_indirect_corners_two_without_depth` — **added 2026-05-23 (this
  session)** for the fog dual-read. It does **not** call `_impl`; it
  copy-pastes the 8-corner fetch/trilinear/validity loop because `_impl` returns
  one direction and this returns two. So the corner-weighting logic now lives in
  **two places** — a bug fixed in one won't propagate to the other.

This is avoidable debt, not inherent complexity. The clean shape is a single
corner loop that fetches bands + weights once and reconstructs N directions; the
one-direction path then becomes the two-direction path with a repeated normal,
or both share an inlined corner-weight helper. (Note the Metal constraint: the
loop must stay inline / register-resident — see the `fog_volume.wgsl` cs_main
comment citing reverted commit b93d31e — so the refactor is "share the loop
body," not "pass arrays to a callee.")

### What is *not* sloppy — the deep nesting in `cs_main`

`fog_volume.wgsl::cs_main` inlines the slab-clip prologue, sort/merge, and 3–4
nested loops (sub-intervals → steps → volumes / spots / points). This looks like
a god-function, but it's a **documented GPU-compiler workaround**: a callee
taking `ptr<function, array<...>>` spills to device-private memory on Metal,
replacing coalesced storage reads with poorly-coalesced private reads (commit
b93d31e, reverted by bda93f4). The inlining is deliberate and correct for the
target. Don't "clean it up" into helpers without re-validating on Apple Silicon.

## Drift between doc and code

Roadmap line 174 ("one runtime path") is already false: forward/billboard use
the depth-aware path, fog deliberately uses `_without_depth` (fog has no surface
normal, so Chebyshev visibility is meaningless). This is a *legitimate* second
path, not a mistake — but the roadmap claims it doesn't exist. Either the
roadmap should admit fog needs a no-depth read, or the claim should be dropped.

## Billboard smoke vs. fog volumes — different shaders is correct

The question of whether billboard smoke should share the fog shader: no, and
they already don't. Fog is a **compute raymarch** (`fog_volume.wgsl`); billboard
smoke is an **additive raster pass** (`billboard.wgsl`). Different techniques,
different shaders — right call. The thing they *share* is the SH irradiance
lookup (`sh_sample.wgsl`), which is a genuine common primitive and the right
kind of sharing — except that primitive is now the fragmenting one (§2 above).
So the worry isn't "one shader for all fog"; it's "one SH helper quietly forking
into three."

## Recommendation (keep / cut / defer)

| Item | Call |
|---|---|
| `radial_falloff`/`falloff`, `_tags` | **Cut** — predate M9, never authored |
| `ambient_scatter` | **Decide** — commit to authoring it in a map this milestone, or cut. New in M9; currently unproven dead weight |
| `tint`/`min_brightness`/`light_range` | **Keep, watch** — one use each; fine as pre-wired, revisit if still single-use post-M10 |
| Three SH corner-sampling entry points | **Collapse §2** — unify the corner loop so the math lives once; cheap, removes the only true brittleness found |
| Roadmap "one SH path" claim | **Reconcile** — update to acknowledge the fog no-depth path |
| `cs_main` inlining | **Leave** — documented Metal workaround, not debt |

Net: this is *mostly* the honest trial-and-error of graphics programming
(figuring out which fog controls matter is product discovery; the GPU inlining
is real domain complexity), plus a thin layer of avoidable debt — three stale
knobs and one duplicated SH loop (the latter introduced this session). It is not
broadly sloppy or brittle. The single highest-value cleanup is collapsing the SH
helper duplication before it forks a fourth time.
