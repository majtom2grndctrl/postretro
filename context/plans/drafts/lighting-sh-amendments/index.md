# Lighting — SH Baker Amendments + Leak Fix

> **Status:** draft.
> **Depends on:** `lighting-dynamic-flag/` (compiler task needs `MapLight.is_dynamic`). `lighting-old-stack-retirement/` should ship first.
> **Concurrent with:** `lighting-lightmaps/`, `lighting-chunk-lists/`, `lighting-spot-shadows/`.
> **Related:** `context/plans/in-progress/lighting-foundation/2-sh-baker.md` (existing baker, amended here) · `context/plans/in-progress/lighting-foundation/6-sh-volume.md` (existing runtime sampling, amended here) · original `lighting-foundation/10-sh-leak-fix.md` (partially superseded — SDF-weighted trilinear component dropped).

---

## Context

Two SH corrections are needed now that the stack has changed:

1. **Static-only filter.** The SH baker currently bakes from all lights. Dynamic lights — which evaluate fully at runtime — must not bake into SH probes or they double-count: both a baked indirect contribution and a runtime direct contribution from the same light. The fix is a one-line filter in the baker.

2. **Exterior-probe invalidation + normal-offset sampling.** The original `lighting-foundation/10-sh-leak-fix.md` specified three composed fixes for SH bleed through thin walls. One of the three (SDF-weighted trilinear sampling) is gone with SDF. The other two are still needed and are implemented here: exterior-leaf probes marked invalid at bake time; sample position offset along the surface normal at runtime.

Both corrections are small, independent of the lightmap and specular reworks, and have parallel compiler and runtime workstreams.

---

## Goal

- **Compiler:** SH baker excludes dynamic lights; exterior-leaf probes are marked invalid in the validity mask.
- **Runtime:** SH sample position is offset along the surface normal to reduce bleed through thin walls.

No new PRL sections. No new bind groups. Changes are amendments to existing bake and shader code.

---

## Concurrent workstreams

Both tasks can start simultaneously and do not depend on each other's output.

```
Task A (compiler): baker filter + exterior invalidation ─── independent
Task B (runtime): normal-offset SH sampling ────────────── independent
```

---

## Task A — SH baker: static-only filter + exterior-probe invalidation

**Crate:** `postretro-level-compiler` · **Existing SH baker module** (from `lighting-foundation/2-sh-baker.md`).

**Fix 1: Static-only filter.** In the per-probe radiance accumulation loop, skip any `MapLight` with `is_dynamic == true`. Dynamic lights will contribute fully at runtime via the direct loop; baking them into SH would add their contribution twice.

**Fix 2: Exterior-probe invalidation.** At bake time, classify each probe as interior or exterior. A probe is exterior if its position is inside a BSP void leaf — the same leaf-type used to seal the level interior from the outside. Mark exterior probes invalid in the existing probe validity mask. The runtime already respects the validity mask (from the original sub-plan 6 implementation); this extends the set of probes it skips.

Classification strategy: ray-cast from the probe position upward (or in multiple directions) against the BSP leaf structure to determine if the probe is in an interior leaf. Implementation details resolved during implementation, but the BSP leaf types from the existing PRL format are the data source — no new bake infrastructure needed.

### Task A acceptance gates

- Compiling a test map with a flagged dynamic light produces an SH section where that light's direct irradiance is absent from all probes (compare probe values against the no-that-light baseline — must match within numerical noise).
- A test map with known exterior probe positions (probes placed outside the sealed hull) shows those probes marked invalid in the validity mask.

---

## Task B — Runtime: normal-offset SH sampling

**Crate:** `postretro` · **Existing SH sampling path in** `src/shaders/forward.wgsl`.

When sampling the SH volume for a fragment, offset the sample world-space position along the surface normal by a small factor before the trilinear probe lookup:

```wgsl
let sh_sample_pos = in.world_position + normal_mapped * SH_NORMAL_OFFSET;
let indirect = sample_sh_indirect(sh_sample_pos, normal_mapped);
```

`SH_NORMAL_OFFSET` is a fraction of the probe grid spacing (default: half a probe spacing — typically 0.5 m at the standard 1 m grid). This pushes the sample position away from the surface toward the lit side, reducing the contribution of probes on the far side of thin walls.

The offset uses the normal-mapped normal (not the flat mesh normal) so it responds to surface detail.

### Task B acceptance gates

- A two-room test case (rooms separated by a thin wall, one well-lit, one dark) shows reduced SH bleed into the dark room after the offset is applied versus the pre-fix baseline. Visual comparison is sufficient.
- SH sampling is visually unchanged on surfaces in open, well-lit spaces (the offset does not introduce visible seams or dark bands on normal geometry).

---

## Acceptance Criteria (both tasks)

1. `cargo test --workspace` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Task A and Task B acceptance gates above.
5. No regression on existing SH visual tests when both fixes are active.

---

## Out of scope

- SDF-weighted trilinear SH sampling — dropped with SDF retirement; not in scope here or anywhere.
- Animated SH layers — remains deprioritized to a future milestone.
- Full DDGI-style dynamic probe updates.
- Three-bounce indirect.
