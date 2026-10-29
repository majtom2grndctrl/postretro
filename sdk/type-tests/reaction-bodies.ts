// Array bodies and system steps: a system reaction is a step unspread, an
// array (nested at any depth) is a body, and a body that is not a step fails to
// compile inside one.
import {
  type EntityId,
  type TriggerEventParams,
  defineReaction,
  fire,
  getMapEntities,
  npcs,
  wait,
} from "postretro";
import { setGhostAudio } from "postretro/audio";
import { flashScreen, playSound, updateState } from "postretro/ui";

declare const doorId: EntityId;
declare const flag: import("postretro").Ref<number>;
const doors = getMapEntities("mover", { tag: "closet" });
const closet = defineReaction("closet", [doors.map((door) => door.start()), npcs({ tag: "m" }).update({ aggro: true })]);

// System steps sit unspread beside member, group and control steps.
const merged = defineReaction("merged", [
  doors.map((door) => door.start()),
  setGhostAudio({ volume: 0.9 }),
  wait(1500),
  fire(closet),
  playSound("sfx/test_tone"),
  updateState(flag, 1),
]);
// Nested arrays and `sequence` lists flatten alike.
const nested = defineReaction({ sequence: [playSound("a"), [wait(5), [playSound("b")]]] });
// A tracer may return an array or a `{ sequence }`.
const traced = defineReaction("traced", (on: TriggerEventParams) => [
  on.activators.grantHealth(25),
  flashScreen([1, 1, 1, 1], 100),
]);
const tracedSequence = defineReaction("tracedSequence", (on: TriggerEventParams) => ({
  sequence: [on.activators.grantHealth(25), playSound("medkit")],
}));
// An empty array is an inert reaction.
const empty = defineReaction("empty", []);
// A system reaction is still a body on its own.
const swell = defineReaction("swell", setGhostAudio({ volume: 0.9 }));

// @ts-expect-error A `progress` body is not a step.
const progressEntry = defineReaction([{ progress: { tag: "m", at: 1, fire: "done" } }]);
// @ts-expect-error A nested `{ sequence }` is not a step; list its steps instead.
const sequenceEntry = defineReaction([{ sequence: [playSound("a")] }]);
// @ts-expect-error A reaction handle is not a step; fire it with `fire(r)`.
const handleEntry = defineReaction([closet]);
// @ts-expect-error `onComplete` belongs to a body, not a step.
const onCompleteEntry = defineReaction([{ primitive: "moverStart", id: doorId, args: {}, onComplete: "next" }]);
// @ts-expect-error The same rules hold inside a `sequence` list.
const handleInSequence = defineReaction({ sequence: [playSound("a"), [closet]] });

export {
  empty,
  handleEntry,
  handleInSequence,
  merged,
  nested,
  onCompleteEntry,
  progressEntry,
  sequenceEntry,
  swell,
  traced,
  tracedSequence,
};
