# Frame Capture (E20)

## Goal

Give agents and content CI a renderer-owned way to capture a rendered frame to
PNG from a `.prl` at a deterministic camera pose — no window, no display server,
no swapchain. The visual sibling of the shipped state dump: where the batch
runner reports *what the sim thinks*, capture shows *what the renderer draws*.
This spec is the static-pose slice — the surfaceless-renderer floor the later
scripted-run capture and live-channel capture build on.

## Scope

### In scope

- A `--capture <scene.json>` mode of the `postretro` binary: build a surfaceless
  renderer, load a `.prl`, upload world geometry + baked lighting, render one
  frame at a static camera pose, read back the composited image, write a PNG.
- A surfaceless (offscreen) `Renderer` construction path: no winit window, no
  `wgpu::Surface`, a fixed capture format, caller-chosen capture resolution.
  Fallible — adapter absence is an error the driver surfaces.
- `COPY_SRC` on the renderer-owned `scene_color` target plus a renderer-owned
  capture API that renders at a supplied view-projection and returns the tight
  RGBA8 image bytes. Readback stays renderer-side (renderer owns GPU).
- A scene-spec input format (map, camera pose, resolution, output path) and PNG
  encode.
- A new `capture` cargo feature on `postretro` (independent of `observability`
  and `dev-tools`) and an `xtask capture` subcommand.
- Byte-identical PNG output across repeated runs on the same GPU adapter.

### Out of scope

- Entity rendering — skinned meshes, particles, dynamic (non-baked) lights,
  billboards driven by the particle system. v1 captures world geometry with baked
  lighting only: static lightmap + SH indirect. Entities need the scripting core,
  the archetype sweep, and per-frame mesh/pose/particle collection; they ride the
  scripted-run extension.
- Fog volumes and temporal-fog warmup. Volumetric fog is fed from the session's
  fog-volume bridge (volume + AABB uploads), and its light shafts need dynamic
  lights — both session/entity tier, not world-only-renderer state. Fog and the
  warmup-frame loop its temporal accumulation needs (§7.5) ride the scripted-run
  extension, which carries the session. v1 renders exactly one frame; with no
  temporal pass, one frame is deterministic.
- Scripted-run capture — run N ticks like the batch runner, then capture from the
  resulting pose. A committed follow-up that reuses this spec's renderer floor and
  the shipped batch runspec; adds entities and fog. Not v1.
- Live-channel capture verb, MCP frontend, video / multi-frame sequences.
- Cross-adapter determinism. GPU rasterization is not bit-identical across
  adapters; the guarantee is same-adapter, same-input repeatability (see AC).
- Capturing the post-resolve swapchain image. Capture reads `scene_color`
  (pre-effects); transient screen effects (flash/vignette/shake) are excluded by
  design so output is reproducible.

## Acceptance criteria

- [ ] `postretro --capture <scene.json>` targeting a compiled `.prl` loads the
      map, renders one frame at the scene's camera pose and resolution, writes a
      PNG to the scene's output path, and exits 0 — on a machine with no display
      server, given a GPU adapter (software or hardware). All logging on stderr.
- [ ] The written PNG's dimensions equal the scene's requested resolution.
- [ ] Two runs of the same scene on the same adapter produce byte-identical PNG
      files.
- [ ] A fixture scene posed at a known spawn facing into level geometry produces a
      non-uniform image (not a single flat color); changing the scene's camera yaw
      changes the output bytes, and changing its pitch also changes the output
      bytes (guards the pitch-inclusive look direction).
- [ ] No GPU adapter available exits non-zero with a diagnostic saying capture
      requires an adapter — distinct from the batch runner, which requires none.
- [ ] An invalid scene exits non-zero with a stderr diagnostic and writes no
      partial PNG. Rejected: missing map file, malformed JSON, unknown field,
      `fov_deg` outside 60–130 (§11 configurable range), a resolution dimension of
      zero or above the renderer's `max_texture_dimension_2d` floor (8192), and a
      non-writable output path.
- [ ] `--capture` on a build compiled without the `capture` feature exits
      non-zero with a diagnostic naming the xtask command.
- [ ] `cargo build -p postretro` (no features) compiles without the capture
      module and pulls no new dependencies; the full existing test suite passes.
- [ ] The offscreen renderer creates no window or surface and runs no event loop;
      windowed launch of a `.prl` is unchanged after the render-entry refactor.
- [ ] `cargo run -p xtask -- capture <scene.json>` builds `postretro` with the
      `capture` feature, runs it, writes the PNG, and forwards the exit code.

## Tasks

### Task 1: Renderer capture surface

In `postretro-renderer`. Three coupled additions, same crate, sequenced together
to avoid worktree conflict on the renderer struct and the frame-orchestration
file.

**(a) Offscreen construction.** Add a surfaceless `Renderer` construction path
alongside `Renderer::new` (`crates/renderer/src/render/renderer_init.rs`),
returning `Result<Self>`: take a capture width/height, create the instance with no
display handle, request the adapter with `compatible_surface: None` (adapter
absence returns `Err`, not a panic), select a fixed srgb capture format
(`Rgba8UnormSrgb` — srgb-capable per the existing boot assertion, render-target
and copy-source capable, adapter-independent output bytes), and build the full
renderer against that format + size instead of `surface_config`. The renderer
holds no surface and never presents. The surface-coupled methods
(`acquire_present_handle`, `present`, splash, the windowed present path) are never
called on an offscreen renderer; whether the surface becomes an `Option`/enum is
the implementer's call, but no offscreen code path may touch a surface. Device
feature/limit request is unchanged (the full set, as `Renderer::new` does). No
adapter fail-fast that windowed relies on may be dropped. The offscreen
constructor returns a **full-ready** renderer: it builds full init
(`build_full_renderer`) immediately, not the boot-only (`full: None`) state
`Renderer::new` leaves for a later `finish_full_init` — there is no boot-splash
phase offscreen. (The install methods in Task 2 do `full_mut().expect(...)` and
panic if `full` is `None`.)

**(b) scene_color readback.** Add `COPY_SRC` to the `scene_color` texture usage
(`create_scene_color`, `crates/renderer/src/render/screen_effects.rs` — today
`RENDER_ATTACHMENT | TEXTURE_BINDING`, no `COPY_SRC`) and expose a `Renderer`-level
accessor to the `scene_color` `wgpu::Texture` reached through the existing
`pub(super) screen_effects` field (none exists today — only a view accessor on the
pass). Promote the test-gated readback pattern
(`read_texture_rgba8` in `crates/renderer/src/render/ui/gpu_test_harness.rs`, a
`#[cfg(test)]` module: `copy_texture_to_buffer` at 256-byte row alignment →
submit → `map_async` → `poll(wait)` → de-pad to tight `width*4` RGBA8) into a
non-test renderer-internal helper. De-gate it and adapt the signature — the test
helper takes a `GpuCtx { device, queue }`, the promoted helper operates on the
renderer's own `device`/`queue`.

**(c) Capture render entry.** Refactor `render_frame_indirect`
(`crates/renderer/src/render/renderer_render_frame.rs`) so the scene-recording
body (pre-scene compute, shadow passes, depth pre-pass, forward, mesh, smoke —
everything that writes `scene_color`) is reusable independent of surface
acquisition and the resolve tail. Today that function acquires a swapchain
(`acquire_present_handle` → `surface_view()`), records the scene, resolves into
`scene_color`, and returns a `PresentHandle` the *caller* (`App`) presents; the
windowed path is unchanged (same pass order, App still presents). Add a public
offscreen capture entry that records the world scene passes only — **no UI pass,
no debug lines, no resolve, no present** — so it needs no `font_system` (the UI
pass is session-owned; world-only capture carries no HUD). Its inputs are the
per-frame visibility set (`cam_vis: CameraCullVisibility`,
`light_reachable_cell_mask`, `reachable_cell_aabbs`, `fog_reachable`,
`camera_cell`), `view_proj`, an empty `particle_collections`, a `clear_color`,
`render_world = true`, and a fixed `now_seconds = 0.0` (so animated-lightmap
inputs are deterministic). It acquires no swapchain, records into `scene_color`,
then reads back `scene_color` (b) after the frame's submit retires and returns the
tight RGBA8 bytes.

### Task 2: Capture driver + scene spec

In `postretro`, new module `crates/postretro/src/capture/`, gated on a new
`capture = []` cargo feature (independent of `observability`/`dev-tools`; pulls no
egui). Serde types, snake_case, `#[serde(deny_unknown_fields)]`: a scene spec
carrying `map` (path to `.prl`), `camera` (`position: [f32;3]`, `yaw_deg`,
`pitch_deg`, `fov_deg` with a 100.0 default matching `camera::HFOV`), `resolution:
[u32;2]`, and `output` (PNG path). Angles are degrees in JSON, converted to
radians in the driver.

The driver:

1. Parse and validate the scene (reject the invalid-scene cases in the AC:
   missing map, bad JSON, unknown field, `fov_deg` outside 60–130, a zero or
   `> 8192` resolution dimension, non-writable output).
2. Load the PRL synchronously via `postretro_level_loader::load_prl`.
3. Build the offscreen renderer (Task 1a) at the scene resolution; map an `Err`
   (adapter absence) to a stderr diagnostic and non-zero exit.
4. Run the world renderer-upload sequence directly on the renderer, in this order
   — `install_level_payload` (`startup/lifecycle.rs:548`) itself is session-coupled
   (`expect("session installed")`, light-bridge, archetype sweep) and is **not**
   callable; the driver replicates only its renderer-only calls. First build
   `texture_materials` via the `derive_material` loop over `world.texture_names`
   (`lifecycle.rs:597-619`) — it feeds both `install_textures` and
   `level_world_to_geometry`. Then, in order: `install_textures` →
   `normalize_world_uvs` → `render::level_world_to_geometry(&world,
   &texture_materials)` + `install_level_geometry` (uploads geometry and the baked
   lightmap/SH atlases — `install_level_geometry` rebuilds them straight from
   `LevelGeometry`, no light bridge). Fog pixel-scale/cell-mask installs are
   skipped (fog is out of scope). No scripting core, `HeadlessSession`, or
   scripts-build sidecar is needed.
5. Supply `install_textures`' `prm_cache_root`: derive it exactly as the windowed
   loader does — `content_root_from_map(Some(map))` (`startup/session.rs:309`) then
   `derive_prm_root_dev_layout` (`startup/worker.rs:108`, currently private — make
   it `pub(crate)` or lift the two-line derivation into a shared `pub(crate)`
   helper).
6. Build the static `view_proj` directly (do **not** route through
   `camera::RenderCamera::new`, which takes no FOV and ignores `fov_deg`):
   `vfov = 2*atan(tan(fov_deg.to_radians()/2)/aspect)`,
   `perspective_rh(vfov, aspect, camera::NEAR, camera::FAR) * look_at_rh(pos, pos + look_dir, Y_up)`,
   aspect from the resolution. Compute `look_dir` **pitch-inclusive** —
   `Vec3::new(-yaw.sin()*pitch.cos(), pitch.sin(), -yaw.cos()*pitch.cos())`,
   matching `render_view_matrix` (`camera.rs:71-75`). Do **not** use
   `Camera::forward()` (`camera.rs:130`) — it is yaw-only and silently drops
   pitch.
7. Compute the one-frame visibility inputs by reproducing the `App::redraw` block:
   `postretro_visibility::determine_visible_cells(eye, view_proj, world,
   capture_portal_walk, &mut scratch)` returns `(VisibilityResult, Frustum)` — the
   `visible_cells`, `fog_reachable`, and `stats` (`stats.camera_cell`,
   `stats.path`) fields live on `VisibilityResult`; the trailing `Frustum` is
   unused here. Derive `light_reachable_cell_mask: Vec<bool>` and
   `reachable_cell_aabbs: Vec<(Vec3,Vec3)>` from `fog_reachable` as `main.rs:2450`
   and `:2476` do.
8. Call the Task 1c capture entry — `cam_vis` from the visibility set above, the
   derived mask/AABBs, `view_proj`, an empty `particle_collections`, a
   `clear_color`, `render_world = true` — and encode the returned `Rgba8UnormSrgb`
   bytes (already sRGB — a direct PNG write, no color conversion) to the output
   path via the already-present `image` dependency. Pre-check output-path
   writability before rendering and write the PNG as the final step, so a failure
   leaves no partial file.

Also wire the `--capture <scene.json>` branch in `startup/session.rs` mirroring
the `--headless` branch: detection sits outside the feature gate; the
`#[cfg(feature = "capture")]` arm runs the driver and terminates the process
(never returns a `BootSession`, so `main` starts no event loop); a
`#[cfg(not(feature = "capture"))]` arm exits non-zero naming
`cargo run -p xtask -- capture`. Errors go to stderr with no partial PNG written.

### Task 3: xtask capture subcommand

Add `capture <scene.json>` to `crates/xtask/src/main.rs`, mirroring
`observe_headless`: parse exactly one scene path (usage error otherwise), then run
a single `cargo run -p postretro --bin postretro --features capture --
--capture <scene>`, inheriting stdio and forwarding the child exit code. Unlike
`observe`, no `build_scripts_sidecar` step — world-only capture runs no scripts
(the scripted-run extension will add it). xtask does not parse the scene JSON.

## Sequencing

**Phase 1 (sequential):** Task 1 — the renderer capture surface; blocks the driver.
**Phase 2 (sequential):** Task 2 — consumes Task 1's offscreen constructor and
capture entry; sole task touching the windowed boot branch (`startup/session.rs`).
**Phase 3 (sequential):** Task 3 — consumes Task 2's `--capture` CLI; doubles as
the end-to-end verification of the plan.

## Boundary inventory

The scene spec is a tool-facing surface (agents / CI), consumed by the driver, not
by mods. No JS/Luau/FGD surface.

| Name | Rust | Wire / serde |
|---|---|---|
| scene spec fields | struct fields | `snake_case`, `deny_unknown_fields` |
| camera pose | `position`/`yaw_deg`/`pitch_deg`/`fov_deg` | `snake_case`; angles in **degrees**, driver converts to radians |
| output image | RGBA8 sRGB bytes | PNG, sRGB, no color transform |

## Rough sketch

- Scene spec (proposed — remove after implementation):

  ```json
  {
    "map": "content/dev/maps/campaign-test.prl",
    "camera": { "position": [0.0, 1.6, 0.0], "yaw_deg": 0.0, "pitch_deg": 0.0, "fov_deg": 100.0 },
    "resolution": [1280, 720],
    "output": "capture.png"
  }
  ```

- Capture reads `scene_color`, not the swapchain: `scene_color` is the composited
  scene *before* the transient screen-effects resolve (rendering_pipeline.md §7.8),
  and is byte-identical to the swapchain at rest. Capturing it excludes
  shake/flash/vignette pulses that would break run-to-run reproducibility.
- Determinism boundary: byte-identity holds for repeated runs on one adapter with
  `now_seconds = 0.0` and no temporal pass. It does *not* hold across different
  adapters — documented, not asserted. Golden-image CI, if added later, pins one
  software adapter; that is a CI-environment decision outside this spec.
- `fov_deg` is honored by building `view_proj` in the driver, because
  `RenderCamera::new` derives vfov from the `HFOV` const and takes no FOV argument.
- Renderer is always a `postretro` dependency (windowed engine); the `capture`
  feature gates only the new module, adding no dependency — same shape as
  `observability`.
- Full grounded call-sites and signatures: sibling `research.md`.

## Open questions

None blocking. Decisions taken during review: static-pose first (scripted-run
capture is the committed extension); same-adapter byte-identity as the determinism
guarantee (no cross-adapter tolerance harness in v1); fog volumes deferred to the
scripted-run extension (session-fed, not world-only-renderer state), so v1 renders
one frame with no warmup; `fov_deg` honored via a direct `view_proj` build.
