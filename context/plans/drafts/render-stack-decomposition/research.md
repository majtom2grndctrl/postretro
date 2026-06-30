# Research — render-stack decomposition

Source-grounded findings (confirmed against `crates/postretro/src` on the `claude/merge-scripting-boundary-hardening-p4s369` branch). Feeds `index.md` and the sub-specs; not the spec contract. Produced by a discovery team: 9 cluster-mapping agents + 1 boundary census + synthesis, then 3 targeted grounding passes.

## Why this plan exists

`compile-time-reduction` (draft) extracts a visibility crate and a PRL-loader crate and *defers* the renderer split, naming two preconditions (`compile-time-reduction/index.md:141`): move root GPU modules under renderer ownership, and hide `wgpu::SurfaceTexture` from engine-facing APIs. This epic owns that deferred work and re-derives the full target crate graph rather than slicing one seam. It is the render-side analog of the `engine-data-floor` → `scripting-core-extraction` work (`scripting.md §12`).

## The headline structural fact: the invariant is already violated

"Renderer owns GPU" is false at the module level today. wgpu lives **outside** `render/`:

| Module | Lines | wgpu refs | Verdict |
|---|---|---|---|
| `lighting/lightmap.rs` | 656 | 116 | GPU pool — must move into renderer crate |
| `lighting/spot_shadow.rs` | 1009 | 88 | GPU pool — must move into renderer crate |
| `compute_cull.rs` | 1274 | 76 | GPU pipeline — must move into renderer crate |
| `candidate_cull.rs` | 726 | 57 | GPU pipeline — must move into renderer crate |
| `shadow_cull.rs` | 426 | 48 | GPU pipeline — must move into renderer crate |
| `lighting/cube_shadow.rs` | 655 | 14 | GPU pool — must move into renderer crate |
| `lighting/chunk_list.rs` | 153 | 1 | GPU pool (storage buffers) — renderer crate |

A renderer crate that honors the invariant must **absorb** these, not just relocate `render/`. `candidate_cull_mirror.rs` (651, 1 ref) and `candidate_cull_probes.rs` (202, 0) are CPU test oracles — dev/test-side, not shipping GPU.

## The central knot: `Renderer` / `FullRenderer`

`render/renderer_types.rs:348` defines a thin boot `Renderer { device, queue, surface, surface_config, …, full: Option<Box<FullRenderer>> }`; `:384` defines `FullRenderer`, a ~120-field god-struct owning every steady-state pipeline/buffer/bind-group/texture and every sub-pass. The ~15 `renderer_*.rs` files are `impl Renderer` blocks reaching into `FullRenderer` fields via `pub(super)` — **zero encapsulation**; one logical object spread across files.

Decisive asymmetry vs. scripting's `ComponentValue`: `FullRenderer` is **wgpu-internal — never a consumer-visible type**. The inbound boundary is mediated by small CPU handoff types (`LevelGeometry<'a>`, `UiReadSnapshot`, `FrameUniforms`, `FocusRectList`) + the `Renderer` method API. So the first crate cut does **not** require untangling the knot. Converting `pub(super)` reach-in to owned `device/queue` + explicit BGL-handle constructors is the analog of the scripting layered-floor refactor — but it is a prerequisite **only** for pass-level sub-crates, which are out of scope unless explicitly wanted.

## Boundary census

**Inbound — non-render code importing FROM `render::`** (the renderer crate's would-be public surface):
- `main.rs` — the whole `Renderer` method API (~40 methods: `resize`, `install_level_geometry`, `update_per_frame_uniforms`, `set_mesh_draws`, `render_frame_indirect`, `set_ui_snapshot`, `export_ui_focus_rects`, `render_debug_ui`, `render_splash_frame`, the `emit_*_diagnostics`/`*_overlay` setters, toggles) + `render::ClearColor`, `render::SpatialDiagnostics`, `render::splash_pass::PresentOutcome`, `render::debug_ui::DebugUi`.
- `startup/lifecycle.rs` — `render::ClearColor`, `render::level_world_to_geometry`, `render::ui::modal_stack::{ModalStack,ScopeTier}`, `render::ui::demo::{FRONTEND_MENU_NAME,build_frontend_menu_descriptor}`, `render::ui::tree::{FocusNeighbors,FocusRect,FocusRectList,NodeInteraction}`, + Renderer install methods.
- `session/mod.rs` — `render::ui::tree::FocusRectList`, `render::ui::modal_stack::ModalStack`, `render::debug_ui::DebugUi`, `render::ui::tree_asset::{register_tree_from_disk,HUD_NAME}`, `render::ui::demo::{PAUSE_MENU_NAME,FRONTEND_MENU_NAME}`, `render::ui::keyboard_asset::KEYBOARD_TREE_NAME`.
- `startup/splash_lifecycle.rs` — `render::splash_pass::PresentOutcome` + splash present flow.
- `input/ui_focus.rs` — `render::ui::tree::{FocusKind,FocusRect,FocusRectList,NodeInteraction,RepeatPolicy,FocusGroup,FocusNeighbors}`.
- `scripting/systems/presentation_cells.rs` — `render::ui::descriptor::{AnchoredTree,CellInit,LocalState,Widget,…}`, `render::ui::tree::CellValues`, `render::ui::layout::Anchor`.
- `scripting/systems/light_bridge.rs` — `render::sh_volume::{…}` (SH/delta packing types).
- `scripting/systems/mesh_render.rs` — `render::mesh_instances::MeshInstanceInput`, `render::mesh_pass::{mesh_visible,ClipMetadata}`; `mesh_anim.rs` — `render::mesh_pass::ClipMetadata`.
- `model/`, `lighting/lightmap.rs`, `lighting/spot_shadow.rs` — **doc-comment references only**; real direction is render → model and render → lighting (correctly layered).

**Outbound — `render/` importing FROM the engine** (deps the renderer crate needs as lower crates):
`crate::prl` (`LevelWorld`, `MapLight`, `LightType`, `FalloffModel`, `ShadowType`, `LightmapMode`, `CellDrawIndex`); `crate::lighting` (CPU: `pack_lights`, `pack_lights_with_slots_into`, `GPU_LIGHT_SIZE`, `influence`, `spec_buffer::{SPEC_LIGHT_SIZE,pack_spec_lights}`, `chunk_list::ChunkGrid` / GPU: `lightmap::LightmapResources`, `spot_shadow::{SpotShadowPool,light_space_matrix}`); `crate::geometry` (`WorldVertex`, `BvhTree`, `BvhLeaf`); `crate::visibility` (`CameraCullVisibility`, `VisibilityPath`, `VisibleCells`); `crate::compute_cull`/`candidate_cull`/`shadow_cull` (pipelines); `crate::model` (CPU loader handles); `crate::material` (`Material`); `crate::fx` (`smoke::{SpriteFrame,SPRITE_INSTANCE_SIZE}`, `fog_volume::{…}`); `crate::nav` (`NavGraph`, diagnostics); `crate::input` (`UiCaptureMode`); `crate::ui_texture` (`UiTexture`); `crate::startup` (`SplashSource`).

## GPU/CPU partition of the lower modules

**`lighting/` splits cleanly** (verified per-file wgpu counts):
- CPU-math (wgpu-free): `mod.rs` (802 — `pack_light_with_slot`, `pack_lights`, `pack_lights_with_slots_into`, `light_reaches_visible_cell`, `entity_occluder_eligible`, `GPU_LIGHT_SIZE`, slot byte-offset consts), `influence.rs` (50 — `LightInfluence`, `pack_influence`), `spec_buffer.rs` (224 — `pack_spec_lights`, `SPEC_LIGHT_SIZE`), `cone_frustum.rs` (440 — `Aabb`, `cone_frustum_planes`, `aabb_intersects_frustum`; also consumed by `weapon/` hit-zones, `model/mesh`, `shadow_cull`), `script_primitives.rs` (1223 — light scripting-primitive wiring; placement per `scripting.md §12` handler rule, see open questions).
- GPU pools (must move into renderer crate): `spot_shadow.rs`, `cube_shadow.rs`, `lightmap.rs`, `chunk_list.rs`.

**`fx/` is pure CPU data** (wgpu-free): `smoke.rs` (156 — `SpriteFrame`, `load_collection_frames`, `SPRITE_INSTANCE_SIZE`, `MAX_SPRITES`), `fog_volume.rs` (315 — `FogVolume`, `FogSpotLight`, packing constants). Consumed by both `scripting/systems/{emitter_bridge,particle_render,fog_volume_bridge}.rs` and the GPU passes `render/smoke.rs` + `render/fog_pass.rs`.

**Lower CPU prerequisite modules** (all wgpu-free, none are crates yet):
- `geometry.rs` (195) — `WorldVertex`, `BvhNode`, `BvhLeaf`, `BvhTree`, `derive_bucket_ranges`. No internal deps. 10 importers.
- `material.rs` (338) — `Material` enum, `MaterialProperties`, `parse_prefix`, `derive_material`. No internal deps. 6 importers.
- `prl.rs` (4279) + `prl_loader.rs` — `LevelWorld`, `load_prl` (re-export; defined in `prl_loader.rs`), `MapLight`, `LightType`, `FalloffModel`, `ShadowType`, `LightmapMode`, `CellDrawIndex`, `PortalData`, `FaceMeta`, `LevelWorld::{locate_cell,spawn_position,cell_*}`. Imports `geometry` (`WorldVertex`, `BvhTree`) + `material` (`Material`). 21 importers. **The internal split (`prl.rs`/`prl_loader.rs`) already exists.**
- `visibility.rs` (1001) — `VisibleCells`, `CameraCullVisibility`, `VisibilityStats`, `VisibilityPath`, `VisibilityResult`, `determine_visible_cells`. `Frustum`/`FrustumPlane` are **`pub(crate)`** (`:117`,`:128`) — need widening to `pub` for a crate split. Imports `portal_vis`, `prl`. 8 importers.
- `portal_vis.rs` (1995) — `portal_traverse`, `narrow_frustum`, `clip_polygon_to_frustum`. Imports `prl` (`LevelWorld`), `visibility` (`Frustum`,`FrustumPlane`). Consumed by `visibility::determine_visible_cells`.
- `ui_texture.rs` (12) — `UiTexture` (CPU RGBA8 bytes). Zero wgpu. Consumers: `render/splash*`, `render/ui/mod`.

### Stale identifiers in the old draft (do not propagate)

`compile-time-reduction` named PRL types that **do not exist** in source: `BspChild`, `NodeData`, `LeafData`, `LevelWorld::find_leaf`. Real equivalents: `CellLocatorChild` (`prl.rs:121`), `CellLocatorNodeData` (`prl.rs:127`); no `LeafData` (leaf data lives on `BvhLeaf`); the locate method is `locate_cell` (`prl.rs:361`), not `find_leaf`. Treat the old draft as raw material.

## Present-handle leak (the public-API hazard)

- `render/renderer_render_frame.rs:9` — `render_frame_indirect(...) -> Result<Option<wgpu::SurfaceTexture>>`. Acquires `self.surface.get_current_texture()` (`:27`, with Suboptimal reconfigure), creates a `TextureView` (`:48`).
- `main.rs:2561` calls it; `main.rs:3367` does `surface_texture.present()` directly — so the binary touches wgpu surface types.
- `render/renderer_splash.rs` / `splash_pass.rs` handle present **internally** (`output.present()`), duplicating the surface acquire/error state machine.
- An opaque present handle must encapsulate: surface acquire (Success/Suboptimal/Outdated/Lost/Timeout/Validation), texture-view creation, encoder completion, and `present()` — so no consumer sees `wgpu::SurfaceTexture`.

## UiCaptureMode (the one outbound dep blocking clean `render::ui`)

- Defined `input/ui_dispatch.rs:46` — `enum UiCaptureMode { Capture, Passthrough }`.
- `render/ui/mod.rs:273` — `impl From<descriptor::CaptureMode> for crate::input::UiCaptureMode`. `descriptor::CaptureMode` already lives in `scripting-core` (`ui/descriptor/envelope.rs:23`), which is *below* the UI.
- Stored in `UiReadSnapshot` (`render/ui/mod.rs:264`); `main.rs` reads it and feeds input dispatch.
- Resolution (chosen): invert — `postretro-ui` uses `descriptor::CaptureMode` (it already depends on scripting-core); the `From<…> -> input::UiCaptureMode` conversion moves to the binary (which depends on both). `UiReadSnapshot` carries `descriptor::CaptureMode`. `postretro-ui` never imports `input`.

## Crate conventions to mirror (`scripting.md §12` + Cargo)

- Workspace: edition 2024, rust 1.85, `[workspace.package]` inheritance, shared `[workspace.dependencies]` via `glam.workspace = true`. Members today: `postretro`, `foundation`, `entities`, `scripting-core`, `level-format`, `level-compiler`, `script-compiler`, `net`. Naming `postretro-<role>`.
- One-way dependency, top→bottom. VM-free crates keep VM bindings behind an optional `script-ffi` feature (`script-ffi = ["dep:rquickjs","dep:mlua"]`); upper crates forward (`"postretro-foundation/script-ffi"`) and depend `default-features = false`. The firewall target is warm incremental edit loops, proven by `cargo tree` isolation.
- Handler placement (`scripting.md §12`): script-primitive *wiring* (`register_*` + closures) co-locates with the subsystem and is invoked from `Session::build`; the marshalling substrate stays in `scripting-core`. Bears on where `lighting/script_primitives.rs` lands.

## Oversized files touched (split-before-extend)

`prl.rs` (4279 — already partly split into `prl_loader.rs`), `render/mesh_pass.rs` (3529), `render/sh_volume.rs` (2443), `portal_vis.rs` (1995), `main.rs` (~6.7k), `startup/lifecycle.rs` (2443). Extractions that must substantially edit these split along the seams first.
