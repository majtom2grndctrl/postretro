# fluid-surface-motion

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.3, §7.3a, §8 · `build_pipeline.md` §Custom FGD, §PRL section IDs · `entity_model.md` §7 · `movement.md` §6 · `networking.md` §What gates, and what replicates instead · read at e8f607b94

## Problem
The owner reports that water looks off-brand. Fluid albedo already goes through Post Retro sampling, but the Quake-style warp shifts the sample point by a continuous offset per screen pixel, so texel edges bend into smooth curves. The owner also asked to tell still water from a current. Still water should look alive, with air moving over it and drips falling in. A current should show which way it pushes, and its push should be adjustable down to purely decorative. Today every pool looks the same. When this is done, water moves in whole-texel steps on its texture's own grid: still water shimmers and rings, and a current drifts the way, and at the speed, it carries a swimmer.

## Decisions
- **Water uses its face texture and that texture's texel grid.** Face UVs are in texels, so the face's texture scale sets the grain, exactly as for walls. A game that wants finer water authors finer textures; no resolution parameter is added. This mirrors Surface Depth's texel mode (`rendering_pipeline.md` §7.3).
- **Pixel motion is engine-owned.** Motion is texture sampling, and Post Retro has been the only sampling path since `plans/done/retire-true-retro`. Bloom and reflection style are author-owned because they are post effects.
- **Shimmer and ripple rings are per texel cell, in the shader.** Each cell of the face's fixed grid gets a whole-texel offset. The offset steps at no more than 3 Hz, the general flash threshold, and each cell carries its own phase, so a view-filling pool never changes on one frame. Rings are a pure function of cell and step, so there is no entity, CPU work or upload. The offset fades to flat once texels shrink below a pixel, as Surface Depth's march does. `sample_post_retro` is unchanged.
- **Drift is a whole-texel scroll at the speed the current carries a swimmer.**
  - Speed is the drift layer's steady state in that fluid, from a function movement also uses, terminal-velocity cap included, so look and feel agree at any viscosity (`movement.md` §6).
  - Viscosity and terminal velocity are the values prediction uses: on a client, the host's fluid table and player tuning, which arrive together (`networking.md` §What gates, and what replicates instead).
  - A scroll moves light through a region rather than adding it, which is motion, not flash. That distinction withdrew the frame limiter (`rendering_pipeline.md` §7.8). So drift needs no step cap.
  - Shimmer and rings stay on the fixed grid while the texture scrolls beneath them.
- **Drift is resolved per face at install.** Each face takes the nominal push winning `resolve_environment` at a point just inside the fluid, so a wind skimming the waterline does not move the water. It is projected into UV once. A push confined to a thin band under the waterline still drifts the surface; this is accepted.
  - Drift re-resolves on every fluid-table or tuning change, viscosity-only changes included.
  - Frame capture loads no fluid table, so it draws no drift.
- **The compiler splits fluid faces along every push brush over them.** A current then shows exactly where its push is, however the fluid brushes were drawn. A bending river is one `fluid_volume` under several push brushes; two fluid entities would draw a surface between them, as the FGD warns. A map compiled before this change loads, and its unsplit faces drift whole until rebuilt.
- **A vertical push looks still from above.** An updraft or down-current (`movement.md` §6) has no in-plane component on a level face. This limit is accepted.
- **`force` on `push_volume` scales how hard a current shoves.**
  - It defaults to 1, takes any finite value ≥ 0, and has no upper cap, like `light_range`. Unlike `light_range`, 0 is valid.
  - `push` becomes the nominal current: it sets the direction and the look. The player feels `push × force` at every movement read of push. Environment resolution therefore yields nominal and felt push as two outputs.
  - Particles are presentation (`entity_model.md` §8), so they follow the nominal push, and a force-0 river still carries foam.
  - Force 0 is decorative. Force above 1 is a calm-looking surface hiding a strong current, a trap the author may choose.
  - A visual-only `flow` key was rejected because it let look and feel point different ways.
- **`force` amends two contracts at promotion, and stays out of every existing map.**
  - Amended: the "further regional field needs its own amendment" rule, stated in both `build_pipeline.md` (Load-time gameplay declarations) and `entity_model.md` (Key-Value Parsing), and the meaning of push in `movement.md` §6.
  - It enters the level content digest beside push (`networking.md` §Level identity and level content digest).
  - It ships in an optional section shaped like `LightTags`, written only when some push volume authors a force other than 1. The environment-volumes section and its version are unchanged, so existing maps load as they are.
- **Non-goals.**
  - Gameplay-driven ripples need an event-fed buffer; splash visuals are already an E24 follow-up (`plans/done/E24--water-and-environment-volumes`).
  - Per-fluid-type looks (sludge without rings, say) need new descriptor fields; every fluid shares one motion here.
  - Reduce motion governs view feel, shake and UI tweens (`player_options.md` §5), not world animation, so it does not still water.

## Acceptance
Rows cite the orderings pinned in `research.md` (R1–R15).

### Automated
- [ ] The fluid shader mirrors the CPU reference: step rate, amplitude, fade and hash constants match it, and `fs_main` samples albedo and emissive at the warped UV.
- [ ] Textual checks on comment-stripped `fluid_surface.wgsl` alone:
  - no `sin(` or `cos(` remains;
  - shimmer and ring cells are floored from the unwarped texel UV, never the scrolled one (R10);
  - the shimmer offset and the drift scroll are each floored to whole texels before they move the sample point.
- [ ] Shimmer and rings, against the GPU-free reference:
  - every offset is a whole number of texels, constant within a cell;
  - a cell's displayed offset, shimmer plus every ring reaching it, changes only at that cell's own step boundaries and never faster than 3 Hz (R7);
  - across a 64×64 grid at 60 fps for 10 s, no frame changes more than 10% of cells, including a grid spanning negative texel coordinates, where cells −1 and 0 hash to different phases (R8);
  - with zero drift and no ring reaching it, a cell's shimmer offset averages zero within 0.05 texel over 10 s and never exceeds the shimmer amplitude; with rings, its displayed offset never exceeds the combined amplitude the shader-constant row pins;
  - the offset reaches zero past the texels-per-pixel fade threshold;
  - with texels per pixel swept through the whole fade band over 1 s at 60 fps, uniformly across a 64×64 grid, no frame changes more than 10% of cells;
  - the same cell and step give the same ring state, steps n and n + 1 differ in at least one cell of a 64×64 grid, and ring state differs somewhere between a cell and the same cell one texture period over.
- [ ] Drift scroll:
  - texture features move toward the push in world space, at its true speed, for faces at scale 1, at 0.25 and under a 45°-rotated projection; reversing the push reverses the scroll;
  - after 24 h of script time in the uniform's f32, a scale-0.25 face under a 3 m/s current scrolls whole texels, its mean speed over a 10 s window is within one texel per second of true speed, and the warped UV stays within one texture period of the unwarped UV (R9);
  - wrapping the scroll changes no cell's shimmer or ring state (R10).
- [ ] Install drift resolve, per face:
  - a face over a push gets that push's projected drift, and a face with none gets zero;
  - a push brush only touching a fluid face from outside, such as a wind whose bottom sits on the waterline, gives no drift (R3);
  - two pushes over one face yield the winner `resolve_environment` picks;
  - a vertical face drifts with the in-plane component only, and a purely vertical push gives a level face zero drift;
  - the `e24-environment-test` current channel gets 3 m/s toward map +Y on its top face (push 6 ÷ water viscosity 2), with no face of it left still.
- [ ] Face splitting:
  - a push covering half of one fluid face splits it, and only the covered part drifts;
  - a push covering half of a face from below, its top flush with the waterline as the FGD's coincident current is drawn, splits the face and only the covered part drifts; the same push mirrored above, its bottom on the waterline, leaves every part still (R3);
  - a push covering a whole face leaves it unsplit;
  - two overlapping pushes over parts of one face split it into parts that each drift with their winner (R4);
  - an edge or corner touch leaves the face whole and still, no part under 1 mm² is kept, and parts sum to the face's area (R5);
  - every part of one face under one push drifts identically, however thin (R6).
- [ ] Drift speed:
  - for `water` and `sludge`, a swimmer released without input in a force-1 current settles, within 2%, at the world speed the surface drifts (R13);
  - for a test fluid with viscosity × dt ≥ 1 (viscosity 120 at 60 Hz), the surface drifts at movement's steady state, not push ÷ viscosity, and a swimmer released without input settles at that speed within 2% (R13);
  - a current whose push ÷ viscosity exceeds terminal velocity drifts at terminal velocity, as a swimmer does;
  - two currents of different strength below the cap drift at different speeds;
  - zero viscosity gives the terminal-velocity cap, not a division fault.
- [ ] Re-resolve:
  - a fluid-table change that alters only one viscosity re-resolves drift on host and client (R1);
  - frame capture installs fluid surfaces with zero drift and still draws them at the capture opacity;
  - a client that installs the level before the host's tuning draws no fluid surface until it arrives, then the same drift as a client tuned first; after a demotion and a new tuning it draws the new table's drift (R2);
  - a mid-level re-resolve still draws every pool at its opacity on the next frame, and each later frame with unchanged opacities uploads no parameter rows (R14).
- [ ] `force`:
  - omitted or blank resolves to 1, and movement is unchanged from today;
  - force 0 leaves the surface drift as authored while a swimmer feels zero push and a particle follows the nominal push;
  - force 0.5 and force 3 scale the felt push and leave the drift unchanged;
  - a force-0 updraft never lifts a grounded player, and a fluid jump under a force-0 down-current strong enough to block it at force 1 decides as in still water (R11);
  - host movement, client prediction and reconcile replay agree under force 0.5;
  - a negative, non-numeric or non-finite value fails the build, naming the entity.
- [ ] Digest: two maps differing only in `force` hash differently. A map whose every force is 1 hashes the same with or without a force section.
- [ ] Force section:
  - a map authoring no `force`, including a push-only map, emits none and loads with every force at 1 (R15);
  - a pre-change map loads, and an unsplit face under part of a push drifts whole by its sample point (R12);
  - a map with one authored force round-trips, and its other records read 1;
  - a count unequal to the environment-volume record count, a version other than 1, a payload not 6 + 4 × count bytes, a negative or non-finite force, or a force other than 1 on a record without push each fail load;
  - the environment-volumes bytes are identical with and without the change for a map with no force and no push over part of a fluid face.
- [ ] The fluid warp reads no reduce-motion, accessibility or author render-profile input (grep gate).
- [ ] The fluid/mover Post Retro parity test passes unchanged (regression guard).
- [ ] Pipeline budget tests pass; no binding is added.
- [ ] The DX12 FXC shader test compiles the fluid shader on the 1660 Super, without printing the no-adapter skip line.

### Manual
- [ ] A still pool beside a current pool: the still pool shimmers and rings in place, and the current drifts. Standing in it carries the player the way, and at about the speed, the surface moves.
- [ ] A force-0 river drifts and pushes nothing. A force-3 pool pushes harder than it looks. A bending river under several push brushes over one fluid entity drifts along each bend, with no surface mid-river.
- [ ] Water at scale 1 and at 0.25 each steps on the same grain as a neighbouring wall at the same scale. Cell borders show no smear. Distant water is calm, not noisy.
- [ ] A view-filling still pool reads as gentle motion, not flicker.
- [ ] A pool's side face through glass drifts without tearing at the top edge.
- [ ] GPU timing (`POSTRETRO_GPU_TIMING=1`, 1660 Super) for the fluid pass, looking down at a view-filling pool, before and after; report in the plan of record.

## Path
- **Warp.** It lives in `fluid_warp_uv` (fluid_surface.wgsl); derivatives stay hoisted from the unwarped UV. The CPU mirror and the shader-constant test follow `surface_depth` (`surface_depth_tests.rs`, `shader_constants_match_the_cpu_reference`).
- **Drift.**
  - Seams: `pack_fluid_surfaces`, `FluidSurfaceVertex` (which already carries a per-face normal), and `Renderer::install_fluid_surfaces`.
  - A per-vertex drift keeps the group-4 slot's free components for `fluid-ssr-reflections`.
  - `collision::resolve` owns the winner order; call or lift its comparator rather than restating it.
  - The steady-state function belongs beside the drift layer in `physics::movement::push`.
- **Re-resolve.** Every table change passes `refresh_environment_fluids` (startup/environment_install.rs): host reload, client tuning install and demotion. Hook drift there, before its early return for unchanged names, and read viscosity from the table by fluid name, not from the installed volumes, which are stale at that point on a rename.
- **Steady state.** Movement's decay factor is `max(0, 1 − viscosity·dt)`, so the steady state is `push·dt / (1 − factor)`, capped at terminal velocity. `mod push` is private, so share the function through a re-export.
- **Face split.** It belongs in surface-face extraction (level-compiler `environment_volumes/surface.rs`); `outside_parts` already computes the inside part before discarding it. Fluid vertices carry no lightmap UV, so a split is cheap.
- **Force.**
  - Compiler: `resolve_environment_volume`, `parse_push`. Format: a new `SectionId` beside `EnvironmentVolumes`, paired to its records as `LightTags` pairs with AlphaLights.
  - Runtime: `collision::EnvironmentVolume` feeds both movement and the particle pool, so it carries nominal push and force apart. Movement's push reads include lift-off, the terminal clamp and the fluid-jump check.
  - Digest: `runtime_movers.rs` hashes record fields one by one, and its exhaustive destructure will not see a sibling section, so add force explicitly.
  - FGD: `push_volume` in `sdk/TrenchBroom/postretro.fgd`, plus the `docs/level_design.md` key table.
  - Keep cached lighting valid for maps that author no force.
- **Rivals.**
  - Hand-drawn animation frames plus a scroll, as Doom and Quake animate liquids: the engine has no texture-animation system, and every game would hand-draw frames.
  - Animating in texture space gives exact seam anti-aliasing, but rings repeat on every tile. It is the fallback if cell borders smear, and choosing it is an owner call.
- **First slice.** Shimmer on pools at scale 1 and 0.25, checking cell borders closely.
- **Fixtures.** `content/dev/maps/e24-environment-test.map` needs:
  - a still pool and a current pool;
  - a two-brush pool with a partial push, and a push across half of one face;
  - a force-0 bending river and a force-3 trap pool;
  - water at scale 1 and 0.25 beside walls at matching scales.

## Wire format
New optional push-force section, mirroring `LightTags`:
- **Layout.** A `u16` LE version (1), then a `u32` LE entry count, then one `f32` LE per environment-volume record, in record order.
- **Validation.** The entry count must equal the environment-volume record count. A record without push carries 1.0. Every value is finite and ≥ 0.
- **Absence.** The section is omitted when every force is 1, and an absent section means every force is 1.

The environment-volumes section is unchanged.

## Boundary inventory

| Name | Rust | PRL | FGD KVP |
|---|---|---|---|
| Current force | one `f32` per environment-volume record | new optional section after `EnvironmentVolumes`; absent means all 1 | `push_volume` key `force`, float ≥ 0, default 1 |

## Open questions
- Shimmer step rate under 3 Hz, ring density and lifetime, shimmer amplitude, and the fade threshold — **delegated**: tune by eye at the first capture, and record the values in the plan of record.
