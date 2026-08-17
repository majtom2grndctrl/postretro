# Enemy Attack Modes / Stances — Design Intent

> **Status:** design intent / forward-looking — **NOT a ready spec.** Records how "attack modes"
> (combat stances) grow on top of the shipped multi-attack `attacks` vocabulary, and which parts are
> authoring convention versus genuinely-new engine primitives. Feeds **Hierarchical behavior
> (statecharts)** and **Epic 16 › combat stances** (`context/plans/roadmap.md`). Built
> demand-driven, with real consumers — never ahead of need.

## What a "mode" is

A stance/mode is a **named constraint-set over the `attacks` vocabulary** — the graph-wide named
attack map that `E10--enemy-multi-attack` ships (`components.behavior.attacks`). A mode names the
subset of attacks it may fire, the standoff it holds, and how it exits. It is the combat analogue of
a movement stance: the same enemy behaves as a different fighter depending on which mode is active.

A mode carries, roughly:

- **Allowed attacks** — a subset of the `attacks` map (by name), the way a graph state's `action`
  already names one attack. A mode widens "one attack per state" to "a set per mode."
- **Per-mode engagement radius** — where the enemy stands while in this mode. Multi-attack already
  attaches an engagement radius per attack entry; a mode generalizes it to a per-mode standoff that
  can differ from any single attack's reach (a grenade mode stands far, a melee mode crowds).
- **A change cost / telegraph** — switching modes is not free. A raised-launcher wind-up, a stance
  animation, a brief commit window before the new mode's attacks are available.
- **Max attacks before a forced switch** — fire N attacks in this mode, then rotate out regardless
  of distance, so an enemy does not machine-gun one attack forever.

## Worked example: the ogre

- **Grenade mode.** Engagement radius far. On entry, raise the launcher (telegraph). Fire the
  `lob` attack once. Then leave the mode.
- **Exit routing.** If the target is now close, switch to **melee mode** (crowd in, swing). If the
  target is far, drop to a plain `walk` state and re-close.

The feel that makes this read as a real fighter — the launcher *raises* (telegraph), the ogre is
*committed* to the lob once it starts (no bail mid-animation), it *recovers* before the next action,
and it *rotates* grenade → melee → grenade rather than spamming one — is exactly the
windup→commit→recover, forced-rotation shape the flat graph deliberately does not express.

## The finding: ~80% convention, two new primitives, one new mechanism

Most of "attack modes" is **authoring convention on the shipped FSM**: a mode is a cluster of
states, mode entry/exit is ordered transitions, allowed-attack subsets are which states declare
which `action: { attack }`, and per-mode standoff is the per-attack engagement radius multi-attack
already ships. Roughly four-fifths of the surface needs no new engine feature.

Two **genuinely-new primitives** are missing:

1. **Per-mode engagement radius** — mostly delivered. Multi-attack ships an engagement radius per
   attack entry (hence per firing state); a mode needs it per *mode* (a standoff decoupled from any
   one attack's reach). A small generalization of a shipped field, not a new subsystem.
2. **An attacks-fired-in-mode counter fact** — genuinely absent. Forced rotation ("fire N, then
   switch") needs a count of attacks fired since entering the mode, readable as a guard. None of the
   13 registered `@brain.*` guard inputs (`BRAIN_INPUTS`, `crates/foundation/src/brain.rs`) is a
   counter: `timeInStateMs` and `attackCooldownMs` are the closest, and both are elapsed-time
   scalars, not event counts. A counter fact is new authored-guard vocabulary.

The rest — the **system feel** — is not a fact or a field. Windup→commit→recover, forced rotation,
and commit-then-lunge all require a brain to enter a phase it **cannot be routed out of** until the
phase completes. The flat graph evaluates every guard every tick and always takes the first true
one, so it has no way to hold a commit. That hold is the nested/scoped-state mechanism that
**Hierarchical behavior (statecharts)** exists to provide (`roadmap.md`: "a nested graph when a
layer needs its own state (attack windup→commit→recover)"). The lunge impulse itself is
**Epic 16 › Resolution Modes › melee** ("melee and quick-melee with a lunge, a combat↔movement
impulse").

## Boundary discipline

Modes ride the shipped vocabulary; they do not replace it. The `attacks` map stays the single
authored substrate, referenced by name — statecharts group it per activity additively, and a mode
is one such grouping plus an exit rule. New surface (the counter fact, any per-mode standoff field)
is an API contract: exposing it updates SDK types, validators, and defaults in the same pass
(`index.md` §2, *Primitive surface is a contract*). Selection correctness and determinism stay
engine-owned; taste — which attacks a mode allows, how it telegraphs, when it rotates — stays
authored.

This note is grounding for the statecharts and combat-stances specs, not a spec itself. It fixes the
vocabulary and the convention/primitive/mechanism split so those specs do not re-derive it.
