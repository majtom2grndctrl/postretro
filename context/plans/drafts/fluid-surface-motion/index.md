# fluid-surface-motion

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.3, §7.3a, §8 · `build_pipeline.md` §Custom FGD, §PRL section IDs · `entity_model.md` §7 · `movement.md` §6 · `networking.md` §Current contract · read at e8f607b94

## Problem
The owner reports that water surfaces look off-brand. The fluid pass already samples albedo through Post Retro sampling, but its Quake-style warp computes a continuous offset per screen pixel, so texel edges bend into smooth curves, the opposite of the blocky world textures. The owner also asked for two capabilities:
- Still water should read as still but alive, with air moving over it and drips falling into it.
- Water with a current should be visibly different, showing the direction it pushes the player.

Today both render identically. When this is done, fluid motion moves in whole-texel steps on the water texture's own texel grid. Still water shows a quiet pixel shimmer and ambient ripple rings. A current visibly drifts the way it pushes, at the speed it carries a swimmer. A mapper can also turn a current's force down, even to zero, without changing its look, for a gentle stream or a purely decorative river.

## Decisions
- **Color stays the face texture, and the grid is its own texel lattice.** No resolution parameter is added. The lattice `sample_post_retro` already snaps to is `textureDimensions` of the bound texture, so a game's texture resolution sets the water's grain and cannot drift from its world textures. This is the Surface Depth texel-mode precedent (`rendering_pipeline.md` §7.3). Distinct animated looks per fluid type are a later direction.
- **Pixel motion is engine-owned, not an author style.** Motion is texture sampling, and Post Retro is the sole texture path since `plans/done/retire-true-retro`. Bloom and reflection style are author-owned because they are post effects over the image, not the texture lookup itself. A game with smooth reflections still has pixel-stepped water.
- **Motion is evaluated per texel cell.** The warp offset is computed from the cell a fragment's UV falls in, rounded to whole texels, and animated on a time base stepped at a fixed engine rate. The `sample_post_retro` body does not change, and the fluid/mover parity test stays the guard (§7.3a). Shimmer amplitude fades to flat with distance, measured in texels per pixel as Surface Depth's fade is, so far water does not turn to sub-pixel noise.
- **Still water: shimmer plus ambient ripple rings, all in the shader.** Ring placement is a pure function of cell and stepped time. No entity, no CPU work, no per-frame upload. Gameplay-driven ripples (player, projectile, splash) are a non-goal: they need an event-fed buffer, and splash visuals are already an E24 follow-up (`plans/done/E24--water-and-environment-volumes`).
- **Photosensitivity.** Shimmer only moves texels, so a surface's mean brightness does not change with it. Rings are small-area and well below 3 Hz. Drift is capped (below), so a fast current never jumps several texels per step. The `E23--photosensitivity-source-floor` draft therefore gains no new source class.
- **Drift is resolved per surface face at install, on the CPU.** Each face resolves the push at its centroid, using the winner order of `resolve_environment`: priority, then the smaller volume, then map order.
  - Drift velocity is the authored push ÷ the fluid's viscosity. That is the speed a swimmer settles at (`movement.md` §6), so the surface moves at the speed it carries you. It is capped at the drift layer's terminal velocity, which also covers zero viscosity.
  - Each face's world-to-UV map is affine and known at install. The drift is projected into UV space once and stored as a per-vertex drift, constant across the face. Every fragment of a cell therefore gets the same drift by construction.
  - On screen, drift is capped at one texel per animation step.
  - The group-4 slot's reserved components stay free for `fluid-ssr-reflections`.
  - Push volumes are static baked data: no per-frame cost, no replication (`entity_model.md` §7).
  - A vertical push (`movement.md` §6 designs updrafts and down-currents) projects to nothing on a level surface, so such a current looks still from above. This limit is accepted.
  - A pool whose current varies, or a river that bends, is one `fluid_volume` with several brushes under several push brushes. The fluid's shared faces are culled (`build_pipeline.md` §Fluid surfaces), and each face resolves its own push. Two fluid entities would draw a surface between them, as the FGD already warns.
  - E24's deferred runtime-mutable volumes would need a re-resolve hook here; accepted.
- **A current's force is its own dial: an optional `force` key on `push_volume`, default 1.** It accepts any finite value ≥ 0.
  - The authored `push` keeps setting direction and strength, and the surface's look always derives from it.
  - Everything physical feels `push × force`: movement, prediction and replay, projectiles and particles. Force 0 is a decorative current: it drifts visibly and pushes nothing. A value below 1 gives a gentle shove under a strong-looking current.
  - It modifies the push field and adds no field to overlap resolution, so "each entity sets one field" holds.
  - It changes gameplay, so it enters the level content digest with `push` (`networking.md` §Current contract). No cosmetic value enters the digest.
  - It ships in the environment-volumes section, so the section version bumps. Pre-stable maps recompile.
- **Non-goal: reduce motion does not still the water.** Reduce motion governs view feel, shake and UI tweens (`player_options.md` §5), not world animation. Animated lights do not follow it either.
- **Reflections are a separate brief.** `fluid-ssr-reflections` builds on the cell grid this brief introduces.

## Acceptance
### Automated
- [ ] Cell warp reference, GPU-free:
  - every offset is a whole number of texels;
  - all UVs in one cell get one offset;
  - the offset changes only at time-step boundaries;
  - zero drift gives no net motion over a full cycle;
  - a nonzero drift moves along its direction, and reversing it reverses the motion;
  - shimmer amplitude reaches zero beyond the fade distance.
- [ ] Ripple placement is deterministic: the same cell and stepped time give the same ring state, and adjacent steps differ.
- [ ] Install drift resolve, per face:
  - a face over a coincident push gets that push's projected drift;
  - a face with no push gets zero;
  - a push beside but not over a face is ignored;
  - two pushes over one face yield the winner `resolve_environment` picks at its centroid;
  - in a two-brush pool with a push over only one brush, only that brush's faces drift;
  - a vertical face drifts along the push's in-plane component only;
  - a purely vertical push gives a level face zero drift.
- [ ] Drift speed:
  - it equals push ÷ viscosity below the cap;
  - zero viscosity gives the terminal-velocity cap, not a division fault;
  - screen drift never exceeds one texel per step.
- [ ] `force`:
  - omitted resolves to 1, and movement is unchanged from today;
  - force 0 gives the authored drift on the surface, while a swimmer, a projectile and a particle in it feel zero push;
  - force 0.5 halves the felt push and leaves the drift unchanged;
  - host movement, client prediction and reconcile replay agree under force 0.5;
  - a negative, non-numeric or non-finite value fails the build with a message naming the entity.
- [ ] Two copies of a map that differ only in `force` produce different content digests.
- [ ] Environment-volumes section: push records with and without an authored force round-trip; a negative or non-finite force is rejected; the previous section version is rejected by name.
- [ ] A per-frame opacity change leaves the installed drift intact.
- [ ] The fluid/mover Post Retro parity test passes with its comparison unchanged.
- [ ] Pipeline budget tests pass; the change adds no binding.
- [ ] The DX12 FXC shader test compiles the fluid shader (Windows, ignored-gated run).

### Manual
- [ ] A still pool beside a current pool: the still pool shimmers and rings in place, and the current visibly drifts. Standing in the current carries the player the way, and at about the speed, the surface drifts.
- [ ] A force-0 river drifts visibly and pushes nothing. A bending river built from several push brushes over one fluid entity drifts along each bend, with no surface drawn mid-river.
- [ ] Motion reads as whole-texel steps on a 64 px and a 256 px water texture, and the 64 px pool is visibly chunkier. Cell borders show no smear or shimmer band. Distant water is calm, not noisy.
- [ ] A pool's side face seen through glass shows the warp and drift without tearing at the top edge.
- [ ] GPU timing (`POSTRETRO_GPU_TIMING=1`, 1660 Super): measure the fluid pass looking down at a pool that fills the view, before and after. Report in the plan of record.

## Path
- **Warp.** It lives in `fluid_warp_uv` (fluid_surface.wgsl). Derivatives stay hoisted from the unwarped UV. A CPU mirror in `postretro-render-cpu` lets the reference test pin it, the same shape as `surface_depth`.
- **Drift.**
  - Seams: `pack_fluid_surfaces`, `FluidSurfaceVertex` (which already carries a per-face normal), and `Renderer::install_fluid_surfaces`, which receives every environment record.
  - `collision::resolve` owns the winner order. Call or lift its comparator rather than restating it.
  - Viscosity comes from the local fluid declaration (`local_fluid_presentation`). Check whether a fluid hot reload reaches install; if it does not, re-resolve drift on reload or report the gap.
- **Force key.**
  - Compiler: `resolve_environment_volume` and `parse_push` (level-compiler parse/environment_volumes.rs).
  - Format: `EnvironmentVolumeRecord` and `ENVIRONMENT_VOLUMES_VERSION` (level-format environment_volumes.rs).
  - Runtime: storing the effective push in `collision::EnvironmentVolume` lets every body consumer (physics movement, the weapon impact stage) read it unchanged. Only the renderer reads the authored push.
  - Digest: `runtime_movers.rs` hashes environment fields one by one, so add `force` beside `push`.
  - FGD: the `push_volume` class in `sdk/TrenchBroom/postretro.fgd`, plus the `docs/level_design.md` key table.
  - Check whether the build cache needs a stage epoch bump for the section change.
- **Classic rival, rejected:** hand-drawn animation frames plus a scroll, as Doom and Quake animate liquids. The engine has no texture-animation system, every game would hand-draw frames, and ripples would repeat per tile.
- **Strongest rival:** animate in texture space. A small pass writes each stepped frame at the texture's own resolution, and the scene samples it unchanged. Seam anti-aliasing and mips are then exact by construction. It loses because ripple rings would repeat on every texture tile.
- **Fallback:** a hybrid, if the first slice shows cell borders smearing (the AA band blends with the source texel, not the neighbouring cell's).
  - Drift stays in the scene as one whole-texel scroll per face; a uniform scroll has no cell borders.
  - Shimmer and rings move to texture space.
- **First slice.** The cell warp on the 64 px and 256 px pools, checking cell borders closely.
- **Fixtures.** `maps/e24-environment-test.map` needs a still pool, a current pool, a two-brush pool with a partial push, and a force-0 bending river. Add them if missing.

## Wire format
Environment-volumes section, version 2. Each volume record gains `force` immediately after the push vector: one little-endian `f32`. It is written as 1.0 when the record has no push, and it must be finite and ≥ 0. Every other field and its order is unchanged from version 1. A version-1 section is rejected as unsupported.

## Boundary inventory

| Name | Rust | PRL | FGD KVP |
|---|---|---|---|
| Current force | `EnvironmentVolumeRecord::force: f32` | environment-volumes v2, after the push vector | `push_volume` key `force`, float ≥ 0, default 1 |

## Open questions
- Stepped animation rate, ring density and lifetime, and shimmer amplitude and fade distance — **delegated**: tune by eye at the first manual capture, and record the values in the plan of record.
