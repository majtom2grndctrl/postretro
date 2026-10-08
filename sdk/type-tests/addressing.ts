// Entity addressing: map members, groups and subject tokens.
// See: context/lib/scripting.md §12 (Entity addressing)
import * as Postretro from "postretro";
import {
  type GroupCommand,
  type SequenceStep,
  type SubjectTokenCommand,
  type TriggerEventParams,
  defineReaction,
  defineTriggerEvent,
  getGravity,
  getMapEntities,
  npcs,
  players,
  setGravity,
  wait,
} from "postretro";

// Members: each map kind yields its own handle, with exactly its verbs.
const doors = getMapEntities("mover", { tag: "door" });
const plates = getMapEntities("trigger");
const lights = getMapEntities("light", { tag: "alarm" });
const fogs = getMapEntities("fog");
const emitters = getMapEntities("emitter");
const closets = getMapEntities("spawner", { tag: "closet_spawner" });

const memberSteps: SequenceStep[] = [
  ...doors.flatMap((m) => m.start()),
  ...lights.flatMap((l, i) => l.pulse({ min: 0.3, max: 1, periodMs: 400 + i * 50 })),
  ...fogs.flatMap((f) => f.pulseSaturation({ min: 0, max: 1, periodMs: 500 })),
  ...closets.flatMap((s) => s.fire()),
];
const spawnedTags: ReadonlyArray<string> = closets[0].spawnedTags;
const emitterRate: number = emitters[0].component.rate;
// Members are plain arrays the author inspects.
const sorted = lights.sort((a, b) => a.position.x - b.position.x);

// @ts-expect-error NPCs are a group, not a map kind.
getMapEntities("npc");
// @ts-expect-error `transform` is not a map kind.
getMapEntities("transform");
// @ts-expect-error A light member has no mover verbs.
lights[0].start();
// @ts-expect-error An emitter member is a snapshot with no verbs.
emitters[0].fire();
// @ts-expect-error A spawner member has no light verbs.
closets[0].pulse({ min: 0, max: 1, periodMs: 1 });

// Groups: one descriptor per verb, legal as a body and directly as a step.
const closet = npcs({ tag: "closet" });
const everyNpc = npcs();
const resupply = defineReaction("closet.resupply", players().grantAmmo("shells.buck", 8));
const reveal = defineReaction("closet.reveal", {
  sequence: [...memberSteps, closet.update({ aggro: true }), closet.damage(5), everyNpc.damage(1)],
});
const groupStep: GroupCommand = players().grantHealth(10);

// @ts-expect-error A group has no length.
closet.length;
// @ts-expect-error A group has no members to map over.
closet.map((npc: unknown) => npc);
// @ts-expect-error A group is not indexable.
players()[0];
// @ts-expect-error `fire` is not an NPC-group verb.
npcs().fire();
// @ts-expect-error NPC groups have no resource grants.
npcs().grantHealth(5);
// @ts-expect-error `update` is an NPC verb, not a player verb.
players().update({ aggro: true });
// Retired spellings are reached by element access so the retired-name grep
// gate over `sdk/` stays clean; each must still fail to compile.
// @ts-expect-error The free target-taking verbs retired with plain tag targets: `damage` taking a tag string.
Postretro["damage"]("boss", 10);
// @ts-expect-error `enemies` retired; NPCs are `npcs({ tag })`.
Postretro["enemies"]({ tag: "closet" });
// @ts-expect-error `world` dissolved into `getMapEntities`, `getGravity` and `setGravity`.
Postretro["world"];
// @ts-expect-error The raw world query stays out of the author-facing typedefs.
Postretro["worldQuery"]({ component: "light" });
// @ts-expect-error Group-step updates are a closed, typed partial.
closet.update({ aggression: true });

// Subject tokens: one descriptor per verb, legal as a body and, unspread, as a
// step before any `wait` — the same dual use a group command has.
const patchUp = defineReaction((on: TriggerEventParams) => on.activators.grantHealth(25));
const shut = defineReaction((on: TriggerEventParams) => on.trigger.disarm());
const ambush = defineReaction((on: TriggerEventParams) => ({
  sequence: [on.activators.grantHealth(10), on.trigger.disarm(), closet.damage(5), ...wait(800), everyNpc.damage(1)],
}));
declare const fired: TriggerEventParams;
const tokenStep: SubjectTokenCommand = fired.activators.grantHealth(5);
const armStep: SequenceStep = fired.trigger.arm();
defineReaction((on: TriggerEventParams) => ({
  // @ts-expect-error A token command is one descriptor, not a step array to spread.
  sequence: [...on.trigger.disarm()],
}));

// Trigger events: a member's `on` is volume-keyed for `setupLevel`; a mod
// rule is tag-keyed for `ModManifest`.
const memberEvents = plates.flatMap((t) => [t.on("enter", [reveal]), t.on("exit", ["closet.patchUp"])]);
const level: Postretro.LevelManifest = { reactions: [reveal, resupply], triggerEvents: memberEvents };
const modRule = defineTriggerEvent({ tag: "story", event: "enter", fire: [resupply], levels: ["campaign"] });
// @ts-expect-error A tag-keyed rule belongs in `ModManifest.triggerEvents`, not a level.
const badLevel: Postretro.LevelManifest = { reactions: [], triggerEvents: [modRule] };
// @ts-expect-error Trigger events publish only enter and exit.
plates[0].on("stay", [reveal]);

// Gravity.
setGravity(getGravity() * 0.5);

void spawnedTags;
void emitterRate;
void sorted;
void groupStep;
void patchUp;
void shut;
void ambush;
void tokenStep;
void armStep;
void level;
void badLevel;
