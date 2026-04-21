# Plan 2 — Light Entity Scripting

> **Status:** draft — first feature plan on top of Plan 1's runtime foundation.
> **Parent:** Scripting Foundation initiative. See `./postretro-scripting-spec-draft.md` for the full system vision.
> **Depends on:** `./plan-1-runtime-foundation.md` — sub-plans 1–5 must ship before any work here begins. Sub-plans 6 (context pool) and 7 (hot reload) are not hard prerequisites but are assumed available for dev iteration.
> **Related (read for architectural context):** `context/lib/development_guide.md` §3 · `context/lib/context_style_guide.md` · `context/plans/done/lighting-foundation/index.md` (plan format reference) · `context/plans/done/lighting-foundation/7-animated-sh.md` (the animation packing path this plan extends).
> **Lifecycle:** Per `development_guide.md` §1.5, this plan is its own spec while it stays a draft. Durable knowledge migrates to `context/lib/scripting.md` (created by Plan 1) and `context/lib/rendering_pipeline.md` when it ships.

---

## Goal

Bring lights into the scripting surface as first-class entities. After this plan ships:

- Definition scripts (TS/Luau) can declare entities with a `light` component, including a nested `animation` sub-object that mirrors the Rust `LightAnimation` shape.
- Behavior scripts can call `set_light_animation(entity, animation)` to change a light's animation curve at runtime.
- Runtime-spawned light entities (`spawn_entity` + `set_component` with a `LightComponent`) appear in the same GPU light buffer as map-authored lights and evaluate every frame through the existing keyframe path in `forward.wgsl`.
- A new `direction` animation channel exists on `LightAnimation` / `AnimationDescriptor`, enabling spotlight aim sweeps without adding per-tick scripting overhead.
- `on_tick` bridge handlers receive a `delta: f32` (seconds since last tick) and an accumulated `time: f32`. No wall-clock / `Date.now()` access from scripts.

**What this plan does not deliver:**

- FGD → `LightComponent` wiring for map-authored `light_omni` / `light_spot` / `light_sun` entities. Map lights continue to flow through the existing `AlphaLightsSection` → `MapLight` path and merge with scripted `LightComponent`s at the bridge (Sub-plan 3). This asymmetry is intentional for now: plan 3 sub-plan 8 wires `env_smoke_emitter` FGD props to `EmitterComponent::from_fgd_props` because emitters have no pre-existing baked representation, whereas map lights already have a battle-tested compile-time path. A future plan may unify both under a `FromFgdProps` trait shape so `LightComponent::from_fgd_props` mirrors `EmitterComponent::from_fgd_props` — **open question, not a blocker**.
- `emitter` component / particle spawning — Plan 3.
- Physics primitives (`apply_impulse`, `is_grounded`, `raycast`, …) — deferred until Rapier lands.
- NPC scripting, perception, patrol — later plan.
- Archetype `extends` — later plan.
- Kinematic brush geometry (moving doors) — future infrastructure.
- Production bytecode caching — later plan.
- Luau reference vocabulary (`light.luau`, `light_animation.luau`). Plan 1's `light_animation.luau` equivalents are a follow-up **inside Plan 2's scope** once the TS vocabulary validates the API shape — tracked as the final sub-plan here.

---

## Settled decisions (do not relitigate)

- **Script API mirrors Rust data-model shape.** `animation` nests inside `light()`, not beside it. This means the component descriptor for a light carries its animation, which matches `MapLight.animation: Option<LightAnimation>` in `postretro-level-compiler/src/map_data.rs`.
- **Keyframe path, not per-tick setters.** Scripts write a `LightAnimation` once (at spawn or on a trigger); the GPU evaluates it every frame via `eval_animated_*` in `forward.wgsl`. The primitive is `set_light_animation(entity, LightAnimation)`. No `setLightIntensity(entity, value)` called every tick.
- **`delta` injected into `on_tick`.** Scripts read `delta: f32` and `time: f32` from the call context. Wall-clock access is not exposed. Required for determinism.
- **`defineNpc` / specializations are composers.** There is no `defineLight`. A light entity is `defineEntity` with a `light` component. `light()` and (future) `emitter()` are component constructors.
- **Pre-release API policy.** Rename freely, no compat shims, fix all consumers in the same change (`MEMORY.md` / `feedback_api_stability.md`).

---

## Prerequisites from Plan 1

Plan 1's sub-plans must be complete and landed:

| Plan 1 sub-plan | What this plan needs from it |
|---|---|
| 1 — Entity / component registry | `ComponentKind` enum to extend with `Light`; `EntityRegistry::set_component` as the write path for `LightComponent`. |
| 2 — Primitive binding layer | `register(...)` builder and `ScriptPrimitive` record used to register `set_light_animation`. `catch_unwind` wrapping of the new primitive. |
| 3 — QuickJS runtime + contexts | Behavior context owning the `set_light_animation` primitive; `ScriptCallContext` extended with `delta`/`time`. |
| 4 — Luau runtime + contexts | Symmetric behavior-context registration of `set_light_animation`. |
| 5 — Type definition generator | Emits `LightComponent` / `LightAnimation` / `AnimationDirection` types into `postretro.d.ts` and `postretro.d.luau` automatically once the new component kind is registered. |

Sub-plans 6 (context pool) and 7 (hot reload) are not gating — the light API works without them — but dev iteration on the reference vocabulary uses hot reload.

---

## Crates touched

| Crate | Role |
|---|---|
| `postretro-level-format` | `AlphaLightsSection` stays unchanged on-disk (interim format per its doc header). No changes unless Sub-plan 1 discovers the direction channel needs to appear on the wire — see that sub-plan. |
| `postretro-level-compiler` | Extend `LightAnimation` in `map_data.rs` with a `direction: Option<Vec<[f32; 3]>>` channel. Update the Quake style table translator to emit the new field (start with `None`). |
| `postretro` | New `LightComponent` in the scripting registry. New `set_light_animation` primitive. New light bridge system diffing components → GPU buffer. `AnimationDescriptor` extended in all three shaders. `eval_animated_direction` added in `forward.wgsl`. `ScriptCallContext` extended with `delta`/`time`. |
| `assets/scripts/sdk/` (new, ships as readable source) | `light.ts`, `light_animation.ts`. Luau ports (`light.luau`, `light_animation.luau`) are the final sub-plan. |

No changes to `postretro-level-format` on-disk layout. The direction channel ships first as an in-memory engine concept; the wire format gets the channel only when scripts need it to persist, which is not this plan.

### Serialization pitfalls surfaced by research

The script-facing `LightAnimation` is serde-round-tripped across both FFI boundaries. Research findings that shape implementation choices below:

- **Fixed-size Rust arrays serialize as tuples by default** (`[f32; 3]` → JS/Lua tuple-style, not `[x, y, z]`). This is a long-standing serde behavior ([serde#989](https://github.com/serde-rs/serde/issues/989), [serde#391](https://github.com/serde-rs/serde/issues/391)). The direction channel type `Vec<[f32; 3]>` deserialized from a script-side `number[][]` will fail without help. **Fix:** at the scripting/FFI boundary, use `Vec<Vec3Lit>` where `Vec3Lit` is a `#[serde(from = "[f32; 3]")]`-friendly newtype with an explicit `Deserialize` impl accepting a JS `Array` / Lua sequence of three numbers. Convert to `[f32; 3]` inside the bridge before writing the `LightComponent`. Do not punt this to runtime — a clear "expected three-number array" error at the FFI boundary beats a cryptic serde tuple mismatch.
- **mlua with `serde` feature** round-trips nested tables and `Vec<T>` cleanly via `LuaSerdeExt::from_value`. The same `Vec3Lit` adapter works unchanged.
- **`notify-debouncer-full` has known edge cases** with rapid save patterns (create-then-modify events can be collapsed; delete-restore rename chains can drop). This affects Plan 1's hot reload, not the light primitive itself, but the reference-vocabulary sub-plan here exercises the watcher. If an editor's atomic-save pattern causes a missed reload, treat it as a Plan 1 follow-up — do not add special-case handling in the light bridge.

---

## Sub-plan dependency graph

```
1. Direction animation channel (pure engine: data + shaders)
    │
    ├─→ 2. LightComponent in the ECS registry
    │        │
    │        ├─→ 3. Light bridge system (map lights + scripted lights → GPU buffer)
    │        │
    │        └─→ 4. set_light_animation primitive
    │
    ├─→ 5. delta / time injection into on_tick
    │
    └─→ 6. TypeScript reference vocabulary (light.ts, light_animation.ts)
           │
           └─→ 7. Luau reference vocabulary (follow-up, closes out the plan)
```

Sub-plan 1 must land first — every downstream piece assumes the new channel exists. Sub-plans 2 and 5 are independent of each other; 3 and 4 both depend on 2. Sub-plan 6 can start in parallel with 3/4 once the `light()` descriptor shape is pinned in 2. Sub-plan 7 waits for 6 so the Luau port is a translation, not a re-design.

---

## Sub-plan 1 — Direction Animation Channel

**Scope:** Pure engine change. Add a third animation channel to `LightAnimation` and its GPU descriptor. No scripting APIs touched.

### Description

The existing `LightAnimation` supports `brightness` (scalar) and `color` (`[f32; 3]`) channels. Static `cone_direction` lives on `AlphaLightRecord`. For spotlight sweeps and animated aim, add a `direction: Option<Vec<[f32; 3]>>` channel evaluated the same way as `color` — linearly interpolated between adjacent samples over the period, wrapping.

Direction samples are normalized at write time. The shader does not re-normalize per frame; a `lerp` between two unit vectors is close enough for low-rate authored curves, and the cost of `normalize` per fragment per animated light is not paid. Document this in the shader comment — if a curve produces visible length drift, re-normalize in the packer before upload, not in the shader.

**Color animation restriction.** `LightAnimation.color` must be `None` for any light where `bake_only == false && is_dynamic == false` (lights whose direct contribution is baked into the static lightmap). A runtime color channel on such a light would vary the SH volume layer while the baked direct contribution stays at the compile-time color, producing a visible mismatch. This restriction lifts when the animated-light-weight-maps spec lands and removes animated lights from the static bake entirely. Until then, the compiler enforces it with an error at pack time. Scripted lights (`is_dynamic == true`) are exempt — they are never baked.

### Files changed

- `postretro-level-compiler/src/map_data.rs` — add `pub direction: Option<Vec<[f32; 3]>>` to `LightAnimation`. Update the `PartialEq` derive (still valid). Update the one test constructor in `sh_bake.rs` that instantiates `LightAnimation`.
- `postretro-level-compiler/src/format/quake_map.rs` — where the Quake style table expands to `LightAnimation`, set `direction: None`. No style produces a direction curve.
- `postretro-level-compiler/src/sh_bake.rs` — extend the packing path near `ANIMATION_DESCRIPTOR_SIZE`. Adjust the pack/unpack routines. Direction samples serialize into `anim_samples` after color samples, three floats each.

  **`AnimationDescriptor` constraint:** `ANIMATION_DESCRIPTOR_SIZE` must stay at 48 bytes — `direction_offset` and `direction_count` must fit in the existing tail padding without growing the struct. Other specs also reserve tail padding for their own fields; the implementor verifies the final placement is clean given what has already landed, and updates the layout doc comment in `sh_volume.rs` accordingly.
- `postretro-level-format/src/sh_volume.rs` — the `AnimationDescriptor` wire struct and its `to_bytes`/`from_bytes`. Update the on-disk layout (this is the one wire-format change in the plan) and bump any section-version constant if one exists. Extend unit tests for the new fields. **Gate:** confirm with the owner before landing the wire-format bump — per `development_guide.md` §1.6, breaking changes need sign-off.
- `postretro/src/shaders/forward.wgsl` — extend `struct AnimationDescriptor` with `direction_offset: u32, direction_count: u32` (replacing the current tail `_padding vec2<f32>`). Add `fn eval_animated_direction(desc, cycle_t) -> vec3<f32>` mirroring `eval_animated_color`. Keep the 48-byte stride commented in the struct preamble — the new fields consume existing padding, so stride is unchanged.
- `postretro/src/shaders/billboard.wgsl` and `postretro/src/shaders/fog_volume.wgsl` — mirror the `struct AnimationDescriptor` change. Neither currently evaluates directions; they only need the struct layout to match so the shared storage buffer binds cleanly.
- `postretro/src/render/sh_volume.rs` — `ANIMATION_DESCRIPTOR_SIZE` stays at 48 (see layout analysis above). Update the layout doc comment on the constant so the field map reflects `direction_offset`/`direction_count` in place of the former pad + `_padding vec2`. Re-check the WGSL parity / stride assert near the `forward.wgsl GpuLight stride` area — confirm during implementation.

### Acceptance criteria

- [ ] `LightAnimation` carries a `direction` channel end-to-end: authored in compiler `map_data`, serialized through `sh_volume`, bound to the shader.
- [ ] `eval_animated_direction` returns the expected lerp for synthetic samples (unit test in a WGSL harness if available; else a CPU-side reimplementation test paired with a pixel-level check).
- [ ] A test map with a `light_spot` whose animation carries a two-sample direction curve visibly sweeps its cone between the two directions over `period`.
- [ ] All three shaders compile. `ANIMATION_DESCRIPTOR_SIZE` remains 48 after adding the direction fields.
- [ ] Compiler errors at pack time if a non-`bake_only`, non-`is_dynamic` light has `animation.color.is_some()`. Error message names the light and explains the restriction.
- [ ] `cargo test -p postretro-level-format`, `cargo test -p postretro-level-compiler`, `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Add the direction fields to the WGSL `AnimationDescriptor` struct in all three shader files, fitting them into the existing tail padding without changing `ANIMATION_DESCRIPTOR_SIZE`. Update the layout doc comment in `sh_volume.rs` to reflect the new fields.
2. Extend the serialize/deserialize paths in `postretro-level-format/src/sh_volume.rs`. Add a unit test that round-trips a descriptor with a 4-sample direction curve.
3. Add the `direction: Option<Vec<[f32; 3]>>` field to `LightAnimation`. Fix all construction sites (compiler tests, translator).
4. Extend `sh_bake.rs` pack logic: append direction samples to `anim_samples`, populate the new descriptor fields.
5. Implement `eval_animated_direction` in `forward.wgsl`. Wire it into the spot-light evaluation path so `cone_direction` becomes the animated value when `direction_count > 0`, else falls back to the static `cone_direction`.
6. Author or modify a test map with an animated spot direction. Verify visually.
7. Add validation in the pack pipeline (`sh_bake.rs` or `pack.rs`) that errors with a clear message if `LightAnimation.color` is `Some` on a baked animated light (`!bake_only && !is_dynamic`). Add a unit test asserting the error fires.

---

## Sub-plan 2 — `LightComponent` in the ECS Registry

**Scope:** Rust only. Add `Light` to `ComponentKind`. Define the script-facing `LightComponent` struct. Wire it into `EntityRegistry::set_component`.

### Description

`LightComponent` is the script's view of a runtime light. Its fields mirror `MapLight` as closely as the wire format allows, minus compiler-only concerns (`bake_only`, `is_dynamic`). Runtime-spawned scripted lights are dynamic by definition — they never feed the SH bake, which is compile-time-only.

### Struct shape

```rust
// Proposed design — lives in postretro/src/scripting/components/light.rs.
// See: context/plans/drafts/scripting-foundation/plan-2-light-entity.md §Sub-plan 2

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LightComponent {
    pub light_type: LightKind,           // Point | Spot | Directional
    pub intensity: f32,
    pub color: [f32; 3],
    pub falloff_model: FalloffKind,      // Linear | InverseDistance | InverseSquared
    pub falloff_range: f32,
    pub cone_angle_inner: Option<f32>,   // radians
    pub cone_angle_outer: Option<f32>,   // radians
    pub cone_direction: Option<[f32; 3]>,
    pub cast_shadows: bool,
    pub animation: Option<LightAnimation>,
}
```

`LightKind`, `FalloffKind`, and `LightAnimation` are re-exports / newtype wrappers around the compiler enums. The scripting module owns its own copies so the FFI boundary has clean serde derives — do not leak `postretro-level-compiler` types into `postretro` runtime code (it's a compiler crate).

**Shared curve-eval helper location.** `LightAnimation` introduces linear-interp keyframe evaluation over `brightness: Vec<f32>` and `color: Vec<[f32; 3]>` (and `direction: Vec<[f32; 3]>` from Sub-plan 1). Plan 3's `EmitterComponent` reuses the identical math for `size_over_lifetime` and `opacity_over_lifetime`. The eval helper lives in a shared `scripting/util/` location (exact path TBD pending a module-layout audit — a sibling research agent is naming the concrete file) — do **not** bury it inside the light module or the light bridge. Plan 3 imports the same helper; if it ends up living inside `scripting/components/light.rs` we will have to move it later.

### FFI adapter layer (research-driven)

The `[f32; 3]` fields and `Vec<[f32; 3]>` inside `LightAnimation.color` / `.direction` will not deserialize cleanly from script-side `number[]` / `number[][]` values because serde serializes fixed-size arrays as tuples. Mitigation:

- Define `Vec3Lit(pub [f32; 3])` in `postretro/src/scripting/ffi_types.rs`.
- Implement `Deserialize` manually: expect a sequence of exactly three numbers, error with "expected [r, g, b] array of three numbers" otherwise.
- Use `Vec3Lit` in the **script-facing** descriptor types only. Convert to `[f32; 3]` when constructing the engine-internal `LightComponent`.
- Apply the same adapter to `cone_direction` and every `Vec3`-shaped script input.

Reference: serde fixed-array behavior per [serde#989](https://github.com/serde-rs/serde/issues/989).

### Registry integration

- Add a `Light` variant to `ComponentKind` (Plan 1 sub-plan 1). The enum is `#[non_exhaustive]` — adding a variant is not a breaking change per Plan 1's design.
- Add `ComponentValue::Light(LightComponent)`.
- `EntityRegistry::set_component(id, ComponentValue::Light(...))` is the write path. No special-case code — the registry is generic.

### Acceptance criteria

- [ ] `LightComponent` round-trips through `serde_json` cleanly with nested `animation` carrying all three channels.
- [ ] A Rust unit test `set_component`s a `LightComponent` on a spawned entity and reads it back unchanged.
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.
- [ ] The `Vec3Lit` adapter errors with a clear message when given a 2-element or 4-element array, or a non-number element.

### Implementation tasks

1. Create `postretro/src/scripting/components/mod.rs` and `light.rs`. Header per `development_guide.md` §5.2.
2. Define `LightKind`, `FalloffKind`, `LightAnimation`, `LightComponent`, `Vec3Lit`.
3. Extend `ComponentKind` and `ComponentValue` in Plan 1's registry module.
4. Unit tests for serde round-trip, registry write/read, `Vec3Lit` validation.

---

## Sub-plan 3 — Light Bridge System

**Scope:** The seam between the scripting registry and the renderer's GPU light buffer.

### Description

Map-authored lights load through `postretro/src/prl.rs` into `LevelWorld.lights: Vec<MapLight>` and are packed by `pack_spec_lights` (defined in `postretro/src/lighting/spec_buffer.rs` around line 21) and `pack_lights_with_slots` (defined in `postretro/src/lighting/mod.rs` around line 123). Both are called from `postretro/src/render/mod.rs` (`pack_spec_lights` around line 805, `pack_lights_with_slots` around line 1664). Scripted lights must end up in the same buffer so the forward shader evaluates them through the same loop.

The bridge system, run once per frame between game logic and render:

1. Queries the entity registry for all entities with a `LightComponent`.
2. Diffs against a stored snapshot (by `EntityId`). New entries, mutated entries, and removed entries generate upload / clear ops.
3. Produces a `Vec<MapLight>`-compatible payload for the renderer — conceptually a merged `map_lights ++ scripted_lights` array, packed by the existing `pack_spec_lights` (`postretro/src/lighting/spec_buffer.rs`) and `pack_lights_with_slots` (`postretro/src/lighting/mod.rs`) paths.
4. Uploads via `queue.write_buffer` on the existing `lights_buffer`.

### Design notes

- **Buffer layout stays single.** Scripted lights append after map lights. This keeps the shader's light loop untouched — it sees one buffer, one count.
- **Capacity.** The 500-lights-per-level budget in `rendering_pipeline.md` §4 is the ceiling for map + scripted combined. The bridge fails noisily (warn log, skip upload of the overflow) if the combined count exceeds capacity. Growing the buffer is a follow-up.
- **Animated scripted lights.** Scripted lights with `Some(animation)` require an `AnimationDescriptor` slot and sample data. The existing animated path in `forward.wgsl` reads from `anim_descriptors` + `anim_samples` + `anim_sh_data`. `anim_sh_data` is baked SH — scripted animated lights do not have baked SH. **Resolution:** scripted animated lights drive the direct-lighting path's animated brightness/color/direction, but do not contribute to the SH-layer animation. Runtime animation on the direct path uses the descriptor's brightness/color/direction channels only; the SH-side animated evaluation remains compile-time-baked.

- **Anim descriptor buffer writability — decision: (c) new per-frame anim-override buffer.** Today `anim_descriptors_buffer` and its siblings are created in `ShVolumeResources::new` (`postretro/src/render/sh_volume.rs` line 131) with `BufferUsages::STORAGE | COPY_DST`, sized exactly to the baked `animated_light_count`, and written once from the PRL `ShVolumeSection`. The baked buffer is load-time only and stays that way: it is sized to match `per_light_sh` (baked SH layers), and growing / rewriting it per frame would couple scripted-light lifecycles to the SH bind group, confuse the baked `animated_light_count` uniform, and tangle two concerns in one binding.

  Instead, add a new per-frame writable descriptor + samples pair — call it group 3 bindings 14/15 — that carries descriptors for scripted animated lights only. Layout is identical to the baked descriptor struct (same 48-byte stride after the direction-channel change in Sub-plan 1). Creation site: sibling to the existing `anim_descriptors_buffer` setup in `ShVolumeResources::new`, sized to a fixed runtime cap (e.g. 128 scripted animated lights, rejected with a warning beyond that). Usages are `STORAGE | COPY_DST`. The light bridge (this sub-plan) calls `queue.write_buffer` on these each frame when the dirty flag is set; the call site lives in the bridge's upload step in `postretro/src/scripting/systems/light_bridge.rs`. The forward shader reads scripted-light descriptor indices from the scripted portion of `GpuLight`, not the baked index space, so the two buffers never alias.

  Why not (a) switch the existing buffer to per-frame writable: would force the bridge to re-pack baked descriptors every frame and risk drift from the PRL source of truth. Why not (b) defer scripted anim changes to level-load: breaks the `set_light_animation` runtime primitive, which is the whole point of Sub-plan 4.

- **Dirty tracking, not full rewrite.** For 500-light budgets the diff is cheap; a simple `HashMap<EntityId, LightSnapshot>` is enough. Do not over-engineer with a sparse-set structure.
- **f64 → f32 origin conversion happens at the bridge boundary.** The runtime `MapLight.origin` is `[f64; 3]` (`postretro/src/prl.rs` line 128) and the compiler-side `MapLight.origin` is `DVec3` (f64) in `postretro-level-compiler/src/map_data.rs`. Script-facing `LightComponent` (and its upload-time representation) carries `[f32; 3]`. The bridge casts with explicit `as f32` at the one spot where a script-provided origin or a map-sourced origin is packed for the GPU — not scattered through the read/write paths. Conversions are one-way at this boundary; never round-trip a scripted origin through f64.
- **Frame ordering.** Run the bridge between Game Logic and Render in the existing frame sequence. `update_dynamic_light_slots` (shadow-slot ranking) runs *after* the bridge so scripted shadow-casting lights participate in slot allocation.

### Acceptance criteria

- [ ] Spawning a light entity from a behavior script (via `spawn_entity` + `set_component` Light) produces a visible light in the next frame.
- [ ] Despawning the entity removes the light within one frame. No residual light visible.
- [ ] Mutating `intensity` on an existing scripted light updates the rendered output within one frame.
- [ ] Map-authored lights continue to render unchanged when no scripted lights exist.
- [ ] Exceeding the combined-light budget logs a single warning per frame (rate-limited) and drops the overflow.
- [ ] An animated scripted light evaluates its animation via the GPU path every frame without per-tick script involvement.
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Create `postretro/src/scripting/systems/light_bridge.rs`. Own the snapshot map and the diff.
2. Extend renderer ingestion: `Renderer::set_dynamic_lights(&[MapLight])` or equivalent. Today the renderer loads `level_lights` once from the PRL — add a per-frame replace path that merges map + scripted and rewrites the spec buffer.
3. Wire the bridge into the frame sequence in `postretro/src/main.rs` (or wherever the frame loop lives).
4. Integration test: spawn a scripted point light via a harness, render one frame, assert the lights_buffer contents contain its origin.
5. Benchmark: 500 mixed lights (say 400 map, 100 scripted) update in under 0.5 ms on the bridge side (excludes GPU upload). Add as a criterion bench if the infra exists; otherwise a `cfg(test)` timing check.

---

## Sub-plan 4 — `set_light_animation` Primitive

**Scope:** One new primitive registered through Plan 1's binding layer. Small scope, pure plumbing.

### Description

Signature: `set_light_animation(id: EntityId, animation: LightAnimation) -> Result<(), ScriptError>`.

Semantics: reads the entity's current `LightComponent`, overwrites its `animation` field with the new value, writes the component back. The light bridge (Sub-plan 3) uploads the change on the next frame.

Passing `null` / `nil` for `animation` clears the animation (static light). This is separate from not calling the primitive at all — scripts need a way to stop an animation they started.

### Error cases

- Entity does not exist → `ScriptError::EntityNotFound`.
- Entity exists but has no `LightComponent` → `ScriptError::ComponentNotFound`. Do not auto-add a default `LightComponent` — the script must `set_component` a light first. Auto-add would mask typo bugs.
- Animation shape invalid (e.g. `period <= 0`, empty channel arrays where `Some` was passed, non-unit direction vectors) → `ScriptError::InvalidArgument { reason: "..." }`. Validation happens in the primitive, not later in the bridge — fail at the call site.
- `direction` samples are not unit length → **normalize silently**. Document this in the primitive's doc string; scripts should not have to hand-normalize. (Contrast with validation errors, which are shape errors that indicate a bug.)

### Acceptance criteria

- [ ] `set_light_animation` is registered as `BehaviorOnly` and visible in both QuickJS and Luau behavior contexts.
- [ ] Calling from the definition context returns `ScriptError::WrongContext`.
- [ ] Passing a well-formed animation descriptor (all channels, nested arrays) works from both runtimes. Research note: both `rquickjs-serde` and `mlua::serde::from_value` handle nested `Vec<T>` cleanly; the only gotcha is the `Vec3Lit` adapter from Sub-plan 2.
- [ ] Passing `null` clears the animation.
- [ ] Invalid inputs (negative period, empty required channel) return `InvalidArgument` with a specific reason.
- [ ] A panicking inner call (impossible in current design, but simulate via a test feature flag) is caught by the FFI wrapper.
- [ ] Generated `.d.ts` and `.d.luau` (Plan 1 sub-plan 5) expose the primitive with accurate types.

### Implementation tasks

1. Implement the primitive function in `postretro/src/scripting/primitives/light.rs`.
2. Register it via the Plan 1 builder in `scripting::primitives::register_all`.
3. Integration test: from a QuickJS test script, spawn a light entity, call `set_light_animation`, assert the registry holds the new animation and the bridge uploads it.
4. Luau mirror of the same test.

---

## Sub-plan 5 — `delta` / `time` in `on_tick`

**Scope:** Extend `ScriptCallContext` with `delta: f32` and `time: f32`. Scripts never use wall-clock.

### Description

Every `on_tick` bridge call packages a `ScriptCallContext` that scripts receive as their handler argument. Add two fields:

- `delta: f32` — seconds since the last tick for this entity (or globally, if the bridge is global-tick).
- `time: f32` — accumulated engine time, reset to 0 at level load. Monotonic within a level.

Both values come from the engine's frame timer (the `self.app_start.elapsed()` path in `Renderer::update_per_frame_uniforms`). The scripting layer does not own a separate clock.

### Why both

- `delta` drives integration (position = position + velocity * delta). Required for determinism.
- `time` drives curves (`sin(time * rate)`). Scripts should not accumulate their own, because drift across reloads is subtle — prefer engine-provided.

### Script-side surface

TypeScript:

```typescript
// Proposed design
onTick((entity, ctx) => {
  const phase = ctx.time * 2.0
  // ...
})
```

`ctx` is `{ delta: number, time: number, transform: Transform }`. The `onTick` vocabulary function (referenced in the spec draft `light_animation.ts`) now takes `(entity, ctx)` instead of `(entity, delta)`. Update the spec draft's example accordingly — it currently uses `Date.now()` which is explicitly disallowed.

Luau:

```lua
onTick(function(entity, ctx)
  local phase = ctx.time * 2.0
end)
```

### Acceptance criteria

- [ ] `on_tick` handlers receive `{ delta, time, ... }`.
- [ ] `delta` is monotonic positive and matches the engine's frame delta within `1e-6`.
- [ ] `time` resets to 0 on level load.
- [ ] A TypeScript test handler computes `sin(time * rate)` and verifies the output matches a Rust-side reference within floating-point tolerance.
- [ ] A script that calls `Date.now()` — if the global is even exposed — should not; document in `sdk/types/postretro.d.ts` that wall-clock is absent. In QuickJS, `Date` is built-in; add a deprecation-style JSDoc marker on a shim or remove it entirely from the behavior context by deleting the global at context init. Recommended: remove `Date` from the behavior context globals after primitive install.

### Implementation tasks

1. Extend `ScriptCallContext` struct in Plan 1's runtime module.
2. Plumb `delta` / `time` through the `on_tick` bridge call site.
3. For QuickJS: before `sandbox(true)`-equivalent freezing (rquickjs doesn't have sandbox, but evaluation happens inside `ctx.with`), set `Date` to `undefined` on the behavior context's globals. Test that a behavior script calling `Date.now()` throws.
4. For Luau: `os.time`, `os.clock` are already in the Plan 1 deny-list. Verify.
5. Update Plan 1's spec draft `light_animation.ts` example to use `ctx.time` and move that update under the reference-vocabulary sub-plan below.

---

## Sub-plan 6 — TypeScript Reference Vocabulary

**Scope:** Two hand-authored TypeScript files shipped under `assets/scripts/sdk/` as readable source. No primitives called from vocabulary beyond the ones registered by Plan 1 and this plan.

### Files

#### `assets/scripts/sdk/light.ts` — `light()` component constructor

- Signature: `light(props: LightProps) -> ComponentDescriptor`.
- Validates props synchronously:
  - `type` is one of `"point" | "spot" | "directional"`.
  - `intensity >= 0`.
  - `color` is `[number, number, number]` with each channel in `[0, 1]`.
  - Spot lights require `cone_angle_inner`, `cone_angle_outer`, `cone_direction`.
  - `animation.period > 0` when present.
  - Channel arrays non-empty when `Some`.
- Returns a `ComponentDescriptor` tagged `{ kind: "light", value: LightComponent }`.
- JSDoc every field. JSDoc is the source of tooltips in the modder's editor.

Example target shape from the design sessions:

```typescript
// Proposed design — ships as assets/scripts/sdk/light.ts
defineEntity({
  components: [
    light({
      type: "point",
      intensity: 80,
      color: [1.0, 0.5, 0.2],
      animation: {
        period: 0.8,
        brightness: [0.7, 1.0, 0.6, 0.9],
        color: [[1.0, 0.5, 0.2], [1.0, 0.3, 0.1]],
      }
    }),
  ]
})
```

#### `assets/scripts/sdk/light_animation.ts` — pre-built animation descriptors

Pure functions returning `LightAnimation` values. No primitive calls, no side effects. Modders can use them or read them as examples.

- `flicker(minBrightness: number, maxBrightness: number, rate: number) -> LightAnimation` — generates an 8-sample irregular brightness curve approximating torch flicker at the given rate.
- `pulse(minBrightness: number, maxBrightness: number, period: number) -> LightAnimation` — 16-sample sine.
- `colorShift(colors: [number, number, number][], period: number) -> LightAnimation` — cycles through `colors` evenly.
- `sweep(directions: [number, number, number][], period: number) -> LightAnimation` — animated `direction` channel (uses Sub-plan 1's new channel).

Each function documents the math and the shape of its output so a modder reading the source learns the format.

### Acceptance criteria

- [ ] `light()` called with the "campfire" shape above returns a `ComponentDescriptor` that the engine accepts via `defineEntity` + `spawn_entity`, producing a rendered light.
- [ ] `light()` called with invalid props throws with a clear message naming the invalid field.
- [ ] `flicker(0.7, 1.0, 5)` returns a `LightAnimation` whose `brightness` channel is 8 samples, all within `[0.7, 1.0]`.
- [ ] `sweep([[1,0,0], [0,0,1]], 2.0)` returns a `LightAnimation` whose `direction` has length 2 and `period == 2.0`.
- [ ] A test map's definition file authored in TypeScript composes `light()` inside `defineEntity` and produces the same runtime behavior as an equivalent map-authored `light_spot` entity.
- [ ] Hot reload (Plan 1 sub-plan 7) works on these files in dev mode.

### Implementation tasks

1. Author `light.ts` with full prop validation and JSDoc.
2. Author `light_animation.ts` with the four helpers plus JSDoc describing the math.
3. Place under `assets/scripts/sdk/` — the SDK-source directory. Update the SDK README to list these as the first vocabulary modules.
4. Write an integration test that compiles the TS via Plan 1's detected compiler (`tsc` or `scripts-build`), loads the produced JS into a definition context, evaluates it, and asserts on the collected archetype.
5. Update the spec-draft example in `postretro-scripting-spec-draft.md` `light_animation.ts` section to use `ctx.time` instead of `Date.now()`. This reconciles the draft with the Sub-plan 5 decision.

---

## Sub-plan 7 — Luau Reference Vocabulary (follow-up within this plan)

**Scope:** `light.luau` and `light_animation.luau` ports of Sub-plan 6. No new engine work.

### Description

Once the TypeScript API shape is validated end-to-end, translate to Luau. The translation is mechanical — Luau types, `:` method syntax, table literals. The purpose is to validate that the primitive + type-definition generation story truly is language-agnostic: authoring a Luau port should require zero Rust changes.

### Acceptance criteria

- [ ] `light.luau` and `light_animation.luau` exist under `assets/scripts/sdk/`.
- [ ] A Luau definition file using `light({ ... })` produces the same runtime light as the TypeScript equivalent — verified by a round-trip test that loads both and compares the resulting archetype.
- [ ] Translation required zero changes to Rust engine code. If any Rust change was required, treat as a bug in Plan 1's cross-language contract and record a follow-up.
- [ ] `luau-lsp` configured against `sdk/types/postretro.d.luau` autocompletes `light` and its props.

### Implementation tasks

1. Translate `light.ts` to `light.luau`. Match validation rules exactly.
2. Translate `light_animation.ts` to `light_animation.luau`.
3. Add a Luau integration test mirroring Sub-plan 6's TypeScript test.

---

## When this plan ships

Durable knowledge migrates to `context/lib/`:

- **`context/lib/scripting.md`** (the file Plan 1 creates) gains a "Light entity" section covering: how scripted lights merge with map lights in the renderer's buffer, the `LightComponent` contract, the `set_light_animation` keyframe-path rationale, and the `delta`/`time` injection rule. Include the "no wall-clock" invariant as a top-level bullet.
- **`context/lib/rendering_pipeline.md`** §4 is updated to reflect: (a) `AnimationDescriptor` now carries a direction channel, (b) the direct-lighting buffer is a merge of compile-time and script-time lights, (c) the 500-light budget covers the combined count.
- **`context/lib/entity_model.md`** gets a short "Scripted lights" paragraph referencing the bridge system and pointing to `scripting.md`.

The plan document moves `drafts/` → `ready/` → `in-progress/` → `done/` and then stays frozen.

---

## Non-goals (consolidated)

- `emitter` component / particle spawning — Plan 3.
- Physics primitives — deferred.
- NPC / perception / patrol — later.
- Archetype `extends` — later.
- Production bytecode caching — later.
- On-disk wire-format additions beyond the `AnimationDescriptor` direction-channel bump — that bump is the only format change and is gated by owner sign-off per `development_guide.md` §1.6.
- Per-tick `setLightIntensity` / `setLightColor` helpers. The keyframe path is the only supported mechanism.
- Wall-clock time access from scripts.
