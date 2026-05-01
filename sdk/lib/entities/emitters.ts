// BillboardEmitter component constructors and types for entity authoring.
// See: context/lib/scripting.md §11

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
  velocity?: [number, number, number];
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
 * `velocity`, `sprite`. Other fields fall back to documented
 * defaults inside `emitter()`.
 */
export type EmitterProps = {
  rate?: number;
  burst?: number;
  spread?: number;
  lifetime: number;
  velocity: [number, number, number];
  buoyancy?: number;
  drag?: number;
  size_over_lifetime?: number[];
  opacity_over_lifetime?: number[];
  color?: [number, number, number];
  sprite: string;
  spin_rate?: number;
};

/**
 * Flat `ComponentValue` shape produced by component constructors.
 * `kind` is the snake_case wire tag; sibling fields carry the component
 * payload directly (no `value` wrapper).
 */
export type ComponentDescriptor = { kind: string; [field: string]: unknown };

/**
 * Build a `billboard_emitter` component descriptor from `props`. Validates
 * synchronously and throws `Error` naming the offending field on failure.
 * Fills defaults for omitted optional fields. `rate = 0` with no `burst`
 * is a valid dormant emitter configuration, not a validation error.
 */
export function emitter(props: EmitterProps): ComponentDescriptor {
  validateEmitterProps(props);

  return {
    kind: "billboard_emitter",
    rate: props.rate ?? 0.0,
    burst: props.burst,
    spread: props.spread ?? 0.2,
    lifetime: props.lifetime,
    velocity: props.velocity,
    buoyancy: props.buoyancy ?? 0.5,
    drag: props.drag ?? 0.5,
    size_over_lifetime: props.size_over_lifetime ?? [1.0],
    opacity_over_lifetime: props.opacity_over_lifetime ?? [1.0, 1.0, 0.8, 0.0],
    color: props.color ?? [1.0, 1.0, 1.0],
    sprite: props.sprite,
    spin_rate: props.spin_rate ?? 0.0,
  };
}

function validateEmitterProps(props: EmitterProps): void {
  if (props === null || typeof props !== "object") {
    throw new Error("emitter: props must be an object");
  }

  // Validate sprite first so a missing sprite isn't masked by the lifetime check.
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

  validateVec3(props.velocity, "velocity");

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

/**
 * Soft, slowly-rising smoke. Use for chimneys, smoldering rubble,
 * incense pots — anything that needs a gentle vertical plume.
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
    velocity: [0, 0.5, 0],
    color: [1.0, 1.0, 1.0],
  };
  return emitter({ ...defaults, ...overrides });
}

/**
 * Fast, falling, tumbling sparks. One-shot burst per trigger; `rate: 0`
 * so the emitter is idle until a burst is requested.
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
    velocity: [0, 2.0, 0],
    color: [1.0, 0.8, 0.3],
  };
  return emitter({ ...defaults, ...overrides });
}

/**
 * Slow drifting dust motes. Use for shafts of light, disturbed floors,
 * and ambient atmospheric particles.
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
    velocity: [0, 0.1, 0],
    color: [0.8, 0.7, 0.6],
  };
  return emitter({ ...defaults, ...overrides });
}
