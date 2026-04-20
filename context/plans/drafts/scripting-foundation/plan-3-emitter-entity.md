# Plan 3 — Emitter Entity Scripting

> **Status:** draft — second feature plan on top of Plan 1's runtime foundation; immediate successor to Plan 2.
> **Parent:** Scripting Foundation initiative. See `./postretro-scripting-spec-draft.md` for the full system vision.
> **Depends on:** `./plan-1-runtime-foundation.md` (sub-plans 1–5) and `./plan-2-light-entity.md` (sub-plans 1–6 — `Vec3Lit`, `ScriptCallContext.delta`, and the reference-vocabulary pattern all originate there).
> **Related (read for architectural context):** `context/lib/development_guide.md` §3 · `context/lib/context_style_guide.md` · `context/plans/done/lighting-foundation/index.md` (plan format reference) · the billboard smoke work salvaged in commit `838d966` and merged in `93bfa80` — `postretro/src/fx/smoke.rs`, `postretro/src/render/smoke.rs`, `postretro/src/shaders/billboard.wgsl`.
> **Lifecycle:** Per `development_guide.md` §1.5, this plan is its own spec while it stays a draft. Durable knowledge migrates to `context/lib/scripting.md` and `context/lib/rendering_pipeline.md` when it ships.

---

## Goal

Bring emitter-driven particles into the scripting surface as first-class entities, composable alongside lights on the same entity. After this plan ships:

- Definition scripts (TS/Luau) can declare entities with an `emitter` component. A campfire is one `defineEntity` carrying both a `light()` and an `emitter()` component — neither subordinate.
- Each live particle is a full ECS entity (from Plan 1's registry) carrying `Transform`, a `ParticleState`, and a sprite visual descriptor. No script behavior on particles — the simulation loop is pure Rust.
- A Rust emitter bridge system spawns particles each tick using Plan 1's `spawn_entity` / `despawn_entity` internally; a Rust particle simulation system integrates velocity, applies gravity and drag, advances age, and despawns at end-of-life.
- Behavior scripts can call `set_emitter_active(entity, bool)` and `set_emitter_rate(entity, f32)` to start/stop or modulate emission.
- A TypeScript reference vocabulary ships: `emitter()` component constructor, plus pre-built `smokeEmitter` / `sparkEmitter` / `dustEmitter` configurations.
- The billboard sprite pass (already built by the volumetric-smoke work in commits `4d4e97f` / `838d966`) receives particle sprites through the existing `SpriteInstance` upload path — no new shader, one new CPU → GPU wiring layer.

**What this plan does not deliver:**

- Particle collision against world geometry — collision infrastructure (Rapier/parry3d) does not exist yet.
- GPU-driven particle simulation — CPU simulation is the starting point; the GPU already instances rendering.
- Sub-emitters (particles that themselves spawn particles) — recursion pushes into a trickier despawn story; explicit follow-up.
- Luau reference vocabulary (`emitter.luau`, `particles.luau`) — same deferral pattern as Plan 2 sub-plan 7. Tracked as the final sub-plan here.
- Mesh particles. Billboard sprites only.
- Particle sorting for correct alpha-blended transparency. The existing billboard pass uses additive blending and does not sort — see the research note under "Settled decisions" below. Additive blending is order-independent, so the practical visual gap is smaller than it would be with straight alpha; document the limit for non-additive future emitters.
- On-disk PRL format additions. Emitter entities spawn at runtime via scripting or level-loader wiring (sub-plan 8); there is no new PRL section for emitters.
- General map-entity → script routing. Sub-plan 8 hardcodes only `env_smoke_emitter` in the level loader; routing arbitrary FGD classnames to definition scripts is future work.

---

## Settled decisions (do not relitigate)

- **Emitters and lights are separate taxonomies that compose freely.** A campfire is one entity with `emitter({...})` and `light({...})` as sibling components. Neither is a subtype of the other. `defineEntity({ components: [emitter(...), light(...)] })` is the canonical fire shape.
- **Scripts configure, Rust simulates.** Per-particle `on_tick` invocation is prohibitively expensive; the Rust particle simulation system runs every frame over `ParticleState`-bearing entities. Scripts set emitter component data at spawn or on trigger and receive emitter-level lifecycle events — never per-particle callbacks.
- **Particles are lightweight ECS entities.** Each live particle has an `EntityId` from Plan 1's registry and carries `Transform`, `ParticleState`, and a sprite visual descriptor. No behavior component. Spawning and despawning use the Rust-side `EntityRegistry::spawn` / `despawn` directly from the bridge — scripts never loop over particles.
- **Particle properties mirror Unreal / Godot conventions.** Researched during planning: `emission_rate`, `burst_count`, `lifetime`, `initial_velocity`, `spread` (cone half-angle, radians), `gravity_scale` (float; negative = rises, positive = falls), `drag` (per-second damping), `size_over_lifetime` / `opacity_over_lifetime` (keyframe curves over normalized age), `color` (base tint), `sprite` (asset name).
- **Curve channels reuse Plan 2's `LightAnimation` keyframe pattern.** `size_over_lifetime` and `opacity_over_lifetime` are `Vec<f32>` keyframe arrays evaluated by linear interpolation over the particle's normalized lifetime (0.0 at spawn, 1.0 at despawn). Same evaluation shape as `LightAnimation.brightness`. Do not invent a parallel curve type.
- **`Vec3Lit` (Plan 2) is reused unchanged.** Any `Vec3`-shaped script input (`initial_velocity`, `color`) goes through `Vec3Lit` at the FFI boundary. A plain `Vec<f32>` (the curve channels) does not need `Vec3Lit` — per research below, rquickjs-serde and mlua-serde both deserialize `Vec<f32>` cleanly from a JS `number[]` / Lua sequence. The `[f32; 3]` tuple problem is specifically about fixed-size arrays.
- **`spawn_entity` / `despawn_entity` from Plan 1 are the particle spawn/despawn path — Rust-side only.** The emitter bridge calls them directly. They are not invoked from script for individual particles.
- **`delta` is in the bridge tick call.** Established by Plan 2 sub-plan 5. The emitter bridge uses `delta` for emission-rate accumulation; the particle simulation uses `delta` for physics integration.
- **No `defineEmitter` specialization.** A "smoke emitter" is `defineEntity({ components: [emitter({...})] })`. `emitter()` is a component constructor, same shape as `light()` from Plan 2.
- **Pre-release API policy.** Rename freely, no compat shims, fix all consumers in the same change (`MEMORY.md` / `feedback_api_stability.md`).

---

## Research findings that shape this plan

### 1. Billboard renderer already has the upload path we need

The volumetric-smoke work (branch `feat/fx-volumetric-smoke`, commits `4d4e97f` and `838d966`) landed a complete billboard pass with a `SpriteInstance` storage buffer and per-frame upload:

- `postretro/src/fx/smoke.rs` defines `SPRITE_INSTANCE_SIZE = 32`. Layout: `vec4` of `(position.xyz, age)` + `vec4` of `(size, rotation, opacity, _pad)`.
- `SmokeEmitter::pack_instances(&self, out: &mut Vec<u8>)` is the canonical packer — appends `live_count * SPRITE_INSTANCE_SIZE` bytes to a caller-owned `Vec<u8>`.
- `postretro/src/render/smoke.rs::SmokePass::record_draw` takes a `packed_bytes: &[u8]` slice per-collection and calls `queue.write_buffer(&instance_buffer, 0, ...)` once per collection per frame.
- Pipeline layout uses groups 0 (camera), 1 (sheet + draw params), 2 (lights), 3 (SH volume), and 6 (instance storage). Groups 4 and 5 are `None`.
- Blending is **additive** (`BlendFactor::One` / `BlendFactor::One` both channels). Depth test enabled, depth write disabled.
- Per-collection sheet registration with frame-stitching is already wired: `SmokePass::register_collection(device, queue, collection, frames, spec_intensity, lifetime)`.

**What this means for Plan 3:** the GPU path is done. Particles need a CPU-side packer of the same `SpriteInstance` layout, plus a code path inside the renderer that asks the emitter subsystem for its packed bytes (by collection) and calls the existing `SmokePass::record_draw`. No new shader, no new pipeline, no new bind groups.

### 2. Transparency ordering — honest limitation

The billboard pass uses **additive blending with depth-write disabled**. Additive blending is commutative over fragments (`A + B + C == C + B + A`), so the draw order of particles does not affect the final color. This makes sorting unnecessary for the current visual target (smoke, sparks, dust — all additive, emissive-looking).

If a future emitter needs non-additive alpha-blend (dark smoke, water splash), particle sorting becomes required for correctness. This plan does not add sorting. The acceptance criteria mark additive as the one supported blend mode. Non-additive emitters are explicitly out of scope and tracked as a future plan.

A second, smaller caveat: because `depth_write_enabled = false`, particles do not occlude each other through depth — they only read depth against the opaque scene. For additive blending this is exactly right. If mesh particles or alpha-blend particles are added later, they will need a separate pipeline that writes depth, and at that point, sorting.

### 3. `rquickjs` / `mlua` deserialization of `Vec<f32>`

Confirmed via rquickjs and mlua serde docs: a script-side `number[]` deserializes cleanly into a Rust `Vec<f32>` through both `rquickjs::Value::from_js` (with the `serde` feature) and `mlua::LuaSerdeExt::from_value`. The `Vec3Lit` workaround from Plan 2 is specifically for *fixed-size* arrays (`[f32; 3]`), where serde's default adapter produces a tuple form that neither runtime's JS/Lua array representation matches. `Vec<f32>` takes the sequence path and works.

**Concrete mapping for this plan:**

- `initial_velocity: Vec3Lit` (script-facing) → converted to `Vec3` / `[f32; 3]` before storing in the engine `EmitterComponent`.
- `color: Vec3Lit` (script-facing) → same conversion.
- `size_over_lifetime: Vec<f32>` — direct, no adapter.
- `opacity_over_lifetime: Vec<f32>` — direct, no adapter.
- `rate: f32`, `lifetime: f32`, `spread: f32`, `gravity_scale: f32`, `drag: f32` — scalar, direct.
- `burst: Option<u32>` — direct.
- `sprite: String`, `archetype: String` — direct.

### 4. Entity count budget

Plan 1's `EntityId` packs 24 bits of index and 8 bits of generation — 16 777 216 live slots, 256 generations per slot. The expected concurrent particle count in this plan's scope is bounded by the billboard pass's already-existing `MAX_SPRITES = 512` per emitter (see `postretro/src/fx/smoke.rs`). Even with dozens of active emitters, the concurrent particle count comfortably fits into four digits — five or six orders of magnitude below the registry's slot ceiling.

The practical budget surfaces in two places:

- **GPU-side**: the billboard `instance_buffer` is sized for `MAX_SPRITES * SPRITE_INSTANCE_SIZE` per collection draw. This plan matches that cap: emission beyond `MAX_SPRITES` live particles per emitter is dropped with a rate-limited warning log, mirroring the existing `SmokeEmitter` policy. Growing the buffer is not in scope.
- **CPU-side**: spawning and despawning ~10 000 particles per second across the world is well within the registry's free-list throughput (Plan 1 sub-plan 1 target: 10 000 cycles in under 10 ms). State this explicitly as the confirmed budget; no registry redesign needed.

---

## Prerequisites from earlier plans

Plan 1's sub-plans must be complete and landed:

| Plan 1 sub-plan | What this plan needs |
|---|---|
| 1 — Entity / component registry | `EntityRegistry::spawn` / `despawn` called from the Rust bridges. `ComponentKind` extended with `Emitter`, `ParticleState`, and `SpriteVisual`. |
| 2 — Primitive binding layer | `register(...)` builder for `set_emitter_active` and `set_emitter_rate`. |
| 3 — QuickJS runtime + contexts | Behavior context owning the two emitter primitives. |
| 4 — Luau runtime + contexts | Symmetric behavior-context registration. |
| 5 — Type definition generator | `EmitterComponent`, `ParticleState`, `SpriteVisual` emitted into `postretro.d.ts` / `postretro.d.luau` automatically when the new component kinds register. |

Plan 2's sub-plans required:

| Plan 2 sub-plan | What this plan needs |
|---|---|
| 2 — `LightComponent` in the ECS registry | Pattern for a component module under `postretro/src/scripting/components/`. `Vec3Lit` adapter reused for `initial_velocity` and `color`. |
| 5 — `delta` / `time` in `on_tick` | `ScriptCallContext` is the vehicle for bridge-system delta in this plan too; the emitter bridge is a Rust system, but `on_burst_complete` (future) will ride the same context. |
| 6 — TypeScript reference vocabulary | Precedent for shipping `emitter.ts` / `particles.ts` in parallel with the Rust plumbing. |

Plan 1's sub-plans 6 (context pool) and 7 (hot reload) are not hard prerequisites but assumed available for dev iteration.

The billboard pass from commits `4d4e97f` and `838d966` (branch `feat/fx-volumetric-smoke`) must be merged to `main` before this plan starts. If it isn't, sub-plan 4 here has nothing to render into.

---

## Crates touched

| Crate | Role |
|---|---|
| `postretro-level-format` | No changes. No new PRL section; `env_smoke_emitter` is read from the existing map-entity KV bag. |
| `postretro-level-compiler` | No changes. |
| `postretro` | New `EmitterComponent`, `ParticleState`, `SpriteVisual` in the scripting registry. New `set_emitter_active` / `set_emitter_rate` primitives. New emitter bridge system (script-adjacent) and particle simulation system (pure Rust). New wiring between the particle entities and the existing `SmokePass` in `postretro/src/render/smoke.rs`. `ScriptCallContext` unchanged. Sub-plan 8: `env_smoke_emitter` wiring in the level loader; removal of `--demo-smoke`. |
| `assets/scripts/sdk/` | New files: `emitter.ts`, `particles.ts`. Luau ports (`emitter.luau`, `particles.luau`) are the final sub-plan. |

No new workspace dependencies.

---

## Sub-plan dependency graph

```
1. EmitterComponent + ParticleState + SpriteVisual in the registry
    │
    ├─→ 2. Particle simulation system (pure Rust, independent of scripts)
    │
    ├─→ 3. Emitter bridge system (consumes EmitterComponent, calls spawn_entity)
    │        │
    │        └─→ 5. set_emitter_active + set_emitter_rate primitives
    │
    ├─→ 4. Render integration: particle entities → SpriteInstance upload
    │
    └─→ 8. env_smoke_emitter FGD → EmitterComponent level-load wiring
             depends on 1 (component struct) + 4 (render path live)

6. TypeScript reference vocabulary (emitter.ts, particles.ts)
    │  depends on 1 (component shape pinned) + 5 (primitives visible)
    │
    └─→ 7. Luau reference vocabulary (follow-up within this plan)
```

Sub-plan 1 gates everything. Sub-plans 2, 3, and 4 can proceed in parallel once 1 lands — they touch different seams. Sub-plan 5 depends on 3 (the bridge is what reads the `active` / `rate` fields at tick time). Sub-plan 6 can start once the component shape in 1 is pinned, and 7 waits for 6 so the Luau port is a translation. Sub-plan 8 depends on 1 (the `EmitterComponent` struct exists) and 4 (the render path is live) — it is independent of scripting runtime work and can land in parallel with sub-plans 5–7.

---

## Sub-plan 1 — `EmitterComponent`, `ParticleState`, `SpriteVisual` in the registry

**Scope:** Rust only. Add three component kinds. Wire them into `EntityRegistry::set_component`.

### Description

Three new components land together because they reference each other by shape.

- **`EmitterComponent`** — the entity-side configuration. Carried by the parent entity (the fire, the smoke stack). Read by the emitter bridge every tick.
- **`ParticleState`** — the per-particle simulation state. Carried by each live particle entity, alongside `Transform` and `SpriteVisual`.
- **`SpriteVisual`** — the per-particle visual descriptor the renderer reads to build `SpriteInstance` bytes. Carrying this as a dedicated component (rather than folding into `ParticleState`) keeps the door open for non-particle sprite entities later (e.g., scripted enemy sprites).

### Struct shapes

```rust
// Proposed design — lives in postretro/src/scripting/components/emitter.rs.
// See: context/plans/drafts/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 1

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EmitterComponent {
    /// Name of the particle archetype or preset. Informational at this plan's
    /// scope; future work keys per-archetype presets off this.
    pub archetype: String,

    /// Particles per second when `active` and `burst` is `None`. Fractional
    /// values accumulate across frames (see bridge §Sub-plan 3).
    pub rate: f32,

    /// If `Some(n)`, spawns `n` particles once and then stops — overrides
    /// `rate`. Consumed at trigger time; the bridge resets this to `None`
    /// after the burst fires.
    pub burst: Option<u32>,

    /// Cone half-angle in radians for randomizing the initial direction
    /// around `initial_velocity`. `0.0` = perfectly directed.
    pub spread: f32,

    /// Per-particle lifetime in seconds.
    pub lifetime: f32,

    /// Initial direction + magnitude, world space.
    pub initial_velocity: [f32; 3],

    /// Gravity scale. Negative = rises (smoke). Positive = falls (sparks).
    /// Multiplied against the world gravity constant in the particle sim.
    pub gravity_scale: f32,

    /// Velocity damping per second.
    pub drag: f32,

    /// Keyframe curve, evaluated by linear interpolation over normalized
    /// lifetime. Same shape as `LightAnimation.brightness`.
    pub size_over_lifetime: Vec<f32>,

    /// Keyframe curve, normalized-lifetime opacity multiplier.
    pub opacity_over_lifetime: Vec<f32>,

    /// Base tint, linear RGB.
    pub color: [f32; 3],

    /// Sprite sheet collection name (sub-directory under `textures/`),
    /// matching the existing `SmokePass::register_collection` contract.
    pub sprite: String,

    /// Whether emission is currently on. `set_emitter_active` toggles this.
    pub active: bool,

    /// Fractional emission tracker — accumulates `delta * rate` and spawns
    /// when it crosses 1.0. Serialized (private) so the field round-trips
    /// through `set_component`, but scripts don't author it.
    #[serde(default, skip_deserializing)]
    pub accumulator: f32,
}
```

```rust
// Proposed design — lives in postretro/src/scripting/components/particle.rs.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParticleState {
    /// World-space velocity.
    pub velocity: [f32; 3],

    /// Seconds since spawn.
    pub age: f32,

    /// Total lifetime (copied from the emitter at spawn so the particle
    /// survives emitter changes).
    pub lifetime: f32,

    /// Back-reference to the spawning emitter. Not authoritative for
    /// simulation — retained for diagnostics and for future `on_particle_*`
    /// hooks. `None` for particles spawned outside an emitter (not used in
    /// this plan; reserved).
    pub emitter: Option<EntityId>,
}
```

```rust
// Proposed design — lives in postretro/src/scripting/components/sprite_visual.rs.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpriteVisual {
    /// Sprite sheet collection name. Matches the emitter's `sprite` field.
    pub sprite: String,

    /// Current world-space size. Driven by the particle sim evaluating
    /// `size_over_lifetime` at the particle's normalized age.
    pub size: f32,

    /// Current opacity in [0, 1]. Driven by the sim evaluating
    /// `opacity_over_lifetime`.
    pub opacity: f32,

    /// Current rotation in radians. Reserved for future spin; the sim
    /// leaves this at 0.0 unless a spin channel lands.
    pub rotation: f32,

    /// Base tint. The emitter copies its `color` field here at spawn so the
    /// particle renders in the emitter's color even if the emitter is later
    /// deleted.
    pub tint: [f32; 3],
}
```

### FFI adapter layer

- `initial_velocity: Vec3Lit` on the script-facing descriptor → converted to `[f32; 3]` when building the engine-internal `EmitterComponent`.
- `color: Vec3Lit` — same.
- `size_over_lifetime` / `opacity_over_lifetime` are plain `Vec<f32>` — no adapter; confirmed by research finding 3.
- Validation at the FFI boundary:
  - `lifetime > 0` (else `ScriptError::InvalidArgument { reason: "lifetime must be positive" }`).
  - `rate >= 0` (zero is valid — disables continuous emission without toggling `active`).
  - `spread >= 0`.
  - `drag >= 0` (negative would unbound velocities).
  - `size_over_lifetime` / `opacity_over_lifetime` nonempty when present (an empty curve is a script bug; require a least one sample).

### Registry integration

- Add `Emitter`, `ParticleState`, `SpriteVisual` variants to `ComponentKind` (Plan 1 sub-plan 1). `#[non_exhaustive]` — additions are not breaking.
- Add matching variants to `ComponentValue`.
- `EntityRegistry::set_component(id, ComponentValue::Emitter(...))` is the write path. No special-case code — generic.

### Acceptance criteria

- [ ] All three components round-trip through `serde_json` cleanly with nested curve arrays.
- [ ] Rust unit test `set_component`s each of the three on a spawned entity and reads it back unchanged.
- [ ] `Vec3Lit` at the FFI boundary rejects 2- or 4-element arrays, non-number elements, and shapes that aren't arrays, with a clear error message naming the field.
- [ ] FFI-boundary validation rejects each of the invalid cases above with `ScriptError::InvalidArgument { reason: "..." }` whose `reason` names the offending field.
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Create `postretro/src/scripting/components/emitter.rs`, `particle.rs`, `sprite_visual.rs`. Headers per `development_guide.md` §5.2.
2. Define the three structs. Reuse `Vec3Lit` from `postretro/src/scripting/ffi_types.rs` (Plan 2).
3. Extend `ComponentKind` and `ComponentValue` in Plan 1's registry module.
4. Unit tests for serde round-trip, registry write/read, `Vec3Lit` validation, FFI validation paths.

---

## Sub-plan 2 — Particle simulation system

**Scope:** Pure Rust system. No scripting involved. Runs each tick over every entity with `ParticleState`.

### Description

The particle simulation is a single function called once per game-logic frame. For each entity with `ParticleState`:

1. Advance `age += delta`.
2. Integrate `Transform.position += velocity * delta`.
3. Apply gravity: `velocity.y += WORLD_GRAVITY * gravity_scale * delta`. `WORLD_GRAVITY` is a constant (`-9.81` world units/s²; negative is down by convention). `gravity_scale` is carried on the particle at spawn — the simulation reads it off the `ParticleState` rather than looking up the emitter, so particles survive emitter despawn.
4. Apply drag: `velocity *= (1.0 - drag * delta).max(0.0)`. Clamping prevents negative multipliers under pathological `drag * delta` values.
5. Compute normalized age: `t = (age / lifetime).clamp(0.0, 1.0)`.
6. Evaluate `size_over_lifetime` and `opacity_over_lifetime` (carried on the particle via the emitter copy at spawn — see sub-plan 3) at `t`. Write results into the particle's `SpriteVisual.size` and `SpriteVisual.opacity`.
7. If `age >= lifetime`, `despawn(entity_id)`. Collect despawn IDs in a first pass and apply in a second pass to avoid mutating the registry mid-iteration.

The curve evaluation reuses the linear-interpolation pattern from Plan 2's `LightAnimation`. For an `N`-sample curve and normalized age `t`:

- `s = t * (N - 1)`
- `i = floor(s)`
- `frac = s - i`
- `value = curve[i] * (1 - frac) + curve[i + 1] * frac` (clamp `i + 1` to `N - 1` at the upper boundary).

A one-sample curve is constant. An empty curve defaults to `1.0` (but empty is rejected at FFI validation — so this path is only reachable via a Rust-side bug).

### Spawn-time emitter data copy

To decouple particles from their emitter's lifecycle, each particle carries enough state to run the sim without re-reading the emitter:

- `gravity_scale` and `drag` → copied into `ParticleState` at spawn (add these fields to `ParticleState`; update sub-plan 1's struct accordingly, or alternately store them indexed by `ParticleState.emitter` but only when the emitter still exists).

**Decision:** carry `gravity_scale` and `drag` on `ParticleState`. Extends sub-plan 1's struct by two `f32` fields. The curves (`size_over_lifetime`, `opacity_over_lifetime`, `tint`) stay on `SpriteVisual` / copied from the emitter into the visual component at spawn — `Vec<f32>` clones are cheap at particle rates. An alternative shared-curve design (reference-counted curve table) is not worth the indirection at this scale.

Update sub-plan 1 to carry the two extra `ParticleState` fields. The plan reads in order, so this is an inline edit when 1 lands.

### Frame ordering

Particle simulation runs in the Game Logic stage per `development_guide.md` §4.3 — after input, before audio and render. Specifically:

1. Input
2. Game logic:
   a. Script on_tick bridge calls (Plan 2 sub-plan 5)
   b. Emitter bridge (this plan, sub-plan 3) — spawns new particles
   c. Particle simulation (this sub-plan) — advances existing + spawned particles
   d. Light bridge (Plan 2 sub-plan 3) — uploads lights
3. Audio
4. Render — particle render integration (sub-plan 4)
5. Present

Ordering c-after-b means particles spawned this frame already tick one `delta` worth of simulation before render — matches the existing `SmokeEmitter::tick` semantics and prevents a one-frame "stuck at origin" visual glitch.

### Acceptance criteria

- [ ] A spawned particle with `velocity = (0, 5, 0)` and `gravity_scale = 1.0` follows a parabolic trajectory over its lifetime (test: sample position at three times, compare to analytic reference within floating-point tolerance).
- [ ] A particle with `drag = 1.0` decays its velocity to near-zero within ~4 lifetimes (test: sample velocity over time).
- [ ] `size_over_lifetime = [0.5, 1.0, 0.5]` produces size `0.5` at `t=0`, `1.0` at `t=0.5`, `0.5` at `t=1.0`, with linear interpolation between.
- [ ] A particle with `age >= lifetime` is despawned in the same tick and no longer appears in later iterations.
- [ ] 500 active particles simulated for 1 frame completes in under 0.5 ms on a release build (sanity check, not a strict perf target).
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Create `postretro/src/scripting/systems/particle_sim.rs`.
2. Implement the curve-eval helper. Extract to a shared location if Plan 2's `LightAnimation` eval lives in a distant module — factor out when the duplication is concrete.
3. Implement the tick function. Collect despawn IDs in a `Vec<EntityId>`, iterate, then despawn.
4. Wire into the frame loop in `postretro/src/main.rs` at the ordering described above.
5. Unit tests for the acceptance criteria. For the parabolic test, use a fixed `delta` and step the sim manually.

---

## Sub-plan 3 — Emitter bridge system

**Scope:** Rust. Reads `EmitterComponent` on each tick, spawns particle entities via `EntityRegistry::spawn`.

### Description

Once per game-logic frame, the emitter bridge iterates every entity with an `EmitterComponent` and emits particles. For each emitter:

1. If `!active`, skip (but still tick the accumulator to zero so reactivation starts fresh).
2. If `burst.is_some()`: spawn `burst.unwrap()` particles immediately, set `burst = None`, continue. Bursts ignore `rate`.
3. Otherwise: `accumulator += delta * rate`. While `accumulator >= 1.0` and the per-emitter cap (see "Budget enforcement" below) allows, spawn one particle and subtract 1.0 from `accumulator`.

For each spawn:

- Allocate a new entity via `EntityRegistry::spawn(Transform::default())`.
- Compute the initial direction: rotate `initial_velocity` by a random vector within the `spread` cone (cone-axis = normalized `initial_velocity`). Deterministic RNG per emitter (seeded from emitter position + a step counter), matching the `SmokeEmitter::rand_u32` pattern — this gives reproducible visuals without global RNG state.
- Write `Transform { position: emitter_entity.position, ..Default::default() }` onto the new entity.
- Write `ParticleState { velocity: initial_direction * magnitude, age: 0.0, lifetime: emitter.lifetime, gravity_scale: emitter.gravity_scale, drag: emitter.drag, emitter: Some(emitter_entity_id) }`.
- Write `SpriteVisual { sprite: emitter.sprite.clone(), size: 0.0, opacity: 0.0, rotation: 0.0, tint: emitter.color }`. Size and opacity get evaluated on the next sim tick; zero at this instant is fine (the sim runs immediately after the bridge).

### Budget enforcement

Per-emitter concurrent particle cap matches the billboard pass: `MAX_SPRITES = 512` from `postretro/src/fx/smoke.rs`. The bridge tracks per-emitter live count (either by snapshot or by reading the registry — snapshot is cheaper and diverges at most one frame, good enough). If spawning would exceed the cap, drop the spawn and emit a rate-limited warning (`log::warn!` at most once per second per emitter).

Total-world cap is the sum of per-emitter caps; no separate global cap in this plan.

### Emitter-entity position

The bridge needs the emitter entity's world position. The emitter entity carries a `Transform` from Plan 1 sub-plan 1. The bridge reads `Transform.position` for each emitter.

### Acceptance criteria

- [ ] An emitter with `rate = 10`, `active = true` spawns approximately 10 particles per second (±1 over a 2-second test, accounting for fractional accumulation across frames).
- [ ] An emitter with `burst = Some(20)` spawns exactly 20 particles in one tick and resets `burst` to `None`.
- [ ] An emitter with `active = false` spawns zero particles.
- [ ] An emitter at per-emitter cap drops further spawns and logs a rate-limited warning.
- [ ] Particle positions at spawn are within a `spread`-cone of `initial_velocity`'s direction, as measured by a statistical test (500 spawns, mean direction within tolerance of `initial_velocity.normalize()`).
- [ ] A particle's `ParticleState.emitter` points back to the spawning emitter's `EntityId`.
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Create `postretro/src/scripting/systems/emitter_bridge.rs`.
2. Port the deterministic-RNG pattern from `fx/smoke.rs` (LCG, seeded from emitter position). Expose as a method on the bridge's per-emitter state, not as a global.
3. Implement cone-direction sampling: uniform within a cone of half-angle `spread` around a given axis. Standard formula (pick azimuth uniform in `[0, 2π)`, pick elevation from `acos(1 - rand * (1 - cos(spread)))`). Document the math inline.
4. Wire into the frame loop after the on_tick bridge, before the particle sim.
5. Integration test: spawn a parent entity with `EmitterComponent`, tick the bridge for 1 second, assert the live particle count falls in the expected band.

---

## Sub-plan 4 — Render integration: particle entities → `SpriteInstance` upload

**Scope:** Connect the particle subsystem to the existing `SmokePass` in `postretro/src/render/smoke.rs`.

### Description

The existing `SmokePass` batches by collection name and uploads packed `SpriteInstance` bytes per collection per frame. Particles need to feed into this same path. The cleanest seam is to introduce a **particle collector** that, each frame, walks the particle entities, buckets by `SpriteVisual.sprite`, and produces `Vec<u8>` per-collection payloads matching the `SPRITE_INSTANCE_SIZE = 32` byte layout.

### Where this lives

`postretro/src/scripting/systems/particle_render.rs` — a CPU-side system that runs in the Render stage, before `SmokePass::record_draw` is called. It writes into a scratch buffer owned by the renderer (passed by `&mut` to avoid per-frame allocation). The renderer then calls `SmokePass::record_draw(queue, pass, collection, &packed_bytes)` once per collection.

Why not inside `SmokePass`: the renderer owns GPU. `SmokePass` consumes byte slices. Walking the entity registry does not belong inside the renderer — it belongs in a system that pipes its output into the renderer. Same boundary as the light bridge (Plan 2 sub-plan 3).

### Sprite sheet registration

`SmokePass::register_collection` expects frames to be loaded at level-load time. Scripted emitters reference sprite sheets by name. The simplest path for this plan:

- Load every sprite collection referenced by *any* definition-phase `emitter()` during level load. The script definition sweep (Plan 1) produces a list of archetypes; each archetype's `emitter.sprite` goes into a set; each set entry triggers a `SmokePass::register_collection` call once the sprite frames are resolved.
- Runtime `set_component` calls that introduce a *new* sprite name not seen at load time are an error — log a warning and fall back to the emitter's default collection (the first one registered). Loading new sprite sheets mid-frame is future work.

This is a reasonable restriction: modders ship their emitter archetypes in the definition files, so the sprite set is knowable at load time.

### Pack path

Mirror `SmokeEmitter::pack_instances`:

```rust
// Proposed design — lives in particle_render.rs
fn pack_particle_instance(
    transform: &Transform,
    particle: &ParticleState,
    visual: &SpriteVisual,
    out: &mut Vec<u8>,
) {
    // SPRITE_INSTANCE_SIZE layout — see postretro/src/fx/smoke.rs
    // position (vec3) + age (f32)
    out.extend_from_slice(&transform.position.x.to_ne_bytes());
    out.extend_from_slice(&transform.position.y.to_ne_bytes());
    out.extend_from_slice(&transform.position.z.to_ne_bytes());
    out.extend_from_slice(&particle.age.to_ne_bytes());
    // size + rotation + opacity + pad
    out.extend_from_slice(&visual.size.to_ne_bytes());
    out.extend_from_slice(&visual.rotation.to_ne_bytes());
    out.extend_from_slice(&visual.opacity.to_ne_bytes());
    out.extend_from_slice(&0.0f32.to_ne_bytes());
}
```

Note the `tint` field on `SpriteVisual` is **not currently packed** — the existing `SpriteInstance` layout has no color channel. This is a known limitation: scripted emitters render at their sprite-sheet native color until a tint channel is added. Extending `SpriteInstance` to 48 bytes (adding a `vec4<f32>` for `tint + extra`) is a small shader edit but changes the `SmokePass` layout and is a follow-up plan that lands alongside the first emitter needing non-white tint. Document this in the plan's non-goals and in a code comment at the pack site.

### Per-collection bucketing

Each particle entity has a `SpriteVisual.sprite` collection name. The collector builds a `HashMap<String, Vec<u8>>` each frame (or reuses a persistent map with `.clear()` on inner buffers). Final output is a sequence of `(collection, &[u8])` pairs fed to `SmokePass::record_draw`.

### Acceptance criteria

- [ ] A live particle entity with a valid `SpriteVisual` appears as a visible billboard sprite in the next rendered frame.
- [ ] Multiple emitters using the same collection share one `record_draw` call (verified via a render-pass debug marker or test instrumentation).
- [ ] Emitters using different collections each produce their own `record_draw` call.
- [ ] A particle entity whose `SpriteVisual.sprite` is not registered logs a one-time warning and falls back to the default collection.
- [ ] The existing `env_smoke_emitter`-driven smoke (via `SmokeEmitter` in `postretro/src/fx/smoke.rs`) continues to render unchanged — the scripted path is additive, not replacing.
- [ ] Tint (color) is known to not be applied — a particle's visible color matches its sprite-sheet texture regardless of `SpriteVisual.tint`. This is documented as a non-goal here.
- [ ] `cargo test -p postretro` passes.

### Implementation tasks

1. Create `postretro/src/scripting/systems/particle_render.rs`. Implement the collector + pack function.
2. Extend the definition-phase scan (Plan 1 archetype collection) to harvest `emitter.sprite` names and pre-register collections on `SmokePass`.
3. Hook the collector into the existing render path that calls `SmokePass::record_draw` — today that path draws the `SmokeEmitter`-owned bytes; extend it to also draw the script-owned bytes, in the same render pass, same pipeline.
4. Integration test: spawn a scripted emitter, tick one frame, assert the per-collection packed-bytes `Vec<u8>` has the expected length and non-zero content.

---

## Sub-plan 5 — `set_emitter_active` and `set_emitter_rate` primitives

**Scope:** Two new primitives registered through Plan 1's binding layer. Small scope.

### Description

Two primitives, both `BehaviorOnly` scope:

- `set_emitter_active(id: EntityId, active: bool) -> Result<(), ScriptError>` — writes `EmitterComponent.active`.
- `set_emitter_rate(id: EntityId, rate: f32) -> Result<(), ScriptError>` — writes `EmitterComponent.rate`. Validates `rate >= 0.0`, else `InvalidArgument`.

Both read the current `EmitterComponent`, mutate the one field, write back. The emitter bridge picks up the change on the next tick.

### Error cases

- Entity does not exist → `ScriptError::EntityNotFound`.
- Entity exists but has no `EmitterComponent` → `ScriptError::ComponentNotFound`. No auto-add.
- `set_emitter_rate` with negative rate → `ScriptError::InvalidArgument { reason: "rate must be non-negative" }`.

### Acceptance criteria

- [ ] Both primitives are registered as `BehaviorOnly` and visible in QuickJS and Luau behavior contexts.
- [ ] Calling from the definition context returns `ScriptError::WrongContext`.
- [ ] `set_emitter_active(id, false)` on a live emitter stops new particle spawns within one tick; existing particles continue until their lifetime ends.
- [ ] `set_emitter_rate(id, 20.0)` on an emitter previously at `rate = 5.0` doubles-plus the observed spawn rate within ~0.5 s.
- [ ] Negative rate is rejected with `InvalidArgument`.
- [ ] Generated `.d.ts` and `.d.luau` (Plan 1 sub-plan 5) expose both primitives with accurate types.

### Implementation tasks

1. Implement the two primitive functions in `postretro/src/scripting/primitives/emitter.rs`.
2. Register them via the Plan 1 builder in `scripting::primitives::register_all`.
3. Integration test in QuickJS: spawn an emitter, toggle active, assert on spawn count.
4. Luau mirror of the same test.

---

## Sub-plan 6 — TypeScript reference vocabulary

**Scope:** Two hand-authored TypeScript files under `assets/scripts/sdk/`. No primitives called from vocabulary beyond what Plans 1, 2, 3 register.

### Files

#### `assets/scripts/sdk/emitter.ts` — `emitter()` component constructor

- Signature: `emitter(props: EmitterProps) -> ComponentDescriptor`.
- Validates props synchronously:
  - `archetype` non-empty string.
  - `lifetime > 0`.
  - `rate >= 0`.
  - `spread >= 0`.
  - `drag >= 0`.
  - `initial_velocity` is `[number, number, number]`.
  - `color` is `[number, number, number]`, each channel in `[0, 1]`.
  - `size_over_lifetime` / `opacity_over_lifetime` are nonempty number arrays if present.
  - `sprite` is a nonempty string.
- Applies defaults for omitted fields:
  - `gravity_scale: -1.0` (rises — smoke default; fire embers use positive explicitly).
  - `drag: 0.5`.
  - `spread: 0.2` (radians, ~11°).
  - `opacity_over_lifetime: [1.0, 1.0, 0.8, 0.0]` (hold, soft fade at the end).
  - `size_over_lifetime: [1.0]` (constant — no grow/shrink).
  - `color: [1.0, 1.0, 1.0]`.
  - `rate: 0.0` and `burst: undefined` both default to "no continuous emission"; the caller sets at least one.
  - `active: true`.
- Returns a `ComponentDescriptor` tagged `{ kind: "emitter", value: EmitterComponent }`.
- Every field carries JSDoc. JSDoc powers editor tooltips.

Example target shape (the campfire referenced in the planning decisions):

```typescript
// Proposed design — ships as assets/scripts/sdk/emitter.ts
defineEntity({
  components: [
    emitter({
      archetype: "flame_sprite",
      rate: 12,
      spread: 0.3,
      lifetime: 1.2,
      initial_velocity: [0, 1.5, 0],
      color: [1.0, 0.6, 0.2],
      sprite: "flame",
    }),
    light({
      type: "point",
      intensity: 80,
      color: [1.0, 0.5, 0.2],
      animation: {
        period: 0.8,
        brightness: [0.7, 1.0, 0.6, 0.9],
      },
    }),
  ],
})
```

#### `assets/scripts/sdk/particles.ts` — pre-built emitter configurations

Pure functions returning `EmitterComponent` descriptors. No primitive calls, no side effects. Modders use them directly or read them as examples.

- `smokeEmitter(props?: Partial<EmitterProps>)` — soft, rising smoke. Defaults: `rate: 6`, `lifetime: 3.0`, `initial_velocity: [0, 0.8, 0]`, `spread: 0.4`, `gravity_scale: -0.2` (slight rise), `drag: 0.8`, `size_over_lifetime: [0.3, 1.5]` (grows), `opacity_over_lifetime: [0.0, 0.8, 0.6, 0.0]` (fade in, hold, fade out), `color: [0.8, 0.8, 0.8]`, `sprite: "smoke"`.
- `sparkEmitter(props?: Partial<EmitterProps>)` — fast, falling sparks. Defaults: `rate: 0`, `burst: 12` (trigger by script), `lifetime: 0.6`, `initial_velocity: [0, 3.0, 0]`, `spread: 0.6`, `gravity_scale: 1.0`, `drag: 0.2`, `size_over_lifetime: [1.0, 0.3]` (shrink), `opacity_over_lifetime: [1.0, 1.0, 0.0]`, `color: [1.0, 0.7, 0.2]`, `sprite: "spark"`.
- `dustEmitter(props?: Partial<EmitterProps>)` — slow, drifting dust. Defaults: `rate: 2`, `lifetime: 5.0`, `initial_velocity: [0, 0.1, 0]`, `spread: 1.2`, `gravity_scale: -0.05`, `drag: 1.0`, `size_over_lifetime: [0.5, 1.0]`, `opacity_over_lifetime: [0.0, 0.3, 0.0]`, `color: [0.6, 0.55, 0.45]`, `sprite: "dust"`.

Each function documents its intent and the rationale behind its default shape — a modder reading the source learns the property format by example.

### Acceptance criteria

- [ ] `emitter()` called with the "campfire" shape above returns a `ComponentDescriptor` that the engine accepts via `defineEntity` + `spawn_entity`, producing visibly spawning particles at the emitter's position.
- [ ] `emitter()` called with invalid props throws with a clear message naming the invalid field.
- [ ] `smokeEmitter()` called with no args returns a fully populated `EmitterComponent` descriptor. Called with `{ rate: 20 }` overrides rate but preserves other defaults.
- [ ] `sparkEmitter({ burst: 40 })` triggered via `set_component` on a runtime entity produces a 40-particle burst in one frame.
- [ ] A test map's definition file authored in TypeScript composes `emitter()` inside `defineEntity` alongside `light()` on the same entity — both components appear on the resulting entity; both render.
- [ ] Hot reload (Plan 1 sub-plan 7) works on these files in dev mode.

### Implementation tasks

1. Author `emitter.ts` with full prop validation, defaults, and JSDoc.
2. Author `particles.ts` with the three helpers plus JSDoc describing each preset's visual intent.
3. Place under `assets/scripts/sdk/` — the SDK-source directory Plan 2 established.
4. Update the SDK README (created in Plan 2 sub-plan 6) to list these as the second vocabulary modules.
5. Integration test: compile the TS via Plan 1's detected compiler, evaluate the campfire definition, spawn the entity at runtime, tick the sim for 0.5 s, assert the child particle count is non-zero and no component validation errors surfaced.

---

## Sub-plan 7 — Luau reference vocabulary (follow-up within this plan)

**Scope:** `emitter.luau` and `particles.luau` ports of sub-plan 6. No new engine work.

### Description

Translate the TypeScript API to Luau. Mechanical — Luau types, table literals, `nil` where TypeScript has `undefined`. The purpose is to validate that the dual-runtime story still holds for the second feature component family.

### Acceptance criteria

- [ ] `emitter.luau` and `particles.luau` exist under `assets/scripts/sdk/`.
- [ ] A Luau definition file using `emitter({ ... })` produces the same runtime behavior as the TypeScript equivalent — verified by a round-trip test loading both, spawning entities, ticking the sim, and comparing live particle counts after a fixed interval within a small tolerance (RNG streams differ by seed but rate semantics match).
- [ ] Translation required zero changes to Rust engine code. If any Rust change was required, treat as a bug in Plan 1's cross-language contract and record a follow-up.
- [ ] `luau-lsp` configured against `sdk/types/postretro.d.luau` autocompletes `emitter` and its props.

### Implementation tasks

1. Translate `emitter.ts` to `emitter.luau`. Match validation rules exactly.
2. Translate `particles.ts` to `particles.luau`.
3. Add a Luau integration test mirroring sub-plan 6's TypeScript test.

---

## Sub-plan 8 — `env_smoke_emitter` FGD → `EmitterComponent` level-load wiring

**Scope:** Rust level-loader only. No scripting runtime changes, no new primitives, no PRL format additions. Removes the `--demo-smoke` hack.

### Description

Map entities with `classname = env_smoke_emitter` have historically been handled by a dedicated hardcoded Rust path (the `--demo-smoke` argument). After sub-plans 1 and 4 land, the scripted emitter system provides a proper host for these entities. This sub-plan wires FGD property values directly to `EmitterComponent` fields at level load — no script file required.

When the level loader encounters a map entity with `classname = env_smoke_emitter`, it reads the entity's KV properties, constructs an `EmitterComponent` using `EmitterComponent::from_fgd_props`, spawns an ECS entity via `EntityRegistry::spawn` at the entity's world-space origin, and writes the component via `set_component`. From that point, the emitter bridge (sub-plan 3) and particle sim (sub-plan 2) own it exactly as if a script had spawned it.

This is the **narrow version**: the loader has hardcoded knowledge of `env_smoke_emitter`'s FGD schema. General map-entity → script routing (letting modders bind arbitrary FGD classnames to definition scripts) is out of scope.

### FGD property mapping

| FGD key | Type | `EmitterComponent` field | Default |
|---|---|---|---|
| `origin` | `point` (x y z) | `Transform.position` at spawn | *(required — entity origin)* |
| `particle_rate` | `float` | `rate` | `6.0` |
| `particle_lifetime` | `float` | `lifetime` | `3.0` |
| `particle_spread` | `float` | `spread` (half-angle, radians) | `0.4` |
| `gravity_scale` | `float` | `gravity_scale` | `-0.2` |
| `drag` | `float` | `drag` | `0.8` |
| `sprite` | `string` | `sprite` | `"smoke"` |
| `active` | `bool` | `active` | `true` |

Defaults match `smokeEmitter()` from sub-plan 6. Fields absent from the map entity use their default. Unrecognized keys are ignored at debug-log level.

`initial_velocity` is hardcoded to `[0.0, 0.8, 0.0]` (upward, matching `smokeEmitter()` default). `size_over_lifetime` and `opacity_over_lifetime` use the `smokeEmitter()` defaults. No FGD keys for curve channels in this plan.

### Level loader integration

The map-entity sweep in `postretro/src/level/` gains one new match arm. The `--demo-smoke` flag and its `SmokeEmitter`-backed spawn are removed in the same pass. Before deleting anything, `grep` for all uses of `--demo-smoke` and `SmokeEmitter` to confirm no other consumers remain.

```rust
// Sketch — lives in postretro/src/level/
"env_smoke_emitter" => {
    let comp = EmitterComponent::from_fgd_props(&entity.props);
    let id = registry.spawn(Transform { position: entity.origin, ..Default::default() });
    registry.set_component(id, ComponentValue::Emitter(comp));
}
```

`from_fgd_props` parses each KV entry with `str::parse::<f32>()` / `str::parse::<bool>()`. A parse failure logs a warning naming the key and entity origin, then falls back to the default for that field rather than failing the load.

### Acceptance criteria

- [ ] A map with one or more `env_smoke_emitter` entities, loaded without `--demo-smoke`, produces visible smoke particles at each entity's world-space origin.
- [ ] A map with no `env_smoke_emitter` entities loads cleanly with zero warnings from this path.
- [ ] All property keys in the mapping table are read and applied when present; absent keys use the documented defaults.
- [ ] An invalid property value (e.g. `particle_lifetime = "bad"`) logs a warning naming the key and origin, falls back to the default, and load continues.
- [ ] `--demo-smoke` flag and all associated code are removed; `cargo build` produces no dead-code warnings.
- [ ] Existing scripted emitter behavior (sub-plans 3, 4) is unchanged — no regression.
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Add `EmitterComponent::from_fgd_props(props: &HashMap<String, String>) -> EmitterComponent` to `postretro/src/scripting/components/emitter.rs`.
2. In the map-entity sweep in `postretro/src/level/`, add the `env_smoke_emitter` arm.
3. Remove `--demo-smoke`, its argument parsing, the demo spawn site, and `SmokeEmitter`-backed code if no other consumer remains. `grep` before deleting.
4. Integration test: load a minimal level with one `env_smoke_emitter` entity, tick one frame, assert non-zero live particle count at the expected world position (within a small epsilon).

---

## When this plan ships

Durable knowledge migrates to `context/lib/`:

- **`context/lib/scripting.md`** (the file Plan 1 creates, extended by Plan 2) gains an "Emitter entity and particles" section covering: the emitter-vs-light composition rule (both are components, neither is a subtype), the "scripts configure, Rust simulates" invariant, the particle-as-lightweight-entity contract, and the per-emitter `MAX_SPRITES` cap. Include the "no per-particle script callbacks" invariant as a top-level bullet.
- **`context/lib/rendering_pipeline.md`** §7.4 (billboard pass) is updated to reflect: (a) the pass's instance buffer is fed by *two* sources — legacy `SmokeEmitter` and scripted particles — sharing the same `SPRITE_INSTANCE_SIZE` layout and pipeline, (b) additive blending is the only supported blend mode for scripted emitters in this plan.
- **`context/lib/entity_model.md`** gets a short "Particles" paragraph: each live particle is an ECS entity; the emitter bridge spawns/despawns them; scripts do not observe individual particles.

The plan document moves `drafts/` → `ready/` → `in-progress/` → `done/` and stays frozen.

---

## Non-goals (consolidated)

- Per-particle `on_tick` script callbacks. The Rust simulation loop is authoritative.
- Particle-against-geometry collision. Deferred until Rapier/parry3d lands.
- Sub-emitters (particles that spawn particles).
- Mesh particles. Billboard sprites only.
- Particle sorting for correct alpha blending. Additive is the only supported blend mode in this plan; non-additive alpha-blend is future work and will require a sort pass.
- GPU-driven simulation. CPU simulation is the starting point.
- Per-particle tint / color application in the GPU pipeline. The `SpriteVisual.tint` field exists on the CPU side but is not packed into `SpriteInstance`; extending the instance layout is a follow-up.
- Runtime sprite-sheet loading. All sprite collections must be resolvable at level-load from the definition sweep.
- General map-entity → script routing. Sub-plan 8 hardcodes only `env_smoke_emitter`; routing arbitrary FGD classnames to definition scripts is a future plan.
- Production bytecode caching (Plan 1 non-goal, restated).
- On-disk PRL format additions.
