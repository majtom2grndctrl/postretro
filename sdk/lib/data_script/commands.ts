// Target/command builders over subject tokens and tags: damage, resource
// grants, per-owner slot deltas, and fired-trigger arm/disarm.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

import type {
  ActivatorsTarget,
  PrimitiveReactionDescriptor,
  SequenceStep,
  StateRef,
  TriggerTarget,
} from "../data_script";
import { ACTIVATORS_TARGET, TRIGGER_TARGET } from "./reactions";

/** Apply damage to the current trigger activators or every entity with a tag. */
export function damage(target: ActivatorsTarget | string, amount: number): PrimitiveReactionDescriptor {
  if (typeof target === "string") {
    return { primitive: "applyDamage", tag: target, args: { amount } };
  }
  const wireTarget = target === ACTIVATORS_TARGET ? "@activators" : "@invalid";
  return { primitive: "applyDamage", target: wireTarget, args: { amount } } as PrimitiveReactionDescriptor;
}

/** Grant health to the current trigger activators or every entity with a tag. */
export function grantHealth(
  target: ActivatorsTarget | string,
  amount: number,
): PrimitiveReactionDescriptor {
  if (typeof target === "string") {
    return { primitive: "grantHealth", tag: target, args: { amount } };
  }
  const wireTarget = target === ACTIVATORS_TARGET ? "@activators" : "@invalid";
  return { primitive: "grantHealth", target: wireTarget, args: { amount } } as PrimitiveReactionDescriptor;
}

/** Grant an ammo-reserve pool to the current trigger activators or every entity with a tag. */
export function grantAmmo(
  target: ActivatorsTarget | string,
  type: string,
  amount: number,
): PrimitiveReactionDescriptor {
  if (typeof target === "string") {
    return { primitive: "grantAmmo", tag: target, args: { type, amount } };
  }
  const wireTarget = target === ACTIVATORS_TARGET ? "@activators" : "@invalid";
  return {
    primitive: "grantAmmo",
    target: wireTarget,
    args: { type, amount },
  } as PrimitiveReactionDescriptor;
}

/** Add a delta to a per-owner numeric slot for the current trigger activators or every pawn with a tag. */
export function addSlot(
  target: ActivatorsTarget | string,
  slot: StateRef<number>,
  delta: number,
): PrimitiveReactionDescriptor {
  if (typeof target === "string") {
    return { primitive: "addSlot", tag: target, args: { slot: slot.slot, delta } };
  }
  const wireTarget = target === ACTIVATORS_TARGET ? "@activators" : "@invalid";
  return {
    primitive: "addSlot",
    target: wireTarget,
    args: { slot: slot.slot, delta },
  } as PrimitiveReactionDescriptor;
}

/** Arm the trigger volume that fired the current event. */
export function armTrigger(target: TriggerTarget): SequenceStep[] {
  const wireTarget = target === TRIGGER_TARGET ? "@trigger" : "@invalid";
  return [{ id: wireTarget, primitive: "armTrigger", args: {} } as SequenceStep];
}

/** Disarm the trigger volume that fired the current event. */
export function disarmTrigger(target: TriggerTarget): SequenceStep[] {
  const wireTarget = target === TRIGGER_TARGET ? "@trigger" : "@invalid";
  return [{ id: wireTarget, primitive: "disarmTrigger", args: {} } as SequenceStep];
}
