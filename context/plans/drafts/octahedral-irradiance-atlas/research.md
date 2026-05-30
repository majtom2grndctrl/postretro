# Research notes — Octahedral Irradiance Atlas

Background investigation behind the spec. Not a contract; decisions live in `index.md`.

## Problem framing

SH volumes are among the most expensive renderer passes. The cost is structural, not the SH order or texture format: the hot path does in ALU what GPU texture units do for free. Current sampler (`sh_sample.wgsl::sample_sh_indirect_corners_pair`) loads nine `Rgba16Float` 3D band textures per corner with `textureLoad` and reconstructs L2 irradiance per corner — 8 reconstructions + ~72–80 fetches per fragment.

Constraints: wgpu; pre-RTX late-GTX (Pascal/Turing) — no RT cores, but fp16 + compute fine. Baked-over-computed architectural pillar holds: octahedral sampling needs no ray tracing because the volume is baked. NVIDIA documents the bake-offline / load-static workflow as the supported non-RT path, which matches the engine's pipeline.

## Why octahedral (the key insight)

On the **SH** encoding, hardware filtering and per-probe Chebyshev visibility are mutually exclusive — per-probe weighting requires per-probe access, which a single hardware-filtered tap precludes.

Octahedral resolves it and grants all three goals at once:

| Goal | SH | Octahedral |
|---|---|---|
| Hardware filtering | only via aggressive single-tap | within-tile bilinear, always |
| Less ALU | reconstruct-once helps | SH reconstruction eliminated |
| Per-probe Chebyshev by default | blocks hardware filtering | preserved (8-probe loop) |

Octahedral keeps the 8-probe loop (per-probe weights survive) but lets hardware bilinear do the *directional* reconstruction inside each probe's tile. McGuire/DDGI: ~16 coherent bilinear fetches + ≤9 divisions per sample, "extremely fast." Octahedral is also non-negative (no SH ringing).

## Alternatives considered, not chosen

- **Reconstruct-once on SH (blend coeffs, then one eval).** ~8× SH-ALU cut, no bake change. Rejected as primary: throwaway once octahedral lands (octahedral deletes SH reconstruction). Was the interim option; user chose octahedral-only.
- **Aggressive single-tap SH (hardware trilinear across the grid).** Biggest fetch cut, but drops per-probe Chebyshev — i.e. on SH, "hardware filtering" *is* the aggressive path. Kept the concept as the runtime toggle, but on octahedral it reduces to "skip Chebyshev," a clean uniform branch.
- **L1 SH + Geomerics non-negative reconstruction.** ~half ALU, 56% memory, near-L2, no ringing. Viable cheaper encoding, but a lateral move vs octahedral and still per-pixel-reconstruction-bound. Available later as an L2→L1 runtime truncation if wanted.
- **Ambient cube (Bevy/HL2).** Cheapest (3 MADs) but boxy gradients and no per-probe visibility. Floor option only.
- **Ambient Dice / Spherical Gaussians.** Raise per-pixel cost to *exceed* L2 quality — wrong direction for a perf goal. Revisit only for glossy indirect.

Shipping context: TLOU2/Frostbite kept L2 SH but paid with bake-time windowing + shader de-ringing; HL2/Source split per-pixel lightmaps (static) vs per-vertex/ambient-cube (dynamic); DDGI/RTXGI chose octahedral irradiance specifically to move directional reconstruction onto texture units and avoid SH ringing.

## Visibility is isotropic today (scope boundary)

Confirmed against source: the shipped Chebyshev stores **one `(E[d], E[d²])` pair per probe** (`ShProbe.mean_distance`/`mean_sq_distance`), and `sh_corner_depth_visibility` compares `length(sample_world - probe_world)` to the single per-probe mean — distance-based, **not** directional. So the migration changes irradiance encoding only; visibility data is reused unchanged. Full DDGI directional octahedral depth would be an upgrade (more data/cost), scoped out as future.

## Fog interaction

Fog (`fog_volume.wgsl::sample_sh_fog_dual`) explicitly disables Chebyshev (`rendering_pipeline.md`: "Chebyshev depth visibility stays off for fog"). Fog's directional response comes from the dual-direction SH read mapped to a Henyey-Greenstein `scatter_bias`, not from Chebyshev. Therefore the aggressive (skip-Chebyshev) toggle does not affect fog. The octahedral sampler must still serve fog's two-direction read from one 8-probe iteration.

## M9 reconciliation

Milestone 9 (diffuse GI upgrade) is shipped: probe-weight correctness, depth/visibility atlas bake, depth-aware Chebyshev interpolant, directional fog (`context/plans/done/M9--*`). This spec is a perf re-encoding on top, not an M9 sub-spec.

`sdf-per-light-shadows` merged into the branch (`e362975`). Verified compatible — no spec task invalidated. Two contract changes it introduced, now reflected in `index.md`:
- **Delta SH is indirect-only** (bounce). `bake_probe_direct_rgb` was removed; the animated light's direct term lives in `lm_anim`. The octahedral delta (Task 2) must encode bounce only.
- **`delta_scale` retired** — compose adds delta at full weight; the `GridDims` uniform field is now padding (Task 4).

All other grounding confirmed unchanged: `RAYS_PER_PROBE=256`, `sh_basis_l2`/`apply_cosine_lobe_rgb`/`bake_probe_rgb_with_moments`, `ShProbe`/`PROBE_STRIDE=116`, `SectionId::ShVolume=20`/`DeltaShVolumes=27`, `AFFINITY_FACTOR=4` CSR structure, bind group 3 indices, isotropic per-probe Chebyshev, fog dual read, 5 timing pairs (none for SH). Note: "K=4" in the merge is the per-light SDF visibility slot count (forward pass), unrelated to the 4×4×4 delta affinity cells.

## wgpu / hardware notes

- `Rgba16Float` / `Rg16Float` are filterable by default in wgpu; `float32-filterable` gates only 32-bit float formats. Hardware `textureSample`/`textureSampleLevel` needs no feature flag here.
- Pascal/Turing have fixed-function bilinear/trilinear in the texture units and a unified L1/texture cache; a filtered tap overlaps memory latency and beats N manual `textureLoad`s even before the ALU saving.
- Memory follow-up only: `Rg11b10Float`/`Rgb9e5Ufloat` (filterability adapter-dependent) or `texture-compression-bc-sliced-3d` BC6H (no Z compression, RGB-only) — validate per adapter; conflicts with the default-filterable assumption.

## Sources

- Majercik et al., "Dynamic Diffuse Global Illumination with Ray-Traced Irradiance Fields," JCGT 2019 — https://www.jcgt.org/published/0008/02/01/
- "Scaling Probe-Based Real-Time Dynamic GI for Production," JCGT 2021 — https://jcgt.org/published/0010/02/01/
- McGuire, DDGI overview — https://morgan3d.github.io/articles/2019-04-01-ddgi/overview.html
- NVIDIA RTXGI-DDGI `Irradiance.hlsl` — https://github.com/NVIDIAGameWorks/RTXGI-DDGI
- Hazel, SH radiance→irradiance / non-negative L1 — https://grahamhazel.com/blog/2017/12/22/converting-sh-radiance-to-irradiance/
- Valve, HL2/Source shading (ambient cube) — https://cdn.fastly.steamstatic.com/apps/valve/2004/GDC2004_Half-Life2_Shading.pdf
- Bevy irradiance volumes (ambient cubes, wgpu) — https://docs.rs/bevy/latest/bevy/pbr/irradiance_volume/index.html
- wgpu TextureFormat / Features — https://docs.rs/wgpu/latest/wgpu/enum.TextureFormat.html
