# hidpi-render-scale

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.8, §11, §12 · `context/lib/player_options.md` §4 · `context/lib/ui.md` §5, §6 · read at c0def0e9e

## Problem
The developer found that frames on a HiDPI laptop sit well below vsync on every map, tiny or stress. This is an observed defect, and the cause is that the engine shades every scene pixel at the surface's physical size. A Retina window therefore costs about 4× the fill of the same window on a 1× display, and the scene has no render scale. When done:
- On a HiDPI display the scene renders at logical resolution by default and is upscaled with nearest filtering.
- Players can choose an integer render resolution.
- Game UI stays sharp at native resolution, untouched by scene screen effects.

The test GPU, a Radeon Pro 5300M, sits at the documented compatibility floor (`rendering_pipeline.md` §10 Target hardware: 5500M-class, must run, not perf-tuned). This brief makes such hardware playable through resolution. It sets no frame-time target. Fewer pixels remove the resolution multiplier, not all per-pixel cost: in a debug build, 4× fewer pixels removed about 42% of the frame.

## Decisions
- **The swapchain keeps the physical (surface) size, and the scene gets its own smaller size (scene extent).** Every scene target follows the scene extent: depth, HDR scene colour, bloom, fog scatter, SDF half-res, the spot-shadow bind group and capture. So does the camera aspect. Scene extent = ceil(surface / divisor) per axis, with a minimum of 1. Both extents are derived at one renderer-owned chokepoint, so no target can read the surface size by accident.
- **Default "Auto" divisor = max(1, floor(surface height / 720)).** It is the largest integer divisor that keeps the scene at least 720 rows tall, recomputed on every resize. This bounds cost by pixels rather than by OS density. Examples:
  - 1080p gives native.
  - A 2560×1440 Retina window gives ½.
  - A 3072×1920 panel gives ½.
  - 4K gives ⅓ at any OS scale factor, the common Windows setup on floor-class hardware.
  - A 1× 1440p desktop monitor also gives ½. Native stays one option away.
  - Rejected: keying on the OS scale factor. It renders native 4K at 100% scaling and loses the cost bound at fractional scale factors.
- **New player option, Render resolution: Auto / Native / ½ / ⅓ / ¼, integer divisors only.**
  - It is a `PlayerOptions` field mapped at the render-profile chokepoint (`player_options.md` §4).
  - It applies live through a resize.
  - It is not a per-map key.
  - The options screen is mod content that reads `options.*` slots, so the new slot is a scripting-surface change. It follows the existing graphics-row pattern, and the SDK typedefs are regenerated.
- **The upscale is nearest-neighbor, inside the existing resolve.** That pass already samples with nearest filtering. Integer pixel replication adds no new filter, so the "retro filters, sparingly" northstar (`index.md` §1) is not engaged. Smooth and fractional filters are out of scope. So is temporal upscaling: `drafts/motion-reprojection-layer` names it as a possible consumer, and nothing here forecloses it.
- **Game UI renders into its own native-resolution layer, which the resolve composites over the upscaled, effected scene.**
  - The layer is a premultiplied-alpha sRGB target at surface extent. UI keeps its private depth target (`ui.md` §5).
  - The resolve remains the gameplay path's sole swapchain writer (`rendering_pipeline.md` §7.8). That keeps M13's no-parallel-resolves rule and E23-accessibility's reliance on the seam.
  - Frontend and title frames run the same resolve, so they composite the layer too.
  - UI is not tonemapped.
- **Screen effects apply to the scene only, chosen per effect inside the one resolve shader.**
  - Shake, flash and vignette each have a "covers HUD" switch, all off in this brief.
  - Turning UI shake or flash on later, in sync with the scene, flips a switch in the same shader. There is no second source of truth. Building that is a non-goal.
  - World-only shake is M13's own wanted follow-up (`done/M13--screen-space-effects`).
  - Flash no longer covers the HUD by default. That diverges from M13's flash-over-scene-and-HUD criterion, by owner decision.
  - A future CRT or scanline filter (`roadmap.md` §Post-processing) then has one home: the resolve, after compositing.
- **Rivals, rejected:**
  - **UI drawn straight to the swapchain after the resolve.** It overturns the sole-writer contract and splits any HUD-covering effect, the CRT filter included, across two passes. On tile-based GPUs it also costs a full-resolution load and store.
  - **Upscaling into a surface-sized scene colour and drawing UI there.** It keeps the contracts at the cost of one extra full-resolution blit (~0.3 ms on this GPU), but it still tonemaps the UI.
- **The map-owned `fog_pixel_scale` divides the scene extent, not the surface.** An authored map's fog blocks then look the same on a 1× display and on HiDPI at Auto. This keeps fog coarser than the scene, never finer.
- **Scale-factor changes are handled.** When the window moves to a display with a different scale factor, the debug UI's scale is refreshed, and the following resize recomputes both extents. No handler exists today.
- **Where things live:** the renderer owns both extents, the upscale, the UI layer and the resolve composite. The binary forwards scale-factor events. Player options own the policy.
- **Not in this brief:**
  - **Window modes** (windowed, borderless, exclusive fullscreen). A follow-up brief owns them, because they raise boot-order and E23 U4 window-creation questions this work does not need.
  - UI shake and flash, beyond the seam above.
  - Dynamic resolution.
  - Reducing forward-shader cost. It is the dominant per-pixel work, but separate.
  - The intermittent heavy-paging slow regime in dev builds. It is unexplained and parked.
  - The submitted-leaf count that also runs in release. That is a separate small fix.
- **Coordination:** the drafts `E23--photosensitivity-source-floor` and `coop-trigger-screen-effects` assume flash acts in the resolve. That still holds. Because the HUD is off by default for flash, the source-floor work treats UI as its own source.

## Acceptance
### Automated
- [ ] Scene extent is ceil(surface / divisor) per axis for odd and even surfaces, never below 1×1. A divisor larger than the surface clamps.
- [ ] Auto resolves to divisors 1, 1, 1, 2, 2, 3 for surface heights 719, 1080, 1439, 1440, 1920, 2160. Auto is independent of scale factor.
- [ ] At Auto, a surface under 1440 rows tall gets a scene extent equal to the surface extent. This is the no-regression row for common 1× displays.
- [ ] A resize rebuilds every scene-sized target at the scene extent and the swapchain at the surface extent. No scene target reads the surface size.
- [ ] Changing the render-resolution option triggers exactly one rebuild with the new extent. So does a scale-factor change and the resize that follows it.
- [ ] Fog scatter dimensions derive from the scene extent and `fog_pixel_scale`.
- [ ] Capture produces an image at the scene extent.
- [ ] Game UI records into the native-resolution UI layer with its own depth target, never into scene colour. The resolve is the only gameplay pass that writes the swapchain. The debug UI still records after it. Frontend frames composite the layer too.
- [ ] Each screen effect's covers-HUD switch defaults to off. With all switches off, effect strength leaves the composited UI pixels unchanged. With one switch on in a test, only that effect reaches the UI.
- [ ] Settings without the new field load with Auto. An unknown value falls back to Auto. The field round-trips through save.
- [ ] Every render-resolution value maps at the chokepoint through an exhaustive match with no wildcard.
- [ ] The regenerated SDK typedefs include the new `options.*` slot. The dev frontend's graphics tab reads and writes it.
### Manual
- [ ] HiDPI Mac, release build, on campaign-test and stress-warren-hallway-inspection, Auto vs Native: report forward-pass and total GPU ms per frame (Metal System Trace, `rendering_pipeline.md` §12) and the `[CpuTiming]` total. Expected: Auto's forward pass is roughly a quarter of Native's.
- [ ] Visual: the scene upscale shows crisp, uniform pixels with no filtering blur. HUD text and widgets stay sharp at native resolution under every render-resolution value.
- [ ] Visual: during shake, flash and vignette, the scene responds and the HUD neither moves nor tints. HUD colours match today's within tonemap tolerance.
- [ ] Dragging the window between displays with different scale factors updates the scene extent. No stretched frame appears, and the debug UI scale is not stale.

## Path
- **Seams:**
  - `Renderer::resize` and `renderer_full_init` size the targets.
  - `screen_effects` (`encode_resolve`, its resize) owns scene colour and the resolve.
  - `UiPass::new` currently takes `SCENE_COLOR_FORMAT`, and its private depth target sizes to its viewport.
  - The UI viewport is set in `renderer_render_frame`; `ui::layout::device_scale` handles UI scale.
  - `startup/render_profile.rs` and the options bridge carry the option.
  - `WindowEvent::Resized` in `main.rs` triggers resize, and the camera aspect is set beside it.
- **Shape:** an explicit scene-extent value threaded through resize. Rival: per-target scale fields. Rejected, because each target would re-derive the extent and could drift.
- **First slice:** add the scene extent with a hardcoded divisor of 2 and the nearest upscale in the resolve, leaving UI where it is. Measure Auto against Native on the Mac. That falsifies the fill-bound premise before the UI pass moves.
- **UI layer:** it clears to transparent each frame and is sampled 1:1 by the resolve. The text atlas is commented as built for an sRGB surface, so check colour parity now that it targets an sRGB layer rather than HDR scene colour.
- **Large file:** `renderer_render_frame.rs` (~1.2k lines). UI-layer recording moves out of it rather than growing it.
- **Background:** derivation, measurements and lifecycle diagrams are in `research.md`.

## Open questions
- Label text and placement of the options row — **delegated**
- Whether the covers-HUD switches are uniform flags or compile-time constants — **delegated**
