// Fog volume entity handle and capability-method implementations
// (pulse / fade / flicker / pulseSaturation / fadeSaturation) returning
// sequence-step arrays. Mirrors the LightEntity vocabulary in `./lights`,
// adapted to the fog_volume ComponentValue surface.
// See: context/lib/scripting.md §7 (SDK library globals), §10.2 (fog reaction primitives).

import type {
  EntityId,
  FogAnimation,
  FogVolumeEntity as GeneratedFogVolumeEntity,
} from "postretro";
import type { AnimatableScalar } from "../animation";
import type { SequenceStep } from "../data_script";

/**
 * Typed handle returned by `world.query({ component: "fog_volume" })`.
 * Carries the snapshot fields plus capability methods that emit
 * `setFogAnimation` step arrays.
 *
 * Authors animate fog by registering sequenced reactions —
 * `defineReaction("levelLoad", { sequence: fog.pulse({ ... }) })` —
 * rather than mutating the handle. Static one-shot tweaks still go
 * through the `setFogDensity` / `setFogScatter` / `setFogEdgeSoftness` /
 * `setFogFalloff` / `setFogParams` step descriptors.
 *
 * Fog ambient color is derived from the SH irradiance volume and is not
 * settable via script. When no SH irradiance volume is baked, ambient
 * scatter contribution is zero — fog is effectively invisible without
 * dynamic lights nearby.
 */
export interface FogVolumeHandle
  extends GeneratedFogVolumeEntity,
    AnimatableScalar<"density"> {
  /** Looping sine pulse on the `saturation` channel. */
  pulseSaturation(opts: { min: number; max: number; periodMs: number }): SequenceStep[];
  /** One-shot linear ramp on the `saturation` channel. */
  fadeSaturation(opts: { from: number; to: number; periodMs: number }): SequenceStep[];
}

// Fixed 8-step irregular pattern in [0, 1] — same curve `LightEntityHandle.flicker` uses.
const FLICKER_PATTERN: ReadonlyArray<number> = [
  0.95, 0.40, 1.00, 0.72, 0.15, 0.88, 0.30, 0.65,
];

// 17 samples (16 + 1 wrap): fog uses a CPU linear sampler that treats N
// samples as N-1 intervals on [0, 1]; the wrap sample closes the final
// interval so the sine interpolates cleanly back to the start rather than
// snapping. Lights use GPU Catmull-Rom and don't need the wrap sample.
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

function densityAnim(periodMs: number, density: number[], playCount: number | null): FogAnimation {
  return {
    periodMs,
    phase: null,
    playCount,
    density,
    saturation: null,
    minBrightness: null,
    lightRange: null,
  };
}

function saturationAnim(periodMs: number, saturation: number[], playCount: number | null): FogAnimation {
  return {
    periodMs,
    phase: null,
    playCount,
    density: null,
    saturation,
    minBrightness: null,
    lightRange: null,
  };
}

export function wrapFogVolumeEntity(
  snapshot: GeneratedFogVolumeEntity,
): FogVolumeHandle {
  const id: EntityId = snapshot.id;

  const handle: FogVolumeHandle = {
    ...snapshot,

    pulse(opts: { min: number; max: number; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setFogAnimation",
          args: densityAnim(opts.periodMs, buildPulseSamples(opts.min, opts.max), null),
        },
      ];
    },

    fade(opts: { from: number; to: number; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setFogAnimation",
          args: densityAnim(opts.periodMs, buildFadeSamples(opts.from, opts.to), 1),
        },
      ];
    },

    flicker(opts: { min: number; max: number; rate: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setFogAnimation",
          args: densityAnim(1000 / opts.rate, buildFlickerSamples(opts.min, opts.max), null),
        },
      ];
    },

    pulseSaturation(opts: { min: number; max: number; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setFogAnimation",
          args: saturationAnim(opts.periodMs, buildPulseSamples(opts.min, opts.max), null),
        },
      ];
    },

    fadeSaturation(opts: { from: number; to: number; periodMs: number }): SequenceStep[] {
      return [
        {
          id,
          primitive: "setFogAnimation",
          args: saturationAnim(opts.periodMs, buildFadeSamples(opts.from, opts.to), 1),
        },
      ];
    },
  };

  return handle;
}
