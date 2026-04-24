# Plan 2 — Light Entity Scripting

> **Status:** ready — first feature plan building on Plan 1's shipped runtime foundation.
> **Parent:** Scripting Foundation initiative. See `./postretro-scripting-spec-draft.md` for the full system vision.
> **Depends on:** `../../done/scripting-foundation/plan-1-runtime-foundation.md` — Plan 1 sub-plans 1–5 have shipped. Sub-plans 6 (context pool) and 7 (hot reload) are not hard prerequisites but are assumed available for dev iteration.
> **Related (read for architectural context):** `context/lib/development_guide.md` §3 · `context/lib/context_style_guide.md` · `context/plans/done/lighting-foundation/index.md` · `context/plans/done/lighting-foundation/7-animated-sh.md` · `context/plans/ready/animated-light-weight-maps/`.
> **Lifecycle:** Per `development_guide.md` §1.5, this plan is its own spec while it stays in `ready/`. Durable knowledge migrates to `context/lib/scripting.md` and `context/lib/rendering_pipeline.md` when it ships.

---

## Goal

Bring lights into the scripting surface as first-class entities. After this plan ships:

- The scripting primitive surface for Plan 2 is three calls: `registerHandler(event, fn)` for event subscription, `world.query(filter)` for typed entity handle queries, and `set_light_animation` exposed as `light.setAnimation(anim)` on the `LightEntity` handle.
- Behavior scripts register event handlers with `registerHandler(event, fn)`. Plan 2 delivers the `levelLoad` and `tick` events.
- Behavior scripts query live entities with `world.query({ component, tag })` and get back typed entity handles (`LightEntity[]` when filtered by `component: "light"`). Entity handles carry methods — `light.setAnimation(anim)`, not a free-standing `setLightAnimation(id, anim)`.
- Map-authored lights carry a `phase` offset and an optional direction animation channel. Runtime scripting mutates the existing map-authored lights through `LightEntity` handles queried by tag; the lights continue to flow through the existing `AlphaLightsSection` → `MapLight` path and evaluate every frame through the existing keyframe path in `forward.wgsl`.
- A new `direction` animation channel exists on `LightAnimation` / `AnimationDescriptor`, enabling spotlight aim sweeps without per-tick scripting overhead.
- Map authors can attach per-light keyframe animations directly in TrenchBroom via one FGD key per channel: `brightness_curve`, `color_curve`, `direction_curve`. Each key accepts a list of timestamped keyframes that resolve to the same internal curve representation at compile time.
- `tick` handlers receive a `ctx: { delta: f32, time: f32 }` argument. No wall-clock / `Date.now()` access from scripts.

**What this plan does not deliver:**

- `emitter` component / particle spawning — Plan 3.
- Physics primitives (`apply_impulse`, `is_grounded`, `raycast`, …) — deferred until Rapier lands.
- NPC scripting, perception, patrol — later plan.
- Archetype `extends` — later plan.
- Kinematic brush geometry (moving doors) — future infrastructure.
- Production bytecode caching — later plan.
- Linear or step interpolation modes. Catmull-Rom is the only mode in Plan 2. Fast transitions are authored by placing keyframes close together (dense samples = near-linear segment). Linear is a future plan if authored curves demand it.
- Luau reference vocabulary is the final sub-plan **within** Plan 2 — TS ships first to validate the API shape, Luau follows as a translation.

---

## Settled decisions (do not relitigate)

- **Three-primitive behavior surface for Plan 2.** `registerHandler`, `world.query`, and `set_light_animation` (reached as `light.setAnimation` on the handle). Nothing else is in scope for this plan.
- **External APIs stay close to internal shapes, but prioritize modder ergonomics.** When internal naming, hardware constraints, or usability concerns diverge, the external API simplifies rather than exposes the constraint. The mapping should be traceable, not required to be identical. A `LightComponent` carries its animation in a nested `animation` field, matching `MapLight.animation: Option<LightAnimation>`; the `LightEntity` handle wraps `origin: [f32; 3]` in a `transform.position` accessor because array-indexed fields are hostile to script authors.
- **`phase` is a first-class field on `LightAnimation`.** Required for per-light offset effects (rolling waves, phase-staggered flickers). The GPU already evaluates `phase` in every compose pass (`fract(time / period + phase)` in `animated_lightmap_compose.wgsl` line 139 and equivalents), where `time` and `period` are in seconds. The bridge converts `periodMs / 1000.0` when writing the GPU descriptor. Exposing it to scripts is table stakes — per-light animation offsets are a core authoring need, and without `phase` a wave effect across N lights would need N separate animation curves.
- **Keyframe path, not per-tick setters.** Scripts write a `LightAnimation` once (at level load, or on a trigger); the GPU evaluates it every frame through the shared Catmull-Rom helpers in `postretro/src/shaders/curve_eval.wgsl`. Per-tick `setLightIntensity(entity, value)` helpers do not exist.
- **`registerHandler(event, fn)` over per-entity callbacks.** Behavior scripts register top-level handlers for engine events. Lighting setup happens in `registerHandler("levelLoad", ...)`. The `tick` event exists and carries `ctx: { delta, time }`, but lighting use cases do not need it — GPU keyframe evaluation handles animation without per-frame script involvement.
- **`world.query` returns typed entity handles.** `world.query({ component: "light", tag: "..." })` returns `LightEntity[]`. The handle type is inferred from the component filter. Methods live on the handle (`light.setAnimation(anim)`), not as free-standing primitive imports.
- **No animated color on baked-path lights.** `LightAnimation.color` must be `None` for any light where `bake_only == false && is_dynamic == false`. The animated SH indirect path was removed (commit `5b9a25f`); a color-animated baked light would have a dynamic direct contribution shading against a static indirect contribution at the compile-time color, producing visible mismatch. Scripted lights (`is_dynamic == true`) are never baked and keep color animation.
- **Catmull-Rom only.** All three animation channels (brightness, color, direction) sample via the shared Catmull-Rom helpers in `curve_eval.wgsl`. Linear and step modes are out of scope. "Fast transitions" are authored by densifying keyframes, not by switching evaluator mode.
- **`delta` injected into `tick` handlers.** Scripts read `delta: f32` and `time: f32` from `ScriptCallContext`. Wall-clock access is not exposed. Required for determinism.
- **Pre-release API policy.** Rename freely, no compat shims, fix all consumers in the same change (`MEMORY.md` / `feedback_api_stability.md`).
- **Lights are always interruptible.** Last `setAnimation` call wins. No coordination mechanism exists and none is needed — scripts that want sequenced behavior track their own state. Async completion callbacks (Promise/await) are a future direction; they require a Promise API that has been deliberately avoided so far.
- **`playCount` is CPU-managed.** `LightAnimation` carries `play_count: Option<u32>` (script: `playCount?: number`; omit = loop forever). The light bridge tracks `animation_start_time` in `LightSnapshot` and detects completion by comparing elapsed time to `n × periodMs`. On completion it samples the final keyframe value, writes it as static `intensity`/`color` on the component, and clears the animation. The GPU always sees a looping descriptor — `AnimationDescriptor` gains no `loop_count` or `start_time` fields and stays 48 bytes.
- **`setIntensity`/`setColor` are vocabulary helpers, not primitives.** They construct a `LightAnimation` with `playCount: 1` and a one-cycle transition curve, then call `setAnimation`. Defined in `world.ts` as handle methods alongside `setAnimation`. Three Rust primitives hold.

---

## Prerequisites from Plan 1

| Plan 1 sub-plan | What this plan needs from it |
|---|---|
| §Sub-plan 1 — Entity / component registry | `ComponentKind` enum to extend with `Light`; `EntityRegistry::set_component` as the write path for `LightComponent`. |
| §Sub-plan 2 — Primitive binding layer | `register(...)` builder and `ScriptPrimitive` record used to register new primitives. `catch_unwind` wrapping at the FFI boundary. |
| §Sub-plan 3 — QuickJS runtime + contexts | Behavior context owning the new primitives. |
| §Sub-plan 4 — Luau runtime + contexts | Symmetric behavior-context registration. |
| §Sub-plan 5 — Type definition generator | Emits `LightComponent` / `LightAnimation` / `LightEntity` types and the `world.query` / `set_light_animation` primitive signatures into `postretro.d.ts` and `postretro.d.luau` automatically once registered. |

Sub-plans 6 (context pool) and 7 (hot reload) are not gating, but dev iteration on the reference vocabulary uses hot reload.

---

## Crates touched

| Crate | Role |
|---|---|
| `postretro-level-format` | On-disk format change: format-crate `AnimationDescriptor` gains direction-channel support. No version bump — pre-release API policy; fix all consumers in the same change. |
| `postretro-level-compiler` | Extend `LightAnimation` in `map_data.rs` with `direction: Option<Vec<[f32; 3]>>`. Parse the three FGD `*_curve` keys on `light_*` point entities and resample to the internal uniform-sample representation. Enforce the no-animated-color-on-baked-lights rule at pack time. |
| `postretro` | New `LightComponent`, `LightEntity` handle, and scripting primitives (`registerHandler`, `world.query`, `set_light_animation`). New light bridge system. `AnimationDescriptor` extended in all three shaders for the direction channel. `ScriptCallContext` introduced. |
| `assets/scripts/sdk/` | `light_animation.ts`, `world.ts` (reference vocabulary, ships as readable source). Luau ports are the final sub-plan. |

### Serialization notes carried forward from research

- **Fixed-size Rust arrays serialize as tuples by default.** `Vec<[f32; 3]>` deserialized from a script-side `number[][]` fails without help. Use `Vec3Lit(pub [f32; 3])` at the FFI boundary with an explicit `Deserialize` impl accepting a 3-number sequence. Error message: "expected [x, y, z] array of three numbers". Convert to `[f32; 3]` inside the bridge before writing the component.
- **mlua with `serde` feature** round-trips nested tables and `Vec<T>` cleanly via `LuaSerdeExt::from_value`. The `Vec3Lit` adapter works unchanged.

---

## Sub-plan dependency graph

```
1. Direction animation channel (engine: data + shaders)
    │
    ├─→ 2. FGD *_curve keyframe authoring (compiler)
    │
    └─→ 3. LightComponent in the ECS registry (adds `phase`)
            │
            ├─→ 4. Light bridge system (map lights + scripted lights → GPU buffer)
            │
            └─→ 5. registerHandler + ScriptCallContext
                    │
                    └─→ 6. world.query + set_light_animation primitives
                            │
                            └─→ 7. TypeScript reference vocabulary (light_animation, world)
                                    │
                                    └─→ 8. Luau reference vocabulary (closes out the plan)
```

Sub-plan 1 lands first — every downstream piece assumes the direction channel exists. Sub-plans 2, 3, and 5 are mutually independent once 1 ships. Sub-plans 4 and 6 both depend on 3. Sub-plan 7 can start once the API shape is pinned; Sub-plan 8 follows 7 so Luau is a translation, not a re-design.

---

## Sub-plan 1 — Direction Animation Channel

**Scope:** Pure engine change. Add a third animation channel to `LightAnimation` and its GPU descriptor. No scripting APIs touched.

### Description

The existing `LightAnimation` supports `brightness` (scalar) and `color` (`[f32; 3]`) channels. Static `cone_direction` lives on `AlphaLightRecord`. For spotlight sweeps, add a `direction: Option<Vec<[f32; 3]>>` channel evaluated identically to `color` — sampled via the shared Catmull-Rom helpers in `postretro/src/shaders/curve_eval.wgsl` (landed by `animated-curve-eval` in commit `9408930`) so brightness / color / direction share one vocabulary.

Direction samples are normalized at write time. The shader does not re-normalize per frame; a Catmull-Rom spline between unit vectors drifts slightly off the sphere but is close enough for low-rate authored curves. If a curve produces visible length drift, densify the samples in the packer — not in the shader.

**Unit-length enforcement.** The single authoritative gate is the `set_light_animation` primitive (Sub-plan 6), which silently normalizes non-zero-length vectors and rejects zero-length with `InvalidArgument`. Past that seam, the packer and GPU writer treat direction samples as unit. Debug builds add a `debug_assert!((v.dot(v) - 1.0).abs() < 1.0e-4)` over each direction sample in the GPU writer as a belt-and-suspenders check.

**Color animation restriction.** `LightAnimation.color` must be `None` for any light where `bake_only == false && is_dynamic == false`. The pack stage errors with a clear message naming the light. Scripted lights (`is_dynamic == true`) are exempt.

### Files changed

- `postretro-level-compiler/src/map_data.rs` — add `pub direction: Option<Vec<[f32; 3]>>` to `LightAnimation`. Fix construction sites.
- `postretro-level-compiler/src/format/quake_map.rs` — where the Quake style table expands to `LightAnimation`, set `direction: None`.
- `postretro-level-compiler/src/sh_bake.rs` — extend the packing path to include the direction channel. Add the pack-time check that errors on `animation.color.is_some() && !bake_only && !is_dynamic`.
- `postretro-level-format/src/sh_volume.rs` — extend format-crate `AnimationDescriptor` with direction-channel fields. `ANIMATION_DESCRIPTOR_SIZE` (GPU/render side) stays 48. Extend the round-trip unit test.
- `postretro/src/shaders/forward.wgsl` — extend `struct AnimationDescriptor` to match. Spot-light evaluation samples the direction channel via `sample_color_catmull_rom` when `direction_count > 0`; falls back to static `cone_direction` when zero.
- `postretro/src/shaders/billboard.wgsl`, `postretro/src/shaders/fog_volume.wgsl`, and `postretro/src/shaders/animated_lightmap_compose.wgsl` — mirror the struct change for binding compatibility. None evaluate direction.
- `postretro/src/render/sh_volume.rs` — update layout doc comment on `ANIMATION_DESCRIPTOR_SIZE`. Re-verify the WGSL parity assert.

### Acceptance criteria

- [ ] `LightAnimation` carries `direction` end-to-end: compiler authoring → format serialization → shader binding.
- [ ] Direction-channel evaluation via `curve_eval.wgsl` returns expected values for synthetic samples (unit test).
- [ ] Test map with `light_spot` whose animation carries a two-sample direction curve visibly sweeps between the directions over `periodMs`.
- [ ] All three shaders compile. `ANIMATION_DESCRIPTOR_SIZE` stays 48.
- [ ] Compiler errors at pack time when a non-`bake_only`, non-`is_dynamic` light has `animation.color.is_some()`. Error names the light.
- [ ] `cargo test -p postretro-level-format`, `cargo test -p postretro-level-compiler`, `cargo clippy -p postretro -- -D warnings` clean.

---

## Sub-plan 2 — FGD `*_curve` Keyframe Authoring

**Scope:** Compiler-side. Parse one FGD key per animation channel on `light_*` entities and resample to the internal uniform-sample representation.

### Description

Map authors working in TrenchBroom need a way to attach per-light keyframe animations without writing a script. One FGD key per channel carries timestamped keyframes as a compact array-of-arrays. TrenchBroom's value input is a small text field — verbose key-value pair formats are hostile to that UI. Each key's value is a space-separated list of bracketed entries:

- **`brightness_curve`** — `[t_ms, value]` entries. Scalar value slot.
- **`color_curve`** — `[t_ms, r, g, b]` entries. 3-element value slot.
- **`direction_curve`** — `[t_ms, x, y, z]` entries. 3-element value slot.

The arity per key is fixed and known to the compiler — `brightness_curve` is always scalar, `color_curve` and `direction_curve` are always 3-vector. The parser for each key enforces its expected inner-array length and rejects entries with the wrong count. `t_ms` is the timestamp in milliseconds, absolute from zero. The format is self-describing: no separate encoding field.

`period_ms` is a separate scalar FGD key in milliseconds.

**Resampling at compile time.** The internal `LightAnimation` stores uniform samples along `period_ms`. Authored keyframes are resampled at a fixed rate — target: `round(period_ms / 1000.0 * 32)` samples (32 per second of period), capped at 256 total — using Catmull-Rom over the authored timestamps. This keeps the wire format and GPU evaluator unchanged.

The 32 Hz rate and 256-sample cap are new limits introduced by Plan 2 — not derived from any existing GPU buffer constraint. Rationale: a conservative ceiling to bound descriptor buffer growth as authored maps scale; revisit if authored content approaches the cap.

**Why resample rather than change the wire format.** The GPU path already supports uniform Catmull-Rom sampling and is well-tested. Adding timestamp-aware evaluation would require a second shader code path and a larger descriptor. Resampling costs a one-time compile-time loss of fidelity (negligible at 32 Hz for authored curves) and buys zero runtime cost. If authored content demands timestamp fidelity, a follow-up plan can extend the wire format.

**Interaction with the baked-light rule.** `color_curve` is rejected at pack time on non-`bake_only`, non-`is_dynamic` lights (same restriction as Sub-plan 1). Error message points to the FGD key that introduced the color animation.

**Interaction with the legacy `style` key.** If both `style` and `brightness_curve` are present on the same light, `brightness_curve` wins and `style` is ignored. The compiler emits a warning naming the light and the ignored key. A follow-up plan removes `style` entirely; this precedence rule is the bridge.

**`_curve_phase` vs. `_phase`.** The existing `_phase` key is the style cycle offset — semantically tied to `style` and removed alongside it in the follow-up plan. The curve system introduces `_curve_phase` as a separate FGD key. Same semantics (`[0.0, 1.0)` float, defaults to 0.0), clean separation. Scripted lights set phase directly on `LightAnimation`; FGD-authored lights use `_curve_phase`.

### Files changed

- `postretro-level-compiler/src/format/quake_map.rs` — parse `brightness_curve`, `color_curve`, `direction_curve` on `light`, `light_spot`, `light_sun`. Reject malformed entries (wrong arity, non-monotonic timestamps) with a message naming the light and the offending key.
- `postretro-level-compiler/src/map_data.rs` — add the resampler: `resample_keyframes(keyframes: &[(f32, T)], period_ms: f32, samples_per_second: u32) -> Vec<T>`.
- `assets/postretro.fgd` — add `brightness_curve`, `color_curve`, `direction_curve`, and `_curve_phase` keys to the `light`, `light_spot`, `light_sun` definitions. Document the `[t_ms, …]` keyframe shape in the FGD descriptions. `_curve_phase` is a float key in `[0.0, 1.0)`; document its separation from the legacy `_phase` key (which is tied to `style` and removed in the follow-up plan).

### Acceptance criteria

- [ ] A test map authoring `brightness_curve "[0, 0.1] [500, 1.0] [1000, 0.3]"` and `period_ms "1000"` produces a visible pulse matching the curve.
- [ ] A `direction_curve` with two keyframes drives a visible sweep over `periodMs`.
- [ ] Malformed keyframes (wrong arity, out-of-order timestamps) error at compile time with a message naming the light and key.
- [ ] `color_curve` on a baked light (`!bake_only && !is_dynamic`) errors at pack time — same error as Sub-plan 1, now with the FGD key named in the message.
- [ ] Resampled curves at 32 Hz match the authored keyframes within 1% over the full period (unit test).
- [ ] `cargo test -p postretro-level-compiler` passes.

---

## Sub-plan 3 — `LightComponent` in the ECS Registry

**Scope:** Rust only. Add `Light` to `ComponentKind`. Define the script-facing `LightComponent` struct.

### Description

`LightComponent` is the script's view of a map-authored light. Its fields mirror `MapLight` minus compiler-only concerns (`bake_only`, `is_dynamic`). Map lights are populated into the scripting entity registry at level load by the light bridge; the scripting surface in Plan 2 is mutation-only — no script can spawn or destroy a light.

### Struct shape

```rust
// Proposed design — lives in postretro/src/scripting/components/light.rs.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LightAnimation {
    pub period_ms: f32,
    #[serde(default)]
    pub phase: Option<f32>,               // None = 0.0; [0.0, 1.0) when set; wraps via `fract`
    #[serde(default)]
    pub play_count: Option<u32>,          // None = loop forever; Some(n) = play n times then hold at last keyframe
    #[serde(default)]
    pub start_active: Option<bool>,       // None = true; false = define without starting (activated later by an event)
    pub brightness: Option<Vec<f32>>,
    pub color: Option<Vec<[f32; 3]>>,     // None for non-dynamic lights; set_light_animation rejects color on non-dynamic lights
    pub direction: Option<Vec<[f32; 3]>>,
}
```

`LightKind`, `FalloffKind`, and `LightAnimation` mirror the compiler types' shape without a crate dependency — the scripting module owns parallel definitions so the FFI boundary stays clean.

**Shared helpers.** CPU-side validation or sampling helpers needed by the light bridge and future emitter bridge live under `postretro/src/scripting/util/`. Do not bury them inside `scripting/components/light.rs`.

### FFI adapter layer

Use `Vec3Lit(pub [f32; 3])` in `postretro/src/scripting/ffi_types.rs` for the **script-facing** descriptor types only. Convert to `[f32; 3]` when constructing the engine-internal `LightComponent`. Apply to `color`, `cone_direction`, and `Vec<Vec3Lit>` for `color`/`direction` animation channels.

### Registry integration

- Add `Light` to `ComponentKind` (Plan 1 sub-plan 1). The enum is `pub(crate)`; add the new variant to the `VARIANTS` array backing `ComponentKind::COUNT`.
- Add `ComponentValue::Light(LightComponent)`.
- `EntityRegistry::set_component(id, ComponentValue::Light(...))` is the write path — no special-case code.

### Acceptance criteria

- [ ] `LightComponent` round-trips through `serde_json` cleanly with nested `animation` carrying all three channels plus `phase`.
- [ ] Rust unit test `set_component`s a `LightComponent` on a spawned entity and reads it back unchanged.
- [ ] `Vec3Lit` errors with a clear message on 2- or 4-element arrays or non-number elements.
- [ ] `cargo test -p postretro`, `cargo clippy -p postretro -- -D warnings` clean.

---

## Sub-plan 4 — Light Bridge System

**Scope:** The seam between the scripting registry and the renderer's GPU light buffer.

### Description

Map-authored lights load through `postretro/src/prl.rs` into `LevelWorld.lights: Vec<MapLight>`, packed by `pack_spec_lights` and `pack_lights_with_slots`. At level load the bridge also populates each `MapLight` into the scripting entity registry as a `LightComponent` entity, giving scripts a query surface. The bridge runs once per frame between game logic and render:

1. Query the entity registry for all entities with a `LightComponent`.
2. Diff against a stored `LightSnapshot` per `EntityId`. Mutated entries generate upload ops.
3. Re-pack the light buffer from the (possibly mutated) component state. The existing pack paths consume it unchanged.
4. Upload via `queue.write_buffer` on the existing `lights_buffer`.

### Design notes

- **Single buffer layout.** Map lights only — Plan 2 has no spawned lights. The GPU buffer is sized to the authored light count at level load and does not grow.
- **Descriptor-slot pre-reservation.** Every map light gets an `AnimationDescriptor` slot in `anim_descriptors_buffer` at level load, whether or not authoring supplied an animation. Lights with a baked (`*_curve` FGD) animation get a populated descriptor. Lights that are static at compile time get a **sentinel descriptor** — `brightness_count == 0 && color_count == 0 && direction_count == 0`, all sample indices zeroed — which `forward.wgsl` already treats as "no animation, use the static `intensity` / `color` / `cone_direction` fields." This costs 48 bytes per authored light and gives every light a pre-reserved slot; `setAnimation` always overwrites in place and never allocates.

  | Light compile-time state | Bridge action on `setAnimation` |
  |---|---|
  | Had `*_curve` FGD keys (baked animation) | Overwrite the existing slot in `anim_descriptors_buffer` via `queue.write_buffer` at the slot's byte offset. |
  | Static at compile time (no `*_curve` keys) | Overwrite the pre-reserved sentinel slot in place via `queue.write_buffer`. No new buffer, no slot allocation. |
  | `setAnimation(null)` on either | Write the sentinel descriptor back into the slot; light reverts to static. |

  This preserves the "slot count and layout fixed at load time" invariant while allowing slot *contents* to be updated by the bridge. No 128-slot scripted buffer reappears.
- **`LightSnapshot`.** New type; defined here because it does not exist before Plan 2 ships.

  ```rust
  // Proposed design — lives in postretro/src/scripting/systems/light_bridge.rs.
  pub struct LightSnapshot {
      pub component: LightComponent,         // state at last upload; used for dirty detection
      pub animation_start_time: Option<f32>, // Some(t) when play_count-bounded animation is active
  }
  ```

  `animation_start_time` is `Some(current_frame_time)` the moment a `play_count`-bounded animation becomes active (i.e., `start_active` is `true` or absent), and `None` otherwise. An animation written with `start_active: false` does not set `animation_start_time` — `play_count` counting begins only when the animation is actually active. Any `setAnimation` call (including a repeated call with the same animation value) resets it to the current frame time — "last call wins" always restarts the count from zero.
- **Dirty tracking.** `HashMap<EntityId, LightSnapshot>` — one entry per map light that has been written at least once by a script.
- **`play_count` completion tracking.** When a `LightAnimation` has `play_count: Some(n)`, the bridge records the frame timestamp (seconds, from `app_start.elapsed().as_secs_f32()`) as `animation_start_time: f32` in `LightSnapshot` when the animation first becomes active. On each subsequent frame, if `current_time − start_time ≥ n × (period_ms / 1000.0)`, the bridge samples the last brightness/color keyframe value and settles it through the registry:

  1. Call `EntityRegistry::set_component` with a `LightComponent` whose `intensity` / `color` are set to the sampled final values and whose `animation` is `None`. The registry is the source of truth for scripts — a subsequent `world.query` or live re-read (see "Snapshot freshness" in Sub-plan 6) observes the settled static state, not an animation that has already finished.
  2. Update the in-memory `LightSnapshot.component` to match and clear `animation_start_time` to `None`.
  3. Re-pack that light's descriptor from the updated component state into the GPU buffer on the same frame — the GPU sees a static light, not a looping descriptor with a dead `play_count`.

  The GPU descriptor itself never learns about `play_count`: `AnimationDescriptor` is not extended with `loop_count` or `start_time` fields, and the struct stays 48 bytes. Completion handling lives entirely on the CPU side, driven by the registry.
- **f64 → f32 origin conversion at the bridge boundary.** `MapLight.origin` is `[f64; 3]` on the runtime side; script-facing `LightComponent` is `[f32; 3]`. Cast with explicit `as f32` at the one pack point; never round-trip through f64.
- **Frame ordering.** Bridge runs between Game Logic and Render. `update_dynamic_light_slots` runs after so scripted shadow-casters participate in slot allocation.

### Acceptance criteria

- [ ] A behavior script mutating a queried `LightEntity`'s animation produces a visible change in the next frame.
- [ ] Clearing an animation (`light.setAnimation(null)`) removes animation effects within one frame.
- [ ] Mutating `intensity` updates rendered output within one frame.
- [ ] Map-authored lights render unchanged when no scripts touch them.
- [ ] Animated lights evaluate via the GPU path every frame without per-tick script involvement.
- [ ] `cargo test -p postretro`, `cargo clippy -p postretro -- -D warnings` clean.

---

## Sub-plan 5 — `registerHandler` + `ScriptCallContext`

**Scope:** The top-level event dispatch model. Introduces `registerHandler(event, fn)` and the `ScriptCallContext` with `delta` and `time`.

### Description

Behavior scripts register top-level handlers for engine events. Plan 2 delivers two events:

- **`"levelLoad"`** — fires once per level, after the world is populated but before the first frame renders. Handler signature: `() => void` in TS; `function()` with no parameters in Luau. The handler receives no argument — no `ScriptCallContext`. This is where light setup happens — query lights, call `setAnimation`.
- **`"tick"`** — fires once per frame, after game logic and before render. Handler signature: `(ctx: ScriptCallContext) => void`. `ctx.delta` is seconds since the last tick; `ctx.time` is accumulated seconds since level load. `tick` is the only event that carries `ScriptCallContext`. Lighting does not use `tick` — the GPU evaluates animations. `tick` exists for future gameplay logic (NPC steering, triggers, timers).

The handler registry is per-level. `registerHandler` calls in the behavior context accumulate into a table; level unload clears it. Multiple handlers for the same event are allowed and fire in **registration order**, defined as:

1. The behavior-script loader discovers all behavior files for the level, collects their paths relative to `assets/scripts/`, and sorts them lexicographically by UTF-8 byte order (case-sensitive). Filesystem enumeration order is never observed — the sort happens at the loader seam before any script executes.
2. Scripts are executed in that sorted order. `registerHandler` calls within a single file append in source order.
3. Therefore: all handlers from `a.ts` run before any from `b.ts`; two handlers within `a.ts` run in the order their `registerHandler` calls appear. Authors who need a specific cross-file order use a filename prefix (e.g., `00_setup.ts`, `10_waves.ts`).

This rule is independent of the scripting engine — rquickjs and mlua execute whatever the loader hands them in whatever order — so identical ordering holds for TypeScript and Luau behavior files, including mixed-language levels.

`registerHandler` is a `BehaviorOnly` primitive. A call from the definition context returns `ScriptError::WrongContext` — the same pattern as `set_light_animation`. The definition context is for data declarations only; event subscription belongs to behavior.

### Why both events now

`levelLoad` is the natural home for lighting setup — the wave example needs it. `tick` is the general-purpose per-frame hook. Both share the registry plumbing and the same `ScriptCallContext` wiring. Shipping only `levelLoad` would leave `tick` as a half-defined placeholder; shipping both keeps Sub-plan 5's scope coherent.

### `ScriptCallContext`

```rust
// Proposed design — lives in postretro/src/scripting/call_context.rs.

pub struct ScriptCallContext {
    pub delta: f32,  // seconds since previous tick
    pub time: f32,   // seconds since level load; monotonic
}
```

Both values come from the engine frame timer (the `self.app_start.elapsed()` path in `Renderer::update_per_frame_uniforms`). The scripting layer does not own a separate clock.

### Wall-clock denial

- **QuickJS:** after primitive install, delete `Date` from the behavior context's globals. A script calling `Date.now()` throws `ReferenceError`.
- **Luau:** `os.time`, `os.clock` are already in Plan 1's deny-list. Verify.

### Handler exception policy

If a handler throws (QuickJS) or raises (Luau) or the underlying Rust call panics, the engine logs the error with the script file path and the handler's event name, swallows the exception, and continues. Remaining handlers registered for the same event still run. A failed handler does not prevent frame completion. One misbehaving script cannot stall the frame loop or block other handlers.

### Acceptance criteria

- [ ] `registerHandler("levelLoad", fn)` runs `fn` exactly once per level load, after world population, before first render.
- [ ] `levelLoad` handler receives no arguments.
- [ ] `registerHandler("tick", fn)` runs `fn` every frame with `ctx.delta` matching the engine frame delta within `1e-6`.
- [ ] `ctx.time` resets to 0 on level load, monotonic within a level.
- [ ] Multiple handlers for the same event fire in registration order.
- [ ] A behavior script calling `Date.now()` throws `ReferenceError`.
- [ ] A TypeScript tick handler computing `sin(ctx.time * rate)` matches a Rust-side reference within floating-point tolerance.
- [ ] Calling `registerHandler` from the definition context returns `ScriptError::WrongContext`.
- [ ] A handler that throws is logged (with script file and event name), swallowed, and does not prevent other handlers for the same event from running or the frame from completing.
- [ ] Handlers registered across two behavior files (`a.ts` and `b.ts`) for the same event fire with all of `a.ts`'s handlers before any of `b.ts`'s, regardless of on-disk creation order. Rename-to-swap test: swapping the filenames swaps the firing order.
- [ ] Generated `.d.ts` and `.d.luau` expose `registerHandler` and `ScriptCallContext` with accurate types.

---

## Sub-plan 6 — `world.query` + `set_light_animation` Primitives

**Scope:** Two primitives: the entity query and the animation setter. Both are behavior-context-only.

### `world.query(filter)`

Signature: `world.query({ component: ComponentName, tag?: string }) -> Array<EntityHandle>`.

Returns typed entity handles. When no entities match the filter, the result is an empty array — never `null`, never an error. Scripts iterate the result without null-checking. The handle type depends on the `component` filter — `component: "light"` returns `LightEntity[]`.

Handles are **snapshots at query time** — they hold an `EntityId` and a cached view of the component. Calling `light.setAnimation(anim)` writes through to the registry. If the entity is despawned between query and call, the setter returns `ScriptError::EntityNotFound` and the script handles it.

### `LightEntity` handle

Defined in `world.ts` (TS) and `world.luau` (Luau). Members:

| Member | Type | Notes |
|--------|------|-------|
| `transform.position` | `{ x: f32, y: f32, z: f32 }` | Read-only. Wraps `MapLight.origin: [f64; 3]`, cast to f32 at query time. |
| `setAnimation(anim)` | `(LightAnimation \| null) -> void` | Wraps `set_light_animation`. Pass `null`/`nil` to clear. Last call wins — lights are always interruptible. |
| `setIntensity(target, transitionMs?, easing?)` | `(f32, f32?, EasingCurve?) -> void` | Vocabulary helper. Re-reads the live `LightComponent.intensity` from the registry at call time (not the handle's query-time snapshot) and constructs a one-cycle `LightAnimation` transitioning from that live value to `target` over `transitionMs` ms (default 0 = instant), using `easing` (default: `"easeInOut"` when `transitionMs > 0`). Sets `playCount: 1`. Calls `setAnimation` internally. Defined in `world.ts`, not a Rust primitive. |
| `setColor(target, transitionMs?, easing?)` | `([f32, f32, f32], f32?, EasingCurve?) -> void` | Same pattern as `setIntensity` for color; re-reads live `LightComponent.color` at call time. Rejected on non-dynamic lights — see error table below. |

**Snapshot freshness.** Handle fields populated at query time (`transform.position`, `_isDynamic`) are frozen snapshots. Vocabulary helpers that need the current animatable state — `setIntensity` and `setColor` — re-read the live `LightComponent` from the entity registry at call time to derive the "from" value of the constructed transition curve. The cached snapshot is never the source for animation starting values. If the entity has been despawned between query and call, the re-read surfaces `ScriptError::EntityNotFound` and the helper rethrows to the script. This keeps handles usable across `tick` boundaries and across bridge write-backs without introducing "snap to stale" glitches.

`EasingCurve` is a string union: `"linear" | "easeIn" | "easeOut" | "easeInOut"`. Each maps to a 4-keyframe Catmull-Rom brightness or color curve over `transitionMs`. When `transitionMs` is 0 the curve collapses to a single-sample step; the `easing` parameter is ignored.

The `light_type`, `falloff_model`, `cast_shadows`, and cone fields are read-only (not exposed as setters). Position mutation is out of scope for Plan 2. Script-spawned lights and `destroy()` are non-goals for Plan 2 — all handles returned by `world.query` are map-authored lights.

### `set_light_animation(id, animation)`

The low-level primitive. `LightEntity.setAnimation` wraps it in TS / Luau. Semantics: read the entity's current `LightComponent`, overwrite `animation`, write back. The bridge uploads on the next frame. The bridge maps `start_active ?? true` to `AnimationDescriptor.is_active` — a script can define an animation without starting it by passing `startActive: false`, leaving the light on its static `intensity`/`color` until a subsequent event activates it.

### Error cases

- Entity does not exist → `ScriptError::EntityNotFound`.
- Entity has no `LightComponent` → `ScriptError::ComponentNotFound`. Do not auto-add — a typo should fail, not silently succeed.
- `periodMs <= 0` → `InvalidArgument`.
- `Some(vec![])` on any channel → `InvalidArgument`. An empty required channel is a shape error.
- `color` animation on a non-dynamic light (i.e., `MapLight.is_dynamic == false`) → `InvalidArgument`. Color animation on a baked light would produce a direct/indirect mismatch — the SH indirect was baked at compile-time color. The `world.ts` wrapper validates this before calling the primitive and throws a descriptive JS `Error`; the Rust primitive rejects it as a belt-and-suspenders check. The TS check reads `_isDynamic: boolean` from the handle snapshot populated at query time.
- `phase` outside `[0.0, 1.0)` → accepted and normalized via `fract` before writing. Matches the GPU evaluator's treatment.
- Direction sample with zero length → `InvalidArgument`.
- Direction sample non-unit but normalizable → silently normalized. This primitive is the authoritative enforcement site for the unit-length invariant; Sub-plan 1's `debug_assert` is a belt-and-suspenders check.
- Passing `null` / `nil` for `animation` clears it — the light becomes static. The only way to stop a script-started animation.

### Acceptance criteria

- [ ] `world.query({ component: "light" })` returns all `LightComponent`-bearing entities as `LightEntity[]`.
- [ ] Tag filter narrows the result: `world.query({ component: "light", tag: "hallway_wave" })` returns only tagged lights.
- [ ] Query with no matching entities returns an empty array.
- [ ] `light.setAnimation(anim)` updates the registry and the GPU buffer by the next frame.
- [ ] Both primitives registered as `BehaviorOnly` and visible from QuickJS and Luau.
- [ ] Calling from the definition context returns `ScriptError::WrongContext`.
- [ ] `null` / `nil` animation clears it.
- [ ] Non-unit direction sample is silently normalized (unit test asserts stored vector is unit).
- [ ] Zero-length direction sample errors with `InvalidArgument`.
- [ ] Generated `.d.ts` and `.d.luau` expose both primitives with accurate types.

---

## Sub-plan 7 — TypeScript Reference Vocabulary

**Scope:** Two TypeScript files under `assets/scripts/sdk/`, shipped as readable source.

### `assets/scripts/sdk/light_animation.ts`

Pure functions returning `LightAnimation` values. No primitive calls, no side effects. Modders use them or read them as examples.

- `flicker(minBrightness, maxBrightness, rate) -> LightAnimation` — 8-sample irregular brightness curve.
- `pulse(minBrightness, maxBrightness, periodMs) -> LightAnimation` — 16-sample sine.
- `colorShift(colors, periodMs) -> LightAnimation` — cycles colors. `colors` is `[number, number, number][]`.
- `sweep(directions, periodMs) -> LightAnimation` — animated direction channel.
- `timeline<T extends number[]>(keyframes: [number, ...T][]) -> [number, ...T][]` — keyframes are already `[absolute_ms, ...value]`. Pass-through with shape validation. Invalid input (empty entries, wrong arity, non-number slots, non-monotonic timestamps) throws a JS `Error` in QuickJS and raises a Lua error in Luau with a descriptive message naming the offending entry. Silently dropping bad entries is not acceptable — authoring mistakes must surface.
- `sequence<T extends number[]>(keyframes: [number, ...T][]) -> [number, ...T][]` — keyframes are `[delta_ms, ...value]`. Accumulates deltas into absolute timestamps:

  ```
  output[0] = input[0]
  for i in 1..len:
      output[i].t     = output[i-1].t + input[i].t
      output[i].value = input[i].value
  ```

  Same validation rules and error behavior as `timeline`.

`timeline` and `sequence` exist so modders can author in whichever mental model — absolute timestamps or deltas between keyframes — fits the curve. Both return the canonical `[absolute_ms, ...value]` format the engine consumes. Arity per channel is fixed: `brightness_curve` keyframes are always `[t, v]`; `color_curve` and `direction_curve` keyframes are always `[t, r, g, b]` / `[t, x, y, z]`. TypeScript expresses the genericity as a rest-element type parameter; Luau validates arity at runtime.

The other four helpers (`flicker`, `pulse`, `colorShift`, `sweep`) omit `phase` (defaults to 0.0). Callers set `phase` at the call site when staggering.

### `assets/scripts/sdk/world.ts`

Re-exports the `world` primitive as a typed object and declares the `LightEntity` handle type. One thin file — the primitive does the work, `world.ts` exists so the import ergonomics match the other vocabulary files.

`LightEntity` carries a private `_isDynamic: boolean` populated at query time from the component's underlying `MapLight.is_dynamic` flag. The `setAnimation` and `setColor` methods check it before calling the primitive and throw a descriptive `Error` if `color` animation is attempted on a non-dynamic light. This gives script authors a clear message without a round-trip to Rust.

### Wave example (integration test target)

Map authors tag the hallway lights `"hallway_wave"` in TrenchBroom. The behavior script queries them at level load and staggers phase across the row.

```typescript
// Proposed design — rolling pulse down a hallway, 10s loop.
registerHandler("levelLoad", () => {
  const lights = world
    .query({ component: "light", tag: "hallway_wave" })
    .sort((a, b) => a.transform.position.x - b.transform.position.x);

  const pulse: LightAnimation = {
    periodMs: 10000,
    brightness: [
      0.1, 0.1, 0.1, 0.1, 0.1,
      0.3, 0.8, 1.0, 0.8, 0.3,
      0.1, 0.1, 0.1, 0.1, 0.1,
      0.1, 0.1, 0.1, 0.1, 0.1,
    ],
  };

  lights.forEach((light, i) => {
    light.setAnimation({ ...pulse, phase: i / lights.length });
  });
});
```

### Acceptance criteria

- [ ] `flicker(0.7, 1.0, 5)` returns 8 brightness samples in `[0.7, 1.0]`.
- [ ] `sweep([[1,0,0], [0,0,1]], 2000)` returns a `LightAnimation` with 2 direction samples and `periodMs == 2000`.
- [ ] `timeline([[0, 0.1], [500, 1.0]])` returns the same list (validated shape).
- [ ] `sequence([[0, 0.1], [500, 1.0], [500, 0.3]])` returns `[[0, 0.1], [500, 1.0], [1000, 0.3]]`.
- [ ] Both helpers reject wrong-arity value tuples with a clear message.
- [ ] `timeline([[]])` or otherwise malformed input throws a JS `Error` (QuickJS) or raises a Lua error (Luau) with a descriptive message.
- [ ] The wave example above runs against a map tagged `hallway_wave` and visibly produces a rolling pulse.
- [ ] Hot reload works on these files in dev mode when enabled (best-effort, not a ship gate).

---

## Sub-plan 8 — Luau Reference Vocabulary

**Scope:** `light_animation.luau` and `world.luau` ports of Sub-plan 7. No new engine work.

### Luau idioms that shape the port

- **Method call syntax uses `:`.** `light:setAnimation(anim)`, not `light.setAnimation(anim)`. The `.` form passes the receiver as an explicit first argument.
- **Arrays are 1-indexed.** The wave phase formula becomes `(i - 1) / #lights`, not `i / lights.length`. Off-by-one is the single most common porting mistake; call it out in comments on both the TS and Luau files.
- **No spread operator.** Table concatenation is explicit: build a new table field by field.
- **`table.sort` is in-place.** Returns nothing; sort then read. Cannot chain.

### Wave example (Luau)

```lua
registerHandler("levelLoad", function()
  local lights = world:query({ component = "light", tag = "hallway_wave" })
  table.sort(lights, function(a, b)
    return a.transform.position.x < b.transform.position.x
  end)

  local pulse = {
    periodMs = 10000,
    brightness = {
      0.1, 0.1, 0.1, 0.1, 0.1,
      0.3, 0.8, 1.0, 0.8, 0.3,
      0.1, 0.1, 0.1, 0.1, 0.1,
      0.1, 0.1, 0.1, 0.1, 0.1,
    },
  }

  for i, light in ipairs(lights) do
    light:setAnimation({
      periodMs = pulse.periodMs,
      brightness = pulse.brightness,
      phase = (i - 1) / #lights,
    })
  end
end)
```

### Acceptance criteria

- [ ] `light_animation.luau` and `world.luau` exist under `assets/scripts/sdk/`.
- [ ] A Luau behavior file driving the wave effect produces the same runtime output as the TypeScript equivalent — round-trip test loads both against the same tagged map and compares resulting animation state.
- [ ] The Luau wave example renders an identical rolling pulse to the TypeScript version.
- [ ] `timeline` and `sequence` helpers in Luau produce identical outputs to their TS counterparts over a shared fixture set.
- [ ] The commit landing Sub-plan 8 touches zero Rust files. If a Rust change is forced, open a draft plan under `context/plans/drafts/scripting-language-parity-gap/` describing the contract gap before merging.
- [ ] `luau-lsp` configured against `sdk/types/postretro.d.luau` autocompletes `world`, `registerHandler`, and `light_animation` helpers.

---

## When this plan ships

Durable knowledge migrates to `context/lib/`:

- **`context/lib/scripting.md`** gains a "Light entity" section covering:
  - How scripted mutations to map lights flow through the light bridge into the renderer's buffer.
  - The `LightComponent` contract and the `phase` field semantics.
  - `registerHandler` as the top-level event dispatch model, with `levelLoad` and `tick` as the shipped events.
  - `world.query` returning typed entity handles; method-on-handle as the mutation surface.
  - The `set_light_animation` keyframe-path rationale.
  - The "no wall-clock" invariant.
- **`context/lib/rendering_pipeline.md`** §4 updates for:
  - `AnimationDescriptor` now carries a direction channel.
  - Direct-lighting buffer merges compile-time and script-time lights.
- **`context/lib/build_pipeline.md`** gets a brief note under the light-entity FGD section covering the `*_curve` authoring convention, with the authoritative detail staying in the FGD file.
- **`context/lib/entity_model.md`** gets a cross-reference only, not a "Scripted lights" section. One snippet lands verbatim:
  - §8 addition (new bullet): "Scripting entity registry. A separate script-facing entity/component registry holds map lights (and, in future plans, emitters and other scripted objects) as mutable components. It is not ECS and is not the typed collection model described in this document. The light bridge populates map lights into it at level load and syncs script mutations back to the GPU each frame — see [scripting.md](./scripting.md)."

The plan document moves `ready/` → `in-progress/` → `done/` and then stays frozen.

---

## Non-goals (consolidated)

- Runtime light spawning. Scripts can only mutate map-authored lights queried via `world.query`. A `light()` spawn primitive and `destroy()` on `LightEntity` are future work — deferred until dynamic lights need a scripted lifecycle.
- Modder-defined component or entity registration — a future plan.
- `emitter` component / particle spawning — Plan 3.
- Physics primitives — deferred.
- NPC / perception / patrol — later.
- Archetype `extends` — later.
- Production bytecode caching — later.
- Linear or step interpolation modes. Catmull-Rom only; densify keyframes to author fast transitions.
- Timestamp-aware GPU evaluator for FGD keyframes. Compile-time resampling to uniform samples is the Plan 2 approach.
- On-disk wire-format additions beyond the `AnimationDescriptor` direction-channel bump, which reuses bytes reserved by `animated-light-weight-maps`.
- Per-tick `setLightIntensity` / `setLightColor` helpers outside of the convenience-wrapper handle methods on `LightEntity`. Keyframe animation is the only path for time-varying behavior.
- Wall-clock time access from scripts.
- Readback of the current animation state from a `LightEntity`. Scripts own the state they write; lights are always interruptible (last `setAnimation` wins). If current state is needed, re-query via `world.query`.
- Animation completion callbacks (async/await or Promise API). Scripts can sequence behavior via the `tick` handler and a state variable. A Promise surface is a future plan gated on deciding whether the scripting runtime needs one at all.
- FGD → `LightComponent` routing for map-authored lights. Map lights stay on the `AlphaLightsSection` path; the FGD `*_curve` keys extend `MapLight.animation`, not the scripting registry.
