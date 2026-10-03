// Pure weapon-action builders. Rust validates structure, input scope, and domains.
import type { ActivationStepDescriptor, RuntimeValue } from "postretro";
import { numberNode, numberRef } from "./util/expression_refs";
import type { NumberRef, NumberValue } from "./util/expression_refs";

/** Independent shot multipliers. Omitted axes use 1; expressions may read only `activation.charge`. */
export type ActivationShotScale = Readonly<{
  /** Direct or splash damage multiplier, applied once. Finite range: [0, 64]. */
  damage?: NumberValue;
  /** Hitscan distance or projectile travel cap multiplier. Finite range: (0, 64]. */
  range?: NumberValue;
  /** Projectile travel-speed multiplier. Finite range: (0, 64]. */
  projectileSpeed?: NumberValue;
  /** Projectile swept collision-radius multiplier. Finite range: (0, 64]. */
  projectileRadius?: NumberValue;
  /** Projectile visual-size multiplier; collision and splash radius are independent. Finite range: (0, 64]. */
  projectileSize?: NumberValue;
  /** Direct and splash knockback-speed multiplier. Finite range: [0, 64]. */
  knockbackSpeed?: NumberValue;
  /** Per-shot ammo, heat, or cell-cost multiplier. Finite range: (0, 64]; positive ammo costs round up. */
  resourceCost?: NumberValue;
}>;

/** Shot options. Multipliers accept finite literals, fluent refs, or numeric `runtime.*` IR. */
export type ActivationShotOptions = Readonly<{
  /** Independent multipliers, defaulting to 1. Only the read-only `charge` input is allowed. */
  scale?: ActivationShotScale;
}>;

/** Closed weapon-action data builders. Programs allow at most 64 steps and 16 shots. */
export type Activation = Readonly<{
  /** Normalized start-to-release charge in [0, 1]; uncharged actions read 1. No state-store reads or writes. */
  charge: NumberRef;
  /** Build an ordinary shot. First and last steps must be shots, with a positive wait between shots. */
  shot(options?: ActivationShotOptions): ActivationStepDescriptor;
  /** Build a wait in milliseconds: finite (0, 60000], rounded up to whole ticks. Total quantized waits are at most 60 seconds. */
  wait(durationMs: number): ActivationStepDescriptor;
}>;

/** Pure descriptor namespace. It retains no callbacks and executes no gameplay. */
export const activation: Activation = Object.freeze({
  charge: numberRef({ op: "input", name: "charge" }),
  shot: (options?: ActivationShotOptions): ActivationStepDescriptor => {
    if (options?.scale === undefined) return Object.freeze({ kind: "shot" });
    const scale: Record<string, number | RuntimeValue> = {};
    for (const [key, value] of Object.entries(options.scale)) {
      if (value !== undefined) scale[key] = typeof value === "number" ? value : numberNode(value);
    }
    return Object.freeze({ kind: "shot", scale: Object.freeze(scale) });
  },
  wait: (durationMs: number): ActivationStepDescriptor => Object.freeze({ kind: "wait", durationMs }),
});
