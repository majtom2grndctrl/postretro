// Reference scene for fog reactions. Demonstrates BOTH dispatch surfaces
// the fog primitive set supports — a single sample script doubles as the
// canonical example for the API:
//
// 1. `Primitive` (tag-targeted, batch). One reaction descriptor names a
//    primitive plus a tag; the dispatcher resolves the tag to every
//    matching fog volume and applies the change in one batch. Use this
//    when every tagged volume should receive the same value at once
//    (one-shot scene tweak, no per-step animation). Here, every
//    `pulse_fog` volume's `scatter` is set to `0.4` on `levelLoad` in a
//    single dispatch — no per-volume sequence, no per-volume reaction.
//
// 2. `Sequence` (per-id steps). The `fogPulse` SDK constructor emits a
//    step array stamped with one entity id per step. The sequence path
//    is the only way to deliver a *time-varying* curve, because each
//    step carries its own `args` payload. Tag-targeting can't express
//    that — a tag-targeted reaction fires once with one args bag.
//
// Authors picking between the two: if every target gets the same value,
// reach for `Primitive`; if the values differ across steps (animation
// curve, fade, etc.), reach for `Sequence`. `arena-lights.ts` uses only
// the `Sequence` path because its per-light phase staggering needs
// per-id args; this scene shows the other half of the surface.

import {
  fogPulse,
  type NamedReactionDescriptor,
  registerReaction,
  world,
} from "postretro";

export function registerLevelManifest(_ctx: unknown) {
  const reactions: NamedReactionDescriptor[] = [];

  const fogs = world.query({ component: "fog_volume", tag: "pulse_fog" });
  if (fogs.length > 0) {
    // Tag-targeted Primitive: one descriptor, batch-applied to every
    // `pulse_fog` volume. No SDK helper — `registerReaction` with the
    // primitive shape is the API.
    reactions.push(
      registerReaction("levelLoad", {
        primitive: "setFogScatter",
        tag: "pulse_fog",
        args: { scatter: 0.4 },
      }),
    );

    // Per-id Sequence: one reaction per volume, each carrying a single
    // `setFogAnimation` step built by `fogPulse`. The bridge evaluates
    // the density curve per-frame across `periodMs` (here 1500 ms),
    // looping forever.
    for (const fog of fogs) {
      const steps = fogPulse(fog.id, 0.2, 1.0, 500);
      reactions.push(registerReaction("levelLoad", { sequence: steps }));
    }
  }

  return { reactions };
}
