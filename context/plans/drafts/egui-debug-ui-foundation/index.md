# egui Debug UI Foundation

## Goal

Stand up an in-engine debug UI layer on egui, gated behind a `dev-tools` Cargo
feature so production builds carry zero egui code and allocate zero GPU
resources. This is infrastructure: future diagnostics (light/probe viz,
DebugDraw, puffin) consume it. Ships with one proof-of-concept panel that
migrates today's valued diagnostic chords (ambient floor, indirect scale,
lighting isolation) into sliders + dropdowns, plus a live GPU pass-timing
readout.

## Scope

### In scope

- A `dev-tools` Cargo feature on `crates/postretro` that pulls in egui,
  egui-winit, and egui-wgpu. Without the feature, none of these crates compile
  in.
- An `InputFocus` enum on `App` (Gameplay / DevTools / Menu) that owns pointer
  lock + cursor visibility transitions. Lives in `crates/postretro/src/input/`
  and is always compiled — pointer lock is not a dev-tools concern.
- Existing chord-based diagnostic actions (`DiagnosticAction` and
  `DiagnosticInputs`) move behind `#[cfg(feature = "dev-tools")]`. The narrowed
  chord set survives; valued chords (ambient floor, indirect scale, lighting
  isolation) are removed from `DiagnosticAction` and replaced by egui widgets.
- A `DebugUi` subsystem inside the renderer module that owns the
  `egui::Context`, the `egui_winit::State`, and a lazy `Option<DebugUiGpu>`
  holding `egui_wgpu::Renderer`. The GPU half is constructed on first panel
  open and stays resident for the rest of the session.
- A new chord (default `Alt+Shift+Backquote`) that toggles the debug panel
  visible/hidden. Toggling visible-from-hidden triggers lazy GPU init the first
  time only; subsequent toggles just flip a visibility bool.
- Winit event routing: `egui_winit::State::on_window_event` runs before
  PostRetro's keyboard/mouse handlers. Its `EventResponse::consumed` gates the
  game action path and the diagnostic chord path so a key typed into an egui
  text field does not also fire a game action or chord.
- A single egui overlay render pass appended after the wireframe overlay pass
  and before the GPU-timing resolve. One color attachment (the swapchain view),
  `LoadOp::Load`, no depth attachment.
- One proof-of-concept panel: "Diagnostics". Hosts an ambient-floor slider, an
  indirect-scale slider, a lighting-isolation mode dropdown, and a read-only
  GPU pass timing block sourced from `render::frame_timing::FrameTiming`.

### Out of scope

- Light/probe debug visualization (separate follow-on plan).
- Generalizing the wireframe pass into a typed `DebugDraw` channel (separate
  follow-on plan).
- puffin CPU profiling integration (depends on egui landing).
- In-game menus or settings UI. `InputFocus::Menu` is scaffolded but no menu
  consumer ships with this plan.
- MSAA support for the egui pass. Engine does not use MSAA; the egui-wgpu
  pipeline is configured for sample count 1. Re-evaluate when MSAA lands.
- Gamepad navigation of egui widgets.
- Persisting egui panel layout to disk across runs.
- Replacing the on/off diagnostic chords that remain useful at-the-keyboard
  (wireframe, vsync, dump portal walk). These stay as chords.

## Acceptance criteria

- [ ] `cargo build -p postretro` (default features) succeeds without compiling
      egui, egui-winit, or egui-wgpu. `cargo tree -p postretro` confirms none
      appear.
- [ ] `cargo build -p postretro --features dev-tools` succeeds.
- [ ] In a `--features dev-tools` build, launching the engine and not pressing
      the debug-panel chord leaves the egui-wgpu renderer uninitialized: no
      egui pipeline, no font atlas texture, no vertex/index buffer allocations
      attributable to egui. (Verifiable by adding a one-time `log::info!` in
      the lazy-init path and confirming it does not fire.)
- [ ] First press of the debug-panel chord opens the panel; the log line above
      fires exactly once. Second open/close cycle does not fire it again.
- [ ] While the panel is open: cursor is visible, pointer lock is released,
      raw mouse-delta no longer rotates the camera, gameplay `WASD` keys do not
      fire move actions when keyboard focus is inside an egui text widget.
- [ ] Closing the panel restores pointer lock and gameplay input within the
      same frame; camera resumes responding to mouse motion next frame.
- [ ] Window-focus-loss while the panel is closed releases the cursor as today;
      regaining focus restores `Gameplay` lock. The pre-existing
      `handle_focus_change` path is preserved.
- [ ] The Diagnostics panel's ambient-floor slider, indirect-scale slider, and
      lighting-isolation dropdown produce visible scene changes equivalent to
      the chord-driven versions they replace. The slider ranges match the
      existing clamps (ambient floor 0..=1, indirect scale 0..=1) and the
      dropdown lists the same nine `LightingIsolation` variants.
- [ ] With `POSTRETRO_GPU_TIMING=1` on a TIMESTAMP_QUERY-capable adapter, the
      Diagnostics panel shows a per-pass timing block (`cull`,
      `depth_prepass`, `forward`) sampled from the same averaging window the
      log uses. Without timing support, the block shows a single
      "GPU timing unavailable" line.
- [ ] Removed chords (`LowerAmbientFloor`, `RaiseAmbientFloor`,
      `LowerIndirectScale`, `RaiseIndirectScale`, `CycleLightingIsolation`)
      no longer appear in `DiagnosticAction`. Their key bindings are free.
- [ ] Production build (no `dev-tools`) still ships the surviving chord set
      (`ToggleWireframe`, `DumpPortalWalk`, `ToggleVsync`) and the debug-panel
      chord is absent.
- [ ] `cargo clippy --workspace --all-targets` is clean in both feature modes.
- [ ] `cargo test -p postretro` passes in both feature modes. New tests cover
      `InputFocus` transitions and the consumed-event gate on a synthetic
      `EventResponse`.

## Tasks

### Task 1: InputFocus state + pointer-lock ownership

Introduce a general focus enum that consolidates pointer-lock acquire/release.
Lives in the always-compiled input module so future menu work can reuse it.

Define `InputFocus { Gameplay, DevTools, Menu }` in
`crates/postretro/src/input/mod.rs` (or a new sibling `focus.rs`). Add
`input_focus: InputFocus` to `App` in `crates/postretro/src/main.rs`,
defaulting to `Gameplay`. Add `fn set_input_focus(&mut self, focus: InputFocus)`
on `App` that, on transition:

- `Gameplay`: calls `input::cursor::capture_cursor`, clears any
  `input_system` carry-over via `clear_all`, clears
  `diagnostic_inputs.clear_modifiers`.
- `DevTools` / `Menu`: calls `input::cursor::release_cursor`, clears the same
  state to prevent keys held during the transition from sticking.

Replace direct `capture_cursor` / `release_cursor` call sites in
`resumed`, `WindowEvent::CloseRequested`, the Escape branch, and
`WindowEvent::Focused` with `set_input_focus` calls. The `Focused(false)` path
should not change the stored focus — it releases the cursor while the window
is unfocused but the focus mode remains whatever the user chose. Add a
companion `fn reapply_focus(&mut self)` that re-runs the acquire side; call it
from `Focused(true)`.

### Task 2: dev-tools feature flag + dependency wiring

Add a `dev-tools` feature to `crates/postretro/Cargo.toml`. The feature
activates optional dependencies `egui`, `egui-winit`, and `egui-wgpu` pinned to
`0.34` (versions confirmed against `wgpu 29` — see `research.md`).

Move `mod input::diagnostics` and its `pub use` re-exports in
`crates/postretro/src/input/mod.rs` behind `#[cfg(feature = "dev-tools")]`. Do
the same for the `diagnostic_inputs` field on `App`, all its call sites in
`window_event`, and the `handle_diagnostic_action` method. Use
`#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]` on any helper
that becomes orphaned in a no-feature build but is shared with feature-on
code.

### Task 3: Narrow DiagnosticAction; remove valued chords

In `crates/postretro/src/input/diagnostics.rs`, remove the
`LowerAmbientFloor`, `RaiseAmbientFloor`, `LowerIndirectScale`,
`RaiseIndirectScale`, and `CycleLightingIsolation` variants from
`DiagnosticAction`. Drop their entries from `default_diagnostic_chords`. Drop
`AMBIENT_FLOOR_STEP` and `INDIRECT_SCALE_STEP`. Add a new variant
`ToggleDebugPanel` and a chord for it (`Alt+Shift+Backquote`).

Update `handle_diagnostic_action` in `main.rs` to drop the removed arms and
add a `ToggleDebugPanel` arm that flips the debug-panel visibility (Task 5)
and calls `set_input_focus` (Task 1) — `DevTools` on show, `Gameplay` on
hide.

Update tests in `diagnostics.rs` for the new chord table (no duplicates, all
Alt+Shift, includes `ToggleDebugPanel`).

### Task 4: DebugUi scaffolding (CPU side, always-resident)

Create `crates/postretro/src/render/debug_ui/mod.rs` gated on
`#[cfg(feature = "dev-tools")]` and `pub mod debug_ui` from `render/mod.rs`
under the same gate.

`DebugUi` owns:
- `ctx: egui::Context` (constructed eagerly — pure CPU, tiny).
- `winit_state: egui_winit::State` (constructed eagerly).
- `gpu: Option<DebugUiGpu>` (lazy; see Task 5).
- `visible: bool` (starts `false`).
- Diagnostic panel state struct (the snapshot the panel reads/writes — see
  Task 7).

Expose:
- `fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> egui_winit::EventResponse`
- `fn set_visible(&mut self, v: bool)` / `fn is_visible(&self) -> bool`
- `fn wants_pointer_input(&self) -> bool` / `fn wants_keyboard_input(&self) -> bool`
  (consulted by the focus / consumed-event gate)

### Task 5: DebugUiGpu lazy init + overlay pass

`DebugUiGpu` (same module, same gate) owns `egui_wgpu::Renderer` plus any
scratch buffers it needs. Constructor takes `&wgpu::Device`, the swapchain
surface format (passed in from `Renderer::surface_format` — already a field on
`Renderer`), depth format `None`, sample count `1`, dithering `false`.

Add `Renderer::ensure_debug_ui_gpu(&mut self)` (feature-gated) that initializes
`self.debug_ui_gpu: Option<DebugUiGpu>` once. The renderer holds the GPU half;
the CPU half lives on `App` so it can run input-event handling before the
renderer is borrowed in the render call.

Add `Renderer::render_debug_ui(...)` (feature-gated) that takes the egui
full output, the screen descriptor, the encoder, and the swapchain view, and
records one render pass:

- Load swapchain color, store; no depth attachment.
- Calls `egui_wgpu::Renderer::update_buffers` and `render`.
- Calls `free_textures` after the pass.

Call site in `Renderer::render_frame_indirect`: after the wireframe overlay
pass closes (line ~2825), before the `frame_timing.encode_resolve` call (line
~2827). Egui draws on top of the world but its work is measured by the same
present timeline as everything else. If the panel is not visible, skip the
pass entirely — the lazy GPU half may still be `None`.

### Task 6: Event routing — egui first, gated dispatch downstream

In `App::window_event` for `KeyboardInput`, `MouseInput`, `CursorMoved`,
`MouseWheel`, `ModifiersChanged`, and any other event egui-winit consumes:

1. If `cfg(feature = "dev-tools")` and `self.input_focus != Gameplay`, call
   `debug_ui.on_window_event(window, &event)` and capture the
   `EventResponse`.
2. If `response.consumed`, return early — do not forward to the input system
   or the diagnostic chord resolver.
3. The `ToggleDebugPanel` chord must remain reachable: route it through the
   diagnostic chord resolver *before* egui sees the event when the panel is
   closed, and *after* egui sees it when the panel is open. Simplest pin:
   resolve the panel-toggle chord on every keyboard event regardless of
   `consumed` (since `Alt+Shift+Backquote` is not a chord any egui widget
   binds), but skip the rest of the diagnostic chord table when consumed.

Resize / scale-factor events feed `egui_winit::State` unconditionally
(`run_first_pass` reads the pixels-per-point); they do not need to be gated on
focus.

`Focused(false)` calls `set_input_focus(Gameplay)` only if the user had not
explicitly entered `DevTools`. Simpler pin: leave focus as-is on blur (cursor
already releases via the existing path) and re-apply on `Focused(true)` —
this preserves panel state across alt-tab.

### Task 7: Diagnostics panel + renderer setter wiring

In `render/debug_ui/panel_diagnostics.rs` (or inline in `mod.rs` if the file
stays small), implement the immediate-mode panel body:

- Ambient floor: `egui::Slider::new(&mut state.ambient_floor, 0.0..=1.0)`.
  After draw, if changed: write back through a `&mut Renderer` setter.
- Indirect scale: same pattern, range `0.0..=1.0`.
- Lighting isolation: `egui::ComboBox` over the nine `LightingIsolation`
  variants. The renderer already has `cycle_lighting_isolation`; this plan
  adds `Renderer::set_lighting_isolation(&mut self, mode: LightingIsolation)`
  and `Renderer::lighting_isolation(&self) -> LightingIsolation`, both
  feature-gated only if the type is not already exposed (verify against
  `render/mod.rs` — the public surface around `LightingIsolation` may already
  cover this).
- GPU timing block: read averaged-window snapshots from
  `render::frame_timing::FrameTiming`. The current `FrameTiming` logs to
  `log::info!` at the 120-frame boundary and does not retain the result.
  This task adds a `pub fn last_window(&self) -> Option<&FrameTimingSnapshot>`
  (or equivalent) returning the most recent averaged tuple of
  `(label, avg_ms, skip_count)` so the panel reads the same numbers the log
  prints. The snapshot is overwritten each window; missing snapshot or no
  timing support renders "GPU timing unavailable".

Frame integration in `App::window_event` `RedrawRequested`, after
gameplay/snapshot/render setup but before the renderer draws the egui pass:

1. `let raw_input = debug_ui.winit_state.take_egui_input(window);`
2. `debug_ui.ctx.begin_pass(raw_input);` (or `run` if simpler — see egui 0.34
   docs).
3. If `debug_ui.visible`, build the Diagnostics panel UI; widgets read/write
   through a small `DiagnosticsView` value snapshotted from / committed back to
   the renderer.
4. `let full_output = debug_ui.ctx.end_pass();`
5. Pass `full_output` (textures_delta + paint jobs) to
   `renderer.render_debug_ui(...)` after the world is drawn (Task 5).
6. `debug_ui.winit_state.handle_platform_output(window, full_output.platform_output)`.

## Sequencing

**Phase 1 (sequential):** Task 1 — InputFocus is a precondition for the chord
that opens the panel and for the event-routing gate; touches `App` shape that
later tasks read.

**Phase 2 (sequential):** Task 2 — adds the feature flag and gates the
existing diagnostics module; later tasks add code under that same gate, so
the flag must exist first.

**Phase 3 (concurrent):** Task 3, Task 4 — Task 3 reshapes `DiagnosticAction`
and the chord table; Task 4 adds the `DebugUi` CPU scaffolding. They touch
disjoint files (`input/diagnostics.rs` vs `render/debug_ui/`) and only meet at
the `ToggleDebugPanel` arm in `handle_diagnostic_action`, which is a one-line
forward to Task 4's `set_visible`. Resolve that merge point at hand-off.

**Phase 4 (sequential):** Task 5 — depends on Task 4's `DebugUi` type and on
Task 3's `ToggleDebugPanel` chord triggering lazy GPU init via
`Renderer::ensure_debug_ui_gpu`.

**Phase 5 (sequential):** Task 6 — event routing needs the `DebugUi` from
Task 4 and the `ToggleDebugPanel` chord from Task 3.

**Phase 6 (sequential):** Task 7 — panel content consumes the full pipeline
from Tasks 4–6 plus the renderer setter additions; the GPU-timing readout
depends on a `FrameTiming::last_window` accessor introduced in this task.

## Rough sketch

**Files added (all under `#[cfg(feature = "dev-tools")]` except where noted):**

- `crates/postretro/src/input/focus.rs` — `InputFocus` enum + transition
  helper. Always compiled.
- `crates/postretro/src/render/debug_ui/mod.rs` — `DebugUi`, `DebugUiGpu`,
  `DiagnosticsView`.
- `crates/postretro/src/render/debug_ui/panel_diagnostics.rs` — panel body
  (optional split).

**Files modified:**

- `crates/postretro/Cargo.toml` — add `[features] dev-tools = [...]`, three
  optional egui deps.
- `crates/postretro/src/input/mod.rs` — gate `mod diagnostics` and its
  `pub use`s; add `pub use focus::InputFocus`.
- `crates/postretro/src/input/diagnostics.rs` — remove five valued variants;
  add `ToggleDebugPanel`; update default chord table and tests.
- `crates/postretro/src/main.rs` — add `input_focus`, optional `debug_ui`
  field on `App`; replace direct cursor calls with `set_input_focus`; route
  events egui-first; trigger panel toggle from `handle_diagnostic_action`;
  wire egui frame steps inside `RedrawRequested`.
- `crates/postretro/src/render/mod.rs` — `pub mod debug_ui` (gated); optional
  `debug_ui_gpu` field; `ensure_debug_ui_gpu`; `render_debug_ui`;
  `set_lighting_isolation` if missing; pass `surface_format` into
  `egui_wgpu::Renderer::new` (already stored as `Renderer::surface_format`).
- `crates/postretro/src/render/frame_timing.rs` — add a `last_window`
  accessor that retains the most recent averaged snapshot.

**Ownership split (the key call-out for the implementor):** `egui::Context`
and `egui_winit::State` live on `App` (CPU; needed in event handlers before
the renderer is borrowed). `egui_wgpu::Renderer` lives on `Renderer` (GPU; the
boundary rule from `rendering_pipeline.md §9` puts every wgpu call inside the
renderer module). The frame hand-off passes `egui::FullOutput` and the
screen descriptor across the boundary — both are engine-side value types, no
wgpu handles cross.

**InputFocus future-readiness:** `Menu` is wired through `set_input_focus`
identically to `DevTools` (cursor release path). No menu consumer exists yet;
nothing else needs to change here. The variant is *not* `#[allow(dead_code)]` —
it appears in the enum match arms in `set_input_focus`, which counts as a use.

**Egui pass placement:** Sits between the wireframe overlay pass and
`frame_timing.encode_resolve`. The wireframe overlay's color attachment uses
`LoadOp::Load`; the egui pass mirrors that and additionally drops the
depth-stencil attachment (egui is 2D). Verify the swapchain view borrow is not
moved into the wireframe pass — current code re-acquires it per pass.

## Open questions

- **Default `Alt+Shift+Backquote` placement.** Backquote sits to the left of
  `1` on US layouts. `KeyCode::Backquote` matches that position. Acceptable?
  An alternative is `KeyCode::F1` outside the Alt+Shift namespace, but mixing
  namespaces breaks the rule in `input.md §7`. Recommend `Backquote`.
- **Frame ordering of egui input vs game input.** This spec pins egui-first;
  consumed-event short-circuits the game input path. An alternative is to
  always feed both and let game-action resolution ignore consumed flags — but
  that means a key typed into a text field would also fire `Shoot` if bound.
  Recommend egui-first.
- **Pointer-lock policy on alt-tab while panel is open.** Two options:
  (a) on blur, release cursor but keep `InputFocus::DevTools` so re-focus does
  not re-lock; (b) treat blur as a transient overlay that the existing
  `handle_focus_change` covers — keep focus as-is, let the cursor release
  follow the OS. Recommend (a); preserves user intent across task switches.
- **Should `Renderer::set_lighting_isolation` accept a `LightingIsolation`
  enum or a `u32` index?** The enum is safer; verify the public surface in
  `render/mod.rs` and reuse if exposed. The existing
  `cycle_lighting_isolation` returns the new variant — adding `set_` is
  trivial.
