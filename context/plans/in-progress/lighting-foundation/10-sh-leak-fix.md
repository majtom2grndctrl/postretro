# Sub-plan 10 — SH Indirect Light-Leak Fix (Visibility-Aware Sampling)

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals, the BVH dependency, and the baker/runtime split.
> **Scope:** Three surgical fixes that stop the SH irradiance volume from bleeding exterior light through thin interior walls. Composed as one sub-plan because they ship together — each alone leaves a visible seam.
> **Crates touched:** `postretro-level-compiler` (baker) and `postretro` (runtime sampler + shader).
> **Depends on:** sub-plans 2 (baker), 6 (runtime SH sampling), and 8 (SDF atlas — reused as runtime occluder).
> **Blocks:** nothing directly, but cleaner interior lighting is a prerequisite for polished screenshots / art-review.

---

## Problem

Interior probes inside a small building inherit the exterior radiance field of the surrounding room. Symptom observed via the Alt+Shift+4 `LightingIsolation::IndirectOnly` diagnostic: the interior face of a building wall glows with the same intensity — and the same cube-shadow pattern — as the **exterior** face of that wall. The SH volume is treating thin walls as transparent.

Two compounding mechanisms produce this, both verified by reading `postretro-level-compiler/src/sh_bake.rs` and `postretro/src/shaders/forward.wgsl`:

### Mechanism A — exterior-leaf probes are baked

`probe_is_valid` (`sh_bake.rs:213`) only rejects probes in *solid* BSP leaves. Leaves that `visibility::find_exterior_leaves` classifies as "outside the playable volume" are empty-and-exterior: their probes get `validity = 1` and bake a full radiance field. The `exterior_leaves` set is computed in `main.rs:127` but never passed into `BakeInputs`.

Alone this would not produce the observed symptom (exterior-leaf probes sit outside the map), but it wastes bake budget and worsens edge artifacts when the probe grid straddles the playable boundary.

### Mechanism B — trilinear interpolation is wall-blind (the observed symptom)

The probe grid is a regular 1 m lattice (default `DEFAULT_PROBE_SPACING`). The building's south wall is thin relative to that spacing. The probes immediately on the **exterior side** of the wall sit in an empty, non-exterior BSP leaf (the big outer room) and bake correctly: they see the south light plus any cube shadows cast onto the exterior wall face.

At runtime (`sample_sh_indirect` in `forward.wgsl` — function declared ~L555 at time of writing; line numbers drift, grep the name), a fragment on the wall's *interior* face issues a hardware trilinear fetch against the SH 3D textures. The 8 corner probes of the interpolation cell include those exterior-side probes. The wall geometry plays no role in the interpolation weights. The interior surface inherits the exterior radiance field wholesale, cube shadow and all.

The bake itself is arguably correct per-probe. The leak is a **runtime sampling problem**: SH interpolation does not respect occluders.

---

## Approach

Three fixes, composed:

| Id | Fix | Where | Effect |
|----|-----|-------|--------|
| **D** | Exterior-leaf probe invalidation | `sh_bake.rs` | Zeroes out-of-map probes; hygiene; shrinks artifacts at map edges |
| **B** | Normal-offset sampling | `forward.wgsl::sample_sh_indirect` | Pushes the sample point into the interior side of the surface before lookup; cheap pre-filter |
| **A** | SDF-weighted trilinear | `forward.wgsl::sample_sh_indirect` + bind-group reuse of sub-plan 8's SDF | Down-weights corner probes whose SDF ray to the fragment is occluded; principled fix |

Rationale for shipping all three together:

- **D** is ~20 lines, zero risk, and orthogonal to **A**/**B**. It catches map-edge artifacts the other two cannot.
- **B** is a one-line shader change that costs nothing at runtime. It makes **A**'s job easier by moving the fragment away from the surface it sits on, so the SDF ray isn't perpetually self-occluded.
- **A** is the principled fix. The SDF atlas is already baked, bound to group 2 (sub-plan 8), and sphere-traced by the direct shadow path. Reusing it for indirect visibility means the runtime uses one occlusion model for both direct and indirect — no divergence.

Why not per-leaf probe masks (option C from the diagnostic write-up): invasive PRL format change, bake-time per-probe visibility bitset, and the runtime cost (fetch + mask + renormalize) is similar to 8 short SDF traces. The SDF approach reuses existing infrastructure; we revisit C only if A+B+D prove inadequate on complex levels.

---

## Shared context

### The SDF atlas is the authoritative runtime occluder

Sub-plan 8 baked the SDF atlas from the same BVH the SH baker used. Any fix that adds runtime visibility queries **must sample the SDF atlas**, not introduce a second occluder representation. The group 2 bindings (`sdf_atlas`, `sdf_sampler`, `sdf_top_level`, `sdf_meta`) are already live; this sub-plan binds them into the SH sampling path — no new GPU resources.

### SH probe grid layout is unchanged

No probe-grid resampling, no per-probe metadata added to the PRL section, no `ShVolumeSection` schema change. The fix is entirely at probe-bake-classification time (D) and at sample time (A, B). This keeps the PRL format stable and avoids cross-crate churn.

### `LightingIsolation::IndirectOnly` is the acceptance visualization

The Alt+Shift+4 chord already isolates the indirect term. Every acceptance criterion below is verifiable by booting the engine on `assets/maps/test.prl`, pressing Alt+Shift+4 twice to reach IndirectOnly, and walking the interior of the small building. This is the fastest feedback loop and the primary "did it work" check.

---

## Fix D — exterior-leaf probe invalidation

### Change

1. Extend `BakeInputs` (`sh_bake.rs:53`) with `pub exterior_leaves: &'a HashSet<usize>`.
2. Extend `probe_is_valid` (`sh_bake.rs:213`) to also reject probes whose leaf is in the exterior set:
   ```rust
   fn probe_is_valid(tree: &BspTree, exterior: &HashSet<usize>, pos: DVec3) -> bool {
       if tree.leaves.is_empty() { return true; }
       let leaf = find_leaf_for_point(tree, pos);
       if tree.leaves[leaf].is_solid { return false; }
       !exterior.contains(&leaf)
   }
   ```
3. Thread `exterior_leaves` from `main.rs:127` into `sh_bake::BakeInputs` alongside `tree` (`main.rs:168–174`).
4. Update the baker's call sites that build `validity` (`sh_bake.rs:95–98`) to pass `inputs.exterior_leaves`.

### Tests

- New unit test in `sh_bake.rs` tests module: given a 3-leaf tree with one solid leaf, one empty interior leaf, and one empty exterior leaf, probes in the interior leaf are valid; probes in solid and exterior leaves both return `validity = 0`.
- Regression: existing tests that construct `BakeInputs` get a `&HashSet::new()` for the new field and continue to pass.

---

## Fix B — normal-offset sampling

### Change

In `sample_sh_indirect`, shift the sampled world position along the shading normal before computing `cell_coord`:

```wgsl
// Push the sample point away from the surface by a small fraction of the
// probe cell. Without this, a fragment on the interior face of a wall
// lies *on* the wall, so trilinear lookup pulls equally from probes on
// both sides. Offsetting by the normal biases the fetch toward the
// interior side.
let offset_world = world_pos + normal * SH_NORMAL_OFFSET_M;
let cell_coord = (offset_world - sh_grid.grid_origin) /
                 max(sh_grid.cell_size, vec3<f32>(1.0e-6));
```

### Constants

- `SH_NORMAL_OFFSET_M`: start at `0.1` m (≈10% of the default 1 m probe spacing — large enough to matter, small enough that concave-corner fragments don't offset *through* a perpendicular wall). Tune during acceptance; document in-file.

### Rationale and limits

Normal-offset sampling is a well-known cheap trick (Frostbite, Unreal). It fixes the "fragment sits on the wall" case but is fundamentally a heuristic: at concave interior corners it can bias the fetch toward a probe that's on the far side of a perpendicular wall. Fix A catches those cases. Keep B because it's free and makes A's SDF ray start from a point that is already one normal-hop away from the surface — the sphere trace doesn't immediately self-occlude.

### Tests

No new unit tests (shader behavior). Covered by the interior-glow visual check in acceptance.

---

## Fix A — SDF-weighted trilinear

### Change

Replace the single `textureSample` per SH band in `sample_sh_indirect` with a manual 8-corner fetch, weighting each corner by SDF visibility from the offset world position to the probe's world position.

### Sampler

```wgsl
// Visibility weight for one corner probe. Returns 1.0 if the corner is
// fully visible, 0.0 if the SDF reports the segment is blocked, and
// smoothly in between (the soft-shadow formula from sub-plan 8's
// `sample_sdf_shadow`, reused verbatim for consistency with direct-
// light occlusion — `occlusion = min(occlusion, k * d / t)`).
fn sh_corner_visibility(from: vec3<f32>, corner_world: vec3<f32>) -> f32 {
    // No SDF atlas → every corner is "visible"; the weighted trilinear
    // degrades to plain trilinear. (sample_sdf returns SDF_LARGE_POS
    // when absent, so the trace would eventually terminate anyway; the
    // early-out saves ~12 fetches per corner.)
    if sdf_meta.has_sdf_atlas == 0u { return 1.0; }

    let delta = corner_world - from;
    let dist = length(delta);
    if dist < SH_VIS_MIN_DIST { return 1.0; }
    let dir = delta / dist;
    var t = sdf_meta.voxel_size_m * SH_VIS_SELF_BIAS_VOXELS;
    var occlusion = 1.0;
    let k = SH_VIS_SOFTNESS;  // fixed softness; SH has no cone angle
    for (var i = 0u; i < SH_VIS_MAX_STEPS; i = i + 1u) {
        let p = from + dir * t;
        let d = sample_sdf(p);  // shared with sub-plan 8
        if d < sdf_meta.voxel_size_m * SH_VIS_HIT_EPSILON_VOXELS {
            occlusion = 0.0;
            break;
        }
        occlusion = min(occlusion, k * d / t);
        t = t + max(d, sdf_meta.voxel_size_m * SH_VIS_MIN_STEP_VOXELS);
        if t > dist { break; }
    }
    return clamp(occlusion, 0.0, 1.0);
}
```

### Weighted trilinear

```wgsl
// 8 corners of the grid cell containing `cell_coord`.
let c_floor = floor(cell_coord);
let c_frac  = cell_coord - c_floor;
let base_i  = vec3<i32>(c_floor);

var band_accum: array<vec3<f32>, 9>;  // one vec3 per band
for (var i = 0; i < 9; i = i + 1) { band_accum[i] = vec3<f32>(0.0); }
var weight_sum: f32 = 0.0;

for (var dz = 0; dz < 2; dz = dz + 1) {
    for (var dy = 0; dy < 2; dy = dy + 1) {
        for (var dx = 0; dx < 2; dx = dx + 1) {
            let ci  = base_i + vec3<i32>(dx, dy, dz);
            let tri = vec3<f32>(
                select(1.0 - c_frac.x, c_frac.x, dx == 1),
                select(1.0 - c_frac.y, c_frac.y, dy == 1),
                select(1.0 - c_frac.z, c_frac.z, dz == 1),
            );
            let tri_w = tri.x * tri.y * tri.z;

            let corner_world =
                sh_grid.grid_origin + vec3<f32>(ci) * sh_grid.cell_size;
            let vis = sh_corner_visibility(offset_world, corner_world);
            let w = tri_w * vis;

            // point-sample each band texture at the corner
            let uvw = (vec3<f32>(ci) + vec3<f32>(0.5)) /
                      vec3<f32>(sh_grid.grid_dimensions);
            band_accum[0] += w * textureSampleLevel(sh_band0, sh_sampler, uvw, 0.0).rgb;
            // ...bands 1..=8 identically (9 bands total) — see sh_irradiance

            weight_sum += w;
        }
    }
}

// Fallback: if every corner is occluded, weight_sum == 0. Fall back to
// the ambient floor (caller adds this) rather than injecting NaNs.
if weight_sum < SH_VIS_WEIGHT_EPS {
    return vec3<f32>(0.0);
}
let inv_w = 1.0 / weight_sum;
for (var i = 0; i < 9; i = i + 1) { band_accum[i] *= inv_w; }
```

Then reconstruct via the existing `sh_irradiance(...)` call passing `band_accum[0]`..`band_accum[8]` as the 9 band arguments (plus the shading normal).

### Starting constants

All bias/step/epsilon constants are expressed as **voxel multiples** of
`sdf_meta.voxel_size_m`, matching sub-plan 8's `sample_sdf_shadow`
pattern (forward.wgsl — see the `self_shadow_bias = voxel_size_m * 2.0`
family). This way retuning the SDF voxel size automatically retunes the
SH visibility trace.

- `SH_VIS_MAX_STEPS`: 12 (shorter than sub-plan 8's 32; SH segments are at most one cell diagonal)
- `SH_VIS_SELF_BIAS_VOXELS`: 0.5 (half a voxel — segments are short, don't need sub-plan 8's 2.0-voxel bias)
- `SH_VIS_MIN_STEP_VOXELS`: 0.5
- `SH_VIS_HIT_EPSILON_VOXELS`: 0.25 (matches sub-plan 8)
- `SH_VIS_MIN_DIST`: 0.01 m (absolute — skip degenerate same-point case)
- `SH_VIS_SOFTNESS` (`k`): 16.0 — harder edge than direct shadows; indirect benefits from decisive "is the probe visible or not"
- `SH_VIS_WEIGHT_EPS`: 1.0e-4

All constants live at the top of the WGSL module with comments explaining their derivation. Tune during acceptance on the building test map (see §Acceptance below).

### Rationale

- **Why point-sample (not hardware-filter) at the corners?** Because the corner lookup must be exact — we are computing our own trilinear weights from visibility. Hardware trilinear would double-interpolate.
- **Why fallback to 0 when fully occluded?** The ambient floor (added by the caller) provides the baseline for "no visible indirect light." Any other choice (nearest-visible probe, reflect into interior SDF) is a future enhancement; the `max(..., 0.0)` clamp on the caller side is unchanged.
- **Why `textureSampleLevel(..., 0.0)`?** Explicit mip 0. SH textures have a single mip; the explicit level avoids derivative-dependent sampling that breaks inside conditionals.
- **Cost.** 8 sphere traces × ≤12 steps = ≤96 SDF fetches per fragment, plus 72 point-sampled texture fetches (9 bands × 8 corners). Budget: fits inside the sub-plan 8 "1–2 ms total shadow" envelope on test scenes; measure on the first real map. If over budget, the escape hatch is to skip the per-corner visibility when **all 8 corners' SDF distance from the fragment's offset_world > `SH_VIS_SKIP_DIST`** — a one-line early-out.

### Tests

- No new shader-level unit tests (WGSL is integration-tested against actual GPU execution).
- Existing `render::sh_volume::tests::sh_l2_reconstruction_preserves_directional_preference` continues to pass — the reconstruction math is unchanged.
- New Rust-side test in `render/sh_volume.rs`: verify the SDF bindings (group 2) are accessible from the forward-shader pipeline layout after this change. Primarily a compile-time check — if the bind-group wiring broke, the pipeline fails to create at startup, and the existing renderer bootstrap test catches it.

---

## Interaction with animated SH (sub-plan 7)

`sample_sh_indirect` mixes base SH with animated-layer contributions in the
block starting at roughly L577 (grep for `anim_count` / "animated-light"
comment). The animated layers are sampled by manual 8-corner trilinear
inside `sample_anim_mono_band` (around L519 — the helper is a separate
function from the base SH texture fetches).

Two options:

1. **Apply the same visibility weighting to animated layers.** Cleanest; animated lights benefit from the same leak fix. Extra cost: reuses the same 8 per-corner visibilities computed above, no new SDF traces.
2. **Leave animated layers on plain trilinear.** Ship the fix for base SH only, revisit animated as a follow-up.

**Decision: option 1**, with a caveat on sharing. Base SH reads from **9
3D textures** (one per band); animated SH reads from a **storage buffer**
indexed by light and probe — the two fetch paths are structurally
different and can't share a single "fetch one corner's coefficients"
helper. What **can** be shared is the visibility computation: compute the
8 `(world_position, visibility_weight)` pairs once, then pass the weights
to two separate helpers (one for base-SH textures, one for animated-SH
storage). The implementation task list below reflects this split.

---

## Bind group changes

None. The SDF atlas (group 2) and SH textures (group 3) are both declared
at WGSL module scope in `forward.wgsl` and are both reachable from
`fs_main`. WGSL module-scope bindings are visible from any fragment-stage
function; calling `sample_sdf` from inside `sample_sh_indirect` is
already syntactically legal today. No pipeline-layout edits, no new GPU
resources.

---

## Acceptance criteria

- [ ] **Fix D:** `BakeInputs` carries `exterior_leaves`; `probe_is_valid` rejects exterior-leaf probes; unit test pins the behavior.
- [ ] **Fix B:** `sample_sh_indirect` offsets by `normal * SH_NORMAL_OFFSET_M` before computing grid UV; constant is named, documented, and at the top of the WGSL module.
- [ ] **Fix A:** `sample_sh_indirect` replaces the single trilinear fetch with a visibility-weighted 8-corner fetch; all constants named and documented; full-occlusion path returns `vec3(0.0)`.
- [ ] Animated SH layers (sub-plan 7) use the same 8-corner visibility weights — no asymmetric leak path.
- [ ] **Visual (primary):** boot the engine on the map that reproduces the bug — the small-building scene (candidates: `assets/maps/test.prl`, `assets/maps/occlusion-test.prl`; the implementing agent should confirm which map contains the small interior building and note it in the commit message). In `LightingIsolation::IndirectOnly` mode, stand inside the building facing the south wall. **The rectangular cube-shadow pattern cast by the exterior test cube onto the exterior south wall is no longer visible on the interior face.** This is the objective "did it work" check — the artifact was a sharp, recognizable shape before; after the fix, it is absent.
- [ ] **Visual (regression):** `LightingIsolation::Normal` mode renders the building interior with plausible indirect light — no "everything went pitch black" regression. Compare against a screenshot taken before the change.
- [ ] **Visual (regression):** `LightingIsolation::DirectOnly` is pixel-indistinguishable from pre-change — the direct path is untouched by this sub-plan; any visible delta indicates a regression introduced by the bind-group / uniform-layout work.
- [ ] `cargo test -p postretro-level-compiler` passes.
- [ ] `cargo test -p postretro` passes.
- [ ] `cargo clippy -p postretro-level-compiler -- -D warnings` clean.
- [ ] `cargo clippy -p postretro -- -D warnings` clean.
- [ ] No change to the `ShVolumeSection` PRL format version.

---

## Implementation tasks

1. **Fix D — baker-side exterior-leaf invalidation.** Extend `BakeInputs`, thread `exterior_leaves` in, update `probe_is_valid`, add unit test. `postretro-level-compiler` only.

2. **Fix B — normal-offset shader change.** Add `SH_NORMAL_OFFSET_M` and the one-line offset in `sample_sh_indirect`. Verify `LightingIsolation::IndirectOnly` still renders (no NaNs, no black screen).

3. **Fix A part 1 — introduce a weights-taking fetch path for base SH.** Replace the single `textureSample` per band with a manual 8-corner loop that accepts an 8-entry visibility-weight array (all-1s as a placeholder before fix A part 2 lands). Because hardware trilinear and manual 8-tap `textureSampleLevel` differ at the ULP level (sampler clamping + FP ordering), don't expect byte-identical output to the pre-refactor version — the acceptance bar is **visually indistinguishable** in `Normal` mode on the test map (eyeball spot-check; no per-pixel diff needed). Leave `sample_anim_mono_band` alone in this task.

4. **Fix A part 2 — add SDF visibility computation.** Implement `sh_corner_visibility` (including the `has_sdf_atlas == 0u` early-out). In `sample_sh_indirect`, compute the 8 `(corner_world, weight)` pairs once from the offset world position, then pass those weights into the base-SH fetch helper from task 3. Tune constants in `IndirectOnly` mode against the small-building map until the acceptance-criteria visual bar is met. Record any deviations from the starting constants in the WGSL comments.

5. **Fix A part 3 — apply the same weights to the animated-SH path.** Extend `sample_anim_mono_band` (or its caller) to accept the 8 weights computed in task 4 and multiply them into its existing trilinear weights before the per-band accumulation. Do **not** recompute visibilities — reuse the values from task 4.

6. **Performance sanity.** On `test.prl`, measure frame time with and without fix A enabled (temporary compile-time feature or a literal `false` gate). If the delta is > 1.5 ms, add the `SH_VIS_SKIP_DIST` early-out described in fix A's rationale.

7. **Verification pass.** Run the acceptance visual checks. Run `cargo clippy` on both crates. Commit.

---

## Notes

- **Why not flood-fill the probe grid?** A per-room flood fill at bake time would classify probes by reachability without SDF queries. It's a valid C-option, but (a) it requires a new PRL field per probe or a per-probe leaf tag, (b) trilinear interpolation still needs to know how to handle same-cell probes from different rooms, which is exactly the problem fix A solves at runtime. We get more for less by reusing the SDF.
- **Interior-wall AO as a follow-up.** Once fix A is in, extending the 8-corner SDF traces to short-range occlusion cones gives cheap AO. Not in scope; worth noting as the natural next feature, same as sub-plan 8's closing note.
- **Self-shadow bias must be resolution-aware.** If the SDF voxel size changes (sub-plan 8 tunes to 12 or 16 cm on real maps), retune `SH_VIS_SELF_BIAS` and `SH_VIS_HIT_EPSILON`. The two are coupled; the forward shader's sub-plan 8 trace already has this tuning pattern — follow it.
- **Map-author impact.** Zero. Existing `.map` sources bake into cleaner indirect without any authoring change. The fix is entirely in the baker's classification step and the runtime sampler.
- **Not a PRL format change.** Keep it that way. If a future investigation concludes per-leaf masks are needed anyway, that's a separate sub-plan with its own format-version bump discussion.
- **Fix D changes baked probe counts.** Existing `sh_bake::tests::bake_is_deterministic` passes an empty `HashSet` for `exterior_leaves` and should continue to pass. But any golden-file fixtures or integration tests that compare SH output byte-for-byte against a checked-in reference may drift — grep the repo for references to `ShVolumeSection` in tests and re-bake any fixtures that include real exterior leaves. Spot-check before committing.
- **Map identification.** At the time of writing, the author reproduced the leak in a scene with a small building in the northwest corner of a larger room, with a white exterior light to the south and a red exterior light to the east. The implementing agent should verify which map file under `assets/maps/` renders that scene (likely `test.prl` or one of the `test-*.prl` variants) before running acceptance.
- **Tuning diagnostic (optional).** A throwaway debug mode that visualizes `weight_sum` as grayscale (black = fully occluded corners, white = fully visible) is cheap to add and extremely useful while tuning the visibility constants. Can be wired as a temporary uniform flag and ripped out before commit, or shipped as a new Alt+Shift+N chord per the existing diagnostic-chord pattern in `src/input/diagnostics.rs` if the author wants it long-term.
