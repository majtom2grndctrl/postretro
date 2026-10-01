# hidpi-render-scale

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.8, §11, §12 · `context/lib/player_options.md` §4 · `context/lib/ui.md` §5, §6 · read at c0def0e9e

## Problem
The developer found that frames on a HiDPI laptop sit well below vsync on every map, tiny or stress. This is an observed defect, and the cause is that the engine shades every scene pixel at the surface's physical size. A Retina window therefore costs about 4× the fill of the same window on a 1× display, and the scene has no render scale. When done:
- On a HiDPI display the scene renders at logical resolution by default and is upscaled with nearest filtering. Very tall panels cap at 1440 scene rows.
- Players can choose an integer render resolution.
- Game UI stays sharp at native resolution, untouched by scene screen effects.

The test GPU, a Radeon Pro 5300M, sits at the documented compatibility floor (`rendering_pipeline.md` §10 Target hardware: 5500M-class, must run, not perf-tuned). This brief makes such hardware playable through resolution. It sets no frame-time target. Fewer pixels remove the resolution multiplier, not all per-pixel cost: in a debug build, 4× fewer pixels removed about 42% of the frame.

## Decisions
- **The swapchain keeps the physical (surface) size, and the scene gets its own smaller size (scene extent).** Every scene target follows the scene extent: depth, HDR scene colour, bloom, fog scatter, SDF half-res, the spot-shadow bind group and capture. So does the camera aspect. Scene extent = ceil(surface / divisor) per axis, with a minimum of 1. Both extents are derived at one renderer-owned chokepoint, so no target can read the surface size by accident. The viewmodel is a scene pass and scales with the scene. A native-resolution viewmodel would later need its own layer.
- **Default "Auto" divisor = max(1, floor(scale_factor), ceil(surface height / 1440)).**
  - On HiDPI it renders at logical resolution: this Mac gets ½.
  - 1× displays up to 1440 rows stay native.
  - Larger panels cap at 1440 scene rows or fewer: 4K at 100% gets 1080 rows, and 5K at 2× gets 1440.
  - Auto bounds visible pixel size. It does not tune the default for the compatibility floor (`rendering_pipeline.md` §10). Floor-class GPUs on big panels pick a lower option.
  - Rejected: a pure pixel-count rule (largest divisor keeping ≥ 720 rows). It shows 2–3× stair-steps on large 1× panels by default, and it hands performance-floor machines a 720-row scene.
  - Auto is recomputed on every resize and scale-factor change.
  - The 1440-row cap is player-options policy. It crosses the render-profile chokepoint as renderer vocabulary, never as a constant inside the renderer (`player_options.md` §4).
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
- **Why a UI layer:** it gives HUD-covering effects one home, the resolve after compositing. It costs a full-resolution UI store and read, about 1% of a 30 ms frame on this GPU.
- **Rivals, rejected:**
  - **UI drawn after the resolve, in its own pass or inside the resolve's render pass.** A HUD-covering effect such as the CRT filter must sample the composited frame, which neither form can do without a further pass. The separate-pass form also breaks the sole-writer contract.
  - **Upscaling into a surface-sized scene colour and drawing UI there.** It keeps the contracts at the cost of one extra full-resolution blit (~0.3 ms here), but it still tonemaps the UI.
- **The map-owned `fog_pixel_scale` divides the scene extent, not the surface.** An authored map's fog blocks then look the same on a 1× display and on HiDPI at Auto, and fog stays coarser than the scene, never finer. A mod's pixelated bloom blocks likewise scale with the scene extent.
- **Scale-factor changes are handled.** When the window moves to a display with a different scale factor, the engine recomputes the Auto divisor and refreshes the debug UI's scale. Game UI already self-corrects from the physical size. No handler exists today.
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
- [ ] Auto resolves to these divisors for (scale factor, surface height):
  - (1.0, 1080) → 1
  - (1.0, 1440) → 1
  - (1.0, 2160) → 2
  - (1.25, 1080) → 1
  - (1.5, 2160) → 2
  - (2.0, 1440) → 2
  - (2.0, 1920) → 2
  - (2.0, 2880) → 2
  - (3.0, 2160) → 3
  - (0.5, 720) → 1
  - (1.0, 1441) → 2
  - (1.0, 1600) → 2
  - (2.0, 2881) → 3
- [ ] At Auto, a 1× surface up to 1440 rows tall gets a scene extent equal to the surface extent. This is the no-regression row for 1× displays.
- [ ] A resize rebuilds every scene-sized target at the scene extent and the swapchain at the surface extent. No scene target reads the surface size.
- [ ] Changing the render-resolution option triggers exactly one rebuild with the new extent. So does a scale-factor change and the resize that follows it.
- [ ] Fog scatter dimensions derive from the scene extent and `fog_pixel_scale`, both on resize and when a level install or reload sets the map's pixel scale (P10).
- [ ] Capture produces an image at the scene extent.
- [ ] Game UI records into the native-resolution UI layer with its own depth target, never into scene colour. The resolve is the only gameplay pass that writes the swapchain. The debug UI still records after it. Frontend frames composite the layer too.
- [ ] Each screen effect's covers-HUD switch defaults to off. With all switches off, effect strength leaves the composited UI pixels unchanged. With one switch on in a test, only that effect reaches the UI.
- [ ] Settings without the new field load with Auto. An unknown value falls back to Auto. The field round-trips through save.
- [ ] Every render-resolution value maps at the chokepoint through an exhaustive match with no wildcard.
- [ ] The regenerated SDK typedefs, TypeScript and Luau, include the new `options.*` slot. The dev frontend's graphics tab reads and writes it.
- [ ] On a 2× display, with no scale-factor event after the window opens, the first gameplay frame's scene extent is half the surface extent (P1).
- [ ] A saved render resolution other than Auto is in effect on the first full-ready frame, with no rebuild after full init (P2).
- [ ] Any mix of resize, scale-factor and render-resolution changes between two frames rebuilds once, from the final values. A scale-factor change with no resize still rebuilds when the divisor changes, and does not rebuild when neither extent changes (P3–P6).
- [ ] A 0×0 resize builds no target and leaves both extents at the last non-zero surface. A render-resolution change while minimized takes effect at the restored size, never at 1×1 (P7, P8).
- [ ] Auto keeps no history: a window resized from 1440 to 1441 rows and back returns to divisor 1, with one rebuild per step (P9).
- [ ] Bloom's chain dimensions derive from the scene extent after a resize, a render-resolution change and a manifest reload commit (P11).
- [ ] A frame with no UI after a frame with UI shows none of the earlier UI. The composited UI layer always matches the swapchain size, including the first frame after a resize or full init and frames with no UI (P13–P15).
- [ ] After a suspend and resume, the first full-ready frame has the same extents as before the suspend (P16).
- [ ] Camera and viewmodel aspect equal the scene extent's aspect after a resize and after a render-resolution change with no resize (P17).
- [ ] When the surface is not divisible by the divisor, every visible scene pixel covers exactly divisor × divisor surface pixels. Only the overshoot at the frame edge is cropped.
- [ ] Grep gate: the renderer crate holds no 1440-row cap. The cap arrives from player options through the render-profile chokepoint.
- [ ] A test changes the extent on a renderer with no window and reads back every scene target's size against the scene extent.
- [ ] An opaque UI pixel reaches the swapchain with the colour it was drawn with, untouched by the tonemap. This is a GPU-harness test; it self-skips without an adapter, so run it on the Mac.
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
- **Policy crossing:** render-profile maps the option to renderer vocabulary, for example "auto with an N-row cap" or "fixed divisor d". The renderer combines that with the surface size and scale factor at the extent chokepoint.
- **Shape:** an explicit scene-extent value threaded through resize. Rival: per-target scale fields. Rejected, because each target would re-derive the extent and could drift.
- **First slice:** add the scene extent with a hardcoded divisor of 2 and the nearest upscale in the resolve, leaving UI where it is. Measure Auto against Native on the Mac. That falsifies the fill-bound premise before the UI pass moves.
- **UI layer:** it clears to transparent each frame and is sampled 1:1 by the resolve. The text atlas is commented as built for an sRGB surface, so check colour parity now that it targets an sRGB layer rather than HDR scene colour.
- **Large file:** `renderer_render_frame.rs` (~1.2k lines). UI-layer recording moves out of it rather than growing it.
- **Background:** derivation, measurements and lifecycle diagrams are in `research.md`.

## Open questions
- Label text and placement of the options row — **delegated**
- Whether the covers-HUD switches are uniform flags or compile-time constants — **delegated**
