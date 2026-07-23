# Emissive Surfaces + Bloom — Research Notes

Code-grounded facts behind the spec. Ephemeral — not durable context. All
identifiers/line numbers verified against source at draft time; they drift.

## Headline finding: the pipeline is LDR end-to-end

`scene_color` is allocated at the **surface format** (sRGB, `Rgba8UnormSrgb`-class),
**not** `Rgba16Float` (`screen_effects.rs:245`). The forward pass writes lit color
straight into it (`forward.wgsl:1331` `return vec4(rgb, base_color.a)`,
`rgb = base_color.rgb * total_light`, `:1276`). The resolve is an **identity blit +
flash/vignette/shake — no tonemap** (`render/screen_effects.rs:18` documents the
sRGB byte-identity contract, `shaders/screen_effects.wgsl:6` "identity blit"). Emissive
> 1.0 would clamp in 8-bit sRGB. (Confirmed intact post-merge.)

The roadmap/`resource_management.md §4.5` phrasing ("scene_color is the HDR target
with a tonemap") is **aspirational, not current**. Additive HDR emissive + bloom
requires:
- forward + all scene-pass color targets and `scene_color` → `Rgba16Float`;
- a tonemap operator (HDR → sRGB) inserted at/before the resolve;
- rewriting the documented sRGB byte-identity resolve contract
  (`screen_effects.rs:18`, `screen_effects.wgsl:6` "identity blit");
- reconciling E20 capture — `read_texture_rgba8` assumes 4 bytes/px
  (`render/renderer_frame.rs:29`, `checked_mul(4)` `:37`), PNG write is `ColorType::Rgba8`
  (`crates/postretro/src/capture/driver.rs:348`), offscreen `capture_format =
  Rgba8UnormSrgb` (`render/renderer_init.rs:109`).

Frame-graph order (`render/renderer_render_frame.rs`, **1001 lines**,
`record_scene_passes`) — **rebaselined against merged main** (animated-direct-sh
landed; the pre-merge anchors below drifted +82 lines):
pre-scene compute → direct_sh_compose → shadows → depth+SDF → **Forward** (`:414`)
→ Kinematic Brush (`:493`) → Skinned Mesh (`:556`) → Smoke (`:599`) → **Fog
Composite** (`:706-722`) → *[capture returns `:728`]* → Wireframe → debug_lines →
**Viewmodel** (`:774`, new post-merge) → UI → **resolve** (`encode_resolve`, `:945`).
**Bloom must composite into `scene_color` after fog composite (`:722`) and BEFORE the
capture return (`:728`)** — so E20 captures include the bloom (AC 3/AC 4 agree) and the
overlay passes (wireframe/debug/viewmodel/UI) are left un-bloomed. The looser "before
resolve" span is wrong: it straddles the capture return.

## `.prm` format — hardcoded 3 slots

`crates/level-format/src/prm.rs` (1012 lines):
- `PrmSlots: u8` bitflags — `DIFFUSE=0b001`, `SPECULAR=0b010`, `NORMAL=0b100`
  (`:72-81`). Bits 3-7 reserved; `parse_header` (`:470-476`) rejects any unknown
  bit via `PrmSlots::from_bits` → `PrmReadError::ReservedSlotBitsSet`. Bit 3 doc'd
  as emissive placeholder (`:15`); regression test `reserved_slot_bits_are_rejected`
  (`:742-755`) sets bit 3 and asserts rejection — must be **inverted**.
- `PrmFile.slots: [Option<PrmSlot>; 3]` (`:167`) — fixed width. `from_bytes_partial`
  returns `[Result<PrmSlot,_>; 3]` (`:369-373`). Four wire-order iteration arrays
  hardcode 3 bits (`:296, :313, :338, :404`).
- `PrmFormat` enum (`:85-104`): `Rgba8UnormSrgb=0`, `Rgba8Unorm=1`, `R8Unorm=2`,
  `Bc5RgUnorm=3`. **Emissive slot = `Rgba8UnormSrgb`** (color) — unlike linear
  `_s`/`_n`.
- Header 43 B (`HEADER_SIZE`), per-slot header 12 B (`SLOT_HEADER_SIZE`),
  `STAGE_VERSION=2` (`:44`). Bundle content-hash includes the mask byte + per-slot
  PNG bytes (`bundle_hash_for`), so a bundle that gains `_e.png` gets a **new**
  cache filename — no stale collision; existing 3-slot files parse unchanged under
  the widened reader.

## Bake pipeline — `crates/level-compiler/` (log tag `[prl-build]`)

- `texture_mips.rs` (**1553 lines** ⚠️, unchanged by merge — Task 5 split rationale
  intact): `bake_texture_mips` (`:715`); sibling discovery `:743-744` (`{base}_s`/
  `{base}_n`; the map is built by `build_name_to_path_map` `:57`, `name_to_path` is a
  local var `:720`); slot-build cascade `slot_mask |= …` at `:817/:832/:856`;
  per-slot chain builders `build_diffuse_chain` (`:422`), `build_specular_chain`
  (`:470`), `build_normal_bc5_chain` (`:505`); Mitchell-Netravali `mitchell_netravali`
  (`:279`, `MN_B=MN_C=1/3`); `bundle_hash_for` (`:169`), `filename_key_for` (`:203`),
  `cache_entry_has_valid_declared_slots` (`:226`) — each enumerates the 3 slots and
  needs an emissive arm.
- `texture_validation.rs` (400 lines): `collect_sibling_pngs` (`:108`, suffix match
  `:140`, `_n`/`_s` only), required-colorspace table (`:9-10`),
  `validate_sibling_color_spaces` (`:159`). Emissive is **sRGB** — a semantic new
  arm, not a copy of the linear `_s`/`_n` arms.
- `pipeline.rs` (**1392 lines** ⚠️, +52 post-merge): calls `bake_texture_mips`
  (`:1281`), `validate_sibling_color_spaces` (`:284`).

## Renderer material path — `crates/renderer/`

- `render/loaded_texture.rs` (489): `LoadedTexture` (`:26-43`) — diffuse/specular/
  normal `(Texture,View)` + `mip_count`. Built at 3 sites (`:205`, `:316`, `:383`).
  Placeholders are **per-`LoadedTexture`, not shared singletons**:
  `make_specular_placeholder` (`:176`, 1×1 R8 black), `make_normal_placeholder`
  (`:189`, 1×1 Rgba8 neutral). Emissive → `make_emissive_placeholder` (1×1
  `Rgba8UnormSrgb` black `[0,0,0,255]`). Upload: `load_textures` (`:228-328`),
  `upload_slot_or_placeholder` (`:401-445`), `enum Slot{Diffuse,Specular,Normal}`
  (`:395`).
- `render-cpu/src/loaded_texture.rs` (217): `TextureSlotPlan.consume: [bool;3]`
  (`:66-70`) → widen to `[bool;4]`; `WorldBundle` consumes emissive,
  `ModelDiffuseOnly` does not.
- **Group 1 material bindings** (`shaders/forward.wgsl:72-98`, vacated-binding-1
  comment `:97`; BGL `render/pipeline_layout.rs:234`, `[_;5]`): 0=diffuse, **1=VACATED
  (free — the emissive slot; confirmed still free post-merge — the animated-direct
  atlas bound at group-3 binding 15, not group-1)**, 2=specular(R8), 3=shininess
  uniform, 4=normal, 5=sampler.
- **Sampled-texture budget at the ceiling.** `forward_pipeline_sampled_texture_count`
  (`pipeline_layout.rs:400`) = **15** with cube support (confirmed unchanged
  post-merge), against a **16** design floor. Emissive → group 1 = 4, total =
  **16/16, zero headroom**. `crates/renderer/src/render/tests/pipeline_budget_tests.rs`
  hardcodes `[0,3,0,3,5,4]==15` (`:144/:148`) — update. Emissive also joins the
  **shared** group-1 BGL, so verify the kinematic-brush (and, if it shares this BGL,
  skinned-mesh) fragment budgets too, not only forward.
- `build_material_bind_group` (`render/material_plan.rs:80`, shininess packed `:88`)
  `[_;5]→[_;6]`; 4 call sites: `renderer_init_resources.rs:608`,
  `renderer_models.rs:65/84/345`.
- Shaders sharing group 1: `forward.wgsl` (**1332** ⚠️), `kinematic_brush.wgsl`
  (364) — both add emissive sampling + additive term. `skinned_mesh.wgsl` declares
  only bindings 0,5 (legal subset) — unchanged; **model emissive out of scope**.

## Animation-channel seam — net-new, deferred

- Light-brightness pattern (`scripting/systems/light_bridge.rs`, 1742):
  `eval_effective_brightness` (`:723`) per-frame CPU scalar; reaction →
  per-entity `LightComponent.animation` → `LightBridge::update` diff → GPU repack.
  This is a **per-entity component** seam.
- **No per-material / per-surface runtime emissive scalar exists.** `emissive`
  appears only in the baked `.prm` format + docs. World surfaces are static
  material data, not entities — the light seam does **not** transfer. Mesh bridge
  (`scripting/systems/mesh_render.rs`) has no per-material scalar channel either.
- Driving emissive intensity from a trigger (the button-glow case) is therefore
  net-new runtime state (per-surface or per-mesh-entity emissive scalar + GPU
  feed) → **deferred follow-up**, not v1.
- Aside: the `single-source-animated-light-brightness` draft proposes storing a
  scalar in `GpuLight` `.w` pad — that pad is **now taken** (cube shadow slot
  index, `crates/lighting/src/lib.rs:65,80,311`; `forward.wgsl:1221`). Unrelated to
  this spec, but confirms the animation-channel work is non-trivial.

## Material enum — not connected to `.prm`

`crates/render-data/src/material.rs` (335): `Material{Metal,Concrete,Grate,Neon,
Glass,Wood,Default}`. `Neon` exists but carries no emissive flag; the enum drives
footstep/impact/ricochet only and is **not** wired to the slot mask. Emissive is a
texture-slot property, independent of this enum. (Prefix heuristics for
`gen_emissive.py` may still key on `neon_`.)

**Wiring nuance for emissive_strength.** A `MaterialProperties` struct **already
exists** (`:24`) but carries gameplay-only `ricochet: bool`, is `#[allow(dead_code)]`
via `properties()`, and is **never fed to the GPU**. The value that *is* GPU-fed is
`shininess()` — a **standalone method**, not a `MaterialProperties` field — packed into
the group-1 material uniform at `material_plan.rs:88`. So `emissive_strength` must
follow the **`shininess()` path** (a standalone `Material` method packed into the
uniform), **not** be added as a `MaterialProperties` field; the latter would be net-new
GPU wiring, not the parallel it looks like.

## Oversized files this work extends (split-first candidates)

| File | Lines | Edit |
|---|---|---|
| `crates/level-compiler/src/texture_mips.rs` | 1553 | emissive sibling arm across ~6 fns |
| `crates/level-compiler/src/pipeline.rs` | 1392 | 2 call sites (localized) |
| `crates/renderer/src/shaders/forward.wgsl` | 1332 | emissive sample + additive term |
| `crates/renderer/src/render/renderer_types.rs` | 1079 | bloom/tonemap pass fields |
| `crates/level-format/src/prm.rs` | 1012 | slot 3→4 (localized, ~6 sites) |
| `crates/renderer/src/render/renderer_render_frame.rs` | 1001 | bloom insertion + capture |

Only `texture_mips.rs` has an edit that genuinely **tangles across functions** (the
emissive arm threads through ~6) → split-first (Task 5). `forward.wgsl`'s emissive
edit is **localized** (one binding declaration + one additive term), despite the
file's size — no split warranted. The rest take localized/coherent edits.

## Coordination — `plans/done/animated-direct-sh-dynamic-receivers` (MERGED)

Work on how movers/skinned/billboards receive a baked light's animated **direct**
term. **Merged into main**; this draft's branch is rebased onto it. Verified against
merged source:
- Producer-side only: consumers (`kinematic_brush.wgsl`, `skinned_mesh.wgsl`,
  `billboard.wgsl`) unchanged — they already sample the composed direct atlas at
  **group-3 binding 15**. So no shader conflict with emissive's post-`total_light`
  additive term, **and group-1 binding 1 stayed vacated** for emissive.
- Composes its animated-direct atlas in `Rgba16Float` (like all lighting atlases) —
  Task 1's HDR scene target completes that HDR lighting path, not a bolt-on.
- Shared-file merge points (different regions): `pipeline.rs`, `forward.wgsl`,
  `renderer_render_frame.rs`. That plan landed first; the file:line facts above are
  **rebaselined onto the merged result** (renderer_render_frame.rs 919→1001, pipeline.rs
  1340→1392, forward.wgsl 1328→1332, new viewmodel pass at `:774`). Budget still 15.
- Movers bind the world-material bundle (`rendering_pipeline.md §7.3`) → emissive
  reaches movers with no mover-path work (serves the button-as-`kinematic_mover` case).
- Shadowing is orthogonal: emissive is self-illumination, casts/receives no shadow.
