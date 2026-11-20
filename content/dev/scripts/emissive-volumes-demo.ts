// Emissive volumes: the Scripting-surface example, run on
// `emissive-volumes-demo.map`. The two bar signs flicker from load; the floor
// pad that fires `powerCut` holds them dark, and the pad that fires
// `doorLocked` turns the status lamp red. The lamp pulses and the exit sign
// fades up once at load, so the colour write can be seen leaving a running
// animation alone and a finite fade can be seen settling.
//
// Every value here is brightness in [0, 1] across each volume's authored
// `emissive_min`..`emissive_max`, unlike lights and fog, whose methods take
// absolute values. A brightness write clears a running animation; a colour
// write leaves it running.
//
// Every reaction is named, so the Luau twin emits identical wire.
// See: context/lib/scripting.md §10.2a

import {
  type NamedReactionDescriptor,
  defineReaction,
  getMapEntities,
} from "postretro";

export function setupLevel(_ctx: unknown) {
  const reactions: NamedReactionDescriptor[] = [];

  const signs = getMapEntities("emissive", { tag: "bar_sign" });
  for (const sign of signs) {
    // sign.component: { min, max, brightness, color }
    reactions.push(defineReaction("levelLoad", { sequence: sign.flicker({ min: 0.3, max: 1, rate: 6 }) }));
  }
  reactions.push(defineReaction("powerCut", { primitive: "setEmissiveBrightness", tag: "bar_sign", args: { brightness: 0 } }));
  reactions.push(defineReaction("doorLocked", { primitive: "setEmissiveColor", tag: "status_lamp", args: { color: [1, 0.05, 0.02] } }));

  for (const lamp of getMapEntities("emissive", { tag: "status_lamp" })) {
    reactions.push(defineReaction("levelLoad", { sequence: lamp.pulse({ min: 0.6, max: 1, periodMs: 1500 }) }));
  }
  for (const exit of getMapEntities("emissive", { tag: "exit_sign" })) {
    reactions.push(defineReaction("levelLoad", { sequence: exit.fade({ from: 0, to: 1, periodMs: 1500 }) }));
  }

  return { reactions };
}
