// Emissive volume entity handle and capability-method implementations
// (pulse / fade / flicker) returning `setEmissiveAnimation` step arrays.
// Mirrors the fog volume vocabulary in `./fog_volumes`, on one brightness
// channel whose values are brightness in [0, 1], not absolute levels.
// See: context/lib/scripting.md §7 (SDK library globals), §10.2a (emissive reaction primitives).

import type {
  EmissiveAnimation,
  EmissiveVolumeEntity as GeneratedEmissiveVolumeEntity,
  EntityId,
} from "postretro";
import type { AnimatableScalar } from "../animation";
import type { SequenceStep } from "../data_script";

/**
 * Emissive member returned by `getMapEntities("emissive")`.
 * Carries the snapshot fields plus capability methods that emit
 * `setEmissiveAnimation` step arrays.
 *
 * Values are brightness in [0, 1] across the volume's authored
 * `emissive_min`..`emissive_max`, unlike lights and fog, whose methods take
 * absolute values. Out-of-range values clamp. Static writes go through the
 * tag-targeted `setEmissiveBrightness` and `setEmissiveColor` primitives; a
 * brightness write clears any running animation, a colour write leaves it.
 */
export interface EmissiveVolumeHandle
  extends GeneratedEmissiveVolumeEntity,
    AnimatableScalar<"brightness"> {}

// Fixed 8-step irregular pattern in [0, 1] — the curve fog and lights flicker with.
const FLICKER_PATTERN: ReadonlyArray<number> = [
  0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65,
];

// 17 samples (16 + 1 wrap): the bridge samples linearly, treating N samples
// as N-1 intervals on [0, 1]; the wrap sample closes the last interval so the
// sine runs back to its start rather than snapping.
function buildPulseSamples(min: number, max: number): number[] {
  const SAMPLES = 16;
  const lo = Math.min(min, max);
  const hi = Math.max(min, max);
  const mid = (lo + hi) * 0.5;
  const amp = (hi - lo) * 0.5;
  const out: number[] = new Array(SAMPLES + 1);
  for (let i = 0; i <= SAMPLES; i++) {
    const theta = (i / SAMPLES) * Math.PI * 2;
    out[i] = mid + amp * Math.sin(theta);
  }
  return out;
}

// 16 evenly-spaced samples from `from` (sample 0) to `to` (sample 15).
function buildFadeSamples(from: number, to: number): number[] {
  const SAMPLES = 16;
  const out: number[] = new Array(SAMPLES);
  for (let i = 0; i < SAMPLES; i++) {
    const t = i / (SAMPLES - 1);
    out[i] = from + (to - from) * t;
  }
  return out;
}

function buildFlickerSamples(min: number, max: number): number[] {
  const lo = Math.min(min, max);
  const hi = Math.max(min, max);
  const span = hi - lo;
  return FLICKER_PATTERN.map((t) => lo + t * span);
}

// Luau tables cannot hold `nil`, so the Luau twin's wire omits an unset
// `playCount`; omitting it here too keeps both twins' wire byte-identical.
// `playCount` absent loops forever.
function brightnessAnim(periodMs: number, brightness: number[], playCount?: number): EmissiveAnimation {
  return playCount === undefined ? { periodMs, brightness } : { periodMs, playCount, brightness };
}

export function wrapEmissiveVolumeEntity(
  snapshot: GeneratedEmissiveVolumeEntity,
): EmissiveVolumeHandle {
  const id: EntityId = snapshot.id;

  const handle: EmissiveVolumeHandle = {
    ...snapshot,

    pulse(opts: { min: number; max: number; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setEmissiveAnimation",
          args: brightnessAnim(opts.periodMs, buildPulseSamples(opts.min, opts.max)),
        },
      ];
    },

    fade(opts: { from: number; to: number; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setEmissiveAnimation",
          args: brightnessAnim(opts.periodMs, buildFadeSamples(opts.from, opts.to), 1),
        },
      ];
    },

    flicker(opts: { min: number; max: number; rate: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setEmissiveAnimation",
          args: brightnessAnim(1000 / opts.rate, buildFlickerSamples(opts.min, opts.max)),
        },
      ];
    },
  };

  return handle;
}
