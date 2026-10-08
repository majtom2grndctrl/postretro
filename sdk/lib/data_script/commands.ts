// Command targets: the `npcs` and `players` groups and the trigger-event
// subject tokens (`on.activators`, `on.trigger`). Each carries exactly the
// verbs its kind supports; a verb builds the closed wire descriptor.
// See: context/lib/scripting.md §12 (Entity addressing)

import type { PrimitiveReactionDescriptor, SequenceStep, StateRef } from "../data_script";

/** Kinds a group command addresses. */
export type GroupKind = "npc" | "player";

/**
 * One group command: a primitive descriptor that carries `kind` (and an
 * optional `tag` filter) instead of a target. It is legal both as a reaction
 * body and directly as a sequence entry; the group resolves when the command
 * takes effect, so NPCs spawned or players joined before then are included.
 */
export type GroupCommand = {
  primitive: string;
  kind: GroupKind;
  tag?: string;
  args: Record<string, unknown>;
};

/** Selects the NPCs a group command reaches. Omit `tag` for every NPC. */
export type NpcGroupFilter = { tag?: string };

/** Typed, additive partial for NPC state updates. */
export type NpcStateUpdateArgs = { aggro?: boolean };

/**
 * Brain-driven characters that are not players, whatever their faction
 * sentiment. Opaque: no members, no length. Commands run on the host and in
 * single player only.
 */
export interface NpcGroup {
  /** Apply a typed partial to each NPC's state. */
  update(fields: NpcStateUpdateArgs): GroupCommand;
  /** Damage each NPC by a finite, non-negative amount. */
  damage(amount: number): GroupCommand;
}

/**
 * Every player pawn bound to a seat; a seat in a disconnect hold is skipped,
 * and single player reaches the local pawn. Opaque: no members, no length.
 * Commands run on the host and in single player only.
 */
export interface PlayerGroup {
  /** Damage each player by a finite, non-negative amount. */
  damage(amount: number): GroupCommand;
  /** Add health to each player. */
  grantHealth(amount: number): GroupCommand;
  /** Add `amount` to each player's named ammo-reserve pool. */
  grantAmmo(type: string, amount: number): GroupCommand;
  /** Add `delta` to each player's value of a per-owner numeric slot. */
  addSlot(slot: StateRef<number>, delta: number): GroupCommand;
}

declare const activatorsTargetBrand: unique symbol;
declare const triggerTargetBrand: unique symbol;

/** The pawns that caused the current trigger edge. Legal only before any `wait`. */
export interface ActivatorsTarget {
  readonly [activatorsTargetBrand]: true;
  /** Damage this fire's activators. */
  damage(amount: number): PrimitiveReactionDescriptor;
  /** Add health to this fire's activators. */
  grantHealth(amount: number): PrimitiveReactionDescriptor;
  /** Add to this fire's activators' named ammo-reserve pool. */
  grantAmmo(type: string, amount: number): PrimitiveReactionDescriptor;
  /** Add `delta` to this fire's activators' value of a per-owner numeric slot. */
  addSlot(slot: StateRef<number>, delta: number): PrimitiveReactionDescriptor;
}

/** The trigger volume that fired the current edge. Legal only before any `wait`. */
export interface TriggerTarget {
  readonly [triggerTargetBrand]: true;
  /** Arm the volume that fired and clear its once/rearm state. */
  arm(): SequenceStep[];
  /** Disarm the volume that fired. */
  disarm(): SequenceStep[];
}

type CommandBuilder<R> = (primitive: string, args: Record<string, unknown>) => R;

// The four player-facing verbs, shared by `players()` and `on.activators`;
// only the addressing each stamps differs.
function playerVerbs<R>(make: CommandBuilder<R>) {
  return {
    damage: (amount: number): R => make("applyDamage", { amount }),
    grantHealth: (amount: number): R => make("grantHealth", { amount }),
    grantAmmo: (type: string, amount: number): R => make("grantAmmo", { type, amount }),
    addSlot: (slot: StateRef<number>, delta: number): R =>
      make("addSlot", { slot: slot.slot, delta }),
  };
}

function groupCommand(kind: GroupKind, tag?: string): CommandBuilder<GroupCommand> {
  return (primitive, args) =>
    tag === undefined ? { primitive, kind, args } : { primitive, kind, tag, args };
}

/** Address NPCs, optionally narrowed to those carrying `tag`. Resolved when each command takes effect. */
export function npcs(filter?: NpcGroupFilter): NpcGroup {
  const make = groupCommand("npc", filter?.tag);
  return Object.freeze({
    update: (fields: NpcStateUpdateArgs) => make("updateNpcState", fields),
    damage: (amount: number) => make("applyDamage", { amount }),
  });
}

/** Address every seat-bound player pawn. Resolved when each command takes effect. */
export function players(): PlayerGroup {
  return Object.freeze(playerVerbs(groupCommand("player")));
}

// Subject tokens lower to the existing sentinel wire: `target: "@activators"`
// on a primitive body, `id: "@trigger"` on a sequence step.
export const ACTIVATORS_TARGET = Object.freeze(
  playerVerbs<PrimitiveReactionDescriptor>((primitive, args) => ({
    primitive,
    target: "@activators",
    args,
  })),
) as unknown as ActivatorsTarget;

export const TRIGGER_TARGET = Object.freeze({
  arm: (): SequenceStep[] => [{ id: "@trigger", primitive: "armTrigger", args: {} }],
  disarm: (): SequenceStep[] => [{ id: "@trigger", primitive: "disarmTrigger", args: {} }],
}) as unknown as TriggerTarget;
