# dynamic-shadow-world-depth-cache — plan of record

status: approved
read at: f8cd3f34

## Corrections

- campaign-test's cited ~18–19 dynamic fixtures is an authored candidate count, not concurrent pool occupancy. Planning around it by sizing the two caches for the room-local, brightness-qualified wave peaks instead: 3 spot layers and 4 cube slots (24 contiguous cube face layers). The spot wave's 300 ms pulse/150 ms spacing permits two concurrent pulse lights; the point wave's 200 ms pulse/50 ms spacing permits four. The extra untagged dynamic spot is covered by the spot headroom. The final timing gate remains the proof that every occupied fixture slot in the exercised rooms warms.

## Delegated answers

- Cache budgets — 3 full-resolution spot layers and 4 full-resolution cube slots (24 faces): enough for the scripted room-local pulse peaks, below the 96-slot spot and 6-slot cube pools, and with correct pool-only fallback if a future scene exceeds either budget.
- Cached world-map resolution — retain the pool resolutions, 1024² spot and 512² cube face, to preserve the baseline for the required A/B; no lower-resolution variant is introduced in this brief.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| Spot cold frame fills and samples its layer | `dynamic_spot_cold_fill_is_sampleable_same_frame` | achievable as stated |
| Spot warm frame skips world draw and cull | `warm_dynamic_spot_skips_world_render_and_cull` | achievable as stated |
| Cube fills six faces then skips all six | `warm_dynamic_cube_skips_all_six_faces` | achievable as stated |
| Re-tenanting invalidates reused layer | `occupant_change_invalidates_reused_dynamic_layer` | achievable as stated |
| Identity retains layer across slot move | `slot_reassignment_retains_cache_layer` | achievable as stated |
| Vacated slot channel is swept after a move | `moved_light_does_not_leave_cache_layer_on_old_slot` | achievable as stated |
| Changed projection remains cold | `matrix_change_invalidates_dynamic_layer` | achievable as stated |
| Spot overflow is deterministic and pool-only | `spot_budget_overflow_uses_pool_only_without_thrash` | achievable as stated |
| Cube overflow claims atomic six-face units | `cube_budget_overflow_never_claims_partial_faces` | achievable as stated |
| Budget matches campaign wave peaks | `dynamic_cache_budget_matches_campaign_wave_peak` | achievable as stated |
| Departed/unoccupied channel is -1 | `dynamic_cache_layer_channels_reset_for_departed_slots` | achievable as stated |
| Dynamic namespace is isolated | renderer shader/BGL `include_str!` wiring guard | achievable as stated |
| Level reload starts cold | `dynamic_cache_reset_makes_recycled_source_cold` | achievable as stated |
| Empty geometry produces initialized lit depth | `empty_geometry_cache_fill_clears_and_warms_layer` | achievable as stated |
| Entity receivers retain static world occlusion | entity-shader wiring guard plus `sample_*_with_dynamic_world` use | achievable as stated |
| World receivers retain moving entity occlusion | forward shader wiring guard plus ungated entity depth pass | achievable as stated |
| Dynamic lights never enter promoted tail | existing selection test extended in `light_filter_tests` | achievable as stated |
| World-on-world, world-on-entity, entity-on-world parity | owner, in-engine A/B at full cache resolution | manual-visual |
| Warm dynamic slots reduce world-depth time | owner, `POSTRETRO_GPU_TIMING=1` after each room's warm-up | manual-visual |

## Tasks

| # | Task | Status |
|---|---|---|
| 1 | Split the 1,213-line `renderer_shadow_passes.rs` at its existing spot/cube responsibility boundary, behavior-preserving and in its own commit, before extending either path. | |
| 2 | Build the thinnest risky slice: a renderer-owned dynamic spot world-depth cache (pure identity+matrix planner, three 1024² layers, full per-slot `cache_layer` sweep), then wire its cold cache fill, warm cull/world skip, entity-only pool pass, and two-map sample into forward world, skinned, and kinematic receivers. This tests the new world-receiver two-map path end-to-end. | |
| 3 | Extend the planner and GPU resources to four atomic cube slots (24 512² faces); apply the same cache/pool split, six-face warm transition, deterministic overflow fallback, and cube world/entity two-map sampling. | |
| 4 | Complete lifecycle and observability integration: reset/free/recreate behavior, cache-layer uploads and bind-group rebuilds, dedicated dynamic-cache timing/counters, shader/BGL namespace guards, promoted-tail regression guard, and the durable rendering-pipeline documentation update. | |
