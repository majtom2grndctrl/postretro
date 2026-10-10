# E16 — Persistent Firing Modes

Brief · compact · draft · Epic 16 · reads: `context/lib/entity_model.md` §2, `context/lib/scripting.md` §5, §11–12, `context/lib/input.md` §2, `context/lib/networking.md` §Combat authority, §Host input command queue · read at `ecc6adc` · evidence: `research.md`

Builds after `context/plans/in-progress/E16--player-events` lands: the selected mode is one of its per-player owner-private engine values. Builds on `context/plans/done/E16--weapon-activations`.

## Problem

A requested capability, anticipated by the owner: no modder has hit it yet. Activations let an author describe single shots, bursts, held repetition and charge, but primary and secondary are fixed per weapon. A player cannot switch a live weapon between authored configurations — semi/burst/auto, or a weapon mod that changes what alt-fire does. Game designers want different blends: firing modes only, alt-fire only, later ADS only, or a mix. When done, a designer declares a weapon's modes, chooses how the player changes them, and each activation runs the selected mode's programs on host and owner prediction alike.

## Decisions

- **Mechanism, not policy.** The engine names no modes. Modes are authored descriptor data, validated at install in TypeScript and Luau; Rust runs the existing bounded activation language. No burst opcode, gameplay callback, state-store expression or runtime VM (`scripting.md` §1, `plans/done/E16--weapon-activations`). The selection rules below are engine policy on purpose: selection is predicted in-tick and ordered against starts, which deferred reactions cannot do (`scripting.md` §12).
- **A mode overrides primary, secondary, or both.** An omitted lane falls back to the weapon's own action, so `primary` stays required and descriptors without modes keep current behavior. A mode's secondary may be an activation or a selector. Combat bases, resource, reload, placement and bloom stay weapon-wide; existing shot scales cover per-mode tuning. Every mode spends the weapon's one resource (`entity_model.md` §Weapon resources).
- **Input is separate from what it does.** The engine adds one gameplay command, `fire_mode_cycle`, relevant when any weapon declares two or more modes. Independently, alt-fire routes per weapon or mode to a secondary activation, `cycle`, or `select(id)`. Alt-fire stays relevant when any route uses it, which amends the relevance rule in `input.md` §2. The route set is closed and grows additively; ADS adds an `aim` route and its own command later. This lets a designer ship modes on a dedicated key with alt-fire free, modes on alt-fire, or a weapon mod whose alt-fire differs per mode.
- **One fresh press, one selection.** Holding never repeats. Cycle advances in declaration order and wraps. Selecting the current mode is an accepted no-op: no cue, no delay. A selector press consumes the same command's fresh primary and secondary starts, as secondary already wins simultaneous starts (`entity_model.md` §Weapon activations).
- **Idle only, never queued.** Selection is accepted only while the active weapon is idle and no equip transition, reload or fresh reload press owns the tick. Otherwise it is consumed and rejected; cancellation wins over selection in one command. Selection is allowed during recovery, with an empty magazine or cell, and while overheated.
- **Authored switch delay.** A weapon-wide flat stat `modeSwitchMs`, default 0, beside `raiseMs`/`lowerMs`. An accepted change raises the shared recovery to at least that value, so the existing predicted cooldown and host correction carry it; selection never clears or refunds recovery. As a flat stat it is a target for the future stat-modifier seam (`effective()`, roadmap augments) like any other.
- **Each activation captures its resolved action.** A start freezes the action it resolved — a per-weapon action identity built at install — not the mode ID, so modes sharing a lane's action never disagree. Charge release, later due shots, correction and delayed impacts use the captured action. A later selection or correction never retargets a live execution. Only a correction proving the owner started a different action cancels remaining predicted work; it never replays damage or resurrects a projectile. A later ADS aim layer or AI choice resolves into the same identity.
- **Selection belongs to the weapon instance.** Not to the player, archetype or slot (`research/weapon-model.md` §2). Holster, drop and reacquire by any player preserve it; despawn destroys it; a fresh spawn starts at the declared default. Death, suspension and disconnect cancel transient work and leave a retained instance's selection.
- **Restore and carry.** Serialization keeps the stable mode ID; restore validates it, rebuilds caches and never resumes a serialized execution. Hot replacement cancels execution and keeps a still-valid ID. An unknown or missing ID warns once and falls back to the default. The carried loadout gains each slot's mode ID, amending "carried loadout keeps magazines only" (`entity_model.md` §Weapon resources); new or missing weapons use their default.
- **Host-authoritative, owner-predicted, in-band.** The owner predicts an admitted press on a real fixed command. The press rides the input command as a press-lane edge, like reload, use and drop: observed before trim, delivered once, in the same client-tick order as activation starts (`networking.md` §Host input command queue). A Control declaration is rejected: switches' known gap would let a following shot reach the host before the selection. The host's acceptance or refusal rides the reliable owner-outcome stream. Duplicate, stale or reordered requests cannot cycle twice, rewind a newer selection, or reach a replacement weapon or new owner. Mode definitions reach the client only through host tuning; a HIT declaration cannot choose shot tuning.
- **The selected mode is an owner-private per-player engine value.** It joins E16--player-events' catalog through its one pawn-to-value lookup. That one value repairs the owner on admission, resume and client re-materialization; makes the mode readable per player on the host and in `players().on`; and backs the HUD. The owner's HUD shows its prediction immediately and converges silently on correction. Label and icon are local facts derived from the ID and installed tuning. HUD branches on the string ID with `stateEquals`.
- **Conditions read modes through an install-resolved number.** The condition algebra is number and bool only (`plans/done/M14--behavior-ir-substrate`), so `fireMode.is(id)` compiles at install to a numeric comparison against a companion value. Every mode ID across installed descriptors is interned game-wide, never per-weapon position, so `is("burst")` means the active weapon's mode is named `burst` on any weapon. An ID no installed weapon declares fails install, naming the condition. The number is never authored, persisted or carried; the string ID stays the identity. In co-op the interning follows host tuning, as relevance does.
- **One positioned cue per accepted change.** It plays at the pawn emitter, locally and to observers through a new observer weapon cue kind; acknowledgement never replays it. Rejection and no-op stay silent. Fire and impact cues keep their captured mode's sounds.
- **Bounds.** At most 16 modes per weapon. IDs are unique, nonempty ASCII of at most 64 bytes in `[A-Za-z0-9_.:-]`, matching existing content identifiers. Default and `select` targets must exist. Each mode's programs obey existing activation ceilings. Request retention is finite with a safe refusal policy; render-only frames advance no selection.
- **Non-goals.**
  - AI-chosen modes. AI-wielded modal weapons fire the declared default. Enemy mode choice, with a statechart windup matching `modeSwitchMs` (`entity_model.md` §7c), is a follow-up AI brief.
  - ADS itself. This brief reserves the route; view, FOV and accuracy belong to the roadmap ADS item.
  - A stat-modifier or progression system. `modeSwitchMs` is a flat stat; augments own modification, and own how augments compose with modes: modes vary programs and shot scales, augments weapon-wide bases.
  - Mode availability or unlocks. Additive later: cycle skips an unavailable mode, `select` refuses it.
  - Per-mode resources, reload, placement or damage bases; per-mode commands or `select` on a dedicated binding; hold-to-cycle; queued changes.
  - A disk save-game. Component serde and carried loadouts are covered; general save/load stays an engine non-goal.
  - A reference weapon overhaul or new HUD design. One fixture weapon proves the feature.

### Scripting surface

```ts
import { activation, defineEntity, fireMode } from "postretro";

export const selectorRifle = defineEntity({
  canonicalName: "selector_rifle",
  components: {
    weapon: {
      // ...existing combat, resource and placement fields
      primary: { trigger: "press", recoveryMs: 120, steps: [activation.shot()] },
      secondary: fireMode.cycle(),          // or fireMode.select("auto"), or an activation
      modeSwitchMs: 250,
      sounds: { modeSelect: "weapons/selector_click" },
      modes: {
        default: "semi",
        list: [
          { id: "semi", label: "SEMI", icon: "ui/modes/semi.png" },
          { id: "burst", label: "BURST", primary: { trigger: "press", recoveryMs: 400,
              steps: [activation.shot(), activation.wait(60), activation.shot(), activation.wait(60), activation.shot()] } },
          { id: "auto", label: "AUTO", primary: { trigger: "hold", recoveryMs: 90, steps: [activation.shot()] } },
          // Alt-fire launches in this mode; `fire_mode_cycle` still leaves it.
          { id: "launcher", label: "GL", secondary: { trigger: "press", recoveryMs: 900, steps: [activation.shot()] } },
        ],
      },
    },
  },
});
```

Luau mirrors it with `Postretro.fireMode.cycle()` and the same table shape. HUD reads `player.weapon.mode`, `player.weapon.modeLabel` and `player.weapon.modeIcon`.

## Acceptance

### Automated
- [ ] **AC1 — Authoring.** The Scripting surface example installs as a TS fixture with an equivalent Luau fixture. Invalid default or `select` target, duplicate or invalid ID, excess mode count, invalid mode program and invalid `modeSwitchMs` fail with field paths. Existing descriptors and activation tests keep behavior unchanged.
- [ ] **AC2 — Input composition.** `fire_mode_cycle` is bound and listed only when a weapon declares two or more modes. Alt-fire is relevant when any route uses it and unbound when none does. Cycle on the command and on alt-fire wraps in order; `select` reaches its target. A held selector changes once; a no-op plays no cue and adds no delay. A selector press with a fresh primary on one tick selects once and fires nothing. A press/release pair between fixed ticks survives to the next real command.
- [ ] **AC3 — Gates and delay.** Busy, switching, reload, fresh-reload and cancel cases reject with no queued selection. Selection during recovery, depletion and lockout succeeds and preserves resources and bloom. With `modeSwitchMs` 0 the next start obeys only existing recovery. With a positive value, recovery becomes at least that value and is never shortened. Primary and secondary both obey it.
- [ ] **AC4 — Captured execution.** Burst and charge started in one mode keep its program, scales and sounds through delayed outcomes after a selection change. A mode overriding only secondary leaves primary on the weapon action. A mismatched authoritative mode cancels remaining prediction without replaying damage or resurrecting a projectile.
- [ ] **AC5 — Instance lifetime.** Two instances of one archetype select independently. Holster, drop to another player, death/suspension/disconnect on retained instances, carried level transition, hot replacement and component round-trip follow the stated policy. Despawn and reuse inherit nothing. Unknown or missing IDs default with one warning; serialized executions never resume.
- [ ] **AC6 — Co-op convergence.** Host/client tests cover rapid select→fire, several changes before acknowledgement, rejection, duplicate/stale/reordered requests, admission and resume repair, client re-materialization after reacquire, differing local content, tick wrap and session replacement. Both sides agree on mode and host-authorized program; old requests never rewind a newer selection.
- [ ] **AC7 — Per-player value and presentation.** On the host, the selected mode reads per player. A `players().on(becomes(fireMode.is("burst")))` event fires once for the changing player only, on two different weapons declaring `burst` at different positions; an undeclared ID fails install. HUD mode, label and icon follow the active weapon on single-player, host and connected owner, never another instance's. One accepted change plays one positioned cue locally and to observers, none on acknowledgement or rejection.
- [ ] **AC8 — Bounds and delivery.** Boundary-size fixtures prove finite descriptor work and request retention. Render-only frames change no simulation state. SDK types, author docs and context contracts ship; author-doc commands run from an SDK bundle.

### Manual
- [ ] Conditioned-link co-op playtest: host and client switch semi/burst/auto and the launcher mode under load; selection feels immediate, firing matches the shown mode, and observers hear the cue.

## Path

- Seams are in `research.md`. The reload/use/drop press lane (`PressEdges`, `PressGate`) is the intake precedent; `ActivationOutcome` is the acknowledgement precedent. The switch lane's client rollback chain (`PendingSwitchDeclaration`) is a precedent for several unacknowledged changes.
- The cursor stores a lane, not a program (`ActivationCursor`); capture must bind the resolved action at start, or a change retargets live work. Activation shot keys (pawn, tick, lane, ordinal) carry no action; host ordinal validation must use the captured action.
- Recovery precedent: `WEAPON_COOLDOWN_SLOT` and `reconcile_client_weapon_cooldown_from_slot_table` already predict and correct cooldown.
- Rivals rejected. Secondary as an exclusive selector role: takes alt-fire from ADS, dual-wield and deployables, and cannot express a mode that changes alt-fire. Mode policy in script (a settable per-instance mode driven by reactions): reactions run at the frame-end drain without prediction. Selection as a Control declaration: see Decisions.
- First slice: the descriptor plus a single-player cycle that captures mode at start. It falsifies the capture seam before any netcode.
- Split before extending `entities/src/components/weapon.rs`, `foundation/src/data_descriptors/types/combat.rs` and `netcode/src/client.rs`, behavior-preserving, own commit.
- Retire the unused `FireMode` enum in `combat.rs` and its test helpers.

## Open questions

- Confirm idle-only selection, selection during recovery, and preservation across drop and level carry — owner — **blocks build**
- Engine default bindings for `fire_mode_cycle`, keyboard and gamepad, conflict-free with existing defaults — **delegated**
- Exact HUD slot names, observer cue kind spelling, wire/tuning/schema version bumps — **delegated**

## Boundary inventory

| Name | Rust | Wire / tuning | TS | Luau |
|---|---|---|---|---|
| mode set | descriptor modes on `WeaponDescriptor` | host tuning payload (epoch bump) | `modes: { default, list }` | same table |
| selector route | closed secondary variant | host tuning | `fireMode.cycle()`, `fireMode.select(id)` | `Postretro.fireMode.*` |
| switch delay | flat weapon stat | host tuning | `modeSwitchMs` | same |
| selection request | host-applied, client-tick ordered | input-command press edge; outcome on owner-outcome stream (wire bump) | — | — |
| selected mode | owner-private per-player engine value | owner-private state slot | `player.weapon.mode` readonly | same |
| mode condition | interned game-wide numeric companion; never persisted | follows host tuning | `fireMode.is(id)` → `BoolRef` | `Postretro.fireMode.is(id)` |
| mode label / icon | local derived facts | none | `player.weapon.modeLabel`, `modeIcon` | same |
| select cue | observer weapon cue kind | reliable observer cue (wire bump) | `sounds.modeSelect` | same |
| carried mode | `CarriedState` per-slot ID | none (host-local) | — | — |
