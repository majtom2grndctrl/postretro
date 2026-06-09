# Billboard Emitter Performance

## Goal

Filling a room with smoke-puff billboard emitters tanks frame time. Cut the cost across the full emitter lifecycle — GPU fill-rate first (the dominant cost), then culling, CPU sim, and draw-submission scaling. Ship as five sequenced slices so each fix is measured before the next lands.

Root cause (see `research.md`): the smoke pass is additive-blended with depth-write off and a heavy per-fragment shader (~16 SH atlas taps + two light loops), so overlapping puffs re-shade every covered pixel. Cost scales with screen coverage × overlap depth, not draw-call count — which is exactly what "fill a room" maximizes.

## Scope

### In scope

- Smoke-pass GPU timing so the bottleneck is measurable (Slice 0).
- Hoisting billboard lighting from per-fragment to per-vertex (Slice 1).
- Portal-visibility culling of non-visible emitters at render-collect time (Slice 2).
- Reducing per-frame CPU cost in the emitter/particle sim + collect path (Slice 3).
- Single instance buffer with per-collection offsets; lifting the per-collection 4096-sprite cap (Slice 4).

### Out of scope (non-goals)

- Reduced-resolution smoke render target + composite. Bigger blast radius (new intermediate target, upscale/composite, additive-blend interaction). Revisit as its own slice if Slice 1 + culling don't suffice. Noted in Slice 1's sketch as the fallback lever.
- Sim-stage (game-logic tick) visibility culling. Visibility is computed in the Render stage, after the fixed-tick loop, so sim gating needs a one-frame-stale visible set. Slice 2 culls at render-collect time only; sim gating is a follow-up.
- Switching from additive to alpha blending. Additive is commutative (no per-frame depth sort needed); alpha would require sorting. Out of scope.
- Columnar / batched registry mutation API. Slice 3 reduces allocations and redundant walks within the existing `EntityRegistry` contract; it does not redesign component storage.
- GPU instancing rework (indexed/instanced draw). Slice 4 keeps the existing 6-verts-per-sprite non-indexed draw; it only changes buffer layout and the cap.

## Acceptance criteria

Slice 0 — Timing
- [ ] With `POSTRETRO_GPU_TIMING=1`, the `[gpu-timing]` log includes a labeled smoke/billboard pass line alongside the existing passes.
- [ ] On frames where no sprites are drawn, the smoke pass produces no timing entry and no false `0.00ms` reading; other passes still report.
- [ ] `cargo test` green; `cargo fmt --check` and `cargo clippy -- -D warnings` clean.

Slice 1 — Per-vertex lighting
- [ ] Smoke renders with no visible change versus baseline at normal viewing distances (lighting now evaluated per sprite, which already matched per-fragment because all quad corners share the sprite center).
- [ ] The billboard fragment shader no longer performs SH sampling or the light loops; it samples the sprite texture and multiplies by interpolated lighting and opacity.
- [ ] Smoke-pass GPU time (from Slice 0) drops materially in a fill-heavy scene versus the Slice 0 baseline.
- [ ] The billboard WGSL still passes naga validation (parse + uniform-control-flow); the dynamic-direct isolation debug modes still function.
- [ ] `cargo test` green; fmt/clippy clean.

Slice 2 — Culling
- [ ] Particles whose emitter is in a non-visible cell are not packed for drawing; particles in visible cells are.
- [ ] When the frame's visibility is "draw all" (no portal culling), all particles draw — no regression.
- [ ] A test asserts a particle in a non-visible cell is excluded and one in a visible cell is included (mirrors the mesh collector cull tests).
- [ ] All existing particle-collector tests updated to the new collect signature and green.
- [ ] `cargo test` green; fmt/clippy clean.

Slice 3 — CPU cost
- [ ] No per-particle heap allocation on the render-collect path (sprite→collection resolution uses a pre-built map, not a per-particle `String`).
- [ ] The per-tick particle simulation no longer clones per-particle lifetime curves into a snapshot every tick.
- [ ] The 500-particle tick bench stays under its threshold; the bench is extended to also cover the collect path (or a sibling bench is added for it) with a stated threshold.
- [ ] All emitter-bridge, particle-sim, and particle-render tests green; snapshot-then-mutate (no mid-walk registry mutation) preserved.
- [ ] `cargo test` green; fmt/clippy clean.

Slice 4 — Instance buffer
- [ ] A single collection can draw more than 4096 live sprites without silent truncation.
- [ ] Multiple collections in one frame draw correctly from one instance buffer (no cross-collection corruption).
- [ ] Batching behavior preserved: one collection ⇒ one draw call; N collections ⇒ N draw calls.
- [ ] The WGSL sprite-instance stride still matches the CPU-side instance size.
- [ ] `cargo test` green; fmt/clippy clean.

## Tasks

### Slice 0: Smoke-pass GPU timing
Add a timestamp pair for the billboard/smoke render pass so `POSTRETRO_GPU_TIMING=1` reports its GPU time. Define a new `TIMING_PAIR_SMOKE`, extend the pair count and the label vec, and attach `timestamp_writes` to the "Billboard Sprite Pass" descriptor using the existing `FrameTiming::render_pass_writes` helper — building the borrow before `begin_render_pass`, exactly as the forward pass does. The pass is conditional (only runs when there are sheets and sprite collections); the existing timing infra already skips unwritten pairs, so no false readings. This slice is the measurement gate for every later slice.

### Slice 1: Hoist billboard lighting to per-vertex
Move the SH indirect + SH direct sampling, the static-specular loop, and the dynamic-light loop out of the billboard fragment shader and into the vertex shader, passing the combined lighting term through `VertexOutput` as an interpolated value. The fragment shader keeps only the sprite-texture sample (which needs derivatives), alpha, opacity, and the final premultiply. This requires widening the shared SH bind-group-layout visibility (and, if the light loops move too, the camera and lighting BGLs) to include the vertex stage — an additive change validated across every pipeline that shares those layouts. Preserve the dynamic-direct isolation debug modes and keep the WGSL naga-valid with uniform control flow. Re-measure with Slice 0 before proceeding.

### Slice 2: Cull non-visible emitters at render-collect
Thread the level world and the frame's visible-cell set into the particle render collector (both are already in scope at the call site, as the mesh collector proves) and skip particles whose emitter cell is not visible. Cull at emitter granularity — one BSP-leaf lookup per emitter gates all its particles — to avoid a per-particle linear scan over the visible-cell list. Mirror the mesh collector's cull pattern, including the "draw all" short-circuit. Update every collector call site and test to the new signature in the same change.

### Slice 3: Reduce CPU sim/collect cost
Cut per-frame CPU work in the emitter/particle path: pre-build a sprite→collection map so the render collector no longer allocates a `String` per particle, and stop cloning per-particle lifetime curves into the sim's per-tick snapshot (reference or share them instead). Consider collapsing redundant full-registry walks where it doesn't break the snapshot-then-mutate contract. Extend the particle benchmark to cover the collect path with a stated threshold so the win is measurable and regressions are caught.

### Slice 4: Single instance buffer with per-collection offsets
Replace the single fixed 4096-sprite instance buffer (re-uploaded at offset 0 per collection) with one buffer sized for the frame's total live sprites, drawing each collection from its own offset. Lift the silent 4096-per-collection truncation. Keep the 32-byte instance stride (pinned by the WGSL/CPU stride test) and respect storage dynamic-offset alignment. Preserve batching: one collection still issues one draw call.

## Sequencing

Every phase is sequential. Concurrency is unsafe here: Slices 1 and 4 both edit `render/smoke.rs`, and Slices 2 and 3 both edit `particle_render.rs` — parallel agents would merge-conflict. The ordering is also a measure-before-optimize gate.

**Phase 0 (sequential):** Slice 0 — prerequisite. Its smoke-pass timing number gates and validates every later slice.
**Phase 1 (sequential):** Slice 1 — highest expected impact (fill-rate). Re-measure with Slice 0 before continuing.
**Phase 2 (sequential):** Slice 2 — culling. Touches `particle_render.rs` + `main.rs`.
**Phase 3 (sequential):** Slice 3 — CPU sim/collect. Also touches `particle_render.rs`; must follow Slice 2.
**Phase 4 (sequential):** Slice 4 — instance buffer. Edits `render/smoke.rs` again; must follow Slice 1.

Each task: own acceptance criteria, run `cargo check` + `cargo test`, keep the listed tests green.

## Rough sketch

Lifecycle: emitter def → `emitter_bridge` spawn → `particle_sim::tick` → `particle_render::collect` → `SmokePass::record_draw` → `billboard.wgsl`. Full file:line detail in `research.md`. Constraints: renderer owns GPU; no `unsafe`; frame order Input→Game→Audio→Render→Present; breaking an internal API updates all call sites + tests in the same change.

- **Slice 0:** `render/mod.rs:182-188` (pair consts), `:2263-2275` (labels), `:4264-4301` (forward wiring to copy), `:4416-4436` (smoke descriptor — currently `..Default::default()` → `None`; the `None` at `:4481` is the *fog* pass). Helper: `FrameTiming::render_pass_writes` (`render/frame_timing.rs:136-144`); query set padded to 16 slots so a 7th pair needs no resize.
- **Slice 1:** `billboard.wgsl` `VertexOutput` (`:154-159`), `vs_main` (`:203-249`, corners share `sprite_pos` at `:246`), `fs_main` lighting term (`:495-502`). SH fns `sample_sh_indirect`/`sample_sh_direct` (`:325-374`) use `textureSampleLevel`/`textureLoad` — vertex-safe. Widen visibility in `sh_volume.rs:728-807` (`vis = FRAGMENT|COMPUTE` at `:732`, direct atlas FRAGMENT-only at `:798`) to add `VERTEX`; shared by forward/fog/mesh pipelines. Naga tests: `smoke.rs:450/472/486`. Fallback lever (non-goal): fog-style `fog_pixel_scale` (`fog_pass.rs:863-868`).
- **Slice 2:** template `mesh_render.rs:53-90` (`collect(registry, world, visible, alpha)` + `mesh_visible` per instance); `mesh_visible`/`mesh_visible_in_leaf` (`mesh_pass.rs:618-637`); `VisibleCells` (`visibility.rs:13-18`); `LevelWorld::find_leaf` (`prl.rs:312-326`). Particle collector `collect(&mut self, registry)` (`particle_render.rs:58`) called at `main.rs:1228` — world (`self.level`) and `visible_cells` in scope (see mesh path `main.rs:1241-1252`). Emitter back-ref: `ParticleState.emitter`. Mirror cull tests `mesh_render.rs:178/241`.
- **Slice 3:** walks via `iter_with_kind` (`registry.rs:546-563`); `resolve_collection` String alloc (`particle_render.rs:90-107`); sim snapshot curve clones (`particle_sim.rs:33-39`); per-spawn curve clones (`emitter_bridge.rs:343-344`). Bench `bench_500_particles_one_frame_under_half_a_millisecond` (`particle_sim.rs:379-407`). Snapshot-then-mutate contract (`particle_sim.rs:23-24`).
- **Slice 4:** `MAX_SPRITES`/`SPRITE_INSTANCE_SIZE` (`fx/smoke.rs:17,32`); `SmokePass` + buffer (`smoke.rs:105-124`, `:267-287`, `has_dynamic_offset:false` at `:187`); `record_draw` (`:414-437`, cap at `:428`); per-collection loop (`mod.rs:4441-4447`). Batching tests `particle_render.rs:212/227`; stride test `smoke.rs:486`.

## Open questions

- **Slice 1 light-loop scope:** hoist only SH sampling, or also the static-specular + dynamic-light loops? SH alone is the cheap, low-risk win (BGL visibility widening is isolated to group 3). Moving the light loops adds the most fill-rate savings but widens group-0/group-2 visibility too and risks vertex-stage uniformity issues. Recommend: hoist SH first, measure, then decide on the loops within the same slice. Implementer decides based on the Slice 0 numbers.
- **Slice 3 walk-merging depth:** is collapsing the collect walk into the sim walk worth the coupling, or are the allocation fixes (String + curve clones) enough? Leave to the implementer guided by the extended bench — the allocation fixes are the required AC; walk-merging is optional if the bench target isn't met.
- **Slice 4 priority:** the user runs a single smoke collection, so the 4096 cap and per-collection re-upload aren't their current bottleneck. Confirm this slice is still wanted now, or defer until a multi-collection / >4096-sprite scenario exists.
