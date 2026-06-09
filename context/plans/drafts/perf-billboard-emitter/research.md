# Billboard Emitter Performance — Research Notes

Investigation findings backing the spec. Line numbers are from the working tree at draft time; treat them as starting points, not contracts.

## Symptom & root-cause hypothesis

User filled a room with smoke-puff billboard emitters; frame time tanked. Cost ranked:

1. **GPU fill-rate / overdraw (dominant).** Smoke pass is additive-blended (`src=ONE, dst=ONE`), depth-test `Less`, depth-write OFF (`render/smoke.rs:229-262`). Overlapping puffs each re-shade every covered pixel. The fragment shader is heavy: ~16 SH atlas taps (8-corner depth-aware indirect + 8-corner direct) plus a static-specular chunk loop and a dynamic-light loop (`billboard.wgsl:376-507`). Cost ∝ screen-coverage × overlap-depth × per-fragment cost — exactly what "fill a room" maximizes. Draw-call count does NOT scale with the symptom: emitters sharing one sprite collection batch into ONE draw call (`particle_render.rs` test `multiple_emitters_one_collection_share_one_record_draw_call`).
2. **No culling.** Entities don't track BSP leaves (`entity_model.md` §2/§6). Off-screen / adjacent-room smoke is fully simulated AND submitted every frame. Mesh path already culls; particle path does not.
3. **CPU sim/collect.** Three full-registry walks per frame, per-particle allocations.
4. **Instance buffer cap.** Single shared buffer, 4096-sprite silent truncation per collection.

**Confirmation method:** the smoke pass is not currently GPU-timed (Slice 0 fixes this). Zero-code check: shrink emitter `size` — if frame time recovers sharply with the same particle count, fill-rate is confirmed dominant over CPU/draw cost.

## Area A — GPU timing (Slice 0)

- Pair constants: `render/mod.rs:182-188` (`TIMING_PAIR_CULL`=0 … `TIMING_PAIR_SH_COMPOSE`=5, `TIMING_PAIR_COUNT`=6). **Confirmed verbatim.**
- Label vec build (gated on `enable_gpu_timing`): `render/mod.rs:2263-2275`.
- `FrameTiming` in `render/frame_timing.rs`: `QUERIES_PER_PASS=2`; query set padded to `.max(16)` slots, so a 7th pair needs no resize. `render_pass_writes(pair_idx)` (frame_timing.rs:136-144) returns `RenderPassTimestampWrites` and marks the pair written. `accumulate` silently skips pairs not written this frame — so a conditional pass produces no false 0.00ms.
- `forward` wiring pattern to copy: `render/mod.rs:4264-4301` — build the `*_ts = self.frame_timing.as_ref().map(|t| t.render_pass_writes(TIMING_PAIR_FORWARD))` borrow BEFORE `begin_render_pass`, then set `timestamp_writes: forward_ts`.
- Smoke pass descriptor: `render/mod.rs:4416-4436`, label `"Billboard Sprite Pass"`, currently `..Default::default()` → `timestamp_writes: None`. **GOTCHA:** the `timestamp_writes: None` at `mod.rs:4481` is the `"Fog Raymarch Pass"` compute pass, NOT smoke.
- Readback: `encode_resolve` (frame_timing.rs:170-189) at `mod.rs:4637-4639` before submit; `post_submit` at `mod.rs:4670-4672`; averages over `AVG_WINDOW_FRAMES=120`, logs `[gpu-timing]`.
- No `FrameTiming` unit tests — verified by running the engine.

## Area B — Shader lighting hoist (Slice 1)

- Pipeline source = `billboard.wgsl` + `sh_sample.wgsl` concatenated (`render/smoke.rs:25-29`).
- `VertexOutput` (`billboard.wgsl:154-159`, **confirmed verbatim**): `clip_position`, `@location(0) uv`, `@location(1) world_position`, `@location(2) opacity`.
- `vs_main` (`:203-249`): expands 6-vert quad; `out.world_position = sprite_pos` (sprite CENTER, `:246`) — all quad corners already share it, so lighting is already constant across the quad → per-vertex hoist is ~visually identical.
- `fs_main` (`:376-507`): lighting term assembled at `:495-501`:
  ```
  var sh_lighting = sh_ambient + sh_direct;
  if dynamic_direct_isolation == 1u { sh_lighting = sh_direct; }
  else if == 2u { sh_lighting = sh_ambient; }
  let lighting = sh_lighting + static_specular + dynamic_diffuse;
  let rgb = sprite_sample.rgb * lighting * in.opacity;   // :502
  ```
  **Confirmed verbatim.** The `dynamic_direct_isolation` debug branches (`:496-499`) move to vertex with the rest.
- MUST stay per-fragment: `sample_post_retro` sprite sample (`:380-382`, needs `dpdx/dpdy` of `in.uv`), `sprite_sample.a`, `in.opacity`, final premultiply (`:506`).
- `sample_sh_indirect` (`:325-346`) / `sample_sh_direct` (`:354-374`) → `sh_sample.wgsl` corner-blend fns use `textureSampleLevel(...,0.0)` and `textureLoad` — **valid in vertex stage** (no implicit derivatives).
- **CRITICAL GOTCHA — bind-group visibility.** Shared group-3 SH BGL `sh_bind_group_layout_entries` (`render/sh_volume.rs:728-807`): `let vis = FRAGMENT | COMPUTE` (`:732`, **confirmed verbatim**); direct atlas (binding 15) is `FRAGMENT`-only (`:798`). None include `VERTEX`. Hoisting SH reads into `vs_main` requires adding `ShaderStages::VERTEX`. This BGL is SHARED by forward (fragment), fog (compute), and mesh pipelines — widening is additive/safe but validated at pipeline creation across all of them. If the dynamic-light + static-specular loops also move to vertex, group-0 camera and group-2 lighting BGLs need `VERTEX` too.
- Keep green: `render/smoke.rs` naga tests — `billboard_wgsl_parses` (:450), `billboard_wgsl_passes_naga_validation` (:472, validates uniform control flow — vertex stage must stay uniform-control-flow clean), `billboard_wgsl_sprite_instance_stride_matches_cpu` (:486).
- Reduced-res option (stretch): fog uses `fog_pixel_scale` — `fog_pass.rs:863-868` (`scatter_dims_for`), `fx/fog_volume.rs:214` (`clamp_fog_pixel_scale`, default 4×). Smoke draws straight to the swapchain `view` (`mod.rs:4420`) with additive blend, so a downscaled variant needs a new intermediate target + composite. Larger blast radius → optional, likely a separate future slice.

## Area C — Instance buffer (Slice 4)

- `MAX_SPRITES=4096`, `SPRITE_INSTANCE_SIZE=32` in `fx/smoke.rs:17,32`.
- `SmokePass` struct `render/smoke.rs:105-124`: one `instance_buffer`, group-6 `instance_bind_group`, BGL entry `has_dynamic_offset: false` (`:187`).
- Buffer creation `:267-287`, sized `MAX_SPRITES * SPRITE_INSTANCE_SIZE`. Comment at `:267-272` already flags "single buffer with per-collection offsets" as future work.
- `record_draw` `:414-437` (**confirmed verbatim**): `capped = live_count.min(MAX_SPRITES)` (silent truncation, `:428`); `queue.write_buffer(&instance_buffer, 0, ...)` then `draw(0..capped*6, 0..1)`. One write_buffer + one draw per collection.
- Per-collection iteration: `mod.rs:4441-4447`. `particle_collections: &[(&str, &[u8])]` built in `main.rs:1230-1231`.
- Keep green: `particle_render.rs` `multiple_emitters_one_collection_share_one_record_draw_call` (:212), `different_collections_produce_separate_draws` (:227); `smoke.rs:486` stride test (32-byte instance stride pinned). Dynamic-offset reworks must respect 256-byte storage dynamic-offset alignment while keeping the 32-byte instance stride.

## Area D — CPU sim/collect (Slice 3)

Three full-registry walks via `iter_with_kind` (`registry.rs:546-563`; storage `[Vec<Option<ComponentValue>>; COUNT]` columns `:428`; `get_component`/`set_component` re-`validate` each call `:671-689`):

- `emitter_bridge.rs`: live-count tally (`:110-136`) + emitter snapshot with `component.clone()` (`:138-154`); per-spawn curve clones `size_over_lifetime`/`opacity_over_lifetime` into each `ParticleState` (`:343-344`).
- `particle_sim::tick`: snapshot-clone every `ParticleState` (incl. its `Vec<f32>` curves) every tick (`:33-39`); per-particle 3 gets + 2-3 sets (`:43-108`). Snapshot-then-mutate contract — no mid-walk mutation (`:23-24`).
- `particle_render::collect` (`:58-82`, **confirmed verbatim** — signature is `collect(&mut self, registry: &EntityRegistry)`): per-particle `resolve_collection(&visual.sprite)` allocates a `String` (`:94-107`); doc comment already suggests pre-caching a `sprite→collection` map (`:90-93`).
- Bench anchor: `bench_500_particles_one_frame_under_half_a_millisecond` (`particle_sim.rs:379-407`, **confirmed verbatim**), `#[ignore]` release bench, `tick` of 500 particles < 500µs. Covers `tick` only — not bridge or collector.

## Area E — Culling (Slice 2)

- `VisibleCells` enum `visibility.rs:13-18`: `Culled(Vec<u32>)` (cell IDs = BSP leaf indices) | `DrawAll`. Produced by `determine_visible_cells` (`:468-512`), called once per frame in `main.rs:1086-1093`, reclaimed at `main.rs:1383` AFTER render.
- Template — mesh collector `collect(&mut self, registry, world: &LevelWorld, visible: &VisibleCells, alpha: f32)` (`scripting/systems/mesh_render.rs:53-90`, **confirmed verbatim**) culls per-instance via `mesh_visible(world, visible, current.position)` (`:71`).
- `mesh_visible` / `mesh_visible_in_leaf` (`render/mesh_pass.rs:618-637`): `DrawAll` short-circuits true; else `world.find_leaf(pos)` then `cells.contains(&leaf_id)` — a LINEAR scan. Per-particle scan at high counts could regress → cull at EMITTER granularity (one `find_leaf` per emitter gates all its particles; particle back-refs emitter via `ParticleState.emitter`).
- `LevelWorld::find_leaf(pos) -> usize` (`prl.rs:312-326`, **confirmed verbatim**): BSP walk from `self.root`. No baked data needed — ad-hoc query available now. Entities don't store leaves (`entity_model.md` §2/§6).
- Particle collector called at `main.rs:1228` (before mesh collector). `self.level.as_ref()` (world) and `visible_cells` both in scope there — proven by the mesh path at `main.rs:1241-1252`.
- **GOTCHA — sim gating.** Visibility is computed in the Render stage (`main.rs:1086`), AFTER the fixed-tick loop (frame order Input→Game→Audio→Render). Sim-stage culling would need a one-frame-stale visible set. Scope Slice 2 to RENDER-stage collector culling only; note sim gating as a follow-up.
- Test churn: every `particle_render.rs` collector test calls `collect(&registry)` and must move to the new signature in the same change (pre-stable: breaking an internal API updates all call sites + tests together). Mirror mesh cull tests `collect_emits_one_visible_mesh_instance` / `collect_excludes_mesh_in_nonvisible_cell` (`mesh_render.rs:178/241`).
