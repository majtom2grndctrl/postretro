// Light entity vocabulary: the `LightEntity` handle returned by
// `world.query({ component: "light" })`, plus the pure animation
// constructors (`flicker`, `pulse`, `colorShift`, `sweep`) that
// produce `LightAnimation` values for `light.setAnimation`.
//
// The handle wraps the generated `LightEntity` snapshot with
// convenience methods (`setAnimation`, `setIntensity`, `setColor`)
// that call the underlying scripting primitives. `wrapLightEntity`
// is exported so `world.ts` can construct handles for results
// returned from `worldQuery`.
//
// ---------------------------------------------------------------------------
// Canonical modder example — rolling pulse down a hallway, 10s loop.
//
// Map authors tag the hallway lights `"hallway_wave"` in TrenchBroom. The
// behavior script below queries them at level load, sorts along the x
// axis, and staggers `phase` across the row so the pulse travels.
//
// ```typescript
// import { registerHandler } from "postretro";
// import { world } from "./world";
// import type { LightAnimation } from "postretro";
//
// registerHandler("levelLoad", () => {
//   const lights = world
//     .query({ component: "light", tag: "hallway_wave" })
//     .sort((a, b) => a.transform.position.x - b.transform.position.x);
//
//   const pulse: LightAnimation = {
//     periodMs: 10000,
//     brightness: [
//       0.1, 0.1, 0.1, 0.1, 0.1,
//       0.3, 0.8, 1.0, 0.8, 0.3,
//       0.1, 0.1, 0.1, 0.1, 0.1,
//       0.1, 0.1, 0.1, 0.1, 0.1,
//     ],
//   };
//
//   lights.forEach((light, i) => {
//     light.setAnimation({ ...pulse, phase: i / lights.length });
//   });
// });
// ```
// ---------------------------------------------------------------------------

import {
  getComponent,
  setLightAnimation,
} from "postretro";
import type {
  EntityId,
  LightAnimation,
  LightComponent,
  LightEntity as GeneratedLightEntity,
  Vec3,
} from "postretro";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/** Easing family used by `setIntensity` / `setColor` transitions. */
export type EasingCurve = "linear" | "easeIn" | "easeOut" | "easeInOut";

/**
 * Typed handle returned by `world.query` for a light entity. Wraps the
 * generated `LightEntity` snapshot with convenience methods that call the
 * underlying scripting primitives.
 */
export interface LightEntity extends GeneratedLightEntity {
  /**
   * Replace the light's animation. Pass `null` to clear it. Last call
   * wins — lights are always interruptible.
   */
  setAnimation(anim: LightAnimation | null): void;

  /**
   * Transition the light's intensity to `target` over `transitionMs`
   * milliseconds (default `0`, which applies the target instantly).
   *
   * Re-reads the live `LightComponent.intensity` from the registry at
   * call time rather than the handle's query-time snapshot, so chained
   * transitions compose correctly. `easing` defaults to `"easeInOut"`
   * when `transitionMs > 0`; it is ignored for instant transitions.
   *
   * Internally constructs a one-cycle `LightAnimation` (`playCount: 1`)
   * and hands it to `setAnimation`.
   */
  setIntensity(
    target: number,
    transitionMs?: number,
    easing?: EasingCurve,
  ): void;

  /**
   * Transition the light's RGB color to `target` over `transitionMs`
   * milliseconds (default `0`). Same live-read / one-cycle pattern as
   * `setIntensity`.
   *
   * Throws a descriptive `Error` on non-dynamic lights (baked lights
   * cannot have their color animated because their indirect SH was
   * baked with the compile-time color).
   */
  setColor(
    target: [number, number, number],
    transitionMs?: number,
    easing?: EasingCurve,
  ): void;
}

// ---------------------------------------------------------------------------
// Handle construction
// ---------------------------------------------------------------------------

export function wrapLightEntity(snapshot: GeneratedLightEntity): LightEntity {
  const id: EntityId = snapshot.id;

  const handle: LightEntity = {
    ...snapshot,

    setAnimation(anim: LightAnimation | null): void {
      if (anim && anim.color && !snapshot.isDynamic) {
        throw new Error(
          `setAnimation: light ${idDebug(id)} is not dynamic; color animation is only valid on dynamic lights`,
        );
      }
      setLightAnimation(id, anim);
    },

    setIntensity(
      target: number,
      transitionMs: number = 0,
      easing?: EasingCurve,
    ): void {
      const live = readLightComponent(id);
      const anim = buildIntensityAnimation(
        live.intensity,
        target,
        transitionMs,
        easing,
      );
      setLightAnimation(id, anim);
    },

    setColor(
      target: [number, number, number],
      transitionMs: number = 0,
      easing?: EasingCurve,
    ): void {
      if (!snapshot.isDynamic) {
        throw new Error(
          `setColor: light ${idDebug(id)} is not dynamic; color can only be animated on dynamic lights`,
        );
      }
      const live = readLightComponent(id);
      const anim = buildColorAnimation(
        live.color,
        { x: target[0], y: target[1], z: target[2] },
        transitionMs,
        easing,
      );
      setLightAnimation(id, anim);
    },
  };

  return handle;
}

function readLightComponent(id: EntityId): LightComponent {
  const c = getComponent(id, "Light");
  if (c.kind !== "Light") {
    throw new Error(
      `expected Light component on entity ${idDebug(id)}, got ${c.kind}`,
    );
  }
  return c.value;
}

function idDebug(id: EntityId): string {
  // `EntityId` is a branded number — print the underlying value for
  // error messages without leaking the brand in the type.
  return String(id as unknown as number);
}

// ---------------------------------------------------------------------------
// Easing + one-cycle animation builders
// ---------------------------------------------------------------------------

// 8-sample resolution for transitions. Fine enough for a smooth ease
// without bloating the primitive call payload.
const EASE_SAMPLES = 8;

function resolveEasing(
  transitionMs: number,
  easing: EasingCurve | undefined,
): EasingCurve {
  if (transitionMs <= 0) {
    // Irrelevant for step transitions, but pick a stable default.
    return "linear";
  }
  return easing ?? "easeInOut";
}

function easeAt(curve: EasingCurve, t: number): number {
  // t is the normalized sample position in [0, 1].
  switch (curve) {
    case "linear":
      return t;
    case "easeIn":
      return t * t;
    case "easeOut":
      return 1 - (1 - t) * (1 - t);
    case "easeInOut":
      // Smoothstep-style ease-in-out.
      return t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
  }
}

function buildIntensityAnimation(
  from: number,
  to: number,
  transitionMs: number,
  easing: EasingCurve | undefined,
): LightAnimation {
  if (transitionMs <= 0) {
    return {
      periodMs: 1,
      phase: null,
      playCount: 1,
      brightness: [to],
      color: null,
      direction: null,
    };
  }
  const curve = resolveEasing(transitionMs, easing);
  const brightness: number[] = new Array(EASE_SAMPLES);
  for (let i = 0; i < EASE_SAMPLES; i++) {
    const t = i / (EASE_SAMPLES - 1);
    brightness[i] = from + (to - from) * easeAt(curve, t);
  }
  return {
    periodMs: transitionMs,
    phase: null,
    playCount: 1,
    brightness,
    color: null,
    direction: null,
  };
}

function buildColorAnimation(
  from: Vec3,
  to: Vec3,
  transitionMs: number,
  easing: EasingCurve | undefined,
): LightAnimation {
  if (transitionMs <= 0) {
    return {
      periodMs: 1,
      phase: null,
      playCount: 1,
      brightness: null,
      color: [{ x: to.x, y: to.y, z: to.z }],
      direction: null,
    };
  }
  const curve = resolveEasing(transitionMs, easing);
  const color: Vec3[] = new Array(EASE_SAMPLES);
  for (let i = 0; i < EASE_SAMPLES; i++) {
    const t = i / (EASE_SAMPLES - 1);
    const k = easeAt(curve, t);
    color[i] = {
      x: from.x + (to.x - from.x) * k,
      y: from.y + (to.y - from.y) * k,
      z: from.z + (to.z - from.z) * k,
    };
  }
  return {
    periodMs: transitionMs,
    phase: null,
    playCount: 1,
    brightness: null,
    color,
    direction: null,
  };
}

// ---------------------------------------------------------------------------
// Brightness curves
// ---------------------------------------------------------------------------

// Fixed 8-step irregular brightness pattern in [0, 1]. Reused for every
// `flicker` call so the curve is deterministic across reloads and the
// pattern is visually recognizable without importing a PRNG.
const FLICKER_PATTERN: ReadonlyArray<number> = [
  0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65,
];

/**
 * Returns an 8-sample irregular brightness curve flickering between
 * `minBrightness` and `maxBrightness`.
 *
 * `rate` is the flicker frequency in Hz — `periodMs` is `1000 / rate`.
 * Callers set `phase` at the call site if they need to stagger multiple
 * flickering lights.
 */
export function flicker(
  minBrightness: number,
  maxBrightness: number,
  rate: number,
): LightAnimation {
  const lo = Math.min(minBrightness, maxBrightness);
  const hi = Math.max(minBrightness, maxBrightness);
  const span = hi - lo;
  const brightness = FLICKER_PATTERN.map((t) => lo + t * span);
  return {
    periodMs: 1000 / rate,
    phase: null,
    playCount: null,
    brightness,
    color: null,
    direction: null,
  };
}

/**
 * Returns a 16-sample sine-approximating brightness curve oscillating
 * between `minBrightness` and `maxBrightness` over one full `periodMs`.
 *
 * Sample `i` is evaluated at `i / 16` of the period.
 */
export function pulse(
  minBrightness: number,
  maxBrightness: number,
  periodMs: number,
): LightAnimation {
  const SAMPLES = 16;
  const lo = Math.min(minBrightness, maxBrightness);
  const hi = Math.max(minBrightness, maxBrightness);
  const mid = (lo + hi) * 0.5;
  const amp = (hi - lo) * 0.5;
  const brightness: number[] = new Array(SAMPLES);
  for (let i = 0; i < SAMPLES; i++) {
    const theta = (i / SAMPLES) * Math.PI * 2;
    brightness[i] = mid + amp * Math.sin(theta);
  }
  return {
    periodMs,
    phase: null,
    playCount: null,
    brightness,
    color: null,
    direction: null,
  };
}

// ---------------------------------------------------------------------------
// Color / direction curves
// ---------------------------------------------------------------------------

/**
 * Cycles uniformly through the given RGB `colors` over `periodMs`.
 *
 * Only valid on dynamic lights; the engine rejects color animation on
 * baked lights (the `wrapLightEntity` handle wrapper in this file surfaces that with a clearer error).
 */
export function colorShift(
  colors: [number, number, number][],
  periodMs: number,
): LightAnimation {
  const color: Vec3[] = colors.map(([r, g, b]) => ({ x: r, y: g, z: b }));
  return {
    periodMs,
    phase: null,
    playCount: null,
    brightness: null,
    color,
    direction: null,
  };
}

/**
 * Sweeps the light's `direction` channel through `directions` over
 * `periodMs`. Direction samples are normalized defensively even though
 * the primitive also normalizes non-unit inputs — zero-length samples
 * still error at the primitive seam.
 */
export function sweep(
  directions: [number, number, number][],
  periodMs: number,
): LightAnimation {
  const direction: Vec3[] = directions.map(([x, y, z]) => {
    const len = Math.sqrt(x * x + y * y + z * z);
    if (len > 0) {
      return { x: x / len, y: y / len, z: z / len };
    }
    // Pass zero-length through untouched; the primitive will reject it
    // with `InvalidArgument` and a specific error message.
    return { x, y, z };
  });
  return {
    periodMs,
    phase: null,
    playCount: null,
    brightness: null,
    color: null,
    direction,
  };
}
