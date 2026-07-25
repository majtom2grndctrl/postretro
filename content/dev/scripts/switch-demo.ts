// Switch-entity fixture. The switch brush in switch-demo.map is visible, solid,
// and pressable; its `on_fire` KVP names `switchDemo.openDoor`, so one press
// fades up the console indicator light beside the switch and starts the door
// mover in the west half.
//
// Activation is proximity-only -- the runtime has no facing check -- so a press
// can happen while the player looks away, and the door alone is an offscreen
// signal. The indicator is the confirmation visible at the console; the door is
// the independent signal several metres away.
//
// The two effects are authored in one sequence but do not travel one path. The
// trigger binder partitions a sequence by primitive class: `moverStart` is
// consequential and binds as a command that runs inside the trigger's own fixed
// tick, while `setLightAnimation` is presentation and defers to an app-side
// residual drain. The mover therefore runs FIRST, despite being authored second,
// and the halves fail independently:
//
//   Neither moves            The press never landed -- switch trigger volume,
//                            `on_fire` name binding, or dispatch.
//   Door moves, light dark   Either the residual drain never ran, or the light
//                            half failed on its own: empty tag query, or the
//                            `levelLoad` off-state was never replaced.
//   Light lights, door dark  The mover command bound, but its path or
//                            `start_on_spawn` wiring is wrong.
//
// One press only: the switch is `fire_mode once`, so the indicator lights once
// per level load and then stays lit. An off-then-on pair is not authorable
// anyway -- when a finite `playCount` elapses, the light bridge settles the
// animation multiplicatively: it writes `intensity *= final sample` back as
// static state and clears the curve. A fade to 0 would zero the intensity for
// good, and no later fade could recover it. That settle rule is tier-agnostic --
// it applies to this dynamic light exactly as it would to a baked one.

import { defineReaction, world } from "postretro";

export function setupLevel() {
  // The indicator is a `light_dynamic`. Dynamic-tier lights bake into nothing,
  // so this query earns no animated-bake reservation and needs none: with no
  // lightmap atlas there is no array layer for a spilled face to go silently
  // dark in, which is what made the baked version of this indicator unreliable.
  // The compile-time light-membership pass still sees the light, but for a
  // dynamic target it only reports it as runtime-only and reserves nothing.
  //
  // Throw rather than guard. An empty result would otherwise register
  // `sequence: []` as a perfectly valid inert reaction -- no diagnostic at
  // compile time, none at runtime, and a fixture that reads as a broken switch.
  // The light-membership pass evaluates `setupLevel` during prl-build, so a
  // mistyped tag fails the build at authoring time instead of on a tester's
  // machine. In a fixture built to make failure visible, that is the point.
  const indicators = world.query({
    component: "light",
    tag: "switch_demo_console_light",
  });
  if (indicators.length === 0) {
    throw new Error(
      "switch-demo: no light tagged `switch_demo_console_light` -- check `_tags` on the light_dynamic entity in switch-demo.map",
    );
  }

  // Deliberately NOT thrown, unlike the light above. Mover ids resolve only at
  // level install; the compile-time light-membership pass carries a light table
  // and nothing else, so every non-light query legitimately returns [] during
  // prl-build. A throw here would fail the build on a correct map. Spreading an
  // empty result is what leaves the compile-time reaction light-steps-only,
  // which is exactly the subset that pass consumes.
  const door = world.query({
    component: "kinematic_mover",
    tag: "switch_demo_door",
  });

  // Dark on spawn, and the off-state does belong in `levelLoad` -- but not for
  // the reason it looks like. `startActive` is read unconditionally at runtime:
  // a `startActive: false` step from ANY reaction darkens the light. What is
  // levelLoad-specific is the compile-time pass that folds `startActive` into a
  // baked descriptor's initial state, and that pass skips dynamic targets
  // outright. So on this tier nothing is pre-baked dark; the light is dark only
  // because this reaction actually runs during level install, ahead of the light
  // bridge's first pack. `levelLoad` is the reaction that runs there.
  //
  // `playCount: null` keeps this step from ever settling. A finite count would
  // let the bridge write the zero brightness back as static intensity
  // (`intensity *= 0`) and clear the curve, after which no press could light it.
  const indicatorOff = defineReaction("levelLoad", {
    sequence: indicators.map((light) => ({
      id: light.id,
      primitive: "setLightAnimation" as const,
      args: {
        periodMs: 250,
        phase: null,
        playCount: null,
        startActive: false,
        brightness: [0],
        color: null,
        direction: null,
      },
    })),
  });

  // `fade` overwrites `animation` wholesale, so the press replaces the inactive
  // descriptor above rather than layering over it. Its `playCount: 1` settles to
  // `intensity *= 1.0` -- authored intensity, curve cleared -- and the indicator
  // stays lit for the rest of the level.
  const onPress = defineReaction("switchDemo.openDoor", {
    sequence: [
      ...indicators.flatMap((light) =>
        light.fade({ from: 0, to: 1, periodMs: 250 }),
      ),
      ...door.flatMap((mover) => mover.start()),
    ],
  });

  return { reactions: [indicatorOff, onPress] };
}
