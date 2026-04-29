// Reference vocabulary for authoring `BillboardEmitter` components in
// TypeScript.
//
// Exports:
//   - `BillboardEmitter` — TypeScript-only entity-shape type for IDE
//     completions on named instances and per-instance overrides. No
//     runtime value; the engine handles `classname "billboard_emitter"`
//     natively (see plan-3 sub-plan 6).
//   - `SpinAnimation` — tween shape consumed by the `setSpinRate`
//     reaction primitive.
//   - `ComponentDescriptor` — the shape every component constructor in
//     the SDK returns (`{ kind, value }`); the engine deserializes it
//     into a typed component when a preset is spawned.
//   - `emitter(props)` — pure component constructor. Validates props
//     synchronously, fills defaults, and returns a `ComponentDescriptor`
//     with `kind: "billboard_emitter"`.
//   - `smokeEmitter`, `sparkEmitter`, `dustEmitter` — preset wrappers
//     around `emitter()` with curated defaults for the three shipped
//     visual archetypes. Accept `Partial<EmitterProps>` overrides.
//
// Naming intent: `BillboardEmitter` reserves `ParticleEmitter` for
// future mesh-particle work; the rendering primitive is in the type
// name so renames stay deliberate.
//
// See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 7

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * Tween shape consumed by `setSpinRate`. Mirrors the Rust `SpinAnimation`
 * storage struct. `rate_curve` must be nonempty when supplied at the
 * primitive seam; this type is structural and does not enforce that here.
 */
export type SpinAnimation = {
  duration: number;
  rate_curve: number[];
};

/**
 * Shape of a `BillboardEmitterComponent` value as produced by `emitter()`.
 * Authors use this type for named instances or per-instance overrides:
 *
 * ```typescript
 * import { type BillboardEmitter } from "postretro/entities/emitters";
 * export const exhaustPort: BillboardEmitter = { rate: 50, buoyancy: 0.0 };
 * ```
 *
 * Type-only export — no runtime value. The engine recognizes
 * `classname "billboard_emitter"` natively.
 */
export type BillboardEmitter = {
  rate?: number;
  burst?: number;
  spread?: number;
  lifetime?: number;
  initial_velocity?: [number, number, number];
  buoyancy?: number;
  drag?: number;
  size_over_lifetime?: number[];
  opacity_over_lifetime?: number[];
  color?: [number, number, number];
  sprite?: string;
  spin_rate?: number;
};

/**
 * Input shape for `emitter()`. Required fields: `lifetime`,
 * `initial_velocity`, `sprite`. Other fields fall back to documented
 * defaults inside `emitter()`.
 */
export type EmitterProps = {
  rate?: number;
  burst?: number;
  spread?: number;
  lifetime: number;
  initial_velocity: [number, number, number];
  buoyancy?: number;
  drag?: number;
  size_over_lifetime?: number[];
  opacity_over_lifetime?: number[];
  color?: [number, number, number];
  sprite: string;
  spin_rate?: number;
};

/**
 * Return shape from every component constructor in the SDK. The engine
 * dispatches on `kind` and deserializes `value` into the matching
 * Rust component struct.
 */
export type ComponentDescriptor = { kind: string; value: unknown };

// ---------------------------------------------------------------------------
// emitter() — validating component constructor
// ---------------------------------------------------------------------------

/**
 * Build a `BillboardEmitter` component descriptor from `props`. Validates
 * synchronously and throws `Error` naming the offending field on failure.
 * Fills defaults for omitted optional fields:
 *
 * - `rate = 0.0` (dormant continuous emission)
 * - `burst = undefined` (no one-shot burst)
 * - `spread = 0.2` rad (~11° cone)
 * - `buoyancy = 0.5` (rises gently — smoke default)
 * - `drag = 0.5`
 * - `opacity_over_lifetime = [1.0, 1.0, 0.8, 0.0]` (full → fade-out)
 * - `size_over_lifetime = [1.0]` (constant)
 * - `color = [1.0, 1.0, 1.0]` (white tint)
 * - `spin_rate = 0.0` (no rotation)
 *
 * `rate = 0` and `burst = undefined` together describe a dormant emitter
 * — that is a valid configuration, not a validation error.
 */
export function emitter(props: EmitterProps): ComponentDescriptor {
  validateEmitterProps(props);

  const value = {
    rate: props.rate ?? 0.0,
    burst: props.burst,
    spread: props.spread ?? 0.2,
    lifetime: props.lifetime,
    initial_velocity: props.initial_velocity,
    buoyancy: props.buoyancy ?? 0.5,
    drag: props.drag ?? 0.5,
    size_over_lifetime: props.size_over_lifetime ?? [1.0],
    opacity_over_lifetime: props.opacity_over_lifetime ?? [1.0, 1.0, 0.8, 0.0],
    color: props.color ?? [1.0, 1.0, 1.0],
    sprite: props.sprite,
    spin_rate: props.spin_rate ?? 0.0,
  };

  return { kind: "billboard_emitter", value };
}

function validateEmitterProps(props: EmitterProps): void {
  if (props === null || typeof props !== "object") {
    throw new Error("emitter: props must be an object");
  }

  // Required: sprite (validated first so a missing sprite isn't masked
  // by the lifetime check on a default-{} call).
  if (typeof props.sprite !== "string" || props.sprite.length === 0) {
    throw new Error("emitter: `sprite` must be a nonempty string");
  }

  if (typeof props.lifetime !== "number" || !Number.isFinite(props.lifetime) || props.lifetime <= 0) {
    throw new Error("emitter: `lifetime` must be a number > 0");
  }

  if (props.rate !== undefined) {
    if (typeof props.rate !== "number" || !Number.isFinite(props.rate) || props.rate < 0) {
      throw new Error("emitter: `rate` must be a number >= 0");
    }
  }

  if (props.spread !== undefined) {
    if (typeof props.spread !== "number" || !Number.isFinite(props.spread) || props.spread < 0) {
      throw new Error("emitter: `spread` must be a number >= 0");
    }
  }

  if (props.drag !== undefined) {
    if (typeof props.drag !== "number" || !Number.isFinite(props.drag) || props.drag < 0) {
      throw new Error("emitter: `drag` must be a number >= 0");
    }
  }

  if (props.buoyancy !== undefined) {
    if (typeof props.buoyancy !== "number" || !Number.isFinite(props.buoyancy)) {
      throw new Error("emitter: `buoyancy` must be a finite number");
    }
  }

  if (props.burst !== undefined) {
    if (
      typeof props.burst !== "number" ||
      !Number.isFinite(props.burst) ||
      props.burst < 0 ||
      Math.floor(props.burst) !== props.burst
    ) {
      throw new Error("emitter: `burst` must be a non-negative integer");
    }
  }

  if (props.spin_rate !== undefined) {
    if (typeof props.spin_rate !== "number" || !Number.isFinite(props.spin_rate)) {
      throw new Error("emitter: `spin_rate` must be a finite number");
    }
  }

  validateVec3(props.initial_velocity, "initial_velocity");

  if (props.color !== undefined) {
    validateVec3(props.color, "color");
    for (let i = 0; i < 3; i++) {
      const c = props.color[i];
      if (c < 0 || c > 1) {
        throw new Error(`emitter: \`color\` element ${i} (${c}) is outside [0, 1]`);
      }
    }
  }

  if (props.size_over_lifetime !== undefined) {
    validateCurve(props.size_over_lifetime, "size_over_lifetime");
  }

  if (props.opacity_over_lifetime !== undefined) {
    validateCurve(props.opacity_over_lifetime, "opacity_over_lifetime");
  }
}

function validateVec3(v: unknown, field: string): asserts v is [number, number, number] {
  if (!Array.isArray(v) || v.length !== 3) {
    throw new Error(`emitter: \`${field}\` must be a 3-element [number, number, number]`);
  }
  for (let i = 0; i < 3; i++) {
    const n = v[i];
    if (typeof n !== "number" || !Number.isFinite(n)) {
      throw new Error(`emitter: \`${field}\` element ${i} is not a finite number`);
    }
  }
}

function validateCurve(curve: unknown, field: string): void {
  if (!Array.isArray(curve) || curve.length === 0) {
    throw new Error(`emitter: \`${field}\` must be a nonempty number array`);
  }
  for (let i = 0; i < curve.length; i++) {
    const n = curve[i];
    if (typeof n !== "number" || !Number.isFinite(n)) {
      throw new Error(`emitter: \`${field}\` element ${i} is not a finite number`);
    }
  }
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/**
 * Soft, slowly-rising smoke. Use for chimneys, smoldering rubble,
 * incense pots — anything that needs a gentle vertical plume.
 *
 * Defaults: continuous `rate: 6`/sec, `lifetime: 3s`, slight positive
 * `buoyancy: 0.2`, growing-while-fading curves, no spin. Sprite
 * collection is `"smoke"`.
 */
export function smokeEmitter(overrides: Partial<EmitterProps> = {}): ComponentDescriptor {
  const defaults: EmitterProps = {
    rate: 6,
    lifetime: 3.0,
    buoyancy: 0.2,
    drag: 0.5,
    spread: 0.2,
    size_over_lifetime: [0.3, 1.5],
    opacity_over_lifetime: [0.0, 0.8, 0.6, 0.0],
    sprite: "smoke",
    spin_rate: 0.0,
    initial_velocity: [0, 0.5, 0],
    color: [1.0, 1.0, 1.0],
  };
  return emitter({ ...defaults, ...overrides });
}

/**
 * Fast, falling, tumbling sparks. One-shot `burst: 12` per trigger;
 * `rate: 0` so the emitter is idle until a burst is requested.
 *
 * Defaults: `lifetime: 0.6s`, gravity-pulled (`buoyancy: -1.0`), wide
 * `spread: 0.5`, shrinking-while-fading curves, `spin_rate: 1.5` rad/s
 * for a visible tumble. Warm orange tint. Sprite collection is `"spark"`.
 */
export function sparkEmitter(overrides: Partial<EmitterProps> = {}): ComponentDescriptor {
  const defaults: EmitterProps = {
    rate: 0,
    burst: 12,
    lifetime: 0.6,
    buoyancy: -1.0,
    drag: 0.1,
    spread: 0.5,
    size_over_lifetime: [1.0, 0.3],
    opacity_over_lifetime: [1.0, 1.0, 0.0],
    sprite: "spark",
    spin_rate: 1.5,
    initial_velocity: [0, 2.0, 0],
    color: [1.0, 0.8, 0.3],
  };
  return emitter({ ...defaults, ...overrides });
}

/**
 * Slow drifting dust motes. Use for shafts of light, disturbed floors,
 * and ambient atmospheric particles.
 *
 * Defaults: continuous `rate: 2`/sec, long `lifetime: 5s`, near-neutral
 * `buoyancy: 0.05` with high `drag: 1.0` so motes settle quickly,
 * subtle opacity bell, no spin. Warm-grey tint. Sprite collection is
 * `"dust"`.
 */
export function dustEmitter(overrides: Partial<EmitterProps> = {}): ComponentDescriptor {
  const defaults: EmitterProps = {
    rate: 2,
    lifetime: 5.0,
    buoyancy: 0.05,
    drag: 1.0,
    spread: 0.3,
    size_over_lifetime: [0.5, 1.0],
    opacity_over_lifetime: [0.0, 0.3, 0.0],
    sprite: "dust",
    spin_rate: 0.0,
    initial_velocity: [0, 0.1, 0],
    color: [0.8, 0.7, 0.6],
  };
  return emitter({ ...defaults, ...overrides });
}
