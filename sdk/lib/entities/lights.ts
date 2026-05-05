// Light entity handle and pure animation constructors (flicker, pulse, colorShift, sweep).
// Governed by context/lib/entity_model.md.

import { setLightAnimation } from "postretro";
import type {
  EntityId,
  LightAnimation,
  LightEntity as GeneratedLightEntity,
  Vec3,
} from "postretro";

/**
 * Typed handle returned by `world.query` for a light entity. Wraps the
 * generated `LightEntity` snapshot with a single convenience method that
 * calls the underlying scripting primitive.
 *
 * `setIntensity` / `setColor` transition helpers were removed alongside
 * the Live VM tick API — they relied on `getComponent` to read live
 * state. Callers that need a transition build a one-cycle
 * `LightAnimation` (`playCount: 1`) themselves and pass it to
 * `setAnimation`.
 */
export interface LightEntity extends GeneratedLightEntity {
  /**
   * Replace the light's animation. Pass `null` to clear it. Last call
   * wins — lights are always interruptible.
   */
  setAnimation(anim: LightAnimation | null): void;
}

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
  };

  return handle;
}

function idDebug(id: EntityId): string {
  // `EntityId` is a branded number — print the underlying value for
  // error messages without leaking the brand in the type.
  return String(id as unknown as number);
}

// Fixed pattern reused for every `flicker` call — deterministic across reloads, no PRNG needed.
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
