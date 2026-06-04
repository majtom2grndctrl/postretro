# M13 Goal A — UI Render Pass + Splash Reimplementation

## Goal

Stand up a real `render/ui/` peer pass and prove it on a *real screen*:
reimplement the boot splash on the new UI foundation. The splash's panel, logo
image, and a shaped-text line draw end-to-end through the new pass —
native-resolution render, 1280×720 logical-reference layout, device-pixel-snapped
quads, `glyphon` anti-aliased text. `render/splash.rs`'s `SplashPipeline` is
retired by consolidation, not paralleled. Per the M10 thin-vertical-slice
philosophy this is code that survives, built in the target module layout behind
seams Goals B/C/D/F fill in place — locking the UI layer's foundational contracts
against a live screen with no gameplay state.

## Scope

### In scope

- New `render/ui/` module — sibling pass to scene rendering (research §4), all
  wgpu inside it per renderer-owns-GPU.
- Hand-rolled instanced-quad / 9-slice pipeline for **panels and images**: one
  shader, per-instance data (rect, UV rect, color, 9-slice margin), alpha-blended,
  depth disabled (research §5, §13). One batch samples a single bound texture, so
  the slice composes as a few draws: solid panels sample a 1×1 white texel in one
  instanced batch; the logo is a separate draw with its texture bound. Text is
  glyphon's own draw (below), not this pipeline. A general texture-array/atlas
  batch that mixes many textures in one draw is **Goal B**. `SplashPipeline` is
  the structural template and is retired into this successor.
- **Native-res render, 1280×720 logical reference.** UI lays out in
  logical-reference coords, scaled by `native / reference` to the backbuffer,
  rendered at native res. Panel/image quads snap to integer **device** pixels;
  glyphs render AA. No offscreen UI target, no resolve/upscale. Scale factor
  re-derives from `surface_config`.
- **`glyphon`-shaped AA text as the default text path** from this slice onward.
  `glyphon` becomes a workspace dependency; the slice ships one committed TTF.
  glyphon ships its own pipeline and atlas: its `TextRenderer` records text into
  the **same render pass / surface view** as the quad draws, after them. The
  hand-rolled quad pipeline never carries glyphs.
- Pass placement: records inside `render_frame_indirect` after the
  world/fog/wireframe passes, `LoadOp::Load` into the surface `view`, before the
  un-presented texture returns — beneath the egui overlay (research §13). Present
  stays caller-driven (research §4).
- **Splash reimplementation as the slice.** Splash content — fullscreen background
  fill, a framed (non-fullscreen) 9-slice panel, centered logo image, one
  shaped-text line (version/tagline) — is a hardcoded Rust descriptor drawn through
  the UI pass, behind one named seam so B (descriptor model) + G1 (SDK) later make
  it script-authored. A has **no script ingestion**. Runs pre-gameplay (before any
  level/game logic/state). The splash has no text today; the shaped-text line is
  **added here specifically to exercise the glyphon path**, not ported.
- Once-per-frame published read handle: a narrow read-only snapshot **stored on
  the `Renderer`** after game logic, before render (research §4) — not threaded as
  a render-call parameter, keeping both render signatures stable. Carries the
  splash text; the handle *shape* is the contract. Pre-gameplay it holds no
  gameplay state — which stress-tests the once-per-frame contract in isolation.
- Input-stage UI-dispatch seam + modal capture-vs-passthrough contract (research
  §4, §12): a tap point ahead of the gameplay input forward; the descriptor's
  mode decides consume-vs-passthrough; a UI-consumed event on frame N reaches game
  logic no earlier than N+1. Capture routes through the reserved
  `InputFocus::Menu` path; the splash descriptor is capture.
- Test harness: CPU draw-list / layout assertions as the **hard gate**; optional
  tolerance-scoped golden, self-skipping with no GPU adapter.

### Out of scope

- Full script-authored / moddable splash — the **endpoint** reached at **B + G1**.
  A gets on that path and de-risks it against a real screen; B/G1 land with no
  rework because the descriptor seam is already in place. A ingests no script.
- General descriptor model, serde wire format, `taffy`, full widget vocab —
  **Goal B**. A places a fixed handful of splash elements by hand against anchors;
  **no `taffy` dependency in A**.
- State system (`defineState`, `StateValue<T>`, slot table) — **Goal C**.
- Multi-font registration, theme tokens — **Goal D**. A registers one TTF and
  uses `glyphon` directly.
- `styleRanges`, `onStateCrossing` — **Goal E**.
- Input breadth: hit-testing, focus ring, nav intents, hold-to-repeat, modal
  stack, gamepad — **Goal F**. A locks only the dispatch seam and the frame-order
  contract.
- **Refactoring the boot sequence.** A moves only the splash's *drawing*. The boot
  state machine (`run_splash_frame` frame 0/1/2+ schedule, `BootState`, timing
  records, worker spawn, mod-init hook, level handoff) is untouched. Scope-guarded.
- Screen-space effects — **SE**. egui retirement, built-in screens — **BIS**.
  A runs alongside the unmodified egui overlay.

## Acceptance criteria

- [ ] The boot splash renders through the new `render/ui/` pass: a fullscreen
  background fill, a framed 9-slice panel, the existing logo image, and a
  shaped-text line, anti-aliased and crisp at 720p and at 4K with no re-layout
  artifacts between resolutions.
- [ ] `SplashPipeline` (`render/splash.rs`) is removed from the tree; no second
  quad pipeline exists beside the UI pass. The renderer's splash entry points
  (`render_splash_frame` / `install_splash_from_loaded` / `clear_splash`) drive
  the UI pass.
- [ ] The boot timing and frame schedule are unchanged: the engine still paints a
  black frame, then the splash, then polls the worker and transitions to the
  level — observable as identical boot-timing log lines and an unchanged
  black→splash→level progression.
- [ ] Panel and image quads are device-pixel-snapped (no subpixel edge blur);
  glyphs are AA. On window resize the splash stays anchored and re-derives its
  scale from the backbuffer without stretching the logo or text.
- [ ] The 9-slice panel preserves corner sizes when its rect grows — corners do
  not stretch; edges/center tile or stretch per the 9-slice rule.
- [ ] A descriptor marked capture consumes a pointer/key event so it does not
  reach the gameplay input system that frame; a passthrough descriptor lets the
  same event through. Verifiable by toggling the mode on the splash descriptor.
- [ ] An event consumed by UI on a frame is observable to game logic no earlier
  than the following frame — never same-frame.
- [ ] `cargo test -p postretro` runs a draw-list / layout assertion that fails if
  the splash anchor or logical-reference→device scale math regresses (the produced
  quad rects move) — independent of any GPU adapter.
- [ ] An optional headless golden test renders the UI pass and compares within a
  tolerance; it self-skips cleanly when no GPU adapter is present and is not the
  hard gate (AA text makes exact goldens backend-fragile).
- [ ] No new `unsafe`; byte packing goes through `bytemuck`.

## Tasks

### Task 1: UI pass + instanced-quad pipeline scaffolding
Create `render/ui/` with a pass struct owning its pipeline, BGL, sampler,
vertex/instance buffers, and uniform buffer — modeled on `SplashPipeline`. One
`.wgsl` under `src/shaders/` for the quad/9-slice program: instanced draws,
alpha blend, depth disabled; the vertex shader expands a unit quad per instance
(rect, UV rect, color, 9-slice margin). The pass exposes an `encode`-style entry
recording into a target view. Declare `pub mod ui;` in `render/mod.rs`; the
`Renderer` owns the pass and builds it in `Renderer::new` alongside `fog`.
This pipeline draws **panels and images only** — never text. Solid panels sample
a 1×1 white texel (degenerate UV slice) so an untextured panel and a textured
image share one instanced batch; the logo binds its own texture as a separate
draw. Text is glyphon's own pipeline (Task 3), not this one.

### Task 2: Logical-reference scaling model + device-pixel snap
Establish the 1280×720 logical reference and a scale factor derived from
`surface_config.{width,height}` at encode time. A small layout/projection helper
maps logical-reference rects to device-pixel rects: scale, then snap panel/image
rects to integer device pixels (text keeps AA positions). **Architectural
constraint:** layout/projection emits a pure CPU-side **draw list** — a `Vec` of
quad-instance records (device-pixel rect, UV rect, color, 9-slice margin) — built
with **no wgpu call**. The pass then uploads that list to its instance buffer.
The layout step holds no GPU handles; this is what makes the Task 6a assertion
GPU-independent. (Text positions resolve through glyphon's `prepare`, not this
list.) No offscreen target; the pass uniform carries the device viewport so the
shader maps snapped device rects to clip space. On resize the factor re-derives
from the updated `surface_config` — wire through the existing `Renderer::resize`
path.

### Task 3: glyphon shaped-text path
Add `glyphon` as a workspace dep; commit one TTF under `content/base/`. The UI
pass owns glyphon's own state — `FontSystem`, `SwashCache`, `Cache`, `Viewport`,
`TextAtlas` (built with the surface format), and `TextRenderer` — and registers
the font once. glyphon ships its own pipeline; it is **not** routed through the
Task 1 quad pipeline. Per frame the pass calls `TextRenderer::prepare` (CPU
layout + atlas upload, given a `Buffer`/`TextArea` shaped at the device-scaled
font size, positioned in device pixels) before recording draws, then records
`TextRenderer::render(&atlas, &viewport, &mut pass)` **into the same render pass
as the quad draws, after them**, so text composites into the same surface view.
This is the engine default text path. The `Viewport`/`Resolution` is set from the
device backbuffer size. Confirm glyph coverage blends in the correct color space
against the sRGB surface (Open question).

### Task 4: Splash descriptor + read handle + retire SplashPipeline
Define the splash content as a hardcoded Rust descriptor behind one named seam:
a **framed (non-fullscreen) 9-slice panel** centered in logical-reference space,
the logo image (reusing `UiTexture` upload) centered over it, and a shaped-text
line below the logo — all anchored in logical-reference space with a
capture/passthrough mode flag. A separate fullscreen background fill (the existing
`SPLASH_BG_COLOR`) sits behind the framed panel; because the panel is framed, its
9-slice corners are genuinely exercised on screen. The logo draws at a **fixed
logical-reference size** (a constant W×H in 1280×720 space, preserving the PNG's
aspect) so "without stretching" holds at every backbuffer size — only the
uniform device scale applies, never an independent x/y stretch.
**Read-handle delivery (plumbing):** the once-per-frame snapshot is **stored on
the `Renderer`** via a setter the App calls just before each render call — not a
render-call parameter, so both render signatures stay stable (`render_splash_frame`
takes no content args today; `render_frame_indirect` is already wide). `App` calls
the setter before `paint_splash`'s `render_splash_frame()` (splash phase) and
before the `render_frame_indirect` call in the `RedrawRequested` arm (gameplay
path); the pass reads the stored snapshot when it records. Rewire the renderer's
splash entry points (`render_splash_frame` / `install_splash_from_loaded` / `clear_splash`)
to drive the UI pass instead of `SplashPipeline`; **delete `SplashPipeline`** and
its shaders (`splash_vert.wgsl` / `splash_frag.wgsl`), keeping `load_splash` /
`upload_splash_texture` for the logo image. The App-side `run_splash_frame` /
`paint_splash` schedule and all boot timing/hooks stay byte-for-byte intact.

### Task 5: Input-stage UI-dispatch seam + frame-order contract
Add a UI tap point in the Input stage (`App::window_event` / `device_event`)
ahead of the gameplay input forward, mirroring the `egui_consumed` gate. The
active descriptor's capture/passthrough mode decides whether the event is
consumed by UI or forwarded to gameplay; capture routes through the reserved
`InputFocus::Menu` path. Guarantee any UI-consumed result is queued for game
logic no earlier than the next frame's tick — no same-frame path.

### Task 6: Test harness
(a) CPU draw-list / layout assertions (hard gate): feed the splash descriptor + a
known backbuffer size through layout, assert the produced device-pixel quad rects
(anchor, scale, snap, 9-slice corners) — no GPU. (b) Optional headless golden:
build a wgpu device via `pollster` (the `curve_eval_test` /
`sdf_light_select_test` pattern), render the UI pass, read back, compare within a
tolerance; self-skip with no adapter. Wire both into `cargo test -p postretro`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the pipeline + pass topology everything draws through.
**Phase 2 (sequential):** Task 2 — consumes Task 1's pass; establishes the scaling model + snap that Tasks 3–4 lay out against.
**Phase 3 (concurrent):** Task 3 (glyphon text) and Task 5 (input seam) — independent; Task 3 records glyphon's own text draw into the Task 1 pass at the Task 2 device scale, Task 5 touches only the Input stage.
**Phase 4 (sequential):** Task 4 — consumes Tasks 1–3 (panel/image/text draws) and the read-handle plumbing, retires `SplashPipeline`, ties the descriptor to the live boot frame.
**Phase 5 (sequential):** Task 6 — asserts Task 2/4 layout math and (optionally) the Task 1–4 rendered output.

## Rough sketch

Named types/files (behavior is in Tasks above):

- `UiPass` struct in `render/ui/`, owned on `Renderer`, built in `Renderer::new`.
  Holds the quad pipeline, BGL, sampler, instance + uniform buffers, glyphon's own
  state (`FontSystem`, `SwashCache`, `Cache`, `Viewport`, `TextAtlas`,
  `TextRenderer`), and the uploaded logo texture. Quad shader
  `src/shaders/ui_quad.wgsl`; uniform carries the device viewport. The quad
  pipeline draws panels + images; `TextRenderer::render` records glyphon's text
  draw after them into the same pass.
- `render_frame_indirect` records the pass into the surface `view`
  (`LoadOp::Load`) after fog/wireframe; the splash phase records the same pass via
  `render_splash_frame` into its own surface frame. On the gameplay path
  (`render_frame_indirect`) the pass draws an **empty draw list** in A — frame-order
  placement is locked now, UI content arrives with B/BIS.
- Splash descriptor: a `pub(crate)` struct (framed 9-slice panel + image + text
  line + anchor/offset + capture flag) built behind one named builder — the seam B
  replaces with descriptor parsing, G1 with script ingestion.
- Read handle: a narrow `pub(crate)` snapshot **stored on the `Renderer`** via a
  setter the `App` calls just before each render call — not a render-call
  parameter; both render signatures stay stable.
- Input seam: a UI dispatch check in `App::window_event` paralleling
  `egui_consumed`, routed through `InputFocus::Menu`.
- Byte packing via `bytemuck`. No `unsafe`.

## Boundary inventory

Not applicable to A. The splash descriptor is hardcoded behind a named Rust seam;
no script, serde, wire, or FGD name crosses a boundary. The descriptor wire format
and its Rust↔JS↔Luau casing are a first-class **Goal B** deliverable; the
persisted slot format is **Goal C**. No cross-boundary name is introduced here.

## Wire format

Not applicable. A adds no PRL section and no persisted binary. The logo PNG and
the TTF are committed assets, not engine binary formats; persisted UI state is
Goal C.

## Open questions

- **AA text into the sRGB swapchain (color space).** glyphon's atlas/blend vs.
  the sRGB-when-available surface format: confirm glyph coverage blends in the
  correct space so text edges are neither over- nor under-darkened, and that the
  panel alpha-composite matches the existing splash background math
  (`SPLASH_BG_COLOR`, linear sRGB(21,27,35)). Resolve against glyphon's wgpu
  integration at implementation.
- **glyphon ↔ wgpu version pairing.** A pins `glyphon` to the release compatible
  with the workspace `wgpu = "29"`; the exact version is resolved at `cargo add`
  time, not asserted here. If no compatible glyphon release exists yet for the
  pinned wgpu major, raise before committing the dep.
- **Golden-image portability.** AA glyphs rasterize subtly differently per
  backend/driver. The CPU draw-list / layout assertion is the hard gate; the
  golden is tolerance-scoped or skipped. Decide the tolerance (or skip) at
  implementation.
- **Capture-result queueing.** The structure carrying a UI-consumed result to
  next-frame game logic (pending-intent queue vs. flag) is left to Task 5; the
  contract is "not same-frame." Goal F defines the intent vocabulary.
- **Splash text content source.** A's shaped-text line (version/tagline) is
  hardcoded in the descriptor; confirm whether the version string reads from an
  existing build constant or is a literal in A. Either keeps the descriptor seam
  intact for B/G1; pick the simpler at implementation.
