// Co-op presentation fixture: source-aware disposable cues and durable state.
import { defineReaction, getMapEntities } from "postretro";
import type { NamedReactionDescriptor, TriggerEventParams } from "postretro";
import { flashScreen, playSound, screenShake } from "postretro/ui";
import { setupPlayerEvents as playerEventSetup } from "./player-events";

const alarm = defineReaction("coop.plate.alarm", playSound("sfx/test_tone"));
const flash = defineReaction("coop.plate.flash", flashScreen([0.2, 0.7, 1, 0.35], 250));
const shake = defineReaction("coop.plate.shake", (on: TriggerEventParams) =>
  screenShake(6, 350, 18, { audience: on.trigger }));
const click = defineReaction("coop.plate.click", (on: TriggerEventParams) =>
  playSound("fixtures/land", { audience: on.activators }));

export function setupLevel() {
  const events = playerEventSetup();
  const lights = getMapEntities("light", { tag: "coop_prl_light" });
  const fogs = getMapEntities("fog", { tag: "coop_fog" });
  const doors = getMapEntities("mover", { tag: "coop_door" });
  const reactions: NamedReactionDescriptor[] = [
    ...events.reactions, alarm, flash, shake, click,
    defineReaction("coop.atmosphere.lights", { sequence: lights.flatMap((light) => light.fade({ from: 1, to: 0.2, periodMs: 5000 })) }),
    defineReaction("coop.atmosphere.fogDensity", { primitive: "setFogDensity", tag: "coop_fog", args: { density: 0.6 } }),
    defineReaction("coop.atmosphere.fogFade", { sequence: fogs.flatMap((fog) => fog.fade({ from: 0.6, to: 0.1, periodMs: 5000 })) }),
    defineReaction("coop.atmosphere.fogParams", { primitive: "setFogParams", tag: "coop_fog", args: { tint: [0.3, 0.6, 1], saturation: 0.7, minBrightness: 0.1, lightRange: 1.5 } }),
    defineReaction("coop.atmosphere.fogGlow", { primitive: "setFogGlow", tag: "coop_fog", args: { glow: 0.15 } }),
    defineReaction("coop.atmosphere.fogEdge", { primitive: "setFogEdgeSoftness", tag: "coop_fog", args: { edgeSoftness: 0.5 } }),
    defineReaction("coop.atmosphere.fogFalloff", { primitive: "setFogFalloff", tag: "coop_fog", args: { falloff: 2 } }),
    defineReaction("coop.atmosphere.mapEmitter", { primitive: "setEmitterRate", tag: "coop_map_emitter", args: { rate: 0 } }),
    defineReaction("coop.atmosphere.spin", { primitive: "setSpinRate", tag: "coop_map_emitter", args: { animation: { duration: 5, rate_curve: [0.5, 6.28] } } }),
    // Descriptor placements are spawned after setupLevel. Tag resolution runs
    // when the reaction fires, so these never bake install-time entity IDs.
    defineReaction("coop.atmosphere.descriptorLights", { primitive: "setLightAnimation", tag: "coop_descriptor_light", args: { periodMs: 2000, brightness: [0.2, 1, 0.2] } }),
    defineReaction("coop.atmosphere.descriptorEmitter", { primitive: "setEmitterRate", tag: "coop_descriptor_emitter", args: { rate: 0 } }),
    defineReaction("coop.atmosphere.prop", { primitive: "setAnimationState", tag: "coop_mesh_prop", args: { state: "alert" } }),
    defineReaction("coop.atmosphere.door", { sequence: doors.flatMap((door) => door.start()) }),
    // A singleton enemy's install-time progress threshold is the death source.
    // Same-name bodies give one fire both the light change and global flash.
    defineReaction("coop.enemy.dead", { sequence: lights.flatMap((light) => light.pulse({ min: 0.15, max: 1, periodMs: 1500 })) }),
    defineReaction("coop.enemy.dead", flashScreen([1, 0.2, 0.1, 0.3], 300, { audience: "everyone" })),
    defineReaction("coop.enemy.progress", { progress: { tag: "coop_death_enemy", at: 1, fire: "coop.enemy.dead" } }),
    // One-use, observable retained-Health setup: one pistol hit is 12 HP;
    // the use pad removes 12 HP from the fresh world item (48 → 36).
    defineReaction("coop.weapon.mark", { primitive: "applyDamage", tag: "coop_health_weapon", args: { amount: 12 } }),
  ];
  const mark = reactions.find((reaction) => reaction.name === "coop.weapon.mark")!;
  const atmosphere = reactions.filter((reaction) => reaction.name.startsWith("coop.atmosphere."));
  return {
    reactions,
    playerEvents: events.playerEvents,
    crossings: events.crossings,
    triggerEvents: [
      ...getMapEntities("trigger", { tag: "coop_plate" }).map((plate) => plate.on("enter", [alarm, flash, shake, click])),
      ...getMapEntities("trigger", { tag: "coop_weapon_mark" }).map((trigger) => trigger.on("enter", [mark])),
      ...getMapEntities("trigger", { tag: "coop_atmosphere" }).map((trigger) => trigger.on("enter", atmosphere)),
    ],
  };
}
