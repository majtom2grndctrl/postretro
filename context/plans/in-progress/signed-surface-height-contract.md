# Signed surface height — design contract

## Goal

Surface Depth height maps change from carve-only to signed. Today white (255) is the polygon plane and black carves below it. After this change **mid-gray (128) is the polygon plane, darker sinks below it, lighter rises above it.** The re-centering is entirely runtime: the march starts at the top of the relief instead of at the plane and reports a signed height. The `.prm` format, the bake, and every cache key stay byte-identical.

Read with `context/lib/resource_management.md` §4.6, `rendering_pipeline.md` §7.3, and the header comment of `crates/renderer/src/shaders/surface_depth.wgsl`.

## Decisions (owner-settled)

| # | Decision | Consequence |
|---|---|---|
| D1 | 128 (`#808080`) is the surface. | 127.5 is not representable; any other choice makes "flat mid-gray" impossible to author exactly. |
| D2 | The prefix depth `D` applies **in each direction**: black sinks `D`, white rises ~`D`. | Total span doubles (concrete: −6 to +6 texels). Step caps double. Raised-texel edge artifacts are as large as the carve. |
| D3 | Raised-texel artifacts are accepted and documented, not mitigated: flat silhouettes at polygon edges, raised floor texels cut by an adjoining wall plane, feet/props/projectiles drawing at the true plane (look sunk into raised texels). | No depth writes, no discard bounds, no offset limiting. Author docs explain where not to put strong raises. |
| D4 | The zero point lives in the shader and its CPU mirror. The bake keeps storing `G = 255 − h`. | No `.prm` change, no `STAGE_VERSION` bump, no cache-key change. Existing `.prm` files stay valid. |
| D5 | Ambient occlusion (the SH-indirect-only darkening term) is measured from the material's **peak raise**, not the plane: `ao_fraction = clamp(peak_q − s_q, 0, 1)`, then the existing `1 − STRENGTH · fade · ao_fraction`. | AO tracks local relief wherever the author put the plane: mid-gray mortar between raised stones darkens by its depth below the stone tops. An all-mid-gray map (peak 0) gets none. A carve-only map (peak 0) is byte-identical to today. AO touches only the SH indirect term; dynamic light, side-face normals and self-shadow are unaffected. |
| D6 | The march starts at the material's **peak raise**: the highest quantized raise of any texel in any uploaded mip of its surface map, computed once at load on the CPU. | A map that never exceeds mid-gray marches exactly like a pure carve. Flat mid-gray areas don't pay for raise they don't have. |
| D7 | A march that exhausts its step budget resolves **flat at the true plane** (original UV, height 0, geometric normal, top hit) — not at the last boundary crossed. | Grazing starvation looks like today's "Off" rather than smearing the texture toward the viewer. |
| D8 | The three existing `_h.png` assets keep their pixels and take on the new meaning. | Concrete stones now rise and its mortar sinks. *Amended after Track A:* Corrugated's peak quantizes to 0, so it is now carve-only with its top plateau on the plane (cheaper: 0.40× steps). Vent goes from two sunk plateaus 1 texel apart to +1 / −1 / −2 texels (1.42× steps). Owner ruling at landing: keep Vent's new relief; no content edits. |
| D9 | `texture-tool` maps diffuse mean luminance to mid-gray. | Generated maps rise and sink around the surface instead of carrying an arbitrary absolute offset. |

## Invariants (no track may break these)

**Encoding.** Stored `g ∈ [0,1]` (unorm of `G = 255 − h`). Authored `h = 255·(1 − g)`. Signed fraction:

```
s = (h − 128) / 128            // s ∈ [−1, 127/128]; positive = RAISED above the plane
```

`s` is linear in `h` (no piecewise slope). Black is exactly −1. White is 127/128, not 1. Do not "fix" this.

**Quantization.** With `L = quantize_levels` (terraces **per direction**):

```
L >= 1:  s_q = clamp(floor(s·L + 0.5) / L, −1, 1)
L == 0:  s_q = s
```

Use `floor(x + 0.5)` in BOTH CPU and WGSL. Never `round()`: WGSL rounds half to even, Rust rounds half away from zero, and an exact half-step is reachable. Mid-gray must yield `s_q == 0.0` exactly for every `L`.

**Units and sign.** Signed height in meters `height_m = s_q · depth_scale_m`, where `depth_scale_m = D · fade` (post-fade, as today). **Positive = above the plane.** The hit-result field carrying it is renamed from `depth_m` / `depth_meters` to `height_m` / `height_meters` on both sides, so no consumer keeps the old sign by accident. `world_position` moves along the view ray to the hit point; a raised hit lies toward the camera.

**Peak raise (D6).** `peak = max(0, max over every texel of every uploaded mip of s)`, then quantized with the same rule and the material's `L` when the uniform is built. Max of quantized equals quantized of max because the rule is monotonic. The peak is computed on the CPU from the slot's bytes in the renderer's texture load path, by a pure function in `postretro-render-cpu`. No surface map → peak 0. The march's ray starts at height `peak · depth_scale_m` above the plane, at the UV where the view ray crosses that height; DDA traversal from there is unchanged.

**Uniform layout.** The 32-byte material uniform keeps its size. Bytes 8..12 (first half of today's `_pad`) become `surface_depth_peak_raise: f32`, the quantized peak fraction in `[0, 1]`. Bytes 12..16 carry the trough (P1). All other offsets are unchanged. The WGSL struct and `build_material_uniform` change together.

**Gate.** The `has_depth` bit (set only for an `Rg8Unorm` slot) is now the *only* guard against the R8 placeholder: `g = 0` used to mean flat and now means maximum raise. The march must never run when `has_depth` is clear, and a test must pin that.

**Bounds.** `SURFACE_DEPTH_MAX_METERS` (0.2) and `SURFACE_DEPTH_MAX_TEXELS` bound `|height|` in each direction. The resolved `|height_m| ≤ min(depth_scale_m, SURFACE_DEPTH_MAX_METERS)`.

**Step caps.** Every `max_steps` in both prefix tables (texel and meters) doubles. Must stay ≤ `SURFACE_DEPTH_MAX_STEPS` (255). `quantize_levels` values are unchanged; their meaning becomes "per direction".

**Self-shadow.** The light-visibility march toward a dynamic light runs until the ray climbs above the peak height, not the plane. The early-out "a hit on the plane skips the shadow march" becomes "a top hit at the peak height skips it". The budget rule (`max_steps / 2`) is unchanged.

**Fade.** Distance/LOD fade and quality `Off` scale `depth_scale_m` toward zero, which flattens to the true plane.

**Unchanged hard constraints** (from the shader header): no `frag_depth` write; only `base_uv` is offset, never the lightmap UV; no derivative calls inside the snippet; no new binding or sampled texture; the face normal never reaches shadow-map receiver bias. Collision is untouched.

## Performance (owner requirement)

Doubling the span (D2) roughly doubles grazing-angle march length if nothing else changes. The owner's requirement is comparable visual results for comparable or less work. Treat cost as a first-class acceptance target, not a follow-up. Cost is measured two ways: **DDA steps** from the CPU reference (deterministic, test-gated), and **`forward` pass GPU time** from a Metal System Trace on this Mac (manual A/B; method in `rendering_pipeline.md` §12, "Without timestamp support" and "Machine-state confounders").

**P1 — Relief band.** *Track A finding:* the band does not shorten the loop (texel data already bounds it). It feeds P2, P4 and the shadow exit. The real saving is D6's peak start. At load, alongside the peak (D6), compute the material's **trough**: the lowest quantized `s` across every uploaded mip, clamped to `≤ 0`. The march's vertical extent is `[trough, peak]`, not `[−1, peak]`. A map that never goes darker than mid-gray marches only its raised band. Pack the trough into bytes 12..16 of the material uniform as `surface_depth_trough: f32` (fraction in `[−1, 0]`). With this, bytes 8..16 are both used and the uniform has no padding left.

**P2 — Single-texel early-out (exact).** *Track A finding:* step-neutral on CPU, because the loop's first iteration already resolves these rays with zero steps. It is kept as a separate branch and proven bit-identical to the loop. On GPU it may still save loop setup; the Metal trace decides whether it stays. If the view ray's UV footprint across the whole band `[trough, peak]` stays inside the starting texel, the hit is that texel's top. Resolve it with one fetch and no loop. This is exact, not an approximation, and covers the common near-perpendicular view of floors and walls.

**P3 — Band-relative shadow march.** The self-shadow march ends as soon as the ray climbs above the peak. Its budget stays as it is.

**P4 — Flat band skip.** If `peak_q == trough_q == 0`, the material marches nothing: an all-mid-gray map costs the same as having no map.

**Step-count harness (gate).** A deterministic test in `postretro-render-cpu` marches a fixed sweep of view directions × start positions across:
- the three real `_h.png` assets (see D8), decoded or reproduced exactly;
- a synthetic carve-only map;
- a synthetic all-mid-gray map;
- a synthetic ±full-range map.

It records mean steps per fragment, p99 steps, and the starve rate (share of rays that hit the cap).

**Baseline first.** The baseline numbers come from the **unmodified** march on today's encoding of the same assets, measured before the rewrite. Commit both sets of numbers into the test as pinned expectations, so a future change that raises step counts fails the test. Targets:
- carve-only and all-mid-gray maps at or below baseline;
- the three real assets' mean steps within 1.25× of baseline despite the doubled span;
- the starve rate not above baseline.

A target the levers above cannot reach is reported to the session with the numbers, not silently loosened.

**Considered, not in this change:** maximum-mipmap / quadtree acceleration (Tevs et al. 2008) needs either a new texture, and the forward pass is at 16/16 sampled textures, or max-filtered G mips, which is a bake and format change against D4. If the harness shows P1–P4 miss the targets, this is the next lever, and it goes to the owner.

**Track A result (pinned in `step_harness.rs`, 11264 rays per map):**

| Map | Baseline mean / p99 / starve | New mean / p99 / starve |
|---|---|---|
| concrete_stone_030 | 1.547 / 20 / 0.60% | 1.594 / 24 / 0.07% |
| Vent-001 | 3.431 / 14 / 0.71% | 4.868 / 22 / 0% |
| CorrugatedMetalPanel-01V_64 | 4.746 / 15 / 5.06% | 1.880 / 10 / 0% |
| synthetic carve-only | 2.993 / 23 / 1.15% | 3.130 / 24 / 0.36% |
| synthetic all-mid-gray | 0 / 0 / 0 | 0 / 0 / 0 |
| synthetic ±full-range | 2.337 / 12 / 0 | 5.598 / 23 / 0 |

Session ruling: the carve-only miss (1.046×) is accepted. At the old cap it matches the baseline exactly; the excess is formerly-starved rays now finishing under D2's doubled caps. The Vent miss is a D8 content consequence and goes to the owner.

**Track B GPU result (Metal System Trace, `forward` = Textured ∪ Kinematic pass union, one pose on `campaign-test` concrete walkway, 1280×720, 3 interleaved runs, noise ≈ ±0.2 ms):**

| Config | Mean ms/frame | Marginal over Off |
|---|---|---|
| `main` | 5.04 | +0.32 |
| branch as committed (D 6, cap 48) | 5.33 | +0.62 |
| equal relief (D 3, L 3, cap 24) | 5.17 | +0.46 |
| old cap (D 6, cap 24) | 5.27 | +0.55 |
| Off | 4.71 | 0 |

Split: D2's doubled relief ≈ 0.16 ms (the owner's priced choice). The remaining ≈ 0.13 ms is overhead of the signed march itself at equal relief. That overhead is the target of Track B2. Lowering the cap alone buys nothing measurable and introduces grazing flat patches; caps stay doubled.

**Track B2 — GPU march overhead.** Goal: the equal-relief config's marginal ≤ `main`'s marginal, with enough runs to beat the noise. Without changing any D-decision or invariant above, the levers are:
- register pressure / occupancy in the forward shader;
- P2 as a separate GPU branch (may be dropped on the GPU if it costs; the CPU keeps the authority, and parity is defined on results, not code shape);
- loop-invariant math not hoisted;
- per-fetch decode cost.

**Track B2 result (4K clock-pinned Metal System Trace, 5 interleaved rounds, median per-frame `forward` ms; supersedes the 720p table above for decisions):**

| Config | main | HEAD before B2 | after B2 (`9f099b784`) |
|---|---|---|---|
| Off | 22.83 | 23.13 | **22.51** |
| On, equal relief | 25.97 (main's On) | 26.82 | 26.19 |
| On, as committed | — | 27.06 | 26.42 |

Off now beats `main` (−0.32 ms, every round). Equal relief is +0.21 ms (+0.8%) over `main` On: target not met, accepted because the loop restructures that close it raise Off cost on this compiler, and Off is the owner's priority. GPU drops P2 (the CPU keeps it). The light march steps a folded coordinate. The view loop keeps its in-loop starved return. Owner ruling: naga bounds checks stay on (declined: `unsafe` on every backend for a fraction of the +0.8%).

## Tracks and file ownership

| Track | Model | Owns | Starts |
|---|---|---|---|
| A — CPU authority | opus | `crates/render-data/src/material.rs`, `crates/render-cpu/src/surface_depth.rs` (+ any split modules under it), `crates/render-cpu/src/material_plan.rs` | First, on this branch |
| B — GPU mirror | sonnet | `crates/renderer/src/shaders/{surface_depth,forward,kinematic_brush}.wgsl`, `crates/renderer/src/render/{loaded_texture,material_plan}.rs`, `crates/renderer/src/render/tests/surface_depth_tests.rs` | After A lands, on this branch |
| B2 — GPU overhead | opus | B's renderer files, plus `crates/render-cpu/src/surface_depth/` only if parity requires it | After B |
| C — tool + docs | session | `tools/texture-tool/`, `docs/level_design.md`, `context/lib/*`, comments in `level-format/src/prm.rs` and `level-compiler/src/texture_mips.rs` | Alongside / after |

Compile-forced spillover outside your row is allowed if minimal, and must be reported.

## Acceptance

**A.**
- `cargo test -p postretro-render-cpu --lib surface_depth` and `cargo test -p postretro-render-cpu --lib material_plan` pass with a non-zero count.
- `cargo test -p postretro-render-data --lib` passes.
- Tests cover:
  - An all-128 field is an exact no-op for every `L`, including 0.
  - An all-0 field sinks exactly `D`.
  - An all-255 field rises `127/128 · D` with `peak` set.
  - Quantization at an exact half step agrees with `floor(x + 0.5)`.
  - A starved march returns flat (D7).
  - AO follows D5: zero on a texel at the peak height, zero across an all-128 map, today's value for a carve-only map, and nonzero for mid-gray between raised texels.
  - The shadow march does not exit at the plane when the relief rises above it.
  - Peak extraction over a multi-mip `Rg8` payload returns the max raise across levels and 0 for an all-sink map.
  - `|height_m|` never exceeds the bound.

**B.**
- `cargo test -p postretro-renderer --lib surface_depth` passes with a non-zero count.
- The shader-constant pin test covers every new constant and the quantization expression.
- The naga validation and uniformity tests for `forward.wgsl` and `kinematic_brush.wgsl` pass.
- `rg -n "round\(" crates/renderer/src/shaders/surface_depth.wgsl` returns nothing.
- Metal System Trace A/B of the `forward` pass, per §12's method: `main` vs. this branch, same map, same camera, same machine state, window in front. Report per-frame `forward` time for each, the Surface Depth `Off` setting as a floor, and the machine-state readings. Delete the trace bundles and `instruments*.ktrace` temp files afterwards.
- A captured frame of the concrete floor in `maps/campaign-test.prl` at a grazing angle, looked at by the agent, if a headless or offscreen capture path is reachable. Otherwise, report why not.

**C.**
- `cargo test --manifest-path tools/texture-tool/Cargo.toml` passes.
- `docs/level_design.md` §Surface Depth carries a gradient table (black → `#808080` → white) and the D3 caveats.
- No `cargo run -p xtask` appears in `docs/`.

## Open questions

- Windows `POSTRETRO_GPU_TIMING` cost on the perf-floor GPU (GTX 1660 Super) stays an all-backends cross-check handoff. The Mac trace is the gate this session can run.
