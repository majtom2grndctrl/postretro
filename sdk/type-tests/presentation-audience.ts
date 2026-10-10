import {
  type CrossingParams, type EmitterParams, type PlayerEventParams,
  type PrimitiveReactionDescriptor, type Reaction, type TriggerEventParams,
  type TriggerVolumeHandle, type LevelManifest, type BoolRef,
  defineReaction, fire, players, becomes, runtime,
} from "postretro";
import { playSound, rumble, flashScreen, vignette, screenShake, onStateCrossing } from "postretro/ui";

declare const trigger: TriggerEventParams;
declare const player: PlayerEventParams;
declare const emitter: EmitterParams;
declare const plate: TriggerVolumeHandle;
declare const condition: BoolRef;
const activator = defineReaction("activator", playSound("alarm", { audience: trigger.activators }));
const occupants = defineReaction("occupants", rumble(1, 100, undefined, { audience: trigger.trigger }));
const subject = defineReaction("subject", flashScreen([1, 0, 0, 1], 100, { audience: player.player }));
const shared = defineReaction("shared", vignette(1, 100, undefined, { audience: "everyone" }));
const shake = defineReaction((on: TriggerEventParams) => screenShake(1, 100, undefined, { audience: on.activators }));
const positioned = defineReaction("positioned", playSound("door", { at: emitter.emitter, audience: "everyone" }));
plate.on("enter", [activator, occupants, shake, shared]);
players().on(becomes(condition), [subject, shared]);
fire(shared);
const manifest: LevelManifest = { reactions: [activator, occupants, subject, shared, shake, positioned] };
void manifest;
// @ts-expect-error Direct body registration must retain token scope.
fire(activator);
// @ts-expect-error Player audience remains player scoped after direct registration.
fire(subject);
// @ts-expect-error Emitter positioning retains its emitter scope.
fire(positioned);
// @ts-expect-error A trigger scope cannot be erased to a plain primitive body.
const erased: PrimitiveReactionDescriptor = playSound("alarm", { audience: trigger.activators });
// @ts-expect-error A trigger audience cannot be bound to player events.
players().on(becomes(condition), [activator]);
// @ts-expect-error A player audience cannot be bound to a trigger.
plate.on("enter", [subject]);
// @ts-expect-error Crossings publish no audience token, including captured tokens.
defineReaction((on: CrossingParams) => playSound("alarm", { audience: trigger.activators }));
// @ts-expect-error A player tracer cannot launder a captured trigger audience.
defineReaction((on: PlayerEventParams) => flashScreen([1, 1, 1, 1], 100, { audience: trigger.trigger }));
// @ts-expect-error Shared named events publish an emitter, no player audience.
defineReaction((on: EmitterParams) => vignette(1, 100, undefined, { audience: player.player }));
// @ts-expect-error A crossing cannot consume a direct token-audience handle.
onStateCrossing(runtime.constant(true), [activator]);
// @ts-expect-error Unknown audience strings are not tokens.
playSound("alarm", { audience: "activator" });
// @ts-expect-error Emitter anchors are not presentation recipients.
screenShake(1, 100, undefined, { audience: emitter.emitter });
// @ts-expect-error A scoped direct-body reaction cannot erase its scope.
const erasedReaction: Reaction<{}> = occupants;
