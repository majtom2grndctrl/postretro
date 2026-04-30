# Plan 3 — Emitter Entity

> **Status:** ready.
> **Parent:** Scripting Foundation initiative. See `./postretro-scripting-spec-draft.md`.
> **Depends on:** `./plan-1-runtime-foundation.md`, `./plan-2-light-entity.md`, and the data-script-setup work in `context/plans/done/data-script-setup/index.md` (reaction registry, `_tags` on map entities).
> **Related:** `context/lib/development_guide.md` · `context/lib/context_style_guide.md` · `context/lib/scripting.md` · the billboard smoke work salvaged in `838d966` and merged in `93bfa80` (`postretro/src/fx/smoke.rs`, `postretro/src/render/smoke.rs`, `postretro/src/shaders/billboard.wgsl`).
> **Lifecycle:** Per `development_guide.md` §1.5, this plan is its own spec while it stays a draft. Durable knowledge migrates to `context/lib/scripting.md`, `context/lib/rendering_pipeline.md`, and `context/lib/entity_model.md` when it ships.

---

## Goal

Bring emitter-driven particles into the scripting surface as first-class components. After this plan ships:

- `BillboardEmitter` is a built-in engine entity type. Map entities with `classname "billboard_emitter"` are handled natively by the level loader — no script registration. The SDK exports `BillboardEmitter` as a TypeScript type for authoring safety; it has no runtime value.
- The SDK ships an `emitter()` component constructor for use inside user-authored entity presets. Authors compose siblings on one entity by listing `emitter()` and `light()` together.
- Each live particle is a full ECS entity (from Plan 1's registry) carrying `Transform`, `ParticleState`, and `SpriteVisual`. No script behavior on particles — the simulation loop is pure Rust.
- A Rust emitter bridge spawns particles each tick using Plan 1's `spawn_entity` / `despawn_entity`. A Rust particle simulation integrates velocity, applies buoyancy and drag, advances age, and despawns at end-of-life.
- Two named reaction primitives ship: `setEmitterRate` modulates emission (`rate = 0` means inactive — no separate active flag) and `setSpinRate` sets per-emitter spin rate (with optional `SpinAnimation` tween).
- An SDK module ships at `sdk/lib/entities/emitters.{ts,luau}` co-locating the `BillboardEmitter` type, the `emitter()` component constructor, and the `smokeEmitter` / `sparkEmitter` / `dustEmitter` presets — one import path for both the type and the builder functions.
- Map entities resolve to component configurations via the engine's built-in classname routing. KVPs declared on the type populate component fields. Script reactions override at runtime.
- The `--demo-smoke` CLI flag and the legacy `SmokeEmitter` path are removed once the generic FGD-classname routing is live.

---

## Settled decisions (do not relitigate)

- **Entity types are derived from the map file.** The map enumerates entity types by `classname`. The data script does not register entity types — the manifest is reactions-only (`{ reactions: NamedReactionDescriptor[] }`).
- **`BillboardEmitter` is a built-in engine entity type.** The level loader handles `classname "billboard_emitter"` natively. The name reserves `ParticleEmitter` for future mesh-particle work — billboard is one rendering primitive among several. Naming intent: rendering primitive in the type name.
- **`BillboardEmitter` SDK export is a TypeScript type, not a value.** Authors `import { type BillboardEmitter } from 'postretro/entities/emitters'`. Zero runtime effect — purely for IDE support and structural type-checking on script-authored presets.
- **Named entity instances are typed plain objects.** `const exhaustPort: BillboardEmitter = { rate: 50, buoyancy: 0.0 }` is the script-side authoring pattern for a named preset or per-instance override. No registration call. Plain objects flow into spawn calls or sequence steps.
- **Emitter and light compose as sibling components.** A `Campfire` preset names an `emitter()` and a `light()` on one entity. Neither component owns the other.
- **`setEmitterRate` and `setSpinRate` are the reaction primitives.** `rate = 0` is the inactive state for emission. No `setEmitterActive`. `setSpinRate` accepts an optional `SpinAnimation` tween. Both are registered as named reaction primitives in the Rust reaction registry built by data-script-setup task 4 — *not* behavior-context FFI primitives.
- **Tag-based targeting only.** Entities carry `_tags` (a `Vec<String>`, established by data-script-setup). A reaction's `tag` matches any entity whose tag list contains that string.
- **FGD KVPs are load-time defaults; reactions override at runtime.** The built-in type declares which KVPs map to which component fields (KVP map). The level loader resolves an FGD `classname` against the engine's built-in registry, applies KVPs, and spawns the entity with the configured components.
- **Scripts configure, Rust simulates.** Per-particle `on_tick` would be prohibitively expensive. The Rust particle simulation runs every frame. Scripts never see individual particles.
- **Particles are lightweight ECS entities.** Each carries an `EntityId` from Plan 1, plus `Transform`, `ParticleState`, and `SpriteVisual`. Spawning and despawning use `EntityRegistry::spawn` / `despawn` directly from the bridge — never from script.
- **Curve channels reuse the `LightAnimation` keyframe pattern.** `size_over_lifetime` and `opacity_over_lifetime` are `Vec<f32>` linear-interpolated over normalized particle age. Same evaluation shape as `LightAnimation.brightness`. No parallel curve type.
- **`Vec3Lit` is reused unchanged.** Any `[f32; 3]`-shaped script input goes through `Vec3Lit` at the FFI boundary. `Vec<f32>` deserializes natively from JS/Luau arrays — no adapter.
- **`buoyancy` sign convention.** `-1` = normal gravity (falls like a regular object). `0` = floats (no vertical acceleration). `> 0` = rises. `< -1` = falls faster than gravity. Physics formula: `vertical_accel = WORLD_GRAVITY * -buoyancy` with `WORLD_GRAVITY = -9.81`. Documented once here; applies everywhere.
- **Pre-release API policy.** Rename freely, no compat shims, fix all consumers in the same pass (`MEMORY.md`, `feedback_api_stability.md`).

---

## Non-goals

- Per-particle `on_tick` script callbacks.
- Particle-against-geometry collision (deferred until Rapier/parry3d lands).
- Sub-emitters (particles that spawn particles).
- Mesh particles. Billboard sprites only — `BillboardEmitter` reserves `ParticleEmitter` for that future work.
- Particle sorting for non-additive alpha-blend transparency. Additive blending is the only supported blend mode here; the existing billboard pass is order-independent under additive.
- GPU-driven simulation. CPU simulation is the starting point.
- Per-particle tint in the GPU pipeline. `SpriteVisual.tint` exists CPU-side but is not packed into `SpriteInstance` until a future pass extends the layout.
- Runtime sprite-sheet loading. All sprite collections must be resolvable at level-load from the definition sweep.
- A `setEmitterActive` primitive (use `setEmitterRate(id, 0)`).
- On-disk PRL format additions for emitters. Emitter entities spawn at runtime via the FGD-classname route or via scripted `spawn_entity` calls.
- A user-facing `defineEntity` API for authoring custom entity types (e.g. `Campfire`). Composition presets in this plan are plain typed objects — `defineEntity` is a separate, deferred concern.
- Production bytecode caching (Plan 1 non-goal, restated).

---

## Research findings that shape this plan

### 1. Billboard renderer already has the upload path we need

The volumetric-smoke work landed a complete billboard pass with a `SpriteInstance` storage buffer:

- `postretro/src/fx/smoke.rs` defines `SPRITE_INSTANCE_SIZE = 32`. Layout: `vec4` of `(position.xyz, age)` + `vec4` of `(size, rotation, opacity, _pad)`.
- `SmokeEmitter::pack_instances(&self, out: &mut Vec<u8>)` is the canonical packer. Per `render/smoke.rs`, `SmokePass::record_draw(&'a self, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass<'a>, collection: &str, packed_bytes: &[u8])` takes a `queue`, an active render pass, and a `packed_bytes: &[u8]` slice per collection; it calls `queue.write_buffer` once per collection per frame.
- Blending is additive (`BlendFactor::One` / `BlendFactor::One`). Depth test enabled, depth write disabled.
- Per-collection sheet registration with frame-stitching (per `render/smoke.rs`): `SmokePass::register_collection(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, collection: &str, frames: &[SpriteFrame], spec_intensity: f32, lifetime: f32)`.

Particles need a CPU-side packer of the same layout, plus a render-stage system that asks the emitter subsystem for its packed bytes (by collection) and feeds `SmokePass::record_draw`. No new shader, no new pipeline.

### 2. Transparency ordering — honest limitation

Additive blending with depth-write disabled is order-independent (`A + B + C == C + B + A`), so draw order does not affect final color. Sorting is unnecessary for the current visual target (smoke, sparks, dust). Non-additive emitters (dark smoke, water splash) would require a sort pass and a separate pipeline; both are out of scope.

Because depth-write is off, particles do not occlude each other through depth — they only read depth against the opaque scene. Correct for additive. Future alpha-blend would need to revisit this.

### 3. `Vec<f32>` deserialization

Confirmed via rquickjs and mlua serde docs: a script-side `number[]` deserializes cleanly into a Rust `Vec<f32>`. The `Vec3Lit` workaround from Plan 2 applies only to fixed-size arrays (`[f32; 3]`).

| Field | Script type | Rust adapter |
|---|---|---|
| `initial_velocity` | `[number, number, number]` | `Vec3Lit` → `[f32; 3]` |
| `color` | `[number, number, number]` | `Vec3Lit` → `[f32; 3]` |
| `size_over_lifetime` | `number[]` | direct |
| `opacity_over_lifetime` | `number[]` | direct |
| `rate`, `lifetime`, `spread`, `buoyancy`, `drag` | `number` | direct |
| `burst` | `number?` | direct |
| `sprite` | `string` | direct |

### 4. Particle count budget

Concurrent particle count is bounded per-emitter by the existing `MAX_SPRITES = 512`. Even with dozens of emitters the total fits four digits, well within the `EntityId` budget established by Plan 1's registry. The CPU spawn/despawn rate target from Plan 1 sub-plan 1 (10 000 cycles in under 10 ms) covers this comfortably. No registry redesign needed.

GPU-side, the billboard `instance_buffer` is sized for `MAX_SPRITES * SPRITE_INSTANCE_SIZE` per collection. This plan keeps that cap: emission beyond `MAX_SPRITES` live particles per emitter is dropped with a rate-limited warning, mirroring the existing `SmokeEmitter` policy.

---

## Prerequisites from earlier plans

| Source | What this plan needs |
|---|---|
| Plan 1 sub-plan 1 — entity / component registry | `EntityRegistry::spawn` / `despawn` from Rust bridges. `ComponentKind` extended with `BillboardEmitter`, `ParticleState`, `SpriteVisual`. |
| Plan 1 sub-plan 5 — type definition generator | Component descriptors emitted into `postretro.d.ts` / `postretro.d.luau` automatically when the new component kinds register. |
| Plan 2 sub-plan 2 — `LightComponent` | Pattern for a component module under `postretro/src/scripting/components/`. `Vec3Lit` reused for `initial_velocity` and `color`. |
| Plan 2 sub-plan 5 — `delta` in bridge tick | Emitter bridge uses `delta` for emission accumulation; particle sim uses `delta` for physics. |
| Plan 2 sub-plan 6 — TypeScript reference vocabulary | Precedent for shipping component-constructor and preset modules under `sdk/lib/`. |
| Data-script-setup task 4 — reaction registry | Named reaction primitive registration. `setEmitterRate` plugs in here. |
| Billboard pass (commits `4d4e97f`, `838d966`) | Must be merged to `main` before sub-plan 4 starts; otherwise sub-plan 4 has nothing to render into. |

---

## Crates touched

| Crate | Role |
|---|---|
| `postretro-level-format` | No format changes. |
| `postretro-level-compiler` | No changes. |
| `postretro` | New `BillboardEmitterComponent`, `ParticleState`, `SpriteVisual`. New `setEmitterRate` reaction primitive. New emitter bridge and particle simulation systems. New particle-render collector feeding `SmokePass`. New built-in `BillboardEmitter` classname handling in the level loader. New generic FGD-classname routing for built-in entity types. Removal of `--demo-smoke` and `SmokeEmitter`. |
| `sdk/lib/` | New `entities/emitters.{ts,luau}` co-locating the `BillboardEmitter` TypeScript type, the `emitter()` component constructor, and the `smokeEmitter` / `sparkEmitter` / `dustEmitter` presets. |

No new workspace dependencies.

---

## Sub-plan dependency graph

```
1. Components: BillboardEmitterComponent + ParticleState + SpriteVisual
    │
    ├─→ 2. Particle simulation system (pure Rust)
    │
    ├─→ 3. Emitter bridge system
    │        │
    │        └─→ 5. setEmitterRate + setSpinRate reaction primitives
    │
    ├─→ 4. Render integration: particle entities → SpriteInstance upload
    │
    ├─→ 6. Built-in BillboardEmitter classname handling
    │        │ depends on 1 (component shape) + 4 (render path live)
    │        │
    │        └─→ 8. FGD-classname routing for built-in types
    │                 + removal of --demo-smoke / SmokeEmitter
    │
    └─→ 7. SDK module: sdk/lib/entities/emitters.{ts,luau}
             (BillboardEmitter type + emitter() constructor + presets)
             depends on 1 (component shape pinned) + 6 (built-in route live)
```

Sub-plan 1 gates everything. Sub-plans 2, 3, and 4 can proceed in parallel once 1 lands. Sub-plan 5 depends on 3 (the bridge reads `rate` at tick time and evaluates `spin_animation`). Sub-plan 6 depends on 1 and 4. Sub-plan 7 depends on 1 and 6. Sub-plan 8 generalizes the FGD route and depends on 6 — it lands together with the `--demo-smoke` / `SmokeEmitter` removal.

---

## Sub-plan 1 — `BillboardEmitterComponent`, `ParticleState`, `SpriteVisual` in the registry

**Scope:** Rust only. Add three component kinds. Wire them into `EntityRegistry::set_component`.

### Description

Three components land together because they reference each other by shape.

- **`BillboardEmitterComponent`** — entity-side configuration. Carried by the parent entity. Read by the emitter bridge each tick.
- **`ParticleState`** — per-particle simulation state. Carried by each live particle entity.
- **`SpriteVisual`** — per-particle visual descriptor. Distinct from `ParticleState` so future non-particle sprite entities (e.g. scripted enemy sprites) can reuse it.

### Struct shapes

```rust
// Proposed design — postretro/src/scripting/components/billboard_emitter.rs.
// See: context/plans/drafts/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 1

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BillboardEmitterComponent {
    /// Particles per second. `0.0` means inactive — no continuous emission.
    /// Fractional values accumulate across frames (see bridge §Sub-plan 3).
    pub rate: f32,

    /// If `Some(n)`, spawns `n` particles once on the next tick. Consumed at
    /// trigger time; the bridge resets this to `None` after the burst fires.
    /// Independent of `rate` — bursts fire even when `rate == 0`.
    pub burst: Option<u32>,

    /// Cone half-angle in radians for randomizing the initial direction
    /// around `initial_velocity`. `0.0` = perfectly directed.
    pub spread: f32,

    /// Per-particle lifetime in seconds.
    pub lifetime: f32,

    /// Initial direction + magnitude, world space.
    pub initial_velocity: [f32; 3],

    /// Buoyancy. `-1` = normal gravity (falls). `0` = floats. `> 0` = rises.
    /// `< -1` = falls faster than gravity. Physics applies
    /// `vertical_accel = WORLD_GRAVITY * -buoyancy` with `WORLD_GRAVITY = -9.81`.
    pub buoyancy: f32,

    /// Velocity damping per second.
    pub drag: f32,

    /// Keyframe curve, linear interpolation over normalized lifetime.
    pub size_over_lifetime: Vec<f32>,

    /// Keyframe curve, normalized-lifetime opacity multiplier.
    pub opacity_over_lifetime: Vec<f32>,

    /// Base tint, linear RGB.
    pub color: [f32; 3],

    /// Sprite sheet collection name, matching `SmokePass::register_collection`.
    pub sprite: String,

    /// Spin rate in radians per second. Read by the particle sim each tick via
    /// the parent back-reference. Tweened by spin_animation when set.
    pub spin_rate: f32,

    /// Optional tween. When Some, the bridge advances elapsed time and
    /// interpolates spin_rate from rate_curve over duration. Cleared when
    /// the tween completes.
    pub spin_animation: Option<SpinAnimation>,
}
```

```rust
// Proposed design — postretro/src/scripting/components/billboard_emitter.rs
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpinAnimation {
    /// Tween duration in seconds.
    pub duration: f32,
    /// Keyframe curve: spin_rate values linearly interpolated over normalized
    /// tween time (0.0 = start, 1.0 = end). Same evaluation shape as
    /// LightAnimation brightness. Must be nonempty.
    pub rate_curve: Vec<f32>,
}
```

`SpriteVisual.rotation` is the per-particle rotation angle. `ParticleState` does **not** carry a `spin_rate` field — the particle sim reads the live `spin_rate` from the parent emitter each tick via `ParticleState.emitter`. Per-emitter transient state (emission accumulator, spin tween elapsed time) lives bridge-side; see sub-plan 3.

```rust
// Proposed design — postretro/src/scripting/components/particle.rs.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParticleState {
    pub velocity: [f32; 3],
    pub age: f32,
    pub lifetime: f32,
    /// Copied from emitter at spawn so particles survive emitter despawn.
    pub buoyancy: f32,
    /// Copied from emitter at spawn — same reason.
    pub drag: f32,
    /// Copied from emitter at spawn — curve channels for sim evaluation.
    pub size_curve: Vec<f32>,
    /// Copied from emitter at spawn — curve channels for sim evaluation.
    pub opacity_curve: Vec<f32>,
    /// Back-reference for diagnostics. Not authoritative for simulation.
    pub emitter: Option<EntityId>,
}
```

```rust
// Proposed design — postretro/src/scripting/components/sprite_visual.rs.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpriteVisual {
    pub sprite: String,
    pub size: f32,
    pub opacity: f32,
    pub rotation: f32,
    pub tint: [f32; 3],
}
```

The two curve fields and `tint` are also copied from the emitter into per-particle state at spawn (curves into a per-particle `Vec<f32>` clone, `tint` into `SpriteVisual.tint`). At particle rates, `Vec<f32>` clone cost is negligible and avoids an indirection / lifetime entanglement with the emitter entity.

**Where the curves live on the particle:** carry as `size_curve: Vec<f32>` and `opacity_curve: Vec<f32>` on `ParticleState`. Alternative shared-curve table is not worth the indirection at this scale.

### FFI adapter layer

- `initial_velocity: Vec3Lit` and `color: Vec3Lit` at script boundary; converted to `[f32; 3]` before storage.
- `size_over_lifetime` / `opacity_over_lifetime` are plain `Vec<f32>` — no adapter.
- `spin_rate` is a direct `f32`. `spin_animation.rate_curve` is a plain `Vec<f32>` (no adapter, same as `size_over_lifetime`).
- Validation at the FFI boundary (`ScriptError::InvalidArgument { reason }`):
  - `lifetime > 0`.
  - `rate >= 0`.
  - `spread >= 0`.
  - `drag >= 0` (negative would unbound velocities).
  - `size_over_lifetime` and `opacity_over_lifetime` nonempty when present.
  - `sprite` nonempty.
  - When `spin_animation` is `Some`: `duration > 0` and `rate_curve` nonempty.

### Registry integration

- Add `BillboardEmitter`, `ParticleState`, `SpriteVisual` variants to `ComponentKind` and `ComponentValue` (Plan 1).
- `EntityRegistry::set_component` is the write path. No special-case code.

### Acceptance criteria

- [ ] All three components round-trip through `serde_json` cleanly with nested curve arrays.
- [ ] `SpinAnimation` round-trips through `serde_json`.
- [ ] Rust unit test `set_component`s each on a spawned entity and reads it back unchanged.
- [ ] A particle with `spin_rate = π` rotates 180° over 1 second (analytic check).
- [ ] `Vec3Lit` rejects 2- or 4-element arrays, non-number elements, and non-array shapes with a clear message naming the field.
- [ ] FFI validation rejects each invalid case with `ScriptError::InvalidArgument` whose `reason` names the field.
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Create `postretro/src/scripting/components/billboard_emitter.rs`, `particle.rs`, `sprite_visual.rs`. Headers per `development_guide.md` §5.2.
2. Define structs. Reuse `Vec3Lit` from the Plan 2 FFI module.
3. Extend `ComponentKind` and `ComponentValue` in Plan 1's registry module.
4. Extend the `VARIANTS` array in the component registry for `BillboardEmitter`, `ParticleState`, and `SpriteVisual`.
5. Unit tests for serde round-trip, registry write/read, `Vec3Lit` validation, FFI validation paths.

---

## Sub-plan 2 — Particle simulation system

**Scope:** Pure Rust system. No scripting involved. Runs each tick over every entity with `ParticleState`.

### Description

Single function called once per game-logic frame. For each entity with `ParticleState`:

1. `age += delta`.
2. `Transform.position += velocity * delta`.
3. `velocity.y += WORLD_GRAVITY * -buoyancy * delta`. `WORLD_GRAVITY = -9.81` (negative is down by convention). With `buoyancy = -1`, `vertical_accel = -9.81` (normal gravity); with `buoyancy = 0`, `vertical_accel = 0` (floats); with `buoyancy > 0`, the particle rises. `buoyancy` is read off the particle (copied at spawn).
4. `velocity *= (1.0 - drag * delta).max(0.0)`. Clamp prevents negative multipliers under pathological values.
5. `t = (age / lifetime).clamp(0.0, 1.0)`.
6. Evaluate `size_curve` and `opacity_curve` at `t`, write into `SpriteVisual.size` / `.opacity`.
6b. If `ParticleState.emitter` is `Some(id)`, look up the emitter entity's `BillboardEmitterComponent`. If found, `SpriteVisual.rotation += emitter.spin_rate * delta`. If the emitter entity no longer exists (orphaned particle), use `spin_rate = 0.0` — particle completes its lifetime at its last rotation angle.
7. If `age >= lifetime`, queue for despawn. Two-pass: collect IDs, then despawn — never mutate registry mid-iteration.

Curve evaluation reuses Plan 2's `LightAnimation` linear interpolation: the evaluation is CPU-side linear interpolation; Plan 2's Catmull-Rom GPU helpers are not shared here.

- `s = t * (N - 1)`; `i = floor(s)`; `frac = s - i`.
- `value = curve[i] * (1 - frac) + curve[clamp(i + 1, N - 1)] * frac`.
- One-sample curve is constant. Empty curve defaults to `1.0` (unreachable from script; reserved for Rust-side defaulting).

### Frame ordering

Per `development_guide.md` §4.3, in the Game Logic stage:

1. Input
2. Game logic
   a. Script `on_tick` bridge (Plan 2)
   b. Emitter bridge (sub-plan 3) — spawns new particles
   c. Particle simulation (this sub-plan) — advances all particles, including just-spawned
   d. Light bridge (Plan 2)
3. Audio
4. Render — particle render integration (sub-plan 4)
5. Present

Ordering c-after-b means newly spawned particles tick one `delta` before render — matches existing `SmokeEmitter::tick` and avoids a one-frame "stuck at origin" glitch.

Implementers: reconcile with the current `context/lib/scripting.md` frame-order section before wiring into the loop; this sub-stage listing is the intended insertion point, not a replacement of the existing ordering.

### Acceptance criteria

- [ ] A particle with `velocity = (0, 5, 0)` and `buoyancy = -1.0` follows a parabolic trajectory under normal gravity (analytic comparison at three sample times).
- [ ] A particle with `drag = 1.0` decays velocity to near-zero within ~4 lifetimes.
- [ ] `size_over_lifetime = [0.5, 1.0, 0.5]` produces `0.5` at `t=0`, `1.0` at `t=0.5`, `0.5` at `t=1.0`.
- [ ] A particle with `age >= lifetime` is despawned in the same tick.
- [ ] A particle whose parent emitter has `spin_rate = 2π` completes one full rotation per second (verified at t=0.5s: rotation ≈ π ± tolerance).
- [ ] An orphaned particle (emitter despawned) retains its rotation angle without panicking.
- [ ] 500 active particles simulated for 1 frame complete in under 0.5 ms in release.
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Create `postretro/src/scripting/systems/particle_sim.rs`.
2. Implement curve-eval helper. Factor out shared with `LightAnimation` only when the duplication is concrete.
3. Implement the tick function with two-pass despawn collection.
4. Wire into the frame loop.
5. Unit tests for the acceptance criteria.

---

## Sub-plan 3 — Emitter bridge system

**Scope:** Rust. Reads `BillboardEmitterComponent` each tick, spawns particle entities via `EntityRegistry::spawn`.

### Description

Once per game-logic frame, iterate every entity with a `BillboardEmitterComponent`:

1. If `burst.is_some()`: spawn `burst.unwrap()` particles immediately, set `burst = None`. Bursts ignore `rate` and fire even when `rate == 0`. Burst spawns are clamped to available headroom (`min(burst, MAX_SPRITES - live_count)`). Dropped burst particles use the same rate-limited `log::warn!` path as continuous over-cap.
2. If `rate > 0`: `state.accumulator += delta * rate`. While `state.accumulator >= 1.0` and the per-emitter cap allows, spawn one particle and subtract `1.0`. If `rate == 0`, reset `state.accumulator` to `0.0` each tick. Reactivating with a non-zero rate begins fresh accumulation without a burst from prior fractional state.

### Bridge-side per-emitter state

Per-emitter transient simulation state lives on the bridge, not on the component. The bridge holds a `HashMap<EntityId, EmitterBridgeState>`. Pattern follows Plan 2's `LightSnapshot`. Insert an entry when an emitter entity is first seen; remove the entry when the emitter is despawned (or when the entity no longer has a `BillboardEmitterComponent`).

```rust
// Proposed design — postretro/src/scripting/systems/emitter_bridge.rs
struct EmitterBridgeState {
    /// Fractional emission accumulator. Advances by delta*rate each tick;
    /// decremented by 1.0 per spawned particle.
    accumulator: f32,
    /// Elapsed time for the current spin_animation tween, in seconds.
    /// Reset to 0.0 when a new tween starts or the tween completes.
    spin_elapsed: f32,
}
```

### Spin animation evaluation

After accumulator-based emission each tick:

1. If `spin_animation.is_some()`: read `spin_elapsed` from the bridge HashMap entry. Advance `spin_elapsed += delta`. Compute `t = (spin_elapsed / duration).clamp(0.0, 1.0)`. Evaluate `rate_curve` at `t` (same linear-interp as particle curves). Write the result into `BillboardEmitterComponent.spin_rate`. If `t >= 1.0`, write `spin_rate = rate_curve.last()`, clear `spin_animation = None`, and reset `spin_elapsed = 0.0`.
2. If `spin_animation.is_none()`: no-op. `spin_rate` holds its last set value.

For each spawn:

- Allocate via `EntityRegistry::spawn(Transform::default())`.
- Compute initial direction: rotate `initial_velocity` by a random vector within the `spread` cone. Deterministic RNG per emitter (LCG seeded from emitter position + step counter), matching the `SmokeEmitter::rand_u32` pattern.
- Write `Transform { position: emitter.position, ..Default::default() }`.
- Write `ParticleState { velocity: dir * |initial_velocity|, age: 0.0, lifetime, buoyancy, drag, size_curve, opacity_curve, emitter: Some(parent_id) }`.
- Write `SpriteVisual { sprite, size: 0.0, opacity: 0.0, rotation: 0.0, tint: emitter.color }`. Sim runs immediately after the bridge and writes the real values.

### Budget enforcement

Per-emitter concurrent cap matches `MAX_SPRITES = 512`. Track per-emitter live count via snapshot (cheap; one-frame divergence). Over-cap spawns are dropped with a rate-limited `log::warn!` (at most once per second per emitter). No separate global cap — total is the sum of per-emitter caps.

### Acceptance criteria

- [ ] An emitter with `rate = 10` spawns ~10 particles/sec (±1 over 2 s).
- [ ] An emitter with `burst = Some(20)` spawns exactly 20 in one tick and resets `burst` to `None`.
- [ ] An emitter with `rate = 0` and `burst = None` spawns zero particles.
- [ ] An emitter with `accumulator = 0.7` that is set to `rate = 0` has its accumulator cleared; setting `rate > 0` on the following tick does not produce an immediate extra particle.
- [ ] An emitter at `MAX_SPRITES - 5` live particles that fires `burst = Some(20)` spawns exactly 5 particles and logs a rate-limited warning; `burst` is reset to `None`.
- [ ] An emitter at per-emitter cap drops further spawns and logs a rate-limited warning.
- [ ] Spawn directions distribute within the `spread` cone (statistical test: 500 spawns, mean direction within tolerance of `initial_velocity.normalize()`).
- [ ] Spawned particles' `ParticleState.emitter` points at the parent.
- [ ] A `setSpinRate` reaction with a 2-second tween from `0.0` to `2π` produces `spin_rate ≈ π` at t=1.0 s (mid-tween).
- [ ] `EmitterBridgeState` entries are cleaned up when an emitter entity is despawned; no stale entries accumulate over time.
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Create `postretro/src/scripting/systems/emitter_bridge.rs`.
2. Port LCG RNG from `fx/smoke.rs` as a per-emitter helper, not a global.
3. Implement cone-direction sampling (azimuth uniform `[0, 2π)`, elevation `acos(1 - rand * (1 - cos(spread)))`). Document the math inline.
4. Wire into the frame loop: after on_tick bridge, before particle sim.
5. Maintain `HashMap<EntityId, EmitterBridgeState>` on the bridge system. Insert on first-seen; remove on despawn. Follow Plan 2's `LightSnapshot` precedent.
6. Implement `spin_animation` evaluation each tick: advance `spin_elapsed`, interpolate `rate_curve`, write `spin_rate`, clear the tween at completion.
7. Integration test: spawn an emitter parent, tick for 1 s, assert live count.

---

## Sub-plan 4 — Render integration: particle entities → `SpriteInstance` upload

**Scope:** Connect particle subsystem to the existing `SmokePass`.

### Description

Each frame, walk particle entities, bucket by `SpriteVisual.sprite`, produce `Vec<u8>` per-collection payloads matching the `SPRITE_INSTANCE_SIZE = 32` byte layout. Feed each collection to `SmokePass::record_draw(queue, pass, collection, &packed_bytes)` — per `render/smoke.rs`, the call requires an active `wgpu::Queue` and a `wgpu::RenderPass` in addition to the collection name and byte slice. No new pipeline, no new shader — this is a pure data-feed path into the existing billboard pipeline.

Lives in `postretro/src/scripting/systems/particle_render.rs`. Runs in the Render stage, inside the existing `SmokePass::record_draw` call site. Writes into a renderer-owned scratch buffer (`&mut`) to avoid per-frame allocation. The renderer owns GPU; `SmokePass` consumes byte slices. Walking the registry belongs in a system that pipes its output into the renderer.

### Sprite sheet registration

`SmokePass::register_collection` expects frames at level-load. The definition-phase scan (Plan 1 archetype collection) harvests `emitter.sprite` names from every script-authored preset's `emitter()` component, plus the built-in `BillboardEmitter` defaults, and pre-registers each collection. Runtime introduction of a new sprite name is treated as a script bug: log a one-time warning, fall back to the default (first-registered) collection. Loading new sprite sheets mid-frame is future work. Map entity KVPs from `billboard_emitter` entries must also be swept during the level-load pass to harvest any `sprite` values that aren't covered by script presets or built-in defaults.

### Pack path

Mirror `SmokeEmitter::pack_instances`:

```rust
// Proposed design — particle_render.rs
fn pack_particle_instance(
    transform: &Transform,
    particle: &ParticleState,
    visual: &SpriteVisual,
    out: &mut Vec<u8>,
) {
    // SPRITE_INSTANCE_SIZE layout — see postretro/src/fx/smoke.rs
    // (position.xyz, age) + (size, rotation, opacity, _pad)
    out.extend_from_slice(&transform.position.x.to_ne_bytes());
    out.extend_from_slice(&transform.position.y.to_ne_bytes());
    out.extend_from_slice(&transform.position.z.to_ne_bytes());
    out.extend_from_slice(&particle.age.to_ne_bytes());
    out.extend_from_slice(&visual.size.to_ne_bytes());
    out.extend_from_slice(&visual.rotation.to_ne_bytes());
    out.extend_from_slice(&visual.opacity.to_ne_bytes());
    out.extend_from_slice(&0.0f32.to_ne_bytes());
}
```

`SpriteVisual.tint` is **not packed** — current layout has no color channel. Emitters render at sprite-sheet native color until the layout extends. Documented in non-goals.

### Per-collection bucketing

Hold a persistent `HashMap<String, Vec<u8>>` on the render system. At the start of each frame, call `.clear()` on every inner buffer (preserves capacity, drops nothing). Append packed bytes per particle. Output: a sequence of `(collection, &[u8])` pairs. No per-frame allocation in steady state — buffer capacity stabilizes after a few frames at peak load.

### Acceptance criteria

- [ ] A particle with valid `SpriteVisual` appears as a billboard in the next rendered frame.
- [ ] Multiple emitters using one collection share one `record_draw` call.
- [ ] Emitters using different collections each produce their own `record_draw`.
- [ ] An unregistered `sprite` logs a one-time warning and falls back to the default collection.
- [ ] A map-only `billboard_emitter` with `sprite = "dust"` and no script preset referencing `"dust"` renders without falling back to the default collection.
- [ ] `cargo test -p postretro` passes.

### Implementation tasks

1. Create `postretro/src/scripting/systems/particle_render.rs` with the collector + pack function.
2. Extend the definition-phase archetype scan to harvest `emitter.sprite` names from script presets and from `billboard_emitter` KVPs in map entities; pre-register each unique collection on `SmokePass`.
3. Hook the collector into the existing `SmokePass::record_draw` path.
4. Integration test: spawn an emitter, tick one frame, assert per-collection bytes have the expected length and non-zero content.

---

## Sub-plan 5 — `setEmitterRate` and `setSpinRate` reaction primitives

**Scope:** Two named reaction primitives registered into the Rust reaction registry from data-script-setup task 4. Not behavior-context FFI primitives.

### `setEmitterRate`

#### Description

Reactions reference primitives by name. The data-script-setup reaction registry resolves `primitive: "setEmitterRate"` at dispatch time. This sub-plan adds that name to the registry and implements the dispatch handler.

Signature inside the registry:

```rust
// Proposed design — postretro/src/scripting/reactions/set_emitter_rate.rs
pub fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],     // resolved from descriptor.tag
    args: &SetEmitterRateArgs, // { rate: f32 }
) -> Result<(), ReactionError> { ... }
```

The reaction descriptor shape on the script side carries the rate as part of the primitive's args (delivery mechanism follows whatever data-script-setup task 4 settled on — args are typed per primitive in the descriptor union).

For each target entity:
- If no `BillboardEmitterComponent`, emit `log::warn!` and skip (tag matched a non-emitter — most likely a tag typo; `setEmitterRate` is the only deactivation mechanism so a silent skip would be hard to diagnose).
- Else write `BillboardEmitterComponent.rate = args.rate`. The bridge picks up the change next tick.

#### Error cases

- `rate < 0.0` → rejected at *dispatch time* (not manifest load). Emits `log::warn!`, clamps `rate` to `0.0`, continues. TypeScript and Luau cannot natively enforce "positive number" types without branded types, so compile-time enforcement is not feasible; hot reload is the fix path.
- Empty target set → no-op, debug log.

### `setSpinRate`

#### Description

Sets a spin tween on the targeted emitters. Pass `Rate(r)` to set `spin_rate` immediately; any in-flight tween is cancelled. Pass `Animation(anim)` to start a tween; the bridge interpolates `spin_rate` from the curve over the animation's duration (see sub-plan 3).

#### Dispatch signature

```rust
// Proposed design — postretro/src/scripting/reactions/set_spin_rate.rs
pub fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetSpinRateArgs,
) -> Result<(), ReactionError> { ... }
```

```rust
pub enum SetSpinRateArgs {
    /// Set spin_rate immediately with no tween.
    Rate(f32),
    /// Tween spin_rate over the animation's duration.
    Animation(SpinAnimation),
}
```

For each target entity:
- If no `BillboardEmitterComponent`, emit `log::warn!` and skip.
- `SetSpinRateArgs::Rate(r)`: write `BillboardEmitterComponent.spin_rate = r`, clear any in-flight `spin_animation`, reset `spin_elapsed = 0.0`.
- `SetSpinRateArgs::Animation(anim)`: write `spin_animation = Some(anim)`, reset `spin_elapsed = 0.0`. Does not touch `spin_rate` directly — the bridge will interpolate it starting next tick.

### Acceptance criteria

- [ ] `setEmitterRate` is registered in the reaction registry under the name `"setEmitterRate"` and visible to manifest validation.
- [ ] A reaction descriptor `{ primitive: "setEmitterRate", tag: "campfires", args: { rate: 0 } }` fired against entities tagged `campfires` zeroes their rate; new spawns stop within one tick; existing particles continue until lifetime ends.
- [ ] A reaction with `rate: 20.0` against an emitter previously at `rate = 5.0` quadruples observed spawn rate within ~0.5 s.
- [ ] A negative `rate` emits a `warn`-level log at dispatch time, clamps to `0.0`, and the emitter becomes inactive (rate = 0) — no hard error, dispatch continues.
- [ ] Tag matching only fires the primitive on entities whose `_tags` contains the target string (multi-tag entities match on any of their tags).
- [ ] A target entity with `_tags` matching but no `BillboardEmitterComponent` produces a `log::warn!` message (not debug, not error); confirmed by log-capture test.
- [ ] Generated `.d.ts` and `.d.luau` expose the `setEmitterRate` shape inside the reaction descriptor union.
- [ ] `setSpinRate` with `Rate(r)` updates `spin_rate` immediately; particles begin rotating at the new rate within one tick.
- [ ] `setSpinRate` with `Animation(anim)` tweens the rate over the specified duration; the tween is visible in `BillboardEmitterComponent.spin_rate` as it advances each tick.
- [ ] Firing `setSpinRate` while a prior tween is in-flight cancels the prior tween and starts the new one immediately.
- [ ] Tag matching fires `setSpinRate` on entities with `BillboardEmitterComponent` only; non-emitter matches log `warn` and skip.

### Implementation tasks

1. Implement the `setEmitterRate` and `setSpinRate` dispatch handlers under `postretro/src/scripting/reactions/`.
2. Register both via the data-script-setup reaction-registry builder.
3. Extend the descriptor union in `sdk/lib/data_script.{ts,luau}` and `sdk/types/postretro.d.{ts,luau}` to type both primitives' args. Proposed design:

   ```typescript
   export type SetEmitterRateStep = {
     tag: string;
     primitive: "setEmitterRate";
     args: { rate: number };
   };

   export type SetSpinRateStep = {
     tag: string;
     primitive: "setSpinRate";
     args: { rate: number } | { animation: import("postretro").SpinAnimation };
   };
   ```

   And update the `SequenceStep` union to:

   ```typescript
   export type SequenceStep = SetLightAnimationStep | SetEmitterRateStep | SetSpinRateStep;
   ```

   Mirror the same shape in the `.luau` counterpart and in the `.d.ts` / `.d.luau` type declaration files.
4. Integration tests: data script registers an emitter, fires `setEmitterRate` (assert spawn-rate delta) and `setSpinRate` (assert `spin_rate` updates immediately and tweens over time).

---

## Sub-plan 6 — Built-in `BillboardEmitter` classname handling

**Scope:** Engine-side handling of `classname "billboard_emitter"` map entities. No script registration involved.

### Description

`BillboardEmitter` is a built-in engine entity type. The level loader recognizes `classname "billboard_emitter"` natively and spawns an ECS entity with a `BillboardEmitterComponent` configured from the map entity's KVPs. Authors do not need to call any registration function.

The SDK exports `BillboardEmitter` as a TypeScript type — purely for IDE support when authoring named instances or per-instance overrides in the data script. The type carries the same shape as `EmitterProps` (input to `emitter()`). It is not a runtime value.

Naming intent: `BillboardEmitter` reserves `ParticleEmitter` as the umbrella name for future mesh-particle work. The rendering primitive lives in the type name, not in a generic word. Document this prominently in the file header so the next contributor doesn't rename it speculatively.

### KVP map

For `classname "billboard_emitter"`, the level loader applies these FGD-key-to-component-field mappings:

| FGD key | Type | `BillboardEmitterComponent` field | Default |
|---|---|---|---|
| `rate` | `float` | `rate` | `6.0` |
| `lifetime` | `float` | `lifetime` | `3.0` |
| `spread` | `float` | `spread` (radians) | `0.4` |
| `buoyancy` | `float` | `buoyancy` | `0.2` |
| `drag` | `float` | `drag` | `0.8` |
| `sprite` | `string` | `sprite` | `"smoke"` |
| `initial_velocity_x` | `float` | `initial_velocity[0]` | `0.0` |
| `initial_velocity_y` | `float` | `initial_velocity[1]` | `0.8` |
| `initial_velocity_z` | `float` | `initial_velocity[2]` | `0.0` |
| `color_r` / `color_g` / `color_b` | `float` | `color` | `1.0, 1.0, 1.0` |
| `spin_rate` | `float` | `spin_rate` | `0.0` |

Curve fields (`size_over_lifetime`, `opacity_over_lifetime`) have no FGD keys at this scope — defaults match `smokeEmitter()` from sub-plan 7. `spin_animation` has no FGD key — it is runtime-only (set by reactions).

### Engine integration path

A startup hook in `postretro/src/scripting/builtins/` registers the `billboard_emitter` classname handler with the level loader's classname dispatch table. This runs once at engine init, before any level loads. The handler survives level unload — built-in handlers are never cleared.

### Acceptance criteria

- [ ] A map entity with `classname = "billboard_emitter"` and `rate = 12` spawns an ECS entity whose `BillboardEmitterComponent.rate` is `12.0` — with no script involvement.
- [ ] Absent KVPs use the documented defaults.
- [ ] An invalid KVP value logs a warning naming the key and origin, falls back to default, load continues (matches data-script-setup's manifest-error policy).
- [ ] `cargo test -p postretro` passes.

### Implementation tasks

1. Create `postretro/src/scripting/builtins/billboard_emitter.rs`. File header per `development_guide.md` §5.2.
2. Build the classname handler with KVP map and `BillboardEmitterComponent` defaults.
3. Hook into the engine init path that registers built-in classname handlers.
4. Integration test: load a map containing `classname "billboard_emitter"` with no script; assert the entity spawns with component values matching the KVPs.

---

## Sub-plan 7 — SDK module: `sdk/lib/entities/emitters.{ts,luau}`

**Scope:** Hand-authored TypeScript and Luau under `sdk/lib/`.

### Files

| Path | Exports |
|---|---|
| `sdk/lib/entities/emitters.ts` / `.luau` | `BillboardEmitter` (TypeScript type), `emitter(props): ComponentDescriptor`, `smokeEmitter`, `sparkEmitter`, `dustEmitter` |

The entity type and the component constructor / presets that build it co-locate at one path — authors do one import to get both. Method names are short; the rendering primitive (`billboard`) is encoded in the import path (`postretro/entities/emitters`), not in the function name. Future mesh particles would ship under `sdk/lib/entities/mesh-emitters.{ts,luau}` exporting their own short-named presets.

> **Import convention note:** `postretro/entities/emitters` is the first module to follow the domain-driven SDK layout (`postretro/<domain>/<subdomain>`). Existing SDK files (`data_script.ts`, `light_animation.ts`, etc.) predate this convention and do not follow it. Implementers: use the layout in this sub-plan as the model, not the existing flat files. Cleanup of existing paths is tracked separately.

### `emitter()` — component constructor

Validates props synchronously:

- `lifetime > 0`.
- `rate >= 0`.
- `spread >= 0`.
- `drag >= 0`.
- `initial_velocity` is `[number, number, number]`.
- `color` is `[number, number, number]`, each in `[0, 1]`.
- `size_over_lifetime` / `opacity_over_lifetime` are nonempty number arrays when present.
- `sprite` is a nonempty string.

Defaults for omitted fields:

| Field | Default |
|---|---|
| `buoyancy` | `0.5` (rises gently — smoke default) |
| `drag` | `0.5` |
| `spread` | `0.2` (radians, ~11°) |
| `opacity_over_lifetime` | `[1.0, 1.0, 0.8, 0.0]` |
| `size_over_lifetime` | `[1.0]` (constant) |
| `color` | `[1.0, 1.0, 1.0]` |
| `rate` | `0.0` |
| `burst` | undefined |
| `spin_rate` | `0.0` |

`rate = 0` and `burst = undefined` together describe a dormant emitter. That's a valid configuration — no validation error.

Returns `{ kind: "billboard_emitter", value: BillboardEmitterComponent }`.

Canonical campfire shape (the composition decision in action):

```typescript
// Proposed design — author code using the SDK
import { emitter } from "postretro/entities/emitters";
import { light } from "postretro/entities/lights";

export const Campfire = {
  classname: "campfire",
  emitter: emitter({
    rate: 12,
    spread: 0.3,
    lifetime: 1.2,
    initial_velocity: [0, 1.5, 0],
    color: [1.0, 0.6, 0.2],
    sprite: "flame",
  }),
  light: light({
    type: "point",
    intensity: 80,
    color: [1.0, 0.5, 0.2],
    animation: { period: 0.8, brightness: [0.7, 1.0, 0.6, 0.9] },
  }),
};
```

The preset is a plain typed object. No `defineEntity` wrapper. No registration call. Composition intent (emitter + light as siblings on one entity) is preserved structurally.

A named `BillboardEmitter` instance — for use as a per-instance override or sequence-step argument — is a typed object too:

```typescript
// Proposed design
import { type BillboardEmitter } from "postretro/entities/emitters";

export const exhaustPort: BillboardEmitter = {
  rate: 50,
  buoyancy: 0.0,
};
```

### Presets — `smokeEmitter`, `sparkEmitter`, `dustEmitter`

Pure functions returning `ComponentDescriptor`. No primitive calls, no side effects. Each accepts `Partial<EmitterProps>` and merges over its preset defaults. Co-located with `emitter()`, the `BillboardEmitter` type, and `SpinAnimation` at `sdk/lib/entities/emitters.{ts,luau}` — `SpinAnimation` is exported from this module.

| Preset | Visual intent | Notable defaults |
|---|---|---|
| `smokeEmitter` | Soft, rising smoke | `rate: 6`, `lifetime: 3.0`, `buoyancy: 0.2`, `size_over_lifetime: [0.3, 1.5]`, `opacity_over_lifetime: [0.0, 0.8, 0.6, 0.0]`, `sprite: "smoke"`, `spin_rate: 0.0` |
| `sparkEmitter` | Fast, falling sparks | `rate: 0`, `burst: 12`, `lifetime: 0.6`, `buoyancy: -1.0`, `size_over_lifetime: [1.0, 0.3]`, `opacity_over_lifetime: [1.0, 1.0, 0.0]`, `sprite: "spark"`, `spin_rate: 1.5` (fast spark tumble) |
| `dustEmitter` | Slow drifting dust | `rate: 2`, `lifetime: 5.0`, `buoyancy: 0.05`, `drag: 1.0`, `size_over_lifetime: [0.5, 1.0]`, `opacity_over_lifetime: [0.0, 0.3, 0.0]`, `sprite: "dust"`, `spin_rate: 0.0` |

Each function carries JSDoc / Luau docstrings explaining the visual intent and the rationale behind defaults — modders learn the property format by example.

### Acceptance criteria

- [ ] `emitter()` called with the campfire shape returns a descriptor accepted by the engine; the resulting entity renders particles.
- [ ] `emitter()` with invalid props throws naming the invalid field.
- [ ] `smokeEmitter()` with no args returns a fully populated descriptor; `smokeEmitter({ rate: 20 })` overrides `rate` only.
- [ ] `sparkEmitter({ burst: 40 })` triggered at runtime spawns exactly 40 particles.
- [ ] A test map composes `emitter()` and `light()` siblings on one entity; both render.
- [ ] The `BillboardEmitter` TypeScript type structurally matches `EmitterProps` and provides IDE completions on a `const X: BillboardEmitter = { ... }` declaration.
- [ ] Hot reload (Plan 1 sub-plan 7) works on these files in dev.
- [ ] Luau and TypeScript versions produce equivalent runtime behavior (round-trip test: load both, spawn entities, tick, compare live counts within RNG-seed tolerance).

### Implementation tasks

1. Author `sdk/lib/entities/emitters.ts` and `.luau`. Exports the `BillboardEmitter` TypeScript type, the `emitter()` component constructor (with validation, defaults, doc strings), and the `smokeEmitter` / `sparkEmitter` / `dustEmitter` presets.
2. Update the prelude regeneration path so the runtime modules are exposed as globals (`emitter`, `smokeEmitter`, `sparkEmitter`, `dustEmitter`). The `BillboardEmitter` type is type-only and does not enter the runtime prelude. Confirm the prelude bundler supports nested `postretro/<domain>/<subdomain>` paths before exposing these as globals; extend it if needed.
3. Update the SDK README to list the new module.
4. Integration test for both runtimes: compile, evaluate a campfire definition, spawn at runtime, tick, assert non-zero particle count and no validation errors.

---

## Sub-plan 8 — FGD-classname routing for built-in types; remove `--demo-smoke`

**Scope:** Generic level-loader routing from FGD `classname` to the engine's built-in classname-handler table. Removes the hardcoded `env_smoke_emitter` arm and the `--demo-smoke` flag.

### Description

After sub-plan 6 lands, the engine carries a built-in classname-handler table seeded with `billboard_emitter` (and `light`, the existing case). The level loader resolves an FGD `classname` against this table:

1. For each map entity, look up `entity.classname` in the built-in handler table.
2. If found, run the handler: instantiate the configured components, apply the KVP map, spawn the ECS entity at `entity.origin`, copy `_tags`.
3. If the classname is not registered, log a debug message and skip (matches data-script-setup's log-and-continue policy). This is not an error — unregistered classnames are valid in maps that don't use them.

Each built-in entity type is described by a `BuiltinEntityType` carrying a classname and a list of `EntityProperty` entries. Each entry maps one FGD KeyValues key to a component field, with a type (`PropertyKind`) and a default value. User scripts never see this shape.

```rust
// Proposed design — built-in entity type and property descriptors (engine-internal)
pub struct BuiltinEntityType {
    pub classname: &'static str,
    pub properties: Vec<EntityProperty>,
}

pub struct EntityProperty {
    /// Raw FGD KeyValues key name.
    pub key: &'static str,
    pub target: EntityPropertyTarget,
    pub kind: PropertyKind,
    pub default: ComponentValueLit,
}

pub enum EntityPropertyTarget {
    /// Maps a scalar key to a named field on a component.
    Field { component: ComponentKind, field: &'static str },
    /// Maps a scalar key to one element of a vec3 field on a component.
    Vec3Element { component: ComponentKind, field: &'static str, index: usize },
}
```

### Removals

`--demo-smoke` and the legacy `SmokeEmitter` path are deleted in this same sub-plan, after `BillboardEmitter` (sub-plan 6) provides the replacement and the FGD route handles `billboard_emitter` correctly. The map-side classname is renamed from `env_smoke_emitter` to `billboard_emitter`. Sweep test maps in the same change. Before deleting, `grep` for all uses of `--demo-smoke` and `SmokeEmitter`.

### Acceptance criteria

- [ ] A map entity with `classname` matching a built-in handler instantiates that type at the entity's origin with components configured per the KVP map.
- [ ] An unregistered classname is skipped with a debug log; no warning, no error.
- [ ] An invalid KVP value (e.g., `rate = "bad"`) logs a warning naming the key and origin, falls back to default, load continues.
- [ ] `--demo-smoke` is removed; `cargo build` produces no dead-code warnings.
- [ ] `SmokeEmitter` and the smoke-specific level-loader arm are removed; all consumers updated in the same change.
- [ ] Existing test maps that referenced `env_smoke_emitter` are renamed to `billboard_emitter` and continue to render smoke unchanged.
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Implement KVP-map application in the level-loader sweep.
2. Replace the hardcoded `env_smoke_emitter` arm with the generic built-in-handler lookup.
3. Sweep `--demo-smoke`, `SmokeEmitter`, and the demo spawn site. Remove.
4. Migrate `env_smoke_emitter` usages in test maps to `billboard_emitter`.
5. Integration test: load a map with one `billboard_emitter` entity; tick one frame; assert non-zero live particle count at the expected world position.

---

## When this plan ships

Durable knowledge migrates to `context/lib/`:

- **`context/lib/scripting.md`** gains an "Emitter and particles" section: emitter and light compose as siblings; `BillboardEmitter` is a built-in engine entity type handled by the level loader; the SDK's `BillboardEmitter` export is a TypeScript type for authoring safety; naming reserves `ParticleEmitter` for mesh particles; "scripts configure, Rust simulates" invariant; "no per-particle script callbacks"; per-emitter `MAX_SPRITES` cap; `setEmitterRate(id, 0)` is the inactive state — no `setEmitterActive`; `buoyancy` sign convention.
- **`context/lib/rendering_pipeline.md`** §7.4 (billboard pass): the instance buffer is fed by scripted particles via the `SpriteVisual` collector; additive blending is the only supported mode.
- **`context/lib/entity_model.md`**: short "Particles" paragraph — each live particle is an ECS entity; emitter bridge owns spawn/despawn; scripts never observe individual particles.
- **`context/lib/build_pipeline.md`**: built-in classname routing — the level loader resolves FGD `classname` against an engine-side handler table; per-classname KVP maps drive component-field overrides.

The plan moves `drafts/` → `ready/` → `in-progress/` → `done/` and stays frozen.
