# fluid-surface-motion

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.3, §7.3a, §8 · `entity_model.md` §7 · `movement.md` §6 · read at e8f607b94

## Problem
The owner reports that water surfaces look off-brand. The fluid pass already samples albedo through Post Retro sampling, but its Quake-style warp computes a continuous offset per screen pixel. The result is texel edges that bend into smooth curves, the opposite of the blocky world textures. Still water and water with a current also render identically, so the player cannot see which pools will push them. When this is done, fluid motion moves in whole-texel steps on the water texture's own texel grid. Still water shows a quiet pixel shimmer and ambient ripple rings. Water over a push volume visibly drifts the way the player is pushed.

## Decisions
- **Color stays the face texture, and the grid is its own texel lattice.** No resolution parameter is added. The lattice `sample_post_retro` already snaps to is `textureDimensions` of the bound texture, so a game's texture resolution sets the water's grain automatically and cannot drift from its world textures. This matches Surface Depth's texel mode (`rendering_pipeline.md` §7.3). A palette-only procedural fluid would need its own texels-per-meter value and still need a face texture for TrenchBroom. Distinct animated looks per fluid type (lava, sludge, rushing water) are a later direction.
- **Motion is evaluated per texel cell.** The warp offset is computed from the cell a fragment's UV falls in, rounded to whole texels, and animated on a time base stepped at a fixed engine rate. The `sample_post_retro` body does not change, and the fluid/mover parity test stays the guard (§7.3a).
- **Still water: shimmer plus ambient ripple rings, all in the shader.** Ring placement is a pure function of cell and stepped time. No entity, no CPU work, no per-frame upload. Gameplay-driven ripples (player, projectile, splash) are a non-goal: they need an event-fed buffer, and splash visuals are already an E24 follow-up (`plans/done/E24--water-and-environment-volumes`).
- **Current comes from the push volume the player feels.** At install, each fluid volume resolves its push vector once at its surface, using the winner order of `resolve_environment`: priority, then smaller volume, then map order. The vector rides in the reserved components of the volume's group-4 slot.
  - Push volumes are static baked data, so this is load-time work: no per-frame cost and no replication (`entity_model.md` §7, `movement.md` §6).
  - The shader projects the vector into face UV space through the UV/world Jacobian, the Surface Depth precedent.
  - Visual drift speed is an engine curve of push magnitude, not an authored field.
  - One vector per volume: a pool whose current should vary is split into volumes.
  - E24's deferred runtime-mutable volumes would need a re-resolve hook here. That cost is accepted.
- **Non-goal: reduce motion does not still the water.** Reduce motion governs view feel, shake and UI tweens (`player_options.md` §5), not world animation. Animated lights do not follow it either.
- **Reflections are a separate brief.** `fluid-ssr-reflections` builds on the cell grid this brief introduces; this brief owes nothing to it.

## Acceptance
### Automated
- [ ] Cell warp reference, GPU-free:
  - every offset is a whole number of texels;
  - all UVs in one texel cell get one offset;
  - the offset changes only at time-step boundaries;
  - zero current gives no net drift over a full cycle;
  - a nonzero current drifts along its projected direction, and reversing the push reverses the drift.
- [ ] Ripple placement is deterministic: the same cell and stepped time give the same ring state, and two adjacent time steps differ.
- [ ] Install current resolve:
  - a fluid volume with a coincident push volume gets that push;
  - a fluid volume alone gets zero;
  - a push volume beside but not over the surface is ignored;
  - two push volumes over one surface yield the winner `resolve_environment` picks for a body at that point.
- [ ] A per-frame opacity change leaves the installed current intact.
- [ ] The fluid/mover Post Retro parity test passes with its comparison unchanged.
- [ ] Pipeline budget tests pass. Fluid fragment storage stays at its current count, and the change adds no binding.
- [ ] The DX12 FXC shader test compiles the fluid shader (Windows, ignored-gated run).

### Manual
- [ ] Still pool beside a current pool: the still pool shimmers and rings in place, while the current visibly drifts. Standing in the current pushes the player the way the surface drifts.
- [ ] Motion reads as whole-texel steps on a 64 px and a 256 px water texture, and the 64 px pool is visibly chunkier. Texel seams at cell borders show no smear or shimmer band.
- [ ] A pool's side face seen through glass shows the warp and drift without tearing at the top edge.
- [ ] GPU timing (`POSTRETRO_GPU_TIMING=1`, 1660 Super) for the fluid pass looking down at a pool that fills the view, before and after. Report the numbers in the plan of record.

## Path
- **Warp.** It lives in `fluid_warp_uv` (fluid_surface.wgsl). Derivatives stay hoisted from the unwarped UV, as now. A CPU mirror in `postretro-render-cpu` lets the reference test pin it, the same shape as `surface_depth`.
- **Current.** Seams: `pack_fluid_surfaces` and `FluidSurfaceSlots` (render-cpu `fluid_surface`), and `Renderer::install_fluid_surfaces`, which already receives every environment record.
  - `set_draws` writes and dirty-checks opacity only. Write the current at install so later opacity writes keep it.
  - `collision::resolve` owns the winner order. Call or lift its comparator rather than restating it.
- **Strongest rival:** keep the continuous warp and only quantize its output to whole texels. It is rejected because the offset would still vary inside a cell, so cells would tear along curved lines instead of moving as blocks.
- **First slice.** The cell warp on the 64 px and 256 px pools. It falsifies the riskiest assumption, that whole-texel motion reads well through the seam anti-aliasing.
- **Fixtures.** `maps/e24-environment-test.map` or `maps/stress-env-volumes.map` needs a pool with a coincident push volume beside a still pool. Add one if neither has it.

## Open questions
- Stepped animation rate, ring density and lifetime, shimmer amplitude, and the push-to-drift curve — **delegated**: tune by eye at the first manual capture, and record the values in the plan of record.
