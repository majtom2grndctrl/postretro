# hidpi-render-scale

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.8, §10, §11, §12 · `context/lib/player_options.md` §4 · `context/lib/ui.md` §5 · read at c0def0e9e

## Problem
The developer found that frames on a HiDPI laptop sit well below vsync on every map, tiny or stress. This is an observed defect. The cause: the engine shades every scene pixel at the surface's physical size. A Retina window therefore costs about 4× the fill of the same window on a 1× display, and the scene has no render scale.

When done:
- On HiDPI, the scene renders at logical resolution by default and is upscaled with nearest filtering.
- Players can choose an integer render resolution.
- Game UI stays sharp at native resolution, and scene screen effects do not touch it.

The test GPU, a Radeon Pro 5300M, sits at the compatibility floor (§10: must run, not perf-tuned). This brief makes such hardware playable through resolution and sets no frame-time target. Fewer pixels remove the resolution multiplier, not all per-pixel cost (`research.md` §Measurements).

## Decisions
- **The scene gets its own extent, separate from the surface.**
  - The swapchain keeps the surface's physical size.
  - Every scene target, the camera aspect and the viewmodel follow the scene extent: ceil(surface / divisor) per axis, minimum 1.
  - One renderer-owned chokepoint derives both extents, so no target can read the surface size by accident.
- **Auto divisor = max(1, floor(scale_factor), ceil(surface height / 1440)).**
  - HiDPI renders at logical resolution.
  - 1× displays up to 1440 rows stay native.
  - Larger panels cap at 1440 scene rows or fewer.
  - Auto bounds visible pixel size. It does not tune the default for the compatibility floor.
  - The 1440 cap is player-options policy and reaches the renderer through the render-profile chokepoint.
- **Scale factor is read from the window when the renderer is built,** and again on every scale-factor change. macOS sends no event at launch.
- **Size changes rebuild once per frame, from final values.** Resize, scale-factor and option events only record state. The next frame start rebuilds once if either extent changed. Today resize rebuilds inside the event handler, and macOS delivers scale-factor and resize in one dispatch.
- **Render resolution is a player option: Auto / Native / ½ / ⅓ / ¼, integer divisors only.**
  - It applies live and persists in settings.
  - It is not a per-map key.
  - The options screen is mod content reading `options.*` slots, so the new slot is a scripting-surface change, following the existing graphics rows.
- **Headless capture always renders at its requested resolution (divisor 1).** Captures stay comparable across machines, and capture has no window, scale factor or player options.
- **The upscale is nearest-neighbor integer replication inside the existing resolve.** The resolve already samples nearest, so the "retro filters, sparingly" northstar (`index.md` §1) is not engaged.
- **Game UI renders into its own native-resolution layer. The resolve composites it over the upscaled, effected scene.**
  - The resolve stays the gameplay path's sole swapchain writer (§7.8). That keeps M13's no-parallel-resolves rule and E23-accessibility's seam.
  - Frontend frames composite the same way.
  - UI keeps its private depth target and is not tonemapped.
  - The layer gives HUD-covering effects, such as the roadmap's optional CRT filter, one home. Rivals are recorded in `research.md`.
- **Screen effects apply to the scene only.**
  - Each effect has a covers-HUD switch in the one resolve shader, all off. Synced UI shake or flash later flips a switch rather than adding a second source of effect state.
  - Diverges from M13's shipped criteria (`done/M13--screen-space-effects`) by owner decision: shake moved the whole composited image, HUD included, and flash covered scene and HUD.
- **`fog_pixel_scale` and pixelated bloom divide the scene extent, not the surface.** An authored map's fog and bloom blocks then look the same on a 1× display and on HiDPI at Auto.
- **Placement:**
  - The renderer owns the extents, upscale, UI layer and composite.
  - The binary forwards window events.
  - Player options own the policy.
- **Not in this brief:**
  - **Window modes:** a follow-up brief. They raise boot-order and E23 U4 window-creation questions this work does not need.
  - **UI shake and flash:** only the switches above.
  - **Fractional scales, smooth filtering, dynamic resolution:** each needs a filter or budget decision this brief does not make.
  - **Temporal upscaling:** `drafts/motion-reprojection-layer` names it, and nothing here forecloses it.
  - **Forward-shader cost:** the dominant per-pixel work, but a separate problem.
- **Coordination:** the drafts `E23--photosensitivity-source-floor` and `coop-trigger-screen-effects` assume flash acts in the resolve, which still holds. With HUD flash off, the source-floor work treats UI as its own source.

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
- [ ] Capture produces an image at its requested resolution, at divisor 1 regardless of the render-resolution option.
- [ ] Game UI records into the native-resolution UI layer with its own depth target, never into scene colour. The resolve is the only gameplay pass that writes the swapchain. The debug UI still records after it. Frontend frames composite the layer too.
- [ ] Each screen effect's covers-HUD switch defaults to off. With all switches off, effect strength leaves opaque UI pixels unchanged. Translucent UI pixels change only through the scene beneath them. With one switch on in a test, only that effect reaches the UI.
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
- [ ] Dragging the window between displays with different scale factors updates the scene extent. No stretched frame appears.

## Path
- **Seams:**
  - `Renderer::resize` and `renderer_full_init` size the targets.
  - `screen_effects` (`encode_resolve`, its resize) owns scene colour and the resolve.
  - `UiPass::new` currently takes `SCENE_COLOR_FORMAT`, and its private depth target sizes to its viewport.
  - The UI viewport is set in `renderer_render_frame`; `ui::layout::device_scale` handles UI scale.
  - `startup/render_profile.rs` and the options bridge carry the option.
  - `WindowEvent::Resized` in `main.rs` rebuilds in the handler today; the camera aspect is set beside it. egui-winit already tracks scale-factor changes for the debug UI.
- **Policy crossing:** render-profile maps the option to renderer vocabulary, for example "auto with an N-row cap" or "fixed divisor d". The renderer combines that with the surface size and scale factor at the extent chokepoint.
- **Shape:** an explicit scene-extent value threaded through resize. Rival: per-target scale fields. Rejected, because each target would re-derive the extent and could drift.
- **First slice:** add the scene extent with a hardcoded divisor of 2 and the nearest upscale in the resolve, leaving UI where it is. Measure Auto against Native on the Mac. That falsifies the fill-bound premise before the UI pass moves.
- **UI layer:** it clears to transparent each frame and is sampled 1:1 by the resolve. The text atlas is commented as built for an sRGB surface, so check colour parity now that it targets an sRGB layer rather than HDR scene colour.
- **Large file:** `renderer_render_frame.rs` is past the split threshold. UI-layer recording moves out of it rather than growing it.
- **Background:** derivation, measurements and lifecycle diagrams are in `research.md`.

## Open questions
- Label text and placement of the options row — **delegated**
- Whether the covers-HUD switches are uniform flags or compile-time constants — **delegated**
