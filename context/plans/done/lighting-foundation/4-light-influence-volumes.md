# Sub-plan 4 — Light Influence Volumes

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** Per-light bounding volumes computed in `prl-build`, stored in a new PRL section, uploaded to the GPU at level load, and consulted in the fragment light loop to early-out lights that don't touch the fragment. Also used on the CPU to gate shadow-map slot allocation against the camera frustum.
> **Crates touched:** `postretro-level-compiler` (new section writer), `postretro-level-format` (new section ID + record type), `postretro` (section parser, GPU upload, shader change, CPU frustum test).
> **Depends on:** sub-plan 3 (flat per-fragment light loop must exist before it can be optimized).
> **Blocks:** sub-plan 5 (shadow maps rely on the frustum-visibility test for slot allocation).

---

## Description

The flat light loop from sub-plan 3 evaluates every map light for every fragment. That is correct but wasteful: a torch in a distant room contributes nothing to a fragment on the player's current wall, yet the shader still runs its falloff and Lambert before the result collapses to zero.

This sub-plan attaches a **conservative bounding volume** to each light at compile time — a sphere for point and spot lights, infinite for directional — and uses those volumes as spatial early-outs:

- **Fragment shader:** skip a light (`continue`) when the fragment is outside the light's influence volume. Pixel output is bit-identical to sub-plan 3 (the falloff function already returns 0 beyond `range`), but the expensive branches are skipped.
- **CPU, at frame start:** test each shadow-casting light's influence volume against the camera frustum. Lights whose volume does not intersect the frustum do not need an active shadow-map slot this frame. This is the enabler for "many shadow-casting lights per level" — sub-plan 5 can allocate a small pool of shadow-map slots and bind them to whichever lights are actually visible.

The influence volume is an **optimization, not a correctness change.** Nothing in the direct-lighting math changes. A conservative bound must strictly contain every fragment the light can influence; a fragment rejected by the bound must also be rejected (to zero) by the falloff function, guaranteed.

---

## Compile-time computation

`prl-build` derives one influence record per `MapLight` during the light-translation stage, after `format::quake_map::translate_light` has populated canonical fields. Records are written in the **same order as the AlphaLights section (ID 18)** so index `i` in the new section corresponds to index `i` in the light buffer.

### Per-type bounds

| Light type | Bound | Derivation |
|-----------|-------|-----------|
| Point | Sphere | center = `position as f32`, radius = `falloff_range` |
| Spot | Sphere (conservative) | center = `position as f32`, radius = `falloff_range`. A cone-capped bound is tighter but the sphere is correct and simpler. Cone attenuation already handles the angular cull inside the sphere. |
| Directional | None (infinite) | Sentinel radius (`f32::MAX`) marks the light as always-active. Shader tests `radius > 1.0e30`. No spatial test at runtime. |

**Precision note.** `MapLight.origin` is `[f64; 3]` (carried from the compiler's double-precision parse stage). `InfluenceRecord.center` is `[f32; 3]`. The compiler narrows with `origin[n] as f32` when deriving the record. This is correct — a bounding sphere used for early-out culling does not require sub-millimeter precision, and the `f32` center matches the `f32` positions the GPU already uses for fragment-shader evaluation.

**Why spheres for spot lights.** A tight cone-capped bound would save a few fragments near the cone's far edge but costs a more complex CPU-side intersection test (frustum vs. cone) for the shadow-slot gate. The sphere is a provably correct superset of the cone, and the fragment shader's existing `cone_attenuation` already zeros out fragments outside the angular cutoff. Revisit if profiling shows spot lights dominate light-loop cost.

**Rationale for sphere-over-AABB.** Point-light falloff is radial. A sphere is the natural bound and its point-in-sphere test is one `dot` + one compare — cheaper than six AABB-plane tests. AABBs are preferable only when the frustum-intersection test needs to be vectorized across many lights at once. At 500 authored lights, the per-frame CPU frustum test (6 plane tests × 500 lights = 3000 dot products) is comfortably sub-microsecond on modern hardware and does not justify the added complexity of SIMD AABB batching. Revisit if light counts grow to thousands.

### Record layout

```
// Proposed design — 16 bytes per record, one per MapLight in MapData order.
struct InfluenceRecord {
    center: [f32; 3],  // world-space sphere center (unused for directional)
    radius: f32,       // sphere radius in meters; f32::MAX = no bound (always active); shader tests > 1.0e30
}
```

16 bytes matches the natural alignment for the runtime GPU upload (one `vec4<f32>` per light). Directional lights encode as `(0, 0, 0, f32::MAX)`; the shader tests `radius > 1.0e30` as its "skip the spatial test" signal.

---

## PRL section

**New section ID: 21 — `LightInfluence`.** The next available ID after `ShVolume = 20` in `postretro-level-format/src/lib.rs`. Add to the `SectionId` enum and wire through `SectionId::from_u32` and related match arms. A new module `postretro-level-format/src/light_influence.rs` owns the reader/writer. Follow `alpha_lights.rs` as a structural model (enum discriminants with `from_u8`, fixed-size record struct, section wrapper, `to_bytes`/`from_bytes`, round-trip tests) — but **do not copy its on-disk header**. `AlphaLightsSection` uses a bare `u32 record_count` header (4 bytes). `LightInfluence` uses the richer `version + record_count + record_stride + reserved` header (16 bytes) specified below, which enables forward-compatible record extension.

### Section body

```
// Proposed design
u32  version              (= 1)
u32  record_count         (= MapData.lights.len())
u32  record_stride        (= 16)
u32  reserved             (= 0, for future flags e.g. "records are AABBs")
[record_count × 16 bytes] packed InfluenceRecord array
```

16-byte header, then a flat record array. The stride field makes forward-compatible record extension trivial: add fields at the tail; a reader that encounters `record_stride > 16` must read the first 16 bytes of each record and skip the remaining `record_stride - 16` bytes rather than treating the mismatch as an error. A reader that encounters `record_stride < 16` should return a format error — no valid version of this section produces records smaller than 16 bytes.

**Ordering contract.** Record `i` describes the light at index `i` in `AlphaLights` (section 18). `prl-build` writes both sections from the same `MapData.lights` slice in the same order; the engine asserts the counts match at load time and logs an error if they diverge.

**Absence.** A map compiled before this sub-plan lands will not have the section. The engine treats its absence as "every light has an infinite bound" — the fragment shader's early-out becomes a no-op and behavior collapses to sub-plan 3. No load-time error; a `[Loader] LightInfluence section missing, no spatial culling this map` warning only.

---

## Runtime upload

At level load, after the existing `AlphaLights` parse:

1. Parse the `LightInfluence` section into `Vec<InfluenceRecord>`.
2. Assert `records.len() == lights.len()`. Mismatch is a load-time error (the map is inconsistent).
3. Pack each record into a `vec4<f32>` = `(center.x, center.y, center.z, radius)` and upload to a new storage buffer.
4. Bind the buffer alongside the existing light buffer.

The buffer is static for the level lifetime (same cadence as the light buffer).

---

## Bind group changes

Group 2 (lighting) gains one binding:

```
@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;           // existing
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>; // new — xyz center, w radius
```

Sub-plan 5 will subsequently extend group 2 with shadow-map textures and matrices; the binding numbers chosen here must leave room. Existing binding 0 stays; this adds binding 1; shadow bindings land at binding 2+.

No changes to groups 0 or 1.

---

## Fragment shader culling

The light loop from sub-plan 3 becomes:

```
// pseudocode — actual WGSL in implementation
var total_light = ambient_floor;

for (var i = 0u; i < light_count; i++) {
    let influence = light_influence[i];
    let radius = influence.w;

    // Directional lights (and any light marked infinite): no spatial test.
    if (radius != INFINITY_SENTINEL) {
        let d = frag_world_pos - influence.xyz;
        if (dot(d, d) > radius * radius) {
            continue;
        }
    }

    // ... existing per-light-type evaluation from sub-plan 3 ...
}
```

Key details:

- **Squared-distance test.** `dot(d, d) > radius * radius` avoids a `sqrt` on the reject path. The existing `length()` call inside the per-type branch is not affected — it still runs for fragments that pass the gate and need the true distance for falloff.
- **Infinity sentinel.** The shader tests `radius > 1.0e30`. The compiler writes `f32::MAX` (~3.4e38) rather than `f32::INFINITY` to avoid driver quirks with infinity arithmetic. Any plausible physical falloff range is well under 1e30 (levels are bounded to a few thousand meters), so this threshold is safely conservative. Document the sentinel constant at the shader binding site.
- **Correctness invariant.** For every light whose `falloff_range` is finite, sub-plan 3's falloff function already returns 0 when `distance > range`. The sphere test rejects a strict superset of those fragments (the sphere radius equals `falloff_range`), so no fragment that would have received non-zero contribution is skipped. This is a pure early-out.
- **Not a conformance test.** There is no acceptance criterion that pixels change. The criterion is that pixels **do not change** — see below.

---

## Shadow-map slot culling (CPU, per frame)

This is the optimization that unlocks "many shadow-casting lights per level" in sub-plan 5.

### The problem sub-plan 5 faces

Shadow maps are expensive: each shadow-casting light needs a depth texture (1024² for directional/spot, 6 × 512² for point) and a render pass. The plan targets up to **500 authored lights per level**, with potentially hundreds of shadow-casters. Allocating a slot per map-authored light does not scale — a level with 200 torches cannot afford 200 cube shadow maps resident in VRAM.

### The fix this sub-plan provides

A shadow-casting light only needs an active shadow map this frame if its **influence volume intersects the camera frustum**. A torch behind the player, in a sealed room, or beyond the far plane contributes nothing visible and can safely have its slot skipped or reused.

### The test

Sphere-vs-frustum, done on the CPU at frame start, after the view-projection matrix is updated and before the shadow-pass dispatch in sub-plan 5:

```
// pseudocode — runs on CPU, once per shadow-casting light per frame
fn light_affects_frame(light: &LightInfluence, frustum: &Frustum) -> bool {
    if light.radius > 1.0e30 {
        return true; // directional lights (f32::MAX sentinel) always contribute
    }
    for plane in &frustum.planes {
        // FrustumPlane normals are inward-facing (visibility.rs convention).
        // plane.normal · center + plane.dist < -radius ⇒ sphere fully outside.
        if plane.normal.dot(light.center) + plane.dist < -light.radius {
            return false;
        }
    }
    true
}
```

Six plane tests per light. `Frustum` is the existing type from `visibility.rs`; `extract_frustum_planes` already runs once per frame at the start of portal traversal — the same object is passed here, not a second extraction.

**Result:** a `Vec<u32>` of active-this-frame light indices, ordered by whatever priority sub-plan 5 chooses. Sub-plan 5 allocates a fixed pool of shadow-map slots (see sub-plan 5 §Shadow-slot pool sizing for concrete numbers) and binds the first-N active lights to them. Lights beyond the pool size degrade to unshadowed this frame — that's sub-plan 5's policy decision.

### Where the CPU-side test lives

A new module `postretro/src/lighting/influence.rs` (or extension of the existing `postretro/src/lighting.rs`, at author's discretion per `development_guide.md` §2) owns:

- `LightInfluence` runtime struct (center + radius), deserialized from the PRL section.
- `visible_lights(&[LightInfluence], &Frustum) -> Vec<u32>` — takes the existing `Frustum` type from `visibility.rs` directly. No new frustum type; no second extraction.

The renderer calls `visible_lights` once per frame, passing the same `Frustum` already produced by `extract_frustum_planes` for portal traversal. The returned index list is consumed by sub-plan 5.

**Not a frame-order change.** The fragment shader still walks all `light_count` lights in the storage buffer; the CPU-side test only gates which lights get a live shadow map. The shader's per-light spatial test and the CPU-side frustum test are independent optimizations at different scales.

---

## Directional lights and the infinite bound

Directional lights (sunlight) have no position and illuminate everything in their direction. They are always visible, so:

- Their `InfluenceRecord` stores `(0, 0, 0, f32::MAX)`.
- The fragment shader's spatial test is skipped (`radius > 1.0e30` is true, so the early-out branch is not entered).
- The CPU-side `visible_lights` helper returns them unconditionally (`radius > 1.0e30` fast-path).

Directional lights still consume shadow-map slots when they cast shadows — specifically, the CSM slot pool in sub-plan 5. A typical level has 0–1 directional lights, so this is not a pool-pressure concern.

---

## Acceptance criteria

- [x] `SectionId::LightInfluence = 21` added to `postretro-level-format/src/lib.rs` and wired through `from_u32`
- [x] `postretro-level-format/src/light_influence.rs` implements read/write for the section with `version`, `record_count`, `record_stride`, and packed 16-byte records
- [x] `prl-build` emits a `LightInfluence` section alongside `AlphaLights`, one record per `MapLight`, same order
- [x] Point and spot lights encode as `(position, falloff_range)`; directional lights encode as `([0,0,0], f32::MAX)`
- [x] Engine parses `LightInfluence` at level load and uploads to a new storage buffer bound at group 2 binding 1
- [x] Engine asserts record count matches light count; mismatch is a load error
- [x] Missing section is not an error — engine logs a warning and treats all lights as infinite-bound
- [x] Fragment shader skips lights whose influence volume does not contain the fragment, via squared-distance test
- [x] Directional lights bypass the spatial test via the infinity sentinel
- [x] **Pixel output is identical to sub-plan 3** on every test map (verified by visual diff at minimum; golden-image test if the harness supports it)
- [x] `visible_lights(&[LightInfluence], &Frustum) -> Vec<u32>` implemented, taking the existing `Frustum` from `visibility.rs`
- [x] `visible_lights` is called once per frame, passing the `Frustum` already produced for portal traversal — no second `extract_frustum_planes` call
- [x] Result is available for sub-plan 5 to consume (for now, just assert it runs and log the count under `RUST_LOG=debug`)
- [x] `cargo test -p postretro-level-format` covers round-trip read/write of the new section
- [x] `cargo test -p postretro` covers `visible_lights` with hand-rolled `Frustum` cases: light fully inside, fully outside, straddling a plane, `f32::MAX` sentinel (always visible)
- [x] `cargo clippy -p postretro -p postretro-level-format -p postretro-level-compiler -- -D warnings` clean

---

## Implementation tasks

1. ✅ **Format crate.** Add `SectionId::LightInfluence = 21`. Create `light_influence.rs` with `InfluenceRecord`, section header constants, and `read`/`write` functions. Use `alpha_lights.rs` as a structural model but implement the 16-byte `version + record_count + record_stride + reserved` header specified in §PRL section — not `alpha_lights.rs`'s bare `u32 count` header. Unit-test round-trip for a record array containing one of each light type (including a `f32::MAX` sentinel).

2. ✅ **Compiler.** In `postretro-level-compiler`, after the AlphaLights writer runs, derive influence records from `MapData.lights` and write the `LightInfluence` section. Shared iteration order with the AlphaLights writer is load-bearing — factor the ordering into a single iterator if needed to prevent drift.

3. ✅ **Engine load path.** Parse the `LightInfluence` section in the same load stage that parses `AlphaLights`. Assert counts match. Build `Vec<LightInfluence>` (runtime struct, distinct from the format-crate `InfluenceRecord` if alignment differs). Pack as `Vec<[f32; 4]>` and upload to a `wgpu::Buffer` with `STORAGE | COPY_DST`.

4. ✅ **Bind group.** Add binding 1 to the group 2 layout. Rebuild the group 2 bind group after the influence buffer is uploaded. Update the forward pipeline layout.

5. ✅ **Shader.** Add the `light_influence` binding. Insert the early-out at the top of the light loop. Verify visually against sub-plan 3 screenshots — no pixel should change.

6. ✅ **CPU sphere-vs-frustum.** Implement `visible_lights(&[LightInfluence], &Frustum) -> Vec<u32>` using the inward-normal `FrustumPlane` convention from `visibility.rs` (`plane.normal.dot(center) + plane.dist < -radius` ⇒ outside). Unit-test with a hand-constructed `Frustum`: light at camera origin (visible), light behind the far plane (not visible), light whose sphere straddles a side plane (visible), `f32::MAX` sentinel (always visible).

7. ✅ **Frame wiring.** Call `visible_lights` once per frame in the renderer, passing the `Frustum` already produced by `extract_frustum_planes` for portal traversal — no second extraction. Log the count at debug level. Stash the result where sub-plan 5 can pick it up (a field on the renderer's per-frame state is fine).

8. ✅ **Test maps.** Use the existing sub-plan 1 / sub-plan 3 test maps. No new content needed — the goal is no-op correctness plus a measurable reduction in the per-fragment light loop.

---

## Notes for implementation

- **No pixel changes.** The headline acceptance criterion is that this sub-plan is invisible in screenshots. If pixels move, the sphere is tighter than `falloff_range` somewhere, or the fragment test is using `>=` where it should use `>`. Diff against sub-plan 3 captures before declaring done.

- **Infinity sentinel on GPU.** The compiler writes `f32::MAX` (~3.4e38) for directional lights (and any light without a finite range) rather than `f32::INFINITY`. The shader tests `radius > 1.0e30` — a threshold comfortably above any physical falloff range (levels are bounded to a few thousand meters) but well below `f32::MAX`, so no bits-of-infinity land in shader arithmetic. Document the constant at the binding site.

- **Don't tighten the bound during this sub-plan.** The sphere-equals-`falloff_range` choice is deliberate: it matches the falloff cutoff exactly, so correctness is obvious. Cone-capped spot bounds, AABBs around attenuation thresholds (e.g., radius where intensity falls below 1/255), and other tightenings are future optimizations — each one needs its own correctness argument and visual diff. Keep this sub-plan boring.

- **Do not skip the CPU frustum test "because the fragment shader will cull anyway."** The fragment shader early-out saves ALU for fragments that were going to run the loop regardless. The CPU frustum test saves entire shadow-map passes — orders of magnitude more work. Both exist for different reasons and both must ship.

- **Ordering contract between AlphaLights and LightInfluence.** If future work reorders lights (e.g., sorts by type for better branch prediction in the loop), both sections must be reordered together in `prl-build` and the reordering must be deterministic. Factor the sort at the source (`MapData.lights`) rather than post-hoc in each writer.

- **Why not store influence data inside `GpuLight` directly?** The `GpuLight` struct is already 80 bytes and sub-plan 5 activates its last reserved slot for shadow info. Influence data is a separate concern (compile-time-derived bounds) that reads naturally as its own buffer. Keeping it separate also means a future optimization can swap the representation (AABB, cone, multi-sphere) without touching the core light struct or sub-plan 5's shadow packing.

- **Reuse, don't re-extract.** The `Frustum` built by `extract_frustum_planes` for portal traversal is the camera frustum. Pass it directly to `visible_lights` — adding a second `extract_frustum_planes` call would be redundant and slightly wrong (the portal traversal call also applies `slide_near_plane_to` to handle tight corridors; the influence test doesn't need that adjustment, but the cost of reuse is zero and the cost of divergence is a subtle bug surface). The inward-normal sign convention is documented in `visibility.rs`; the sphere test must match it.
