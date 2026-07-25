// Switch-entity fixture. The switch brush in switch-demo.map is visible, solid,
// and pressable; its `on_fire` KVP names `switchDemo.openDoor`, so one press
// fades up the console indicator light beside the switch and starts the door
// mover in the west half.
//
// Both effects ride one sequence reaction. Activation is proximity-only -- the
// runtime has no facing check -- so a press can happen while the player looks
// away, and the door alone is an offscreen signal. The indicator confirms the
// press where the player is looking; keeping the door on the same reaction
// splits the diagnosis. Door moves, light dark: the animated-bake path failed.
// Neither moves: the trigger or the dispatch did.
//
// One press only: the switch is `fire_mode once`, so the indicator lights once
// per level load and then stays lit. An off-then-on pair is not authorable
// anyway -- a finite `playCount` settles multiplicatively (`intensity *= final
// sample`), so a fade to 0 would zero the intensity for good and no later fade
// could bring it back.

import { defineReaction, world } from "postretro";

export function setupLevel() {
  // No `_animated` KVP on the map light: the `setLightAnimation` steps below are
  // what reserve its animated bake.
  const indicators = world.query({
    component: "light",
    tag: "switch_demo_console_light",
  });
  // Mover ids resolve at level install. The compiler's light-membership pass
  // carries only map lights, so this query is empty there -- harmless, because
  // only the light steps drive the reservation.
  const door = world.query({
    component: "kinematic_mover",
    tag: "switch_demo_door",
  });

  // Dark on spawn. `startActive: false` is honoured only from a `levelLoad`
  // reaction, so the off state cannot be deferred to the press. `playCount: null`
  // keeps this one from settling -- a finite count would write the zero
  // brightness back as static intensity and the press could never light it.
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
