# M13 Goal A — Code-grounding notes

Investigation findings that inform the spec but are not themselves decisions.
Source-confirmed against the tree on branch `claude/eloquent-cerf-J4S5r`.

## Render module topology

- Renderer lives at `crates/postretro/src/render/`. `Renderer` struct +
  `render_frame_indirect` in `render/mod.rs`. All wgpu calls are inside this
  module (renderer-owns-GPU holds).
- **No tonemap pass exists.** The forward pass writes directly into the sRGB
  surface texture view (`output.texture` view, called `view`). sRGB encode is
  the surface format's job. The research's "before tonemap/present" (§13)
  resolves in this tree to "after the world+fog+wireframe passes, before the
  caller presents."
- Pass order inside `render_frame_indirect` (all into the same `view`):
  compute cull → animated-LM compose → SH compose → depth pre-pass →
  spot-shadow → SDF shadow → **Textured (forward, LoadOp::Clear)** →
  Billboard sprite (LoadOp::Load) → Fog raymarch+composite (Load) →
  Wireframe overlay (Load, dev) → debug_lines (dev) → timing resolve →
  `queue.submit` → return `Some(output)` **un-presented**.
- `render_frame_indirect` returns `Result<Option<wgpu::SurfaceTexture>>`. The
  caller (`App::window_event`, `WindowEvent::RedrawRequested` in `main.rs`)
  optionally appends the egui overlay (`render_debug_ui`, a *separate* encoder
  submission with `LoadOp::Load`) and then calls `surface_texture.present()`.
  The UI pass must therefore record before that return so it composites into the
  same `view`, ahead of egui and present.

## Frame loop / frame order

- Single driver: `App::window_event` `WindowEvent::RedrawRequested` arm in
  `main.rs`. Fixed-timestep accumulator; `ticks` game-logic ticks per frame.
- **Input stage** is `App::window_event` (and `device_event`): winit events feed
  `input_system` / `diagnostic_inputs`. egui already taps this stage first
  (`debug_ui.on_window_event` → `egui_consumed`), and gameplay forwarding is
  gated on `input_focus == InputFocus::Gameplay` plus `!egui_consumed`. This is
  the exact precedent the UI-dispatch seam mirrors: a UI tap point ahead of the
  gameplay forward, capture gated by focus/passthrough.
- `InputFocus` enum (`input/focus.rs`, re-exported `input::InputFocus`) already
  models coarse capture (`Gameplay` vs. non-gameplay). The modal capture vs.
  passthrough contract extends this idea.
- Game logic (movement/weapon ticks, bridges) runs in the same arm after input
  snapshot; render is `render_frame_indirect`; present is `surface_texture
  .present()`. So "published read handle after game logic" attaches just before
  the `renderer.render_frame_indirect(...)` call.

## egui integration (coexistence)

- `render/debug_ui/mod.rs` (single file). CPU half `DebugUi` on `App.debug_ui`;
  GPU half `DebugUiGpu` on `Renderer.debug_ui_gpu` (lazy). All gated on the
  `dev-tools` cargo feature.
- egui draws in its own encoder/submission *after* `render_frame_indirect`
  returns, `LoadOp::Load` onto the surface. The new UI pass draws *inside*
  `render_frame_indirect` (Load onto `view` after the world passes), so it lands
  underneath the egui overlay — correct: egui is the dev overlay on top.

## Pipeline / shader house pattern

- Shaders are `.wgsl` under `crates/postretro/src/shaders/`, pulled in via
  `include_str!`. Pipelines built per-pass in a struct holding
  `pipeline` + `bind_group_layout` + buffers (see `render/splash.rs`
  `SplashPipeline`, and `fog_pass.rs` `FogPass`).
- `SplashPipeline` is the closest template for the UI quad pipeline: a single
  BGL (UBO + texture + sampler), nearest sampler with `ClampToEdge`, draws a
  fullscreen triangle, `encode(encoder, view)` records the pass. The UI pipeline
  generalizes this to instanced quads into a design-res offscreen target.
- Surface format chosen sRGB when available (`render/mod.rs` `Renderer::new`).
- `UiTexture` (`ui_texture.rs`) is the existing CPU RGBA8 upload struct reused by
  splash; the baked-font atlas upload can reuse the same shape.

## No existing text / bitmap font

- No bitmap-font, glyph, taffy, glyphon, or cosmic-text code anywhere in
  `crates/` or `content/`. Goal A introduces the first baked bitmap font and the
  first instanced-quad UI pipeline. `taffy`/`glyphon` are not yet workspace deps.

## Constraints confirmed

- No `unsafe` in the renderer pipeline code today; `bytemuck` is the byte-packing
  path. The UI pipeline can pack instance/uniform bytes the same way — no
  `unsafe` anticipated.
