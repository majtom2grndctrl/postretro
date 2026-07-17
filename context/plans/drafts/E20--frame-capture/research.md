# Frame Capture — research notes

Grounded against source at draft time. Ephemeral: line numbers drift. Confirm before editing code.

## Renderer is surface-coupled; no offscreen path exists

- `Renderer::new(window: &Arc<Window>)` — `crates/renderer/src/render/renderer_init.rs:24`. Creates the surface (`:32`), requests the adapter with `compatible_surface: Some(&surface)` (`:38`), picks an srgb surface format from surface caps (`:87-92`), sizes `surface_config` from the window (`:97-98`). Device requests the full feature/limit set up front (`request_renderer_device`, `:79`).
- Full renderer builds from `surface_config` (format/width/height): `finish_full_init` → `build_full_renderer(...)` — `renderer_init.rs:136-149`. The full renderer (pipelines, lighting, shadow pools, screen effects, mesh/UI/fog) is **surface-agnostic** — it needs only format + size.
- `render_frame_indirect(...)` — `crates/renderer/src/render/renderer_render_frame.rs:15`. **Unconditionally acquires the swapchain**: `self.acquire_present_handle("gameplay frame")?` (`:34`) → `handle.surface_view()` (`:38`); resolves + presents into it. A surfaceless path cannot call it unmodified.
- Surfaceless adapter is a trodden test path: `compatible_surface: None` in `ui/gpu_test_harness.rs:35`, `curve_eval_test.rs:98`, `sdf_light_select_test.rs:95`.

## scene_color: the capture source

- Allocated by `create_scene_color` — `crates/renderer/src/render/screen_effects.rs:244`. Surface format (srgb), surface-sized, single-sample. Usage `RENDER_ATTACHMENT | TEXTURE_BINDING` (`:261`) — **no `COPY_SRC`**.
- Every gameplay scene pass and gameplay UI pass writes into it; the resolve pass is the sole swapchain writer and composes the transient screen effects (flash/vignette/shake) on top (rendering_pipeline.md §7.8). Capturing `scene_color` gives the clean composited image **before** those transient effects.
- View accessor exists on the pass: `ScreenEffectsPass::scene_color_view() -> &wgpu::TextureView` (`pub`, `screen_effects.rs:178`). The underlying `wgpu::Texture` (`color_texture`, `screen_effects.rs:35`) has **no accessor**.
- The pass is `pub(super) screen_effects: ScreenEffectsPass` (`renderer_types.rs:726`) — reachable only inside the `render` module. There is **no** `Renderer`-level accessor for scene_color (view or texture). Capture must add one.

## Readback primitive already written (test-gated)

- `read_texture_rgba8(ctx, texture, width, height, encoder) -> Readback` — `crates/renderer/src/render/ui/gpu_test_harness.rs:76`. Does exactly: `copy_texture_to_buffer` with 256-byte row alignment (`:83-114`), submit, `map_async`, block on `device.poll(wait)` (`:119-125`), de-pad to a tight `width*4` RGBA8 buffer (`:128-132`). `#[cfg(test)]`. Production capture promotes this pattern to a non-test renderer-internal helper.
- Same pattern in production elsewhere: `sh_diagnostics.rs:373`, `frame_timing.rs:226`, `sdf_light_select_test.rs`.

## Camera view_proj (static pose is trivial)

- `crates/postretro/src/camera.rs`: `NEAR = 0.1` (`:11`), `FAR = 4096.0` (`:12`), `HFOV = 100°` (`:7`).
- App builds the matrix via `RenderCamera::new(position, aspect, yaw, pitch, roll, eye_offset)` — `camera.rs:34`. Projection: `vfov = 2*atan(tan(HFOV/2)/aspect)`, `Mat4::perspective_rh(vfov, aspect, NEAR, FAR)` (`:48-49`); view: `render_view_matrix` (`Mat4::look_at_rh`, `:70`); `view_projection = projection * view` (`:53`).
- Call site: `main.rs:2371` (`RenderCamera::new`), `main.rs:2379` (`view_proj`). Capture at a static pose passes `roll=0`, `eye_offset=Vec3::ZERO`, aspect from the scene resolution.

## Per-frame visibility inputs (`render_frame_indirect` args)

App computes these each frame in `App::redraw`, `main.rs`:
- `determine_visible_cells(eye, view_proj, world, capture_portal_walk, &mut scratch)` — `main.rs:2392`; result destructured `:2413-2417` → `visible_cells`, `fog_reachable`, `stats` (`stats.camera_cell`, `stats.path`).
- `CameraCullVisibility { cells, path }` — `main.rs:2910`.
- `light_reachable_cell_mask: Vec<bool>` from `fog_reachable` — `main.rs:2450-2463`.
- `reachable_cell_aabbs: Vec<(Vec3,Vec3)>` from `fog_reachable` cell bounds — `main.rs:2476-2485`.
- Empty-world fallback (`VisibleCells::DrawAll`, empty `fog_reachable`, `camera_cell: 0`) — `main.rs:2399-2412`.

A static-pose capture reproduces this block: one `determine_visible_cells` call for the static pose, then derive mask + AABBs from `fog_reachable`.

## World renderer-upload sequence (batch runner skips all of this)

On `Renderer`, called from `App::install_level_payload` (`crates/postretro/src/startup/lifecycle.rs:548`), world-relevant subset in order:
- `install_textures(&world.texture_names, &world.texture_cache_keys, &prm_cache_root, &texture_materials)` — `:633`.
- `normalize_world_uvs(&mut world)` — `:644`.
- geometry: `render::level_world_to_geometry(&world, &texture_materials)` (`:648`) → `install_level_geometry(&geometry)` (`:649`). Uploads vertex/index/BVH **and baked lightmap + SH atlases** from the PRL.
- `set_fog_pixel_scale(world.fog_pixel_scale)` (`:825`), `install_fog_cell_masks_for_level(world.fog_cell_masks.clone())` (`:826`).

These are all `pub` on `Renderer` and callable without `App`. World-only capture needs no scripting core, no `HeadlessSession`, no scripts-build sidecar, no archetype sweep.

- Dynamic light-bridge populate (`session.light_bridge.populate_from_level`, `:686`) and the archetype/entity sweep (segment B `install_world_cpu`, `:1241`) are entity-tier — **out of scope for world-only v1**.

## Headless driver template (for the extension, and CLI shape)

- `--headless` detection `headless_arg` — `crates/postretro/src/startup/session.rs:236`; dispatched in `build_session` (`:108-120`) with a `#[cfg(not(feature="observability"))]` loud-fail arm. Terminates the process; never returns a `BootSession`. Mirror this for `--capture`.
- Observability module `crates/postretro/src/observability/` (feature `observability = []`, `Cargo.toml:96`): `runspec.rs` (`RunSpec`, `#[serde(deny_unknown_fields)]`), `document.rs`, `driver.rs` (`run_headless -> !`). Mirror the module shape for `capture`.
- `xtask observe`: `observe_headless` — `crates/xtask/src/main.rs:222`; `parse_observe_args` (`:257`), `build_scripts_sidecar` (`:200`), then `cargo run -p postretro --features observability -- --headless <runspec>` (`:233-248`), inheriting stdio, forwarding exit code. `xtask capture` mirrors it minus the sidecar (world-only needs no scripts).

## Determinism inputs

- `render_frame_indirect` takes `now_seconds: f64` — feeds animated-lightmap compose and fog march jitter (keyed on `FogParams.frame_index`). Capture passes a fixed time and a fixed warmup count → deterministic on one adapter.
- Fog uses temporal accumulation (rendering_pipeline.md §7.5): frame-one history reads cleared → grainy. Warmup frames at the same static pose let it converge.

## Existing deps

- `postretro` already pulls `image.workspace = true` (png feature) — `crates/postretro/Cargo.toml:29`. No new dep for PNG encode.
- Renderer is always a `postretro` dependency (windowed engine). The `capture` feature gates only new modules, not the renderer link — same pattern as `observability`.
