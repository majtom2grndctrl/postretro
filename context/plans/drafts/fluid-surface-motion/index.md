# fluid-surface-motion

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.3, §7.3a, §8 · `build_pipeline.md` §Custom FGD, §PRL section IDs · `entity_model.md` §7 · `movement.md` §6 · read at e8f607b94

## Problem
The owner reports that water surfaces look off-brand. The fluid pass already samples albedo through Post Retro sampling, but its Quake-style warp computes a continuous offset per screen pixel, so texel edges bend into smooth curves, the opposite of the blocky world textures. The owner also asked for two capabilities:
- Still water should read as still but alive, with air moving over it and drips falling into it.
- Water with a current should be visibly different, showing the direction it pushes the player.

Today both render identically. When this is done, fluid motion moves in whole-texel steps on the water texture's own texel grid. Still water shows a quiet pixel shimmer and ambient ripple rings. A current visibly drifts the way it pushes. A mapper can also give water visible flow that pushes nothing, for a decorative river or waterfall.

## Decisions
- **Color stays the face texture, and the grid is its own texel lattice.** No resolution parameter is added. The lattice `sample_post_retro` already snaps to is `textureDimensions` of the bound texture, so a game's texture resolution sets the water's grain and cannot drift from its world textures. This is the Surface Depth texel-mode precedent (`rendering_pipeline.md` §7.3). Distinct animated looks per fluid type are a later direction.
- **Pixel motion is engine-owned, not an author style.** Motion is texture sampling, and Post Retro is the sole texture path since `plans/done/retire-true-retro`. Bloom and reflection style are author-owned because they are post effects over the image, not the texture lookup itself. A game with smooth reflections still has pixel-stepped water.
- **Motion is evaluated per texel cell.** The warp offset is computed from the cell a fragment's UV falls in, rounded to whole texels, and animated on a time base stepped at a fixed engine rate. The `sample_post_retro` body does not change, and the fluid/mover parity test stays the guard (§7.3a). Shimmer amplitude fades to flat with distance, measured in texels per pixel as Surface Depth's fade is, so far water does not turn to sub-pixel noise.
- **Still water: shimmer plus ambient ripple rings, all in the shader.** Ring placement is a pure function of cell and stepped time. No entity, no CPU work, no per-frame upload. Gameplay-driven ripples (player, projectile, splash) are a non-goal: they need an event-fed buffer, and splash visuals are already an E24 follow-up (`plans/done/E24--water-and-environment-volumes`).
- **Photosensitivity.** Shimmer only moves texels, so a surface's mean brightness does not change with it. Rings are small-area and well below 3 Hz. The `E23--photosensitivity-source-floor` draft therefore gains no new source class.
- **Drift is resolved per surface face at install, on the CPU.** Each face resolves the push at its centroid, using the winner order of `resolve_environment`: priority, then the smaller volume, then map order.
  - Each face's world-to-UV map is affine and known at install. The push is projected into UV space once and mapped through an engine push-to-drift curve. The result is stored as a per-vertex drift, constant across the face.
  - Every fragment of a cell therefore gets the same drift by construction. The group-4 slot's reserved components stay free for `fluid-ssr-reflections`.
  - Push volumes are static baked data: no per-frame cost, no replication (`entity_model.md` §7, `movement.md` §6).
  - A pool whose current varies is one `fluid_volume` with several brushes. Their shared faces are culled (`build_pipeline.md` §Fluid surfaces), and each brush's faces resolve their own push. Two fluid entities would draw a surface between them, as the FGD already warns.
  - E24's deferred runtime-mutable volumes would need a re-resolve hook here; accepted.
- **Visual-only flow: an optional `flow` key on `fluid_volume`.** It takes three numbers in map axes, the visible surface drift in m/s.
  - When set, it replaces the push-derived drift on every face of that entity. `"0 0 0"` makes a pushing current look still.
  - It moves nothing: movement, particles and collision never read it.
  - The compiler converts it to engine axes as it does `push`, and it ships in the environment-volumes section, so the section version bumps. Pre-stable maps recompile.
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
  - a vertical face drifts along the push's in-plane component only.
- [ ] `flow` overrides push:
  - flow without push drifts, and movement reads zero push there;
  - flow `"0 0 0"` over a push shows no drift while the player is still pushed;
  - flow on a non-fluid volume fails the build with a message naming the entity;
  - blank or any count but three numbers fails the build.
- [ ] Environment-volumes section: a record with flow round-trips; a record without flow round-trips; flow on a dry record is rejected; the previous section version is rejected by name.
- [ ] A per-frame opacity change leaves the installed drift intact.
- [ ] The fluid/mover Post Retro parity test passes with its comparison unchanged.
- [ ] Pipeline budget tests pass; the change adds no binding.
- [ ] The DX12 FXC shader test compiles the fluid shader (Windows, ignored-gated run).

### Manual
- [ ] A still pool beside a current pool: the still pool shimmers and rings in place, and the current visibly drifts. Standing in the current pushes the player the way the surface drifts.
- [ ] A `flow`-only waterfall or river drifts visibly, and standing in it pushes nothing.
- [ ] Motion reads as whole-texel steps on a 64 px and a 256 px water texture, and the 64 px pool is visibly chunkier. Cell borders show no smear or shimmer band. Distant water is calm, not noisy.
- [ ] A pool's side face seen through glass shows the warp and drift without tearing at the top edge.
- [ ] GPU timing (`POSTRETRO_GPU_TIMING=1`, 1660 Super): measure the fluid pass looking down at a pool that fills the view, before and after. Report in the plan of record.

## Path
- **Warp.** It lives in `fluid_warp_uv` (fluid_surface.wgsl). Derivatives stay hoisted from the unwarped UV. A CPU mirror in `postretro-render-cpu` lets the reference test pin it, the same shape as `surface_depth`.
- **Drift.**
  - Seams: `pack_fluid_surfaces`, `FluidSurfaceVertex` (which already carries a per-face normal), and `Renderer::install_fluid_surfaces`, which receives every environment record.
  - `collision::resolve` owns the winner order. Call or lift its comparator rather than restating it.
- **Flow key.**
  - Compiler: `resolve_environment_volume` and `parse_push` (level-compiler parse/environment_volumes.rs).
  - Format: `EnvironmentVolumeRecord` and `ENVIRONMENT_VOLUMES_VERSION` (level-format environment_volumes.rs).
  - FGD: the `fluid_volume` class in `sdk/TrenchBroom/postretro.fgd`, plus the `docs/level_design.md` key table.
  - Check whether the build cache needs a stage epoch bump for the section change.
- **Strongest rival:** animate in texture space. A small pass writes each stepped frame at the texture's own resolution, and the scene samples it unchanged.
  - Seam anti-aliasing and mips are then exact by construction.
  - It loses because ripples would repeat on every texture tile, and drift would need a target per face or per volume.
  - It is the fallback if the first slice shows cell borders smearing, where the AA band blends with the source texel rather than the neighbouring cell's texel.
- **First slice.** The cell warp on the 64 px and 256 px pools, checking cell borders closely.
- **Fixtures.** `maps/e24-environment-test.map` needs a still pool, a current pool, a two-brush pool with a partial push, and a `flow`-only river. Add them if missing.

## Wire format
Environment-volumes section, version 2. Each volume record gains `flow` immediately after `push`, encoded exactly as `push` is: a `u8` presence flag (0 or 1), then three little-endian `f32` in engine axes (Y-up), with zeros when absent. Units are m/s. Every float must be finite. A record with the flag set and an empty fluid name is rejected. Every other field and its order is unchanged from version 1. A version-1 section is rejected as unsupported.

## Boundary inventory

| Name | Rust | PRL | FGD KVP |
|---|---|---|---|
| Visual flow | `EnvironmentVolumeRecord::flow: Option<[f32; 3]>` | environment-volumes v2, after `push` | `fluid_volume` key `flow`, three numbers, map axes, m/s |

## Open questions
- Stepped animation rate, ring density and lifetime, shimmer amplitude and fade distance, and the push-to-drift curve — **delegated**: tune by eye at the first manual capture, and record the values in the plan of record.
