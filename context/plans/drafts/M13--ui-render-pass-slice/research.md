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
- `render_frame_indirect` returns `Result<Option<wgpu::SurfaceTexture>>`
  (confirmed signature at `render/mod.rs:3279`; on Timeout/Occluded/Outdated it
  returns `Ok(None)`). The caller (`App::window_event`,
  `WindowEvent::RedrawRequested` in `main.rs:1081`) takes the returned texture,
  optionally appends the egui overlay (`render_debug_ui`, a *separate* encoder
  submission with `LoadOp::Load`, `cfg(dev-tools)` only), then calls
  `surface_texture.present()` (`main.rs:1113`). The UI pass must record before
  that return so it composites into the same `view`, ahead of egui and present.

## Frame loop / frame order

- Single driver: `App::window_event` `WindowEvent::RedrawRequested` arm in
  `main.rs`. Fixed-timestep accumulator; `ticks` game-logic ticks per frame.
  **The splash phase short-circuits this arm** — see the boot state machine
  below; gameplay snapshot/tick/render only run once `boot_state == Running`.
- **Input stage** is `App::window_event` (and `device_event`): winit events feed
  `input_system` / `diagnostic_inputs`. egui already taps this stage first
  (`debug_ui.on_window_event` → `egui_consumed`, `main.rs:516`), and gameplay
  forwarding is gated on `input_focus == InputFocus::Gameplay` plus
  `!egui_consumed` (`main.rs:521`, `618`, `630`). This is the exact precedent the
  UI-dispatch seam mirrors: a UI tap point ahead of the gameplay forward,
  capture gated by focus/passthrough.
- `InputFocus` enum (`input/focus.rs`, re-exported `input::InputFocus`):
  variants `Gameplay`, `DevTools`, `Menu`. **`Menu` is reserved with no
  consumer** — wired through `App::set_input_focus` / `reapply_focus`
  identically to `DevTools` (cursor release), `#[allow(dead_code)]`, pinned by an
  exhaustiveness test (`input_focus_variants_are_exhaustive`). The modal
  capture-vs-passthrough contract is the first `Menu` consumer.
- Game logic (movement/weapon ticks, bridges) runs in the same arm after input
  snapshot; render is `render_frame_indirect`; present is
  `surface_texture.present()`. "Published read handle after game logic" attaches
  just before the `renderer.render_frame_indirect(...)` call.

## Boot sequence / splash invocation (grounded — drives the reframe)

- Boot state machine `BootState { Booting, Splash, Running }` on `App`, driven in
  the `RedrawRequested` arm (`main.rs:694`). `Splash` calls
  `run_splash_frame(event_loop)` (`main.rs:1250`); when it returns `false` the
  arm early-returns (only the splash painted), so no gameplay path runs
  pre-`Running`. Confirms Goal A's splash runs **pre-gameplay** — no level, no
  game logic, no state.
- `run_splash_frame` schedule (`main.rs:1250`):
  - **frame 0** — `paint_splash` a black frame (no splash bound). After present:
    `load_splash(SplashSource::Base)` + `install_splash_from_loaded` (decode +
    GPU upload + bind); records `splash_decoded`/`splash_uploaded` timings.
  - **frame 1** — `paint_splash` (splash now visible); records
    `first_splash_frame`; runs `mod_init`; optional `pending_splash_override`
    swap; spawns the level-load worker thread.
  - **frames 2+** — polls the worker channel; `paint_splash` each frame; on
    delivery uploads/installs the level, calls `renderer.clear_splash()`, and
    transitions to `Running`.
- `paint_splash` (`main.rs:1460`) calls `renderer.render_splash_frame()`.
- **What the splash actually draws (important):** a single static logo image,
  nothing else. `SplashPipeline::encode` (`render/splash.rs:284`) begins one
  render pass that **clears to `SPLASH_BG_COLOR`** (linear-space sRGB(21,27,35))
  and, when a splash texture is bound, draws a **fullscreen triangle** sampling
  the logo (`splash_vert.wgsl` centers it at `LOGO_SCALE = 0.4`, aspect-correct;
  `splash_frag.wgsl` alpha-composites the logo over `SPLASH_BG`, fills letterbox
  with `SPLASH_BG`). Texture is `Rgba8UnormSrgb`, nearest sampler, `ClampToEdge`.
  Asset: `content/base/textures/splash/postretro-ascii-art.png` (committed;
  there is also a `--white-on-black` variant).
- **There is NO text and NO fade/alpha-timing in the splash today.** The only
  "timing" is the boot frame counter (black → logo → poll). The reframe adds
  shaped text (a version/tagline line) as a *new* descriptor element; the spec
  must not claim it preserves an existing text or fade path — it preserves the
  boot **frame state machine** (`run_splash_frame`, the frame 0/1/2+ schedule and
  its timing records).
- Renderer splash surface (the call sites the reframe retires/rewires):
  `Renderer::splash_pipeline: SplashPipeline` field (`render/mod.rs:764`);
  `install_splash_from_loaded` (`:2494`), `render_splash_frame` (`:2506`),
  `clear_splash` (`:2546`); `load_splash` / `upload_splash_texture` free fns in
  `render/splash.rs`. `paint_splash` / `run_splash_frame` in `main.rs` are the
  App-side callers. These stay as the seam names; only the *drawing* underneath
  (`SplashPipeline` → UI pass) is replaced.

## Swapchain / resize / scale-factor source

- `Renderer.surface_config: wgpu::SurfaceConfiguration` (`render/mod.rs:563`) is
  the authoritative backbuffer size: `surface_config.width/height`. Surface
  format chosen sRGB-when-available in `Renderer::new`.
- `Renderer::resize(width, height)` (`render/mod.rs:2723`) is the single resize
  path: updates `surface_config`, reconfigures the surface, recreates the depth
  texture, resizes fog/SDF/spot-shadow targets, and calls
  `splash_pipeline.update_screen_size`. Driven from
  `WindowEvent::Resized` (`main.rs:531`). This is where the
  logical-reference→native scale factor is recomputed: native size =
  `surface_config.{width,height}`, logical reference = 1280×720, scale =
  native / reference. No offscreen UI target to resize — the UI pass reads the
  current `surface_config` size at encode time.

## egui integration (coexistence)

- `render/debug_ui/mod.rs`. CPU half `DebugUi` on `App.debug_ui`; GPU half
  `DebugUiGpu` on `Renderer.debug_ui_gpu` (lazy). All gated on `dev-tools`.
- egui draws in its own encoder/submission *after* `render_frame_indirect`
  returns, `LoadOp::Load` onto the surface. The new UI pass draws *inside*
  `render_frame_indirect` (Load onto `view` after the world passes), so it lands
  underneath the egui overlay — correct: egui is the dev overlay on top. During
  the splash phase egui is not drawn (`render_splash_frame` path).

## Pipeline / shader house pattern

- Shaders are `.wgsl` under `crates/postretro/src/shaders/`, pulled in via
  `include_str!`. Pipelines built per-pass in a struct holding
  `pipeline` + `bind_group_layout` + buffers (see `render/splash.rs`
  `SplashPipeline`, `fog_pass.rs` `FogPass`).
- `SplashPipeline` is the closest template for the UI quad pipeline: a single
  BGL (UBO + `Float{filterable:true}` texture + `Filtering` sampler),
  `encode(encoder, view)` records the pass. The UI pipeline generalizes this to
  instanced quads (per-instance rect/UV/color/9-slice margin) drawn directly into
  the surface `view` at native resolution — no offscreen target.
- `UiTexture` (`ui_texture.rs`) is the existing CPU RGBA8 upload struct reused by
  splash; the logo-image upload reuses the same shape unchanged.
- Byte packing via `bytemuck` (workspace dep); no `unsafe` in renderer pipeline
  code today.

## Dependency state (Cargo)

- **No `glyphon`, `taffy`, or `cosmic-text`** anywhere in `crates/` or
  `Cargo.lock` (confirmed). No committed TTF/OTF assets. Goal A introduces the
  first shaped-text path.
- Workspace deps live in root `Cargo.toml` `[workspace.dependencies]`. Goal A
  adds **`glyphon`** there and ships **one TTF** under `content/base/`. **`taffy`
  is NOT added in A** — A places a fixed handful of splash elements (panel, logo
  image, one shaped-text line) by hand against anchors + the logical-reference
  scale; full `taffy` flex/grid layout is Goal B. A's layout routine is a thin
  stand-in behind the same anchor/offset seam B generalizes.
- `glyphon` must match the workspace `wgpu` major (`wgpu = "29"`). The exact
  `glyphon` version that pairs with wgpu 29 is an implementation pick (Open
  question) — glyphon releases track wgpu versions and the compatible release is
  resolved at `cargo add` time, not asserted here.

## GPU-test precedent

- Headless wgpu tests build a device via `pollster` and self-skip when no adapter
  is present (`curve_eval_test` / `sdf_light_select_test` pattern). Reused for the
  optional golden image test. With AA glyphs at native res, exact-match goldens
  are backend-fragile — the CPU draw-list / layout assertion is the hard gate.
