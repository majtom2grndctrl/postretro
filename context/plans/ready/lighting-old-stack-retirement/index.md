# Lighting Old-Stack Retirement — Remove SDF and CSM

> **Status:** ready — standalone plan, ships first.
> **Part of:** the lighting stack rework. Sibling plans: `lighting-dynamic-flag/`, `lighting-compiler-rework/`, `lighting-runtime-rework/`.
> **Supersedes:** `context/plans/in-progress/lighting-foundation/` sub-plans 5 (CSM) and 8 (SDF shadows) — both complete but retired by this plan.
> **Related:** `context/lib/rendering_pipeline.md` §4.

---

## Context

The lighting foundation shipped two runtime shadow systems: SDF sphere-traced shadows for point and spot lights (sub-plan 8) and CSM for directional lights (sub-plan 5). Both are being replaced — SDF by baked directional lightmaps for static point/spot shadows plus a small runtime spot shadow map pool for dynamic spots; CSM by baking sun contribution into the lightmap (no runtime directional shadows in this iteration).

This plan retires both old systems as a coordinated deletion. Ships first so the rest of the rework builds against a clean foundation without overlapping shadow systems.

**Intermediate state.** Between when this plan ships and when `lighting-runtime-rework/` lands, the runtime has no shadows. Dev work tolerates the gap — acceptable pre-release, coordinated with the rework's shipping cadence. Direct lighting still renders; only the shadow modulation is absent.

---

## Goal

Remove the SDF and CSM subsystems entirely. Delete code; delete PRL sections; strip shader paths and bind groups. No deprecation shim, no conditional code path — pre-release permits breaking changes.

Pixel output changes: surfaces previously shadowed by SDF/CSM are unshadowed after this change. Expected and intentional for the intermediate state.

---

## Approach

Two task clusters: SDF removal and CSM removal. They touch mostly disjoint files but share the fragment shader and bind-group layout. Land as one coordinated PR so the shader rebuild happens once.

---

### Task A — Remove SDF

**Crates:** `postretro-level-format`, `postretro-level-compiler`, `postretro`.

1. Remove the SDF baker stage from `prl-build`: the brick atlas construction, BVH closest-point queries, and voxel-packing code.
2. Remove the `SdfAtlas` PRL section: reader, writer, and section-ID registration.
3. Remove SDF-related runtime: atlas upload, bind-group entries, sampler.
4. Remove `sample_sdf_shadow` and every call site in `forward.wgsl`. Point and spot lights emit unshadowed direct contribution after this change.
5. Delete any SDF-specific test fixtures or baker-crate integration tests.

### Task B — Remove CSM

**Crate:** `postretro`.

1. Remove the CSM render pass, shadow atlas allocation, and cascade split computation.
2. Remove directional-light slot assignment tied to CSM.
3. Remove CSM bind-group entries and the cascaded depth texture array.
4. Remove `sample_csm_shadow` and its call sites in `forward.wgsl`. Directional lights emit unshadowed contribution after this change.
5. Delete CSM-specific shader files (e.g. shadow-pass vertex shader) if distinct from `forward.wgsl`.

---

## Files to modify

Boundary files named by role; exact paths resolved during implementation.

| Area | Task | Change |
|------|------|--------|
| `postretro-level-format` — SDF atlas module | A | Delete |
| `postretro-level-format/src/lib.rs` | A | Remove `SdfAtlas` section ID + exports |
| `postretro-level-compiler` — SDF baker module | A | Delete |
| `postretro-level-compiler` — bake orchestrator | A | Remove SDF stage entry |
| `postretro` — SDF runtime module | A | Delete |
| `postretro` — CSM runtime module (cascade allocator) | B | Remove |
| `postretro/src/render/mod.rs` | A, B | Remove SDF + CSM bind-group entries, upload paths, render-pass setup |
| `postretro/src/shaders/forward.wgsl` | A, B | Remove `sample_sdf_shadow`, `sample_csm_shadow`, all call sites, bind-group declarations |
| Any CSM/SDF shadow-pass shader files | A, B | Delete |

---

## Acceptance Criteria

1. `cargo build --workspace` and `cargo test --workspace` pass.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No references to `sdf`, `SdfAtlas`, `csm`, or cascade-related identifiers survive in the target files (grep-verified).
4. `forward.wgsl` compiles cleanly without the removed functions and bind-group entries.
5. Running the engine on a test map produces output with direct lighting but no shadows. Visual regressions are acceptable for the intermediate state.
6. PRL file size decreases for any map previously containing an SDF atlas section.
7. No new `unsafe`.

---

## Out of scope

- Lightmap replacement for static shadows — `lighting-compiler-rework/` + `lighting-runtime-rework/`.
- Spot shadow map pool for dynamic shadows — `lighting-runtime-rework/`.
- FGD `_dynamic` flag — `lighting-dynamic-flag/`.
- Any future reintroduction of CSM or cube shadow maps — out of the rework entirely; may return in a later plan if needed.
