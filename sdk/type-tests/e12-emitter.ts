import {
  type CrossingParams,
  type EmitterParams,
  type Reaction,
  type TriggerEventParams,
  defineReaction,
  fire,
} from "postretro";
import { playSound } from "postretro/ui";

// A named gameplay event publishes its emitter; `at` positions the sound there.
const doorOpen: Reaction<EmitterParams> = defineReaction("door.open", (on: EmitterParams) =>
  playSound("sfx/door_open", { at: on.emitter }),
);

// `at` pairs with the SFX bus; an unanchored sound may name any bus.
defineReaction((on: EmitterParams) => playSound("sfx/thud", { bus: "sfx", at: on.emitter }));
defineReaction("lowHealthAlert", playSound("sfx/test_tone", { bus: "sfx" }));

// @ts-expect-error The emitter is the only anchor token; trigger activators are not one.
defineReaction((on: TriggerEventParams) => playSound("sfx/thud", { at: on.activators }));

// @ts-expect-error Crossings publish no emitter.
const invalidCrossingEmitter = (on: CrossingParams) => on.emitter;

// @ts-expect-error `fire()` dispatches with no emitter, so an emitter-reading reaction cannot be fired.
const invalidFireOfEmitterReaction = fire(doorOpen);

// @ts-expect-error The bus moved into the options object.
playSound("sfx/test_tone", "sfx");
