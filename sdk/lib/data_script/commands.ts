// Command targets: the `npcs` and `players` groups and the subject tokens
// (`on.activators`, `on.trigger`, `on.player`). Each carries exactly the verbs
// its kind supports; a verb builds the closed wire descriptor.
// See: context/lib/scripting.md §12 (Entity addressing)

import type { StateRef } from "../data_script";
import { playerEvent } from "./player_events";
import type {
  PlayerEventDescriptor,
  PlayerEventEdge,
  PlayerEventOptions,
  PlayerEventReaction,
} from "./player_events";

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
  /**
   * Fire `fire` once per player whenever `edge`'s condition crosses for that
   * player. Evaluated on the host (and in single player) once per tick; a
   * connected client registers nothing. Effective only when returned under a
   * manifest's `playerEvents`; `options.levels` scopes a `ModManifest` entry.
   */
  on(edge: PlayerEventEdge, fire: PlayerEventReaction[], options?: PlayerEventOptions): PlayerEventDescriptor;
}

/** The subject a token command addresses: this fire's activators, the volume that fired, or the event's player. */
export type SubjectTokenTarget = "@activators" | "@trigger" | "@player";

/**
 * One subject-token command: a primitive descriptor whose `target` names the
 * fire's subject. Like a group command, it is legal both as a reaction body
 * and directly as a sequence entry — but only before any `wait`, because the
 * fire context does not survive one.
 */
export type SubjectTokenCommand = {
  primitive: string;
  target: SubjectTokenTarget;
  args: Record<string, unknown>;
};

declare const activatorsTargetBrand: unique symbol;
declare const triggerTargetBrand: unique symbol;
declare const playerTargetBrand: unique symbol;

/** The pawns that caused the current trigger edge. Legal only before any `wait`. */
export interface ActivatorsTarget {
  readonly [activatorsTargetBrand]: true;
  /** Damage this fire's activators. */
  damage(amount: number): SubjectTokenCommand;
  /** Add health to this fire's activators. */
  grantHealth(amount: number): SubjectTokenCommand;
  /** Add to this fire's activators' named ammo-reserve pool. */
  grantAmmo(type: string, amount: number): SubjectTokenCommand;
  /** Add `delta` to this fire's activators' value of a per-owner numeric slot. */
  addSlot(slot: StateRef<number>, delta: number): SubjectTokenCommand;
}

/**
 * The player a player event fired for (`on.player`): a command target, and
 * the owner `byPlayer(on.player)` reads. Legal only before any `wait`, and
 * only in reactions a player event fires.
 */
export interface PlayerTarget {
  readonly [playerTargetBrand]: true;
  /** Damage this event's player. */
  damage(amount: number): SubjectTokenCommand;
  /** Add health to this event's player. */
  grantHealth(amount: number): SubjectTokenCommand;
  /** Add to this event's player's named ammo-reserve pool. */
  grantAmmo(type: string, amount: number): SubjectTokenCommand;
  /** Add `delta` to this event's player's value of a per-owner numeric slot. */
  addSlot(slot: StateRef<number>, delta: number): SubjectTokenCommand;
}

/** The trigger volume that fired the current edge. Legal only before any `wait`. */
export interface TriggerTarget {
  readonly [triggerTargetBrand]: true;
  /** Arm the volume that fired and clear its once/rearm state. */
  arm(): SubjectTokenCommand;
  /** Disarm the volume that fired. */
  disarm(): SubjectTokenCommand;
}

type CommandBuilder<R> = (primitive: string, args: Record<string, unknown>) => R;

// The four player-facing verbs, shared by `players()`, `on.activators` and
// `on.player`; only the addressing each stamps differs.
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
  return Object.freeze({ ...playerVerbs(groupCommand("player")), on: playerEvent });
}

// Subject tokens lower to one descriptor, `{ primitive, target, args }`, used
// unchanged as a reaction body or a sequence entry.
function tokenCommand(target: SubjectTokenTarget): CommandBuilder<SubjectTokenCommand> {
  return (primitive, args) => ({ primitive, target, args });
}

export const ACTIVATORS_TARGET = Object.freeze(
  playerVerbs(tokenCommand("@activators")),
) as unknown as ActivatorsTarget;

// `__wire` (non-enumerable) lets an engine state ref's `byPlayer`, built
// before this module loads, recognize the token without seeing it.
const playerTarget = playerVerbs(tokenCommand("@player"));
Object.defineProperty(playerTarget, "__wire", { value: "@player", enumerable: false });
export const PLAYER_TARGET = Object.freeze(playerTarget) as unknown as PlayerTarget;

const triggerCommand = tokenCommand("@trigger");
export const TRIGGER_TARGET = Object.freeze({
  arm: () => triggerCommand("armTrigger", {}),
  disarm: () => triggerCommand("disarmTrigger", {}),
}) as unknown as TriggerTarget;
