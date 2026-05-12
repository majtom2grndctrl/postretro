# Research notes — egui debug UI foundation

## wgpu / egui-wgpu compatibility

PostRetro depends on `wgpu = "29"` (`Cargo.toml` workspace deps) and
`winit = "0.30"`.

Latest egui-wgpu releases on crates.io (queried via API):

| egui-wgpu | wgpu req   | winit req       |
|-----------|------------|-----------------|
| 0.34.2    | `^29.0.1`  | `^0.30.13` opt  |
| 0.34.0    | `^29.0.0`  | `^0.30.13` opt  |
| 0.33.3    | `^27.0.1`  | `^0.30.12` opt  |

Conclusion: pin `egui = "0.34"`, `egui-winit = "0.34"`, `egui-wgpu = "0.34"`.
No version mismatch, no blocker. Caret `^29.0.x` overlaps the workspace's
`"29"` cleanly.

(Crates.io API: `curl https://crates.io/api/v1/crates/egui-wgpu/0.34.2/dependencies`.)

## Existing diagnostic chord surface

Source of truth: `crates/postretro/src/input/diagnostics.rs`.

Current `DiagnosticAction` variants and chords:

| Variant                  | Chord                  | Disposition         |
|--------------------------|------------------------|---------------------|
| `ToggleWireframe`        | `Alt+Shift+Backslash`  | Keep (on/off)       |
| `DumpPortalWalk`         | `Alt+Shift+1`          | Keep (one-shot)     |
| `ToggleVsync`            | `Alt+Shift+V`          | Keep (on/off)       |
| `LowerAmbientFloor`      | `Alt+Shift+[`          | Remove → slider     |
| `RaiseAmbientFloor`      | `Alt+Shift+]`          | Remove → slider     |
| `CycleLightingIsolation` | `Alt+Shift+4`          | Remove → dropdown   |
| `LowerIndirectScale`     | `Alt+Shift+-`          | Remove → slider     |
| `RaiseIndirectScale`     | `Alt+Shift+=`          | Remove → slider     |
| **new** `ToggleDebugPanel` | `Alt+Shift+Backquote` | Add (this plan)     |

`AMBIENT_FLOOR_STEP = 0.00125` and `INDIRECT_SCALE_STEP = 0.05` drop with the
chords; the slider granularity replaces them.

## Renderer-side setters that exist today

In `crates/postretro/src/render/mod.rs`:

- `pub fn ambient_floor(&self) -> f32` (line ~2394)
- `pub fn set_ambient_floor(&mut self, value: f32)` clamps to `0..=1`
- `pub fn indirect_scale(&self) -> f32` (line ~2402)
- `pub fn set_indirect_scale` exists alongside (clamping to `0..=1` per
  uniform layout)
- `pub fn cycle_lighting_isolation(&mut self) -> LightingIsolation` (line ~2035)
- `pub fn toggle_wireframe`, `pub fn toggle_vsync`, `pub fn vsync_enabled`

`set_lighting_isolation(mode)` is **not** confirmed present; verify on
implementation and add if missing. The panel needs both `get` and `set` to
drive a `ComboBox`.

`Renderer::surface_format: wgpu::TextureFormat` is stored on the struct
(line ~1469 inside `Renderer::new`). Pass this to
`egui_wgpu::Renderer::new` rather than re-deriving from surface caps.

## Where the egui pass slots in

`render_frame_indirect` (line ~2420) currently terminates with:

1. Wireframe overlay pass (color = `LoadOp::Load`, depth = `LoadOp::Load`,
   conditional on `wireframe_active`).
2. `frame_timing.encode_resolve` (line ~2828) — query-set copy.
3. `queue.submit` + `output.present()`.
4. `frame_timing.post_submit`.

Insert the egui pass between (1) and (2). One color attachment (swapchain
view, `LoadOp::Load`), no depth, `sample_count = 1`. Skip the pass when
`debug_ui` is absent or `!visible`.

## FrameTiming readback shape

`render/frame_timing.rs` accumulates `accum_ns[pair_idx]` over a
120-frame window (`AVG_WINDOW_FRAMES`). At rollover it formats a single log
line and resets. There is no retained accessor today; the panel needs one.

Proposed shape:

```rust
// Proposed
pub struct FrameTimingSnapshot {
    pub windows: u32,            // frames averaged
    pub passes: Vec<(&'static str, f32 /* avg_ms */, u32 /* skipped */)>,
}
impl FrameTiming {
    pub fn last_window(&self) -> Option<&FrameTimingSnapshot> { ... }
}
```

Snapshot is written at the same point the log line is emitted, then read by
the panel each frame. No locking needed — single-threaded.

## Input event surface (winit 0.30)

`egui_winit::State::on_window_event` returns
`EventResponse { consumed: bool, repaint: bool }`. The `consumed` flag is what
gates downstream dispatch. `repaint` is advisory — the engine already redraws
every frame (`about_to_wait` always requests redraw), so `repaint` need not
drive any state change.

Events egui consumes when focused: `KeyboardInput`, `ModifiersChanged`,
`MouseInput`, `CursorMoved`, `CursorEntered`, `CursorLeft`, `MouseWheel`,
`Ime`, `Touch`, plus `Resized` / `ScaleFactorChanged` (consumed=false but
state-relevant). `DeviceEvent::MouseMotion` (raw delta) is **not** seen by
egui-winit; the current code feeds it directly to `input_system` in
`device_event`. The plan must skip that feed when `input_focus != Gameplay`,
otherwise stored raw deltas would surge into the camera the moment focus
returns to gameplay.
