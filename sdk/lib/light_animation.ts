// Reference vocabulary for authoring `LightAnimation` values in TypeScript.
//
// These helpers are pure: they construct and return `LightAnimation` objects
// without touching the engine. Modders can use them directly, compose them,
// or read them as worked examples when rolling their own curves.
//
// None of the helpers below set `phase`. When staggering across a row of
// lights, set `phase` at the call site (see `world.ts` for the wave example).

import type { LightAnimation, Vec3 } from "postretro";

/** Per-channel keyframe format accepted by `timeline` and `sequence`. */
export type Keyframe<T extends number[]> = [number, ...T];

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
 * baked lights (the `world.ts` wrapper surfaces that with a clearer error).
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

// ---------------------------------------------------------------------------
// Keyframe authoring helpers
// ---------------------------------------------------------------------------

/**
 * Validates a list of `[absolute_ms, ...value]` keyframes and returns it
 * unchanged. Throws a `TypeError`-ish `Error` naming the offending entry
 * if:
 *
 * - The list is empty.
 * - Any entry is empty or has a different arity from the first.
 * - Any slot is a non-finite number.
 * - Timestamps are not strictly increasing.
 *
 * The engine consumes `[absolute_ms, ...value]` directly; `timeline`
 * exists purely for shape validation so authoring mistakes surface
 * instead of being silently dropped.
 */
export function timeline<T extends number[]>(
  keyframes: [number, ...T][],
): [number, ...T][] {
  validateKeyframes(keyframes, /* isSequence */ false);
  return keyframes;
}

/**
 * Accepts `[delta_ms, ...value]` keyframes and returns the canonical
 * `[absolute_ms, ...value]` form by accumulating deltas. The first entry
 * is passed through verbatim; subsequent timestamps are the running sum
 * of all preceding deltas plus the current delta.
 *
 * Validates the accumulated timeline with the same rules as `timeline`,
 * so non-positive deltas (which would produce non-monotonic absolute
 * timestamps after the first keyframe) throw a descriptive `Error`.
 */
export function sequence<T extends number[]>(
  keyframes: [number, ...T][],
): [number, ...T][] {
  if (!Array.isArray(keyframes) || keyframes.length === 0) {
    throw new Error("sequence: keyframes must be a non-empty array");
  }
  const first = keyframes[0];
  if (!Array.isArray(first) || first.length === 0) {
    throw new Error("sequence: entry 0 is empty");
  }
  const arity = first.length;

  const out: [number, ...T][] = new Array(keyframes.length);
  // Copy the first entry so we don't alias the caller's input array.
  out[0] = [...first] as [number, ...T];

  for (let i = 1; i < keyframes.length; i++) {
    const kf = keyframes[i];
    if (!Array.isArray(kf)) {
      throw new Error(`sequence: entry ${i} is not an array`);
    }
    if (kf.length !== arity) {
      throw new Error(
        `sequence: entry ${i} has arity ${kf.length}, expected ${arity}`,
      );
    }
    for (let s = 0; s < kf.length; s++) {
      if (typeof kf[s] !== "number" || !Number.isFinite(kf[s])) {
        throw new Error(
          `sequence: entry ${i} slot ${s} is not a finite number`,
        );
      }
    }
    const delta = kf[0];
    const prevT = out[i - 1][0];
    const absT = prevT + delta;
    if (absT <= prevT) {
      throw new Error(
        `sequence: entry ${i} delta ${delta} produces non-monotonic timestamp (prev=${prevT}, next=${absT})`,
      );
    }
    const copy = [...kf] as [number, ...T];
    copy[0] = absT;
    out[i] = copy;
  }

  // Defensive re-validation of the accumulated output. Catches any
  // inconsistency we didn't already flag (e.g. the first entry's
  // fields being non-finite).
  validateKeyframes(out, /* isSequence */ true);
  return out;
}

function validateKeyframes<T extends number[]>(
  keyframes: [number, ...T][],
  isSequence: boolean,
): void {
  const label = isSequence ? "sequence" : "timeline";
  if (!Array.isArray(keyframes) || keyframes.length === 0) {
    throw new Error(`${label}: keyframes must be a non-empty array`);
  }
  const first = keyframes[0];
  if (!Array.isArray(first) || first.length === 0) {
    throw new Error(`${label}: entry 0 is empty`);
  }
  const arity = first.length;

  let prevT = Number.NEGATIVE_INFINITY;
  for (let i = 0; i < keyframes.length; i++) {
    const kf = keyframes[i];
    if (!Array.isArray(kf)) {
      throw new Error(`${label}: entry ${i} is not an array`);
    }
    if (kf.length !== arity) {
      throw new Error(
        `${label}: entry ${i} has arity ${kf.length}, expected ${arity}`,
      );
    }
    for (let s = 0; s < kf.length; s++) {
      if (typeof kf[s] !== "number" || !Number.isFinite(kf[s])) {
        throw new Error(
          `${label}: entry ${i} slot ${s} is not a finite number`,
        );
      }
    }
    const t = kf[0];
    if (i > 0 && t <= prevT) {
      throw new Error(
        `${label}: entry ${i} timestamp ${t} is not strictly greater than previous ${prevT}`,
      );
    }
    prevT = t;
  }
}
