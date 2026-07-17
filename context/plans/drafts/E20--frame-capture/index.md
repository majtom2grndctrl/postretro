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
- `COPY_SRC` on the renderer-owned `scene_color` target plus a renderer-owned
  capture API that renders at a supplied view-projection and returns the tight
  RGBA8 image bytes. Readback stays renderer-side (renderer owns GPU).
- A scene-spec input format (map, camera pose, resolution, warmup frames, output
  path) and PNG encode.
- A new `capture` cargo feature on `postretro` (independent of `observability`
  and `dev-tools`) and an `xtask capture` subcommand.
- Byte-identical PNG output across repeated runs on the same GPU adapter.

### Out of scope

- Entity rendering — skinned meshes, particles, dynamic (non-baked) lights,
  billboards driven by the particle system. v1 captures world geometry with baked
  lighting only (lightmap, SH indirect, fog volumes). Entities need the scripting
  core, the archetype sweep, and per-frame mesh/pose/particle collection; they
  ride the scripted-run extension.
- Scripted-run capture — run N ticks like the batch runner, then capture from the
  resulting pose. A committed follow-up that reuses this spec's renderer floor
  and the shipped batch runspec; not v1.
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
- [ ] A scene posed to look into level geometry produces a non-uniform image (not
      a single flat color); changing the scene's camera yaw changes the output
      bytes.
- [ ] A scene over a map with fog volumes and `warmup_frames > 0` produces
      byte-identical output across runs (temporal fog converged deterministically);
      `warmup_frames: 0` and `warmup_frames: N` on the same fog map differ.
- [ ] No GPU adapter available exits non-zero with a diagnostic saying capture
      requires an adapter — distinct from the batch runner, which requires none.
- [ ] An invalid scene (missing map, malformed JSON, unknown field) exits
      non-zero with a stderr diagnostic and writes no partial PNG.
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
alongside `Renderer::new` (`crates/renderer/src/render/renderer_init.rs`): take a
capture width/height, create the instance with no display handle, request the
adapter with `compatible_surface: None`, select a fixed srgb capture format
(`Rgba8UnormSrgb` — srgb-capable per the existing boot assertion, render-target
and copy-source capable, adapter-independent output bytes), and build the full
renderer against that format + size instead of `surface_config`. The renderer
holds no surface and never presents. The surface-coupled methods
(`acquire_present_handle`, `present`, splash, the windowed present path) are
never called on an offscreen renderer; whether the surface becomes an
`Option`/enum is the implementer's call, but no offscreen code path may touch a
surface. Device feature/limit request is unchanged (the full set, as
`Renderer::new` does). No adapter fail-fast that windowed relies on may be
dropped.

**(b) scene_color readback.** Add `COPY_SRC` to the `scene_color` texture usage
(`create_scene_color`, `crates/renderer/src/render/screen_effects.rs`) and expose
a `Renderer`-level accessor to the `scene_color` `wgpu::Texture` reached through
the existing `pub(super) screen_effects` field (none exists today — only a
view accessor on the pass). Promote the test-gated readback pattern
(`ui/gpu_test_harness.rs::read_texture_rgba8`: `copy_texture_to_buffer` at
256-byte row alignment → submit → `map_async` → `poll(wait)` → de-pad to tight
`width*4` RGBA8) into a non-test renderer-internal helper.

**(c) Capture render entry.** Refactor `render_frame_indirect`
(`crates/renderer/src/render/renderer_render_frame.rs`) so the scene-recording
body (pre-scene compute, shadow passes, depth pre-pass, forward, mesh, smoke,
fog — everything that writes `scene_color`) is reusable independent of surface
acquisition and the resolve/present tail. Add a public offscreen capture entry
that takes the same per-frame inputs plus `now_seconds` and a warmup count,
records the scene into `scene_color` for `warmup + 1` frames at the same pose
(so temporal fog converges), then reads back `scene_color` (b) after the final
frame's submit retires and returns the tight RGBA8 bytes. It acquires no
swapchain and runs no resolve/present. The windowed path keeps its exact pass
order and behavior (record → resolve into swapchain → present); this is a
behavior-preserving extract for windowed. Pass a fixed `now_seconds` and fixed
warmup so animated-lightmap and fog-jitter inputs are deterministic.

### Task 2: Capture driver + scene spec

In `postretro`, new module `crates/postretro/src/capture/`, gated on a new
`capture = []` cargo feature (independent of `observability`/`dev-tools`; pulls no
egui). Serde types, snake_case, `#[serde(deny_unknown_fields)]`: a scene spec
carrying `map` (path to `.prl`), `camera` (`position: [f32;3]`, `yaw`, `pitch`,
`fov_deg` with a 100° default matching `camera::HFOV`), `resolution:
[u32;2]`, `warmup_frames: u32` (default a small constant, e.g. 4), and `output`
(PNG path). The driver: parse and validate the scene; load the PRL synchronously
via `postretro_level_loader::load_prl`; build the offscreen renderer (Task 1a) at
the scene resolution; run the world renderer-upload sequence directly on the
renderer — texture install, UV normalize, `level_world_to_geometry` +
`install_level_geometry` (uploads geometry and the baked lightmap/SH atlases),
fog pixel scale + cell masks (call sites in `startup/lifecycle.rs`'s
`install_level_payload`; no scripting core, `HeadlessSession`, or scripts-build
sidecar is needed for world-only render); build the static `view_proj` via
`camera::RenderCamera::new` with `roll = 0`, `eye_offset = Vec3::ZERO`, and aspect
from the resolution; compute the one-frame visibility inputs by reproducing the
`App::redraw` block (`determine_visible_cells` for the static pose, then derive
`light_reachable_cell_mask` and `reachable_cell_aabbs` from `fog_reachable`); call
the Task 1c capture entry; encode the returned `Rgba8UnormSrgb` bytes (already
sRGB — a direct PNG write, no color conversion) to the output path via the
already-present `image` dependency. Also wire the `--capture <scene.json>` branch
in `startup/session.rs` mirroring the `--headless` branch: detection sits outside
the feature gate; the `#[cfg(feature = "capture")]` arm runs the driver and
terminates the process (never returns a `BootSession`, so `main` starts no event
loop); a `#[cfg(not(feature = "capture"))]` arm exits non-zero naming
`cargo run -p xtask -- capture`. Errors go to stderr with no partial PNG written;
absent adapter is a clear non-zero diagnostic.

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
| camera pose | `position`/`yaw`/`pitch`/`fov_deg` | `snake_case` |
| output image | RGBA8 sRGB bytes | PNG, sRGB, no color transform |

## Rough sketch

- Scene spec (proposed — remove after implementation):

  ```json
  {
    "map": "content/dev/maps/campaign-test.prl",
    "camera": { "position": [0.0, 1.6, 0.0], "yaw": 0.0, "pitch": 0.0, "fov_deg": 100.0 },
    "resolution": [1280, 720],
    "warmup_frames": 4,
    "output": "capture.png"
  }
  ```

- Capture reads `scene_color`, not the swapchain: `scene_color` is the composited
  scene *before* the transient screen-effects resolve (rendering_pipeline.md §7.8),
  and is byte-identical to the swapchain at rest. Capturing it excludes
  shake/flash/vignette pulses that would break run-to-run reproducibility.
- Fog warmup: temporal fog accumulation (§7.5) reads cleared history on frame one
  and looks grainy. `warmup_frames` re-renders the same static pose so
  accumulation converges; a fixed count keeps it deterministic.
- Determinism boundary: byte-identity holds for repeated runs on one adapter with
  fixed `now_seconds` and warmup. It does *not* hold across different adapters —
  documented, not asserted. Golden-image CI, if added later, pins one software
  adapter; that is a CI-environment decision outside this spec.
- Renderer is always a `postretro` dependency (windowed engine); the `capture`
  feature gates only the new module, adding no dependency — same shape as
  `observability`.
- Full grounded call-sites and signatures: sibling `research.md`.

## Open questions

- None blocking. The two design forks are decided: static-pose first (scripted-run
  capture is the committed extension), and same-adapter byte-identity as the
  determinism guarantee (no cross-adapter tolerance harness in v1).
