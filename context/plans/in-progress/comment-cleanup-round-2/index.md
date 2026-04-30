# Comment Cleanup — Round 2

> **Status:** rounds 1–4 complete (29 files total).
> **Type:** chore — no functional changes.

---

## Description

Second round of editorial comment cleanup across source files, same session as round 1. Strips redundant, obvious, and narrating comments while preserving load-bearing rationale and non-obvious invariants. Goal: reduce comment noise so agent context windows aren't flooded with boilerplate.

## Round 1 — complete

| File | Pre-cleanup ratio | Comments |
|------|-------------------|----------|
| `crates/postretro/src/portal_vis.rs` | 11.7% | 212 |
| `crates/postretro/src/main.rs` | 13.2% | 139 |
| `crates/postretro/src/render/mod.rs` | 6.8% | 164 |
| `crates/postretro/src/render/sh_volume.rs` | 10.2% | 115 |
| `crates/level-compiler/src/map_data.rs` | 13.1% | 36 |

## Round 2 — complete

| File | Comment ratio |
|------|---------------|
| `sdk/lib/data_script.ts` | 62.5% |
| `sdk/types/postretro.d.ts` | 30.0% |
| `crates/postretro/src/render/sh_compose.rs` | 17.5% |
| `crates/level-compiler/src/bvh_build.rs` | 15.1% |
| `crates/level-compiler/src/partition/face_extract.rs` | 14.9% |
| `crates/level-compiler/src/partition/brush_bsp.rs` | 13.9% |
| `crates/postretro/src/scripting/systems/light_bridge.rs` | 11.9% |
| `crates/level-compiler/src/portals.rs` | 11.5% |
| `crates/postretro/src/render/animated_lightmap.rs` | 11.5% |

## Round 3 — complete

| File | Status |
|------|--------|
| `crates/postretro/src/texture.rs` | done |
| `crates/postretro/src/scripting/pool.rs` | done |
| `crates/postretro/src/compute_cull.rs` | done |
| `crates/postretro/src/scripting/conv.rs` | done |
| `crates/level-compiler/src/animated_light_chunks.rs` | done |
| `crates/level-compiler/src/lightmap_bake.rs` | done |
| `crates/level-compiler/src/sh_bake.rs` | done |
| `crates/level-compiler/src/chunk_light_list_bake.rs` | done |
| `crates/level-compiler/src/format/quake_map.rs` | done |
| `crates/postretro/src/scripting/primitives_light.rs` | done |

## Round 4 — complete

| File | Status |
|------|--------|
| `sdk/lib/entities/emitters.ts` | done |
| `sdk/lib/entities/lights.ts` | done |
| `sdk/lib/util/keyframes.ts` | done |
| `sdk/lib/world.ts` | done |
| `crates/postretro/src/render/frame_timing.rs` | done |
| `crates/postretro/src/shaders/forward.wgsl` | done |
| `crates/postretro/src/scripting/runtime.rs` | done |
| `crates/level-compiler/src/main.rs` | done |
| `crates/postretro/src/scripting/reaction_dispatch.rs` | done |
| `crates/level-compiler/src/animated_light_weight_maps.rs` | done |

## Round 3 candidates

### By comment ratio (tokei, excludes all cleaned files)

| File | Ratio | Comments | Code |
|------|-------|----------|------|
| `crates/postretro/src/texture.rs` | 12.0% | 80 | 586 |
| `crates/postretro/src/scripting/pool.rs` | 12.9% | 71 | 479 |
| `crates/postretro/src/compute_cull.rs` | 11.9% | 74 | 546 |
| `crates/postretro/src/scripting/conv.rs` | 10.8% | 84 | 695 |
| `crates/level-compiler/src/animated_light_chunks.rs` | 10.3% | 88 | 765 |
| `crates/level-compiler/src/lightmap_bake.rs` | 9.7% | 149 | 1382 |
| `crates/level-compiler/src/sh_bake.rs` | 9.7% | 130 | 1215 |
| `crates/level-compiler/src/chunk_light_list_bake.rs` | 9.9% | 109 | 987 |
| `crates/level-compiler/src/format/quake_map.rs` | 7.6% | 97 | 1174 |
| `crates/postretro/src/scripting/primitives_light.rs` | 7.4% | 88 | 1099 |

### By blocks of 4+ consecutive comment lines (excludes all cleaned files)

| Blocks | Block lines | File |
|--------|-------------|------|
| 26 | 176 | `crates/postretro/src/shaders/forward.wgsl` |
| 21 | 130 | `crates/level-compiler/src/sh_bake.rs` |
| 20 | 120 | `crates/level-compiler/src/lightmap_bake.rs` |
| 14 | 91 | `crates/level-compiler/src/main.rs` |
| 14 | 84 | `crates/postretro/src/scripting/runtime.rs` |
| 13 | 93 | `crates/postretro/src/scripting/reaction_dispatch.rs` |
| 13 | 87 | `crates/level-compiler/src/animated_light_weight_maps.rs` |
| 12 | 96 | `crates/postretro/src/compute_cull.rs` |
| 12 | 80 | `crates/level-compiler/src/format/quake_map.rs` |
| 11 | 103 | `crates/script-compiler/src/main.rs` |
| 11 | 81 | `crates/postretro/src/render/frame_timing.rs` |
| 11 | 74 | `crates/postretro/src/scripting/luau.rs` |
| 11 | 71 | `crates/postretro/src/texture.rs` |
| 11 | 71 | `crates/level-compiler/src/geometry.rs` |
| 11 | 70 | `crates/level-compiler/src/chunk_light_list_bake.rs` |

## Cleanup rules

- Remove: restating what the code does, "now we do X" narration, section dividers, obvious field/parameter descriptions.
- Keep: non-obvious invariants, safety rationale, cross-file contracts, authoring intent that isn't recoverable from the code.
