# E16 — Persistent Firing Modes

Brief · compact · draft · Epic 16 · reads: `context/lib/entity_model.md` §2, §5, `context/lib/scripting.md` §5, §11–12, `context/lib/input.md` §2, §5, `context/lib/networking.md` §Combat authority · grounded at `a6074007e` on `codex/weapon-activations` · evidence: `research.md`

**Prerequisite:** [weapon activations, secondary fire, and charge — PR #549](https://github.com/majtom2grndctrl/postretro/pull/549). This implementation is unmerged at drafting time. Main lacks it. Build this brief after that prerequisite lands; re-ground the seams against its merged form.

## Problem

An author can describe a single shot, a timed burst, held repetition, or a charged shot with the activation language. Primary and secondary still resolve to fixed actions. Secondary cannot select a configuration that persists and changes later primary activations. A weapon therefore cannot offer an authored semi/automatic/burst selector without adding another engine feature or pretending an author-time script runs during play.

## Outcome

Secondary can select or cycle author-defined firing modes. Primary starts the selected mode's activation program. Selection belongs to the weapon instance, survives ordinary wielding changes, and stays consistent between host authority and the owner's prediction in friends-only PvE. Authors name and present the modes; Rust executes the existing bounded activation language.

## Scope and constraints

- Opt-in mode set with a declared default and ordered, stable mode IDs. Each mode supplies a primary activation and optional authored display label, icon, and selection sound. Modes vary trigger, charge, sequence, scales, and action presentation through the existing activation vocabulary. Weapon-wide combat bases, resource kind, reload tuning, and placement remain shared.
- Secondary has one role per descriptor: ordinary firing action, cycle through declared modes, or select one named mode. A selector and a firing secondary are mutually exclusive. A descriptor without modes retains its current primary/secondary behavior.
- All declarations, references, and bounds validate at install in TypeScript and Luau. The VM drops after authoring. No gameplay callback, arbitrary state-store expression, new burst opcode, or unbounded program is introduced.
- One component-owned selection per live weapon. No global, per-player, per-archetype, or inventory-slot selector. Resource balances, bloom, reload, and recovery are never cloned per mode.
- Host-selected mode definitions reach connected prediction through host tuning. Clients cannot use differing local combat definitions as a fallback. Mode selection cannot let a HIT declaration choose shot tuning.

## Proposed behavior for review

These are the brief's default semantics, not existing APIs.

1. **One press, one selection.** A secondary selector uses a fresh press only. Holding it never cycles repeatedly. Cycling advances in declaration order and wraps. Selecting the current ID is an accepted no-op with no selection cue. Secondary wins simultaneous fresh primary/secondary starts; that primary press is consumed.
2. **No interruption or queue.** A selector is accepted only while the active weapon is idle and no equip transition, reload, or fresh reload press owns the tick. A press while charging, executing, reloading, or switching is consumed and rejected. Cancellation wins over selection in the same command. Rejected presses never become delayed selections.
3. **Shared recovery remains owed.** An idle weapon may change mode during recovery, with an empty magazine/cell, or while overheated. Selection spends nothing, fires nothing, and neither clears nor adds recovery. The next primary activation must pass the ordinary shared gate. A fresh primary press blocked by recovery remains consumed; held repetition follows the selected mode's trigger.
4. **Each activation captures its mode.** Starting primary freezes the mode ID and installed action/program for that activation. Charge release, later due shots, predicted shot correction, and delayed impacts use that captured identity. A later authoritative selection correction cannot substitute another mode's program halfway through execution. A correction proving the prediction used the wrong mode cancels remaining predicted work and corrects future starts; it never replays damage or resurrects a projectile.
5. **Lifecycle follows the instance.** Holster/re-equip and successful drop/reacquire preserve selection, including acquisition by another player. Existing cancellation stops future shots and retains paid resources/recovery. Despawn destroys selection; a fresh spawn starts at the declared default. Death, input suspension, and disconnect cancel transient work without changing a retained instance's selection.
6. **Restore is explicit.** Component serialization retains the stable selected ID. Restore validates it against installed modes, rebuilds executable caches, and cancels transient charge/execution; it cannot resume a serialized stale shot. In-memory speculative checkpoint/restore also restores selection and captured activation identity together with existing timing/resource state. Same-weapon carried loadouts preserve the ID across level changes; new/missing weapons use their defaults. Hot replacement cancels execution and preserves a still-valid ID; missing or unknown restored IDs warn once and fall back to the declared default. Old data without selection uses the default.
7. **Selection is authoritative and predicted.** The owner predicts an admitted selector press on a real fixed command; the host validates the same instance, lifecycle, and ordering rules. Reliable acknowledgement/correction associates the request and selected ID with the bound weapon instance and content/session lifetime. Duplicate, stale, reordered, or pre-drop packets cannot cycle twice, rewind newer selection, or affect a replacement weapon or new owner. Admission/resume supplies authoritative selection for occupied weapons. Synthetic held/neutral commands create no selection.
8. **Presentation is observable.** Readonly local HUD facts expose the active weapon's selected ID and authored display data on every role, with an empty/no-mode value when absent. The owner sees predicted selection immediately and converges after correction. An actual accepted change emits the authored selection cue once at the weapon's emitter; acknowledgement does not play it again. Rejection/no-op plays none. A correction updates the HUD silently. Other peers hear accepted changes through the existing observer presentation route. Shot/impact sounds continue to follow their captured mode action.
9. **Work is bounded.** At most 16 modes per descriptor. IDs are unique, nonempty ASCII strings of at most 64 bytes, using only `[A-Za-z0-9_.:-]`, matching existing content identifiers; the declared default and selector targets must exist. Existing per-program step, shot, duration, and expression ceilings apply to every mode. Request/correction history has a finite capacity and expiry with a safe refusal/resynchronization policy; render-only frames advance no selector or activation.

## Acceptance

- [ ] **AC1 — Authoring and compatibility.** Equivalent TS/Luau fixtures define semi, automatic, three-shot burst, and charged modes using existing activation builders. Invalid defaults/targets, duplicate or invalid IDs, excessive mode count, conflicting secondary roles, and invalid mode programs fail with field paths. Existing descriptors and their activation tests retain behavior without migration.
- [ ] **AC2 — Selection.** Secondary presses cycle and wrap or select the authored target. A held secondary produces one change; a no-op produces no cue. Simultaneous primary/secondary presses select once and produce no primary shot. A press/release pair captured between fixed ticks survives until the next real command.
- [ ] **AC3 — Gates and resources.** Busy/switching/reload/fresh-reload/cancel cases follow the rules above with no queued selection. Idle recovery, depleted resources, and heat lockout permit selection while preserving all resource/recovery/bloom values. Switching modes never bypasses a subsequent primary gate or refunds a shot.
- [ ] **AC4 — Captured execution.** Burst and charge executions retain the selected program, charge rules, per-shot scales, and presentation through delayed outcomes and impacts. A mismatched authoritative mode cancels remaining prediction without changing another activation, replaying damage, or resurrecting a projectile.
- [ ] **AC5 — Instance lifetime and restore.** Two instances of one archetype select independently. Holster, successful drop to another owner, death/suspension/disconnect on retained instances, carried level transition, hot replacement, component round-trip, and speculative checkpoint/restore obey the stated policy. Despawn/reuse cannot inherit selection or old requests. Missing/unknown restored IDs default safely; serialized executions never resume.
- [ ] **AC6 — Co-op convergence.** Host/client tests cover rapid select→fire, multiple changes before acknowledgement, rejection, duplicate/stale/out-of-order outcomes, lost state repaired on admission/resume, differing local content, tick wrap, drop/reacquire, and instance/session replacement. Both agree on the selected mode and host-authorized shot program; old packets cannot rewind a newer selection. Conditioned-link play-test confirms predictable selection and firing for host and client.
- [ ] **AC7 — Presentation.** Authored HUD label/icon follows the selected mode on single-player, host, and connected owner; switching/absence/correction never shows another instance's mode. Accepted changes play one positioned cue locally and to observers, with no acknowledgement duplicate. Rejected/no-op changes stay silent. Fire/impact cues retain the originating mode.
- [ ] **AC8 — Bounds and delivery.** Boundary-size fixtures prove finite descriptor/program work and request retention. Zero-tick render frames change no simulation state. Focused lifecycle/network tests and required project preflight pass. SDK types, author docs, and surviving context contracts ship with the feature; author-doc commands work from an SDK bundle. No new `unsafe` or GPU ownership change.

## Non-goals

- Engine-named semi/auto/burst modes, new firing opcodes, runtime VM callbacks, or arbitrary mode-switch reactions.
- Per-mode resource pools, reload styles, geometry/resolution changes, damage bases, placement, or inventory replacement. Existing shot scales cover permitted tuning variation.
- Selection during an active execution, queued changes, timed selector animations, hold-to-cycle, or a new input binding/rebinding system.
- Full weapon rollback, new ammo/heat/cell prediction, competitive PvP, server rewind, or remote prediction.
- A disk save-game system or persistence of weapon selection in player settings/state-store files. Existing component and carried-loadout seams are covered; general entity save/load remains an engine non-goal.
- A reference weapon balance overhaul or a new HUD design. One authored fixture/demo is enough to prove the selector and presentation.

## Implementation decisions

Build-time decisions: exact descriptor/builder/HUD names, closed selector data shape, Rust ownership layout, and wire transport/encoding. Pin spelling and identity mapping across serde, TS/Luau, tuning, outcomes, and HUD before code. Stable IDs cross those boundaries; table positions are not saved identity. Change protocol/tuning/schema versions where their contracts change. This brief prescribes correlation and behavior, not packet field order.

Existing entry seams are listed in `research.md`. Selection must join authoritative execution, connected prediction, component restore, carried loadouts, and presentation together. This is a bounded feature across several subsystems, not an assumed small patch. Before extending large modules, split the affected responsibilities along their existing seams; keep those refactors behavior-preserving and directly before feature edits.

**Owner review:** confirm idle-only selection, selection during recovery, and preservation across drop/level carry. The defaults above fully define the draft until review changes them. No implementation task breakdown belongs in this brief.
