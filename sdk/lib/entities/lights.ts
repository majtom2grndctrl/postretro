// Light entity handle and capability-method implementations
// (pulse / fade / flicker / colorShift / sweep) returning sequence-step
// arrays. Governed by context/lib/entity_model.md.

import type {
  EntityId,
  LightAnimation,
  LightEntity as GeneratedLightEntity,
  Vec3,
} from "postretro";
import type { AnimatableScalar } from "../animation";
import type { SequenceStep } from "../data_script";

/**
 * Typed handle returned by `world.query` for a light entity. Composes
 * the generated `LightEntity` snapshot with capability methods that emit
 * `setLightAnimation` step arrays. Authors call methods on the handle
 * rather than passing `light.id` into free functions.
 *
 * Each capability method returns a single-element `SequenceStep[]`
 * suitable for splicing into a `defineReaction({ sequence: [...] })`
 * body.
 */
export interface LightEntityHandle
  extends GeneratedLightEntity,
    AnimatableScalar<"brightness"> {
  /** Cycle through RGB colors over `periodMs`. Dynamic lights only. */
  colorShift(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
  /** Sweep the `direction` channel through unit vectors over `periodMs`. */
  sweep(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
}

// Fixed pattern reused for every `flicker` call — deterministic across
// reloads, no PRNG needed.
const FLICKER_PATTERN: ReadonlyArray<number> = [
  0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65,
];

function buildPulse(min: number, max: number, periodMs: number): LightAnimation {
  const SAMPLES = 16;
  const lo = Math.min(min, max);
  const hi = Math.max(min, max);
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
    startActive: null,
    brightness,
    color: null,
    direction: null,
  };
}

function buildFade(from: number, to: number, periodMs: number): LightAnimation {
  const SAMPLES = 16;
  const brightness: number[] = new Array(SAMPLES);
  for (let i = 0; i < SAMPLES; i++) {
    const t = i / (SAMPLES - 1);
    brightness[i] = from + (to - from) * t;
  }
  return {
    periodMs,
    phase: null,
    playCount: 1,
    startActive: null,
    brightness,
    color: null,
    direction: null,
  };
}

function buildFlicker(min: number, max: number, rate: number): LightAnimation {
  const lo = Math.min(min, max);
  const hi = Math.max(min, max);
  const span = hi - lo;
  const brightness = FLICKER_PATTERN.map((t) => lo + t * span);
  return {
    periodMs: 1000 / rate,
    phase: null,
    playCount: null,
    startActive: null,
    brightness,
    color: null,
    direction: null,
  };
}

function buildColorShift(values: Vec3[], periodMs: number): LightAnimation {
  const color: Vec3[] = values.map((v) => ({ x: v.x, y: v.y, z: v.z }));
  return {
    periodMs,
    phase: null,
    playCount: null,
    startActive: null,
    brightness: null,
    color,
    direction: null,
  };
}

function buildSweep(values: Vec3[], periodMs: number): LightAnimation {
  // Direction samples are normalized defensively even though the
  // primitive also normalizes non-unit inputs — zero-length samples
  // still error at the primitive seam.
  const direction: Vec3[] = values.map(({ x, y, z }) => {
    const len = Math.sqrt(x * x + y * y + z * z);
    if (len > 0) {
      return { x: x / len, y: y / len, z: z / len };
    }
    return { x, y, z };
  });
  return {
    periodMs,
    phase: null,
    playCount: null,
    startActive: null,
    brightness: null,
    color: null,
    direction,
  };
}

export function wrapLightEntity(snapshot: GeneratedLightEntity): LightEntityHandle {
  const id: EntityId = snapshot.id;

  const handle: LightEntityHandle = {
    ...snapshot,

    pulse(opts: { min: number; max: number; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setLightAnimation",
          args: buildPulse(opts.min, opts.max, opts.periodMs),
        },
      ];
    },

    fade(opts: { from: number; to: number; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setLightAnimation",
          args: buildFade(opts.from, opts.to, opts.periodMs),
        },
      ];
    },

    flicker(opts: { min: number; max: number; rate: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setLightAnimation",
          args: buildFlicker(opts.min, opts.max, opts.rate),
        },
      ];
    },

    colorShift(opts: { values: Vec3[]; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setLightAnimation",
          args: buildColorShift(opts.values, opts.periodMs),
        },
      ];
    },

    sweep(opts: { values: Vec3[]; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setLightAnimation",
          args: buildSweep(opts.values, opts.periodMs),
        },
      ];
    },
  };

  return handle;
}
