# M13 Goal A — UI Render Pass + Thin Vertical Slice

## Goal

Stand up a real `render/ui/` peer pass and drive one hardcoded UI descriptor — a
9-slice panel plus a line of baked-bitmap-font text — end-to-end through the live
render path. Per the M10 thin-vertical-slice philosophy, this is real code that
survives, built in the target module layout from the first commit, behind seams
that Goals B/C/D/F fill in place. The slice exists to lock the foundational
contracts of the UI layer, not to ship a feature.

## Scope

### In scope

- New `render/ui/` module, a sibling pass to scene rendering (research §4), with
  all wgpu inside it per renderer-owns-GPU.
- A hand-rolled instanced-quad / 9-slice pipeline: one shader, one vertex buffer,
  per-quad instance data, alpha-blended (research §5, §13, §15).
- A fixed design-resolution offscreen target (320×240 base) drawn into at integer
  pixel coordinates, then upscaled nearest-neighbor into the final color target.
- Integer pixel snapping of all computed quad rects in design space before draw
  (research §7).
- Render-pass placement: UI composites after the scene/fog passes, before the
  caller presents (and beneath the egui overlay). Alpha-blended into the swapchain
  view (research §13).
- A baked bitmap font: a committed glyph-atlas PNG + a metrics table, uploaded
  once, sampled by the text quads. The engine default text path (research §5).
- One hardcoded descriptor behind a single named seam: a panel anchored
  bottom-left with a text line inside it. Anchor is one of the nine screen
  positions plus a pixel offset (research §7).
- A once-per-frame published read handle: a narrow read-only snapshot of engine
  state handed to the UI pass after game logic completes, before render
  (research §4). In A it carries placeholder content for the text line; the handle
  shape is the contract.
- The Input-stage UI-dispatch seam: a tap point ahead of the gameplay input
  forward where a top-level descriptor's capture-vs-passthrough mode decides
  whether an event is consumed by UI or passed to game logic, plus the frame-order
  guarantee that a UI-consumed event resolved on frame N reaches game logic no
  earlier than frame N+1 (research §4, §12). A wires the seam and the modal
  capture/passthrough mode flag; the hardcoded HUD descriptor is passthrough.
- A test harness proving GPU-drawn UI is verifiable: layout-tree / draw-list
  assertions below the draw call, plus a headless readback (golden) check of the
  rendered design-res target. Self-skips when no GPU adapter (existing test
  precedent).

### Out of scope

- General descriptor model, serde wire format, taffy integration, full widget
  vocabulary — **Goal B**.
- State system: `defineState`, `StateValue<T>`, the slot table, static proxy —
  **Goal C**. A's read handle carries placeholder bytes, not a slot schema.
- glyphon shaped text, TTF registration, theme tokens — **Goal D**. A uses only
  the baked bitmap font.
- `styleRanges`, `onStateCrossing` — **Goal E**.
- Input breadth: hit-testing, focus ring, nav intents, hold-to-repeat,
  pointer-vs-focus mode switching, the full modal stack, gamepad — **Goal F**.
  A locks only the dispatch seam and the capture/passthrough frame-order contract.
- SDK / script ingestion of descriptors — **G1/G2**. The A descriptor is hardcoded
  in Rust.
- Screen-space effects (vignette, flash, shake) — **SE**.
- Built-in screens, egui retirement — **BIS**. A runs alongside the egui overlay.
- Replacing or modifying the egui debug overlay.

## Acceptance criteria

- [ ] On a running level, the hardcoded panel renders at the bottom-left anchor
  with its text line inside, composited over the scene and beneath the egui debug
  overlay when the overlay is open.
- [ ] The panel and text are pixel-crisp (nearest-neighbor, no subpixel blur) and
  stay anchored bottom-left across window resizes and aspect-ratio changes; the
  design-res content scales by integer-consistent nearest upscale, not stretch.
- [ ] The 9-slice panel preserves its corner sizes when the panel rect grows —
  corners do not stretch, edges/center tile or stretch as the 9-slice rule dictates.
- [ ] Disabling the egui overlay leaves the UI panel + text still drawn; enabling
  it draws egui on top — the two coexist with no flicker or z-fighting.
- [ ] A top-level descriptor marked capture consumes a pointer/key event so it does
  not reach the gameplay input system that frame; a descriptor marked passthrough
  (the A HUD) lets the same event through to game logic. Verifiable by toggling the
  mode on the hardcoded descriptor.
- [ ] An event consumed by UI on a frame is observable to game logic no earlier
  than the following frame — never same-frame (frame-order contract).
- [ ] `cargo test -p postretro` runs a layout/draw-list assertion that fails if the
  bottom-left anchor math or pixel-snap regresses (the produced quad rects move).
- [ ] `cargo test -p postretro` runs a headless render of the design-res target and
  compares it to a committed golden image; it fails on a visual regression and
  self-skips cleanly when no GPU adapter is present.
- [ ] No new `unsafe`; byte packing goes through `bytemuck` as elsewhere in the
  renderer.

## Tasks

### Task 1: UI pass + instanced-quad pipeline scaffolding
Create `render/ui/` with a pass struct that owns its pipeline, bind-group layout,
sampler, vertex/instance buffers, and uniform buffer — modeled on `SplashPipeline`
(`render/splash.rs`). One `.wgsl` under `src/shaders/` for the quad/9-slice
program: instanced draws, alpha blend, depth disabled. The pass exposes an
`encode`-style entry that records into a target view. Declare `pub mod ui;` in
`render/mod.rs`; the `Renderer` struct owns the pass and builds it in
`Renderer::new` alongside `splash_pipeline` / `fog`.

### Task 2: Design-resolution target + nearest upscale
Allocate a fixed 320×240 offscreen color texture owned by the UI pass. Task 1's
quad pass renders into it at integer design-space coordinates; a second draw
upscales it nearest-neighbor into the swapchain `view`. Recreate-or-keep on window
resize (the design target is fixed; only the upscale rect changes). Pixel-snap all
computed rects to integers in design space before emitting quads.

### Task 3: Baked bitmap font draw path
Commit a glyph-atlas PNG + a metrics table (per-glyph atlas rect + advance) under
`content/base/`. Upload the atlas once (reuse the `UiTexture` RGBA8 shape). A text
layout routine turns an ASCII string into a run of textured quads using monospace
or table advances, snapped to integer pixels, sampled from the atlas by the Task 1
pipeline. This is the engine default text path.

### Task 4: Hardcoded descriptor + once-per-frame read handle
Define the single named seam: a hardcoded descriptor (bottom-left-anchored panel +
text line, with a capture/passthrough mode flag) and a narrow read-only
per-frame handle published after game logic and before `render_frame_indirect`.
In `main.rs`'s `RedrawRequested` arm, publish the handle just before the render
call; the UI pass reads it, lays out the descriptor (anchor + offset → snapped
rects), and emits Tasks 1–3 draws. The handle carries placeholder text bytes in A.

### Task 5: Input-stage UI-dispatch seam + frame-order contract
Add a UI tap point in the Input stage (`App::window_event` / `device_event` in
`main.rs`) ahead of the gameplay input forward, mirroring the existing
`egui_consumed` gate. The active descriptor's capture/passthrough mode decides
whether the event is consumed by UI or forwarded to the gameplay `input_system`.
Extend `InputFocus` usage so UI capture routes like the reserved `Menu` focus.
Guarantee any UI-consumed result is queued for game logic no earlier than the next
frame's tick — no same-frame path.

### Task 6: Test harness
Two test layers. (a) Layout/draw-list assertions: feed the hardcoded descriptor +
a known viewport through layout, assert the produced quad rects (anchor, snap,
9-slice corners) — pure CPU, no GPU. (b) Headless golden: build a wgpu device via
`pollster` (the `curve_eval_test` / `sdf_light_select_test` pattern), render the
design-res target, read it back, compare to a committed golden PNG; self-skip when
no adapter. Wire both into `cargo test -p postretro`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the pipeline + pass topology everything draws through.
**Phase 2 (sequential):** Task 2 — consumes Task 1's pass; establishes the design-res target and snap that Tasks 3–4 draw into.
**Phase 3 (concurrent):** Task 3 (font path) and Task 5 (input seam) — independent; Task 3 draws into the Task 2 target, Task 5 touches only the Input stage.
**Phase 4 (sequential):** Task 4 — consumes Task 1–3 draws and the read handle plumbing; ties the descriptor to the live frame.
**Phase 5 (sequential):** Task 6 — asserts the behavior of Tasks 2–4 (layout/snap) and Task 1/2 (rendered target).

## Rough sketch

- Pass module `render/ui/` peers `render/splash.rs`, `render/fog_pass.rs`. A
  `UiPass` struct holds the quad pipeline, BGL, nearest `ClampToEdge` sampler,
  instance + uniform buffers, the 320×240 offscreen texture + view, the upscale
  pipeline, and the uploaded font atlas. Built in `Renderer::new`, owned on the
  `Renderer` struct.
- Quad shader `src/shaders/ui_quad.wgsl`: per-instance rect, UV rect, color, and a
  9-slice margin; vertex shader expands a unit quad. One pipeline serves panel
  9-slice and text glyphs (text quads use a 1×1 logical slice).
- The UI draw records into the offscreen target inside `render_frame_indirect`
  after the fog/wireframe passes; the upscale draw blits into `view` with
  `LoadOp::Load`, before the function returns the un-presented surface texture.
  egui (`render_debug_ui`) still composites after, on top.
- Read handle: a small `pub(crate)` snapshot struct published once per frame
  (placeholder text bytes in A). Owner is `App`; writer call-site is the
  `RedrawRequested` arm, just before `render_frame_indirect`.
- Input seam: a UI dispatch check in `App::window_event` paralleling the
  `egui_consumed` precedent; capture routes through an `InputFocus`-style gate so
  the gameplay `input_system.handle_*` calls are skipped on capture. The
  capture/passthrough flag lives on the hardcoded descriptor.
- Byte packing via `bytemuck` (workspace dep). No `unsafe`.

## Boundary inventory

Not applicable. Goal A is internal Rust: the descriptor is hardcoded behind a
named Rust seam; no script, serde, wire, or FGD name crosses a boundary. The
descriptor wire format and its Rust↔JS↔Luau casing are a first-class Goal B
deliverable; the persisted slot format is Goal C. No cross-boundary name is
introduced here.

## Wire format

Not applicable. Goal A adds no PRL section and no persisted binary. The bitmap-font
atlas is a committed PNG asset, not an engine binary format; persisted UI state is
Goal C.

## Open questions

- **Font atlas authoring.** Does A hand-author the glyph atlas PNG + metrics, or
  add a tiny offline baker? A committed PNG keeps A minimal; a baker is Goal D's
  territory. Leaning committed asset for the slice.
- **Design-res target color space.** The offscreen target format vs. the sRGB
  swapchain: confirm the upscale blends in the correct space so the golden image is
  stable across adapters. (Surface format is sRGB-when-available; the offscreen
  target's format is the implementor's call within the snap/no-blur constraint.)
- **Golden-image portability.** Headless rasterization can differ subtly per
  backend/driver. If exact-match goldens prove flaky in CI, fall back to a
  tolerance threshold or rely on the CPU draw-list assertion as the hard gate and
  treat the golden as advisory. Decide at implementation.
- **Capture queueing mechanism.** The exact structure that carries a UI-consumed
  result to next-frame game logic (a pending-intent queue vs. a flag) is left to
  Task 5; the contract is "not same-frame." Goal F defines the intent vocabulary.
