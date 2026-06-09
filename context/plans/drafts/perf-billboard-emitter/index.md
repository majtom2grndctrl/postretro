# Billboard Emitter Performance

## Goal

Filling a room with smoke-puff billboard emitters tanks frame time. Cut the cost across the full emitter lifecycle — GPU fill-rate first (the dominant cost), then culling, CPU sim, and draw-submission scaling. Ship as six sequenced slices so each fix is measured before the next lands, and so the work sets up future at-scale scenes (arenas heavy on billboard effects plus hundreds of projectiles).

Root cause (see `research.md`): the smoke pass is additive-blended with depth-write off and a heavy per-fragment shader (~16 SH atlas taps + two light loops), so overlapping puffs re-shade every covered pixel. Cost scales with screen coverage × overlap depth, not draw-call count — which is exactly what "fill a room" maximizes.

## Scope

### In scope

- Smoke-pass GPU timing so the bottleneck is measurable (Slice 0).
- Hoisting SH lighting from per-fragment to per-vertex (Slice 1).
- Hoisting the static-specular + dynamic-light loops to per-vertex (Slice 2).
- Portal-visibility culling of non-visible emitters at render-collect time (Slice 3).
- Reducing per-frame CPU cost in the emitter/particle sim + collect path, including collapsing redundant full-registry walks (Slice 4).
- Single instance buffer with per-collection offsets; lifting the per-collection 4096-sprite cap (Slice 5).

### Out of scope (non-goals)

- Reduced-resolution smoke render target + composite. Bigger blast radius (new intermediate target, upscale/composite, additive-blend interaction). Revisit as its own slice if the lighting hoist + culling don't suffice. Noted in Slice 2's sketch as the fallback lever.
- Sim-stage (game-logic tick) visibility culling. Visibility is computed in the Render stage, after the fixed-tick loop, so sim gating needs a one-frame-stale visible set. Slice 3 culls at render-collect time only; sim gating is a follow-up.
- Switching from additive to alpha blending. Additive is commutative (no per-frame depth sort needed); alpha would require sorting. Out of scope.
- Columnar / batched registry mutation API. Slice 4 reduces allocations and redundant walks within the existing `EntityRegistry` contract; it does not redesign component storage.
- GPU instancing rework (indexed/instanced draw). Slice 5 keeps the existing 6-verts-per-sprite non-indexed draw; it only changes buffer layout and the cap.

## Acceptance criteria

Slice 0 — Timing
- [ ] With `POSTRETRO_GPU_TIMING=1`, the `[gpu-timing]` log includes a labeled smoke/billboard pass line alongside the existing passes.
- [ ] On frames where no sprites are drawn, the smoke pass produces no timing entry and no false `0.00ms` reading; other passes still report.
- [ ] `cargo test` green; `cargo fmt --check` and `cargo clippy -- -D warnings` clean.

Slice 1 — Per-vertex SH lighting
- [ ] Smoke renders with no visible change versus baseline at normal viewing distances (SH now evaluated per sprite, which already matched per-fragment because all quad corners share the sprite center).
- [ ] The billboard fragment shader no longer performs SH indirect or SH direct sampling; SH lighting arrives as an interpolated vertex output.
- [ ] Smoke-pass GPU time (from Slice 0) drops in a fill-heavy scene versus the Slice 0 baseline.
- [ ] The billboard WGSL still passes naga validation (parse + uniform control flow); the dynamic-direct isolation debug modes still function.
- [ ] `cargo test` green; fmt/clippy clean.

Slice 2 — Per-vertex light loops
- [ ] The billboard fragment shader no longer runs the static-specular loop or the dynamic-light loop; their combined contribution arrives as an interpolated vertex output (folded into the Slice 1 lighting term).
- [ ] The fragment shader's remaining work is the sprite-texture sample, alpha, opacity, and the final premultiply — no lighting computation.
- [ ] Smoke renders with no visible change versus the Slice 1 baseline at normal viewing distances; lit-by-dynamic-light smoke still responds to dynamic lights.
- [ ] Smoke-pass GPU time drops further versus the Slice 1 baseline in a scene with dynamic lights on smoke.
- [ ] WGSL still passes naga validation; fmt/clippy clean; `cargo test` green.

Slice 3 — Culling
- [ ] Particles whose emitter is in a non-visible cell are not packed for drawing; particles in visible cells are.
- [ ] When the frame's visibility is "draw all" (no portal culling), all particles draw — no regression.
- [ ] A test asserts a particle in a non-visible cell is excluded and one in a visible cell is included (mirrors the mesh collector cull tests).
- [ ] All existing particle-collector tests updated to the new collect signature and green.
- [ ] `cargo test` green; fmt/clippy clean.

Slice 4 — CPU cost
- [ ] No per-particle heap allocation on the render-collect path (sprite→collection resolution uses a pre-built map, not a per-particle `String`).
- [ ] The per-tick particle simulation no longer clones per-particle lifetime curves into a snapshot every tick.
- [ ] Redundant full-registry walks are collapsed: the emitter/particle path makes fewer passes over the registry per frame than before, with the count stated in the task's notes and asserted measurable via the bench.
- [ ] The 500-particle tick bench stays under its threshold; the bench is extended to also cover the collect path (or a sibling bench is added) with a stated threshold.
- [ ] All emitter-bridge, particle-sim, and particle-render tests green; snapshot-then-mutate (no mid-walk registry mutation) preserved.
- [ ] `cargo test` green; fmt/clippy clean.

Slice 5 — Instance buffer
- [ ] A single collection can draw more than 4096 live sprites without silent truncation.
- [ ] Multiple collections in one frame draw correctly from one instance buffer (no cross-collection corruption), without a separate full re-upload at offset 0 per collection.
- [ ] Batching behavior preserved: one collection ⇒ one draw call; N collections ⇒ N draw calls.
- [ ] The WGSL sprite-instance stride still matches the CPU-side instance size.
- [ ] `cargo test` green; fmt/clippy clean.

## Tasks

### Slice 0: Smoke-pass GPU timing
Add a timestamp pair for the billboard/smoke render pass so `POSTRETRO_GPU_TIMING=1` reports its GPU time. Define a new `TIMING_PAIR_SMOKE`, extend the pair count and the label vec, and attach `timestamp_writes` to the "Billboard Sprite Pass" descriptor using the existing `FrameTiming::render_pass_writes` helper — building the borrow before `begin_render_pass`, exactly as the forward pass does. The pass is conditional (only runs when there are sheets and sprite collections); the existing timing infra already skips unwritten pairs, so no false readings. This slice is the measurement gate for every later slice.

### Slice 1: Hoist SH lighting to per-vertex
Move the SH indirect + SH direct sampling out of the billboard fragment shader into the vertex shader, passing the SH lighting term through `VertexOutput` as an interpolated value. SH reads use non-derivative texture ops (`textureSampleLevel`/`textureLoad`) and are valid in the vertex stage. This requires widening the shared SH bind-group-layout visibility to include the vertex stage — an additive change validated across every pipeline that shares that layout (forward, fog, mesh). Preserve the dynamic-direct isolation debug modes (they move to vertex with the SH term) and keep the WGSL naga-valid with uniform control flow. The static-specular and dynamic-light loops stay in the fragment shader for now (Slice 2). Re-measure with Slice 0 before proceeding.

### Slice 2: Hoist static-specular + dynamic-light loops to per-vertex
Move the static-specular loop and the dynamic-light loop out of the fragment shader into the vertex shader, folding their result into the interpolated lighting term established in Slice 1. After this slice the fragment shader does no lighting — only the sprite-texture sample, alpha, opacity, and premultiply. This widens the camera (group 0) and lighting (group 2) bind-group-layout visibility to include the vertex stage. Watch vertex-stage control-flow uniformity (the dynamic-light loop iterates a uniform `light_count`); keep the WGSL naga-valid. Split from Slice 1 because it touches different bind groups and carries more uniformity risk; measure after Slice 1 to confirm the loops are worth hoisting.

### Slice 3: Cull non-visible emitters at render-collect
Thread the level world and the frame's visible-cell set into the particle render collector (both are already in scope at the call site, as the mesh collector proves) and skip particles whose emitter cell is not visible. Cull at emitter granularity — one BSP-leaf lookup per emitter gates all its particles — to avoid a per-particle linear scan over the visible-cell list. Mirror the mesh collector's cull pattern, including the "draw all" short-circuit. Update every collector call site and test to the new signature in the same change.

### Slice 4: Reduce CPU sim/collect cost
Cut per-frame CPU work in the emitter/particle path on three fronts: (a) pre-build a sprite→collection map so the render collector no longer allocates a `String` per particle; (b) stop cloning per-particle lifetime curves into the sim's per-tick snapshot (reference or share them instead); (c) collapse the redundant full-registry walks — today the path walks the registry multiple times per frame (live-count tally, emitter snapshot, sim snapshot, render collect) — into the minimum that preserves the snapshot-then-mutate contract (no mid-walk registry mutation). Extend the particle benchmark to cover the collect path with a stated threshold so the win is measurable and regressions are caught.

### Slice 5: Single instance buffer with per-collection offsets
Replace the single fixed 4096-sprite instance buffer (re-uploaded at offset 0 per collection) with one buffer sized for the frame's total live sprites, drawing each collection from its own offset. Lift the silent 4096-per-collection truncation. This sets up the at-scale target: an arena heavy on billboard effects plus hundreds of projectiles spreads sprites across many collections and can exceed 4096 in a single smoke collection — the per-collection offset removes the redundant per-collection re-upload, and lifting the cap stops puffs from silently disappearing. Keep the 32-byte instance stride (pinned by the WGSL/CPU stride test) and respect storage dynamic-offset alignment. Preserve batching: one collection still issues one draw call.

## Sequencing

Every phase is sequential. Concurrency is unsafe here: Slices 1 and 2 both restructure the billboard shader; Slices 3 and 4 both edit `particle_render.rs`; Slice 5 re-edits `render/smoke.rs`. The ordering is also a measure-before-optimize gate.

**Phase 0 (sequential):** Slice 0 — prerequisite. Its smoke-pass timing number gates and validates every later slice.
**Phase 1 (sequential):** Slice 1 — SH hoist (highest-impact fill-rate win). Re-measure with Slice 0 before continuing.
**Phase 2 (sequential):** Slice 2 — light-loop hoist. Builds on Slice 1's interpolated lighting term; same shader.
**Phase 3 (sequential):** Slice 3 — culling. Touches `particle_render.rs` + `main.rs`.
**Phase 4 (sequential):** Slice 4 — CPU sim/collect. Also touches `particle_render.rs`; must follow Slice 3.
**Phase 5 (sequential):** Slice 5 — instance buffer. Edits `render/smoke.rs` again; must follow the shader slices.

Each task: own acceptance criteria, run `cargo check` + `cargo test`, keep the listed tests green.

## Rough sketch

Lifecycle: emitter def → `emitter_bridge` spawn → `particle_sim::tick` → `particle_render::collect` → `SmokePass::record_draw` → `billboard.wgsl`. Full file:line detail in `research.md`. Constraints: renderer owns GPU; no `unsafe`; frame order Input→Game→Audio→Render→Present; breaking an internal API updates all call sites + tests in the same change.

- **Slice 0:** `render/mod.rs:182-188` (pair consts), `:2263-2275` (labels), `:4264-4301` (forward wiring to copy), `:4416-4436` (smoke descriptor — currently `..Default::default()` → `None`; the `None` at `:4481` is the *fog* pass). Helper: `FrameTiming::render_pass_writes` (`render/frame_timing.rs:136-144`); query set padded to 16 slots so a 7th pair needs no resize.
- **Slice 1:** `billboard.wgsl` `VertexOutput` (`:154-159`), `vs_main` (`:203-249`, corners share `sprite_pos` at `:246`), `fs_main` SH usage in the lighting term (`:495-502`). SH fns `sample_sh_indirect`/`sample_sh_direct` (`:325-374`) use `textureSampleLevel`/`textureLoad` — vertex-safe. Widen visibility in `sh_volume.rs:728-807` (`vis = FRAGMENT|COMPUTE` at `:732`, direct atlas FRAGMENT-only at `:798`) to add `VERTEX`; shared by forward/fog/mesh pipelines. Naga tests: `smoke.rs:450/472/486`.
- **Slice 2:** static-specular loop + dynamic-light loop in `fs_main` (assembled into `lighting` at `billboard.wgsl:501`). Bindings: group-0 camera + group-2 lighting (`lights`, `light_influence`, `spec_lights`, `chunk_grid`, `chunk_offsets`, `chunk_indices`, `billboard.wgsl:53-72`) — add `VERTEX` to their BGL visibility (BGLs defined in `render/mod.rs`). Fallback lever (non-goal): fog-style `fog_pixel_scale` (`fog_pass.rs:863-868`).
- **Slice 3:** template `mesh_render.rs:53-90` (`collect(registry, world, visible, alpha)` + `mesh_visible` per instance); `mesh_visible`/`mesh_visible_in_leaf` (`mesh_pass.rs:618-637`); `VisibleCells` (`visibility.rs:13-18`); `LevelWorld::find_leaf` (`prl.rs:312-326`). Particle collector `collect(&mut self, registry)` (`particle_render.rs:58`) called at `main.rs:1228` — world (`self.level`) and `visible_cells` in scope (see mesh path `main.rs:1241-1252`). Emitter back-ref: `ParticleState.emitter`. Mirror cull tests `mesh_render.rs:178/241`.
- **Slice 4:** walks via `iter_with_kind` (`registry.rs:546-563`): live-count tally + emitter snapshot (`emitter_bridge.rs:110-154`), sim snapshot curve clones (`particle_sim.rs:33-39`), collect walk + `resolve_collection` String alloc (`particle_render.rs:58-107`). Per-spawn curve clones (`emitter_bridge.rs:343-344`). Bench `bench_500_particles_one_frame_under_half_a_millisecond` (`particle_sim.rs:379-407`). Snapshot-then-mutate contract (`particle_sim.rs:23-24`).
- **Slice 5:** `MAX_SPRITES`/`SPRITE_INSTANCE_SIZE` (`fx/smoke.rs:17,32`); `SmokePass` + buffer (`smoke.rs:105-124`, `:267-287`, `has_dynamic_offset:false` at `:187`); `record_draw` (`:414-437`, cap at `:428`); per-collection loop (`mod.rs:4441-4447`). Batching tests `particle_render.rs:212/227`; stride test `smoke.rs:486`.

## Open questions

None blocking. Decisions locked during drafting:
- Lighting hoist split into Slice 1 (SH) + Slice 2 (light loops) — separate bind-group visibility changes, separately measurable.
- Slice 4 collapses redundant registry walks (required AC), not just the allocation fixes.
- Slice 5 retained and reframed toward the at-scale arena target (heavy billboards + hundreds of projectiles); the 4096 cap is per-collection, so it bites a room-filling smoke collection more than projectiles alone, but the per-collection-offset change benefits many-collection frames broadly.
