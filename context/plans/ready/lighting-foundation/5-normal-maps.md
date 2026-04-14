# Sub-plan 5 — Normal Maps

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** Normal map loading, TBN reconstruction in the vertex shader, tangent-space normal perturbation in the fragment shader. No new lighting math — this sub-plan changes which normal the existing lighting evaluates against.
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 3 (direct lighting must be working to validate normal map effects visually).
> **Blocks:** nothing directly. Sub-plans 6–7 are independent. Normal maps improve both direct and indirect shading once both are running.

---

## Description

Add tangent-space normal maps to the world rendering pipeline. Each material pairs an albedo (base color) texture with an optional normal map. The vertex shader reconstructs the TBN matrix from the octahedral-encoded normal, packed tangent, and bitangent sign already present in the `WorldVertex` format (shipped in Milestone 3.5). The fragment shader samples the normal map, decodes the tangent-space normal, transforms it to world space via TBN, and uses it as the shading normal for all lighting terms.

This is a **surface detail** feature. It does not add new lights or change the lighting equation — it changes the per-fragment normal that the existing light loop (sub-plan 3) and future indirect sampling (sub-plan 6) evaluate against.

---

## Normal map loading

### Per-material texture pair

Each material in the texture set gains an optional normal map alongside its existing base color texture. The loader looks for a normal map using a naming convention (e.g., `texture_name_normal.png` or `texture_name_n.png`) relative to the base texture.

| Case | Behavior |
|------|----------|
| Normal map found | Load, upload as a second texture in the per-material bind group |
| Normal map missing | Bind a 1×1 neutral normal map (`(128, 128, 255)` → tangent-space `(0, 0, 1)`) |

The neutral fallback means the shader path is identical for all materials — no branching on "has normal map." Materials without a normal map simply get no perturbation.

### Texture format

- **Preferred:** `Bc5RgUnorm` (two-channel block compression). Stores X and Y; Z is reconstructed in the shader via `z = sqrt(1 - x² - y²)`. Requires the `TEXTURE_COMPRESSION_BC` device feature. On desktop Vulkan/DX12/Metal this is universally available.
- **Fallback:** `Rg8Unorm` (uncompressed two-channel). Same shader reconstruction. Used if BC compression is unavailable.
- **Not used:** `Rgba8Unorm` with precomputed Z. Wastes two channels and 2× the memory for no quality benefit.

Normal map values are stored in [0, 1] and remapped to [-1, 1] in the shader: `n.xy = texel.rg * 2.0 - 1.0`.

---

## Bind group changes

Extend **group 1 (per-material)** with the normal map texture:

```
@group(1) @binding(0) var base_texture: texture_2d<f32>;     // existing
@group(1) @binding(1) var base_sampler: sampler;             // existing
@group(1) @binding(2) var normal_texture: texture_2d<f32>;   // new
```

The normal map shares the base sampler (same filtering, same wrap mode). No separate sampler needed.

This changes the per-material bind group layout, which means all material bind groups are recreated. The forward pipeline layout must be updated to match.

---

## Vertex shader: TBN reconstruction

The `WorldVertex` already carries octahedral-encoded normal and packed tangent with bitangent sign (Milestone 3.5). The current forward shader decodes these into `world_normal`, `world_tangent`, and `bitangent_sign` but does not use them. This sub-plan activates them.

The vertex shader passes the TBN components to the fragment shader as interpolated varyings:

```wgsl
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_tangent: vec3<f32>,
    @location(3) bitangent_sign: f32,
    @location(4) world_position: vec3<f32>,
}
```

`world_position` is already needed for the light loop (sub-plan 3). The only new varyings are `world_normal`, `world_tangent`, and `bitangent_sign` — all three are already computed in the vertex shader but were discarded.

---

## Fragment shader: normal map sampling

```wgsl
// Sample normal map (RG channels only)
let normal_sample = textureSample(normal_texture, base_sampler, in.uv);
var tangent_normal: vec3<f32>;
tangent_normal.x = normal_sample.r * 2.0 - 1.0;
tangent_normal.y = normal_sample.g * 2.0 - 1.0;
tangent_normal.z = sqrt(max(1.0 - tangent_normal.x * tangent_normal.x - tangent_normal.y * tangent_normal.y, 0.0));

// Reconstruct TBN matrix
let N = normalize(in.world_normal);
let T = normalize(in.world_tangent);
let B = cross(N, T) * in.bitangent_sign;
let TBN = mat3x3<f32>(T, B, N);

// Transform to world space
let shading_normal = normalize(TBN * tangent_normal);
```

`shading_normal` replaces the geometric normal in all lighting calculations — both the direct light loop (sub-plan 3) and future indirect SH sampling (sub-plan 6).

**Normalization after interpolation** is important. Interpolated normals and tangents are not unit-length after rasterization; `normalize()` corrects this.

---

## Acceptance criteria

- [ ] Normal map loader: discovers normal maps by naming convention alongside base textures
- [ ] Missing normal maps fall back to 1×1 neutral `(128, 128, 255)` texture
- [ ] Normal maps uploaded as `Bc5RgUnorm` when `TEXTURE_COMPRESSION_BC` is available, `Rg8Unorm` otherwise
- [ ] Per-material bind group (group 1) extended with normal map texture binding
- [ ] Vertex shader passes `world_normal`, `world_tangent`, `bitangent_sign` as varyings (activating existing dead code)
- [ ] Fragment shader samples normal map, reconstructs Z, transforms via TBN to world space
- [ ] Shading normal used in the direct light loop (replaces geometric normal)
- [ ] Materials without a normal map render identically to pre-normal-map behavior (neutral fallback produces no perturbation)
- [ ] Test map with normal-mapped surfaces shows correct lighting response across viewing angles and light positions
- [ ] `cargo test -p postretro` passes
- [ ] `cargo clippy -p postretro -- -D warnings` clean

---

## Implementation tasks

1. Implement normal map discovery: scan texture set for `_normal` / `_n` suffix variants alongside each base texture.

2. Load normal maps into GPU textures (`Bc5RgUnorm` preferred, `Rg8Unorm` fallback). Create 1×1 neutral fallback texture.

3. Extend per-material bind group layout and bind group creation to include normal map at binding 2.

4. Update forward pipeline layout to match the new group 1 layout.

5. Activate TBN varyings in the vertex shader output (remove dead-code path, wire through to fragment shader).

6. Fragment shader: sample normal map, decode tangent-space normal, reconstruct TBN, compute world-space shading normal, use in light loop.

7. Author or extend test map with normal-mapped surfaces under varied lighting. Validate visually: rotate camera around a normal-mapped wall under a point light and spot light, confirm detail responds correctly to light direction.

---

## Notes for implementation

- **OpenGL vs. DirectX normal map convention.** OpenGL convention: +Y points up in tangent space. DirectX convention: +Y points down. Most tools export DirectX convention. If normal maps look inverted (lighting responds backwards on one axis), flip the Y channel in the shader: `tangent_normal.y = -tangent_normal.y`. Determine the convention from the test textures and document it.
- **BC5 requires a feature check.** Call `adapter.features().contains(Features::TEXTURE_COMPRESSION_BC)` at startup. On desktop this is always available; on WebGPU/web it may not be. The fallback to `Rg8Unorm` is seamless — same shader code, just uncompressed.
- **No mipmapping in the initial cut.** Normal maps benefit from mipmapping (avoids aliasing at distance), but the current pipeline has no mip generation. Add mipmapping as a follow-up if aliasing is visible.
