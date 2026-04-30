// Generic keyframe authoring helpers (`Keyframe`, `timeline`, `sequence`).
// See: context/lib/scripting.md §7

/** Per-channel keyframe format accepted by `timeline` and `sequence`. */
export type Keyframe<T extends number[]> = [number, ...T];

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

  // Defensive re-validation: catches non-finite values in the first entry that the loop above skips.
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
