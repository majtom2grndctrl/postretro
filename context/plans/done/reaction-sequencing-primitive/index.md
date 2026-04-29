# Reaction Sequencing Primitive

## Goal

Extend the data-script reaction system with ordered, per-entity sequenced
operations. Add a `sequence` reaction shape that targets specific entity IDs
computed inside `registerLevelManifest()`, and ship `setLightAnimation` as the
first concrete sequenced primitive. Migrate the existing light-wave behavior
scripts onto data scripts and retire the behavior-script implementation.

---

## Scope

### In scope

- A `sequence` reaction descriptor shape: an ordered list of per-entity
  operations, each carrying an `EntityId` and primitive-specific arguments
- ID-based targeting on the new `sequence` shape — pure declarative payload,
  no tag writebacks from `registerLevelManifest()`
- `setLightAnimation` as the first sequenced primitive: applies a
  `LightAnimation` to a single entity ID
- `registerLevelManifest()` ergonomics: `world.query()` already exposes
  `EntityId` handles; spec confirms this is the supported sort+order pathway
- SDK type declarations for the `sequence` shape and `setLightAnimation` arg
  payload in `postretro.d.ts` and `postretro.d.luau`
- Migration of the two existing arena light-wave scripts
  (`content/tests/scripts/arena-wave.ts`) to data-script descriptors
- Retirement of the behavior-script light-wave path: delete
  `arena-wave.ts`/`.js` and confirm the data-script path produces the same
  visual outcome
- `sequence` steps do **not** carry an `onComplete` field in this plan;
  per-step completion semantics are deferred (see Open questions)

### Out of scope

- Other sequenced primitives (`moveGeometry` ordered, ordered geometry
  reveals, sequenced spawns) — add when authored maps need them
- Hot reload of data scripts — separate plan
- Per-entity targeting on the existing `primitive` shape — `primitive` stays
  tag-only by design
- Tag writeback API from `registerLevelManifest()` — rejected. This plan
  resolves the *ID-based reaction targeting* open question in `data-script-setup`
  by adopting the ID-based descriptor approach instead
- Runtime-computed sequences (sequences derived from gameplay state after
  level load) — `registerLevelManifest()` is the only producer
- Time-ordered fire-event chains beyond the existing `onComplete` field

---

## Acceptance criteria

- [ ] A data script that returns `registerReaction("levelLoad", { sequence:
  [...] })` with a list of `{ id, primitive: "setLightAnimation", args: {...} }`
  entries fires each operation in list order when the named event dispatches
- [ ] `world.query({ component: "light", tag: ... })` inside
  `registerLevelManifest()` returns handles whose `id` field can be embedded
  directly into a `sequence` step (only the light component path is required
  by this plan)
- [ ] `setLightAnimation` dispatches against an `EntityId`, applies the
  supplied `LightAnimation`, and reports the same errors as the
  `setLightAnimation` primitive (`crates/postretro/src/scripting/primitives_light.rs`)
  (zero-length direction, color on non-dynamic light)
- [ ] A stale `EntityId` (entity destroyed between `registerLevelManifest()`
  and event dispatch) logs a warning and continues with remaining steps — not
  fatal
- [ ] A `sequence` step naming an unknown primitive causes that reaction to be
  dropped from the manifest at `registerLevelManifest()` time, with a clear
  diagnostic naming the step index and offending primitive name. The level
  loads; other reactions in the manifest are unaffected.
- [ ] Rust unit test covers stale-`EntityId` skip-and-log behavior across a
  multi-step sequence — surviving steps still execute
- [ ] The arena 1 and arena 2 light waves render identically before and after
  migration: same period, same pulse shape, same per-light phase ordering
- [ ] `content/tests/scripts/arena-wave.ts` and its compiled `.js` are deleted;
  the test map's `worldspawn` references the new data script via `data_script`
- [ ] `postretro.d.ts` and `postretro.d.luau` declare the `sequence` shape on
  the reaction descriptor union and the `setLightAnimation` step payload
- [ ] After `registerLevelManifest()` returns, the data-script VM context is
  released — no per-frame VM cost for the wave (validated by absence of a
  behavior handler on `levelLoad` for arena lights)

---

## Tasks

### Task 1: Sequence descriptor type and dispatch

Extend the reaction descriptor union in Rust with a `sequence` variant — an
ordered `Vec` of step records. Each step carries an `EntityId`, a primitive
name, and a primitive-specific argument payload. Dispatch evaluates steps in
order, resolves each ID against the entity registry, and invokes the named
primitive. Stale-ID warnings are logged and non-fatal. Unknown primitive names
are reported at `registerLevelManifest()` time, not at dispatch.

Lives in the same module as the reaction registry introduced by
`data-script-setup` Task 4. Adds a Rust-internal sequenced-primitive dispatch
table (separate from the script-facing primitive registry, which stays unchanged).

### Task 2: setLightAnimation sequenced primitive

Register `setLightAnimation` in the sequenced-primitive dispatch table.
Argument payload mirrors the existing `LightAnimation` shape. Implementation
reuses `validate_light_animation` and the registry-write helper currently
exercised by `setLightAnimation` in
`crates/postretro/src/scripting/primitives_light.rs` — same validation, same
errors. Difference is only the registration surface.

Sequenced-primitive dispatch is a Rust-internal call path; the existing
script-facing scope (behavior-only) on `setLightAnimation` is unchanged.

### Task 3: SDK types and authoring helpers

Add the `sequence` shape and `setLightAnimation` step payload to:
- `sdk/types/postretro.d.ts`
- `sdk/types/postretro.d.luau`
- `sdk/lib/light_animation.ts` (or a new sibling file) — thin authoring helper
  for building a step list from a sorted `LightHandle[]` and a stagger function,
  encapsulating the "phase = i / N" pattern from the existing scripts, styled
  parallel to the existing `flicker` / `pulse` helpers

Update the `registerReaction` overload set (extending the discriminated union
introduced by `data-script-setup` Task 5) so authors get type checking on
`sequence` step entries. The `sequence` field is always an array — no
single-step sugar form; `reactions: []` is the only supported syntax regardless
of list length.

After SDK changes, regenerate `sdk/lib/prelude.js` per `scripting.md` §9 and
run `gen-script-types` to refresh the committed `.d.ts` / `.d.luau` files. The
drift-detection test in `cargo test` must pass.

### Task 4: Migrate arena light waves

Rewrite both arena waves as a data script returning two `sequence` reactions
keyed to `levelLoad`. Each reaction's steps come from `world.query()` → sort
by position (sort logic stays in the data script; the helper maps sorted
handles to steps) → map to `{ id, primitive: "setLightAnimation", args: {
periodMs, phase, brightness, ... } }`. Delete the behavior-script
implementation (`content/tests/scripts/arena-wave.ts` and the compiled `.js`).
Wire the new data script into the test map's `worldspawn` (`data_script` KVP)
by locating the existing arena test map under `content/tests/maps/` that today
references `arena-wave` via the legacy `script` KVP.

---

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 3 — independent: dispatch path and
type declarations live in different files.

**Phase 2 (sequential):** Task 2 — depends on Task 1's dispatch table.

**Phase 3 (sequential):** Task 4 — depends on Tasks 1–3 shipping end to end.

---

## Rough sketch

```typescript
// level-data.ts — proposed authoring shape
import { registerReaction } from "postretro";
import { world } from "./sdk/world";

export function registerLevelManifest(ctx) {
    const arena1 = world
        .query({ component: "light", tag: "arena_1_light" })
        .sort(/* angle around centroid, anchored NW — sort stays in script */);

    const steps = arena1.map((light, i) => ({
        id: light.id,
        primitive: "setLightAnimation",
        args: { periodMs, phase: (i * lightSpacingMs) / periodMs, /* ... */ },
    }));

    return {
        reactions: [
            registerReaction("levelLoad", { sequence: steps }),
        ],
    };
}
```

**Why ID-based, not tag writeback:** `world.query()` already returns
entity handles with stable `EntityId`s. Embedding IDs in descriptors keeps
the `registerLevelManifest()` return value pure data — no side-effecting tag
writes from script. The dispatch path resolves each ID at fire time, exactly
as `setLightAnimation` does today. Tag writeback would require a writable
entity API in the data context (`world.tag(id, "...")`), expanding the
data-script surface and breaking the "descriptors only" boundary.

**Group vs. sequence boundary:** the existing tag-only `primitive` shape
covers operations that act uniformly on a group (`moveGeometry`,
`activateGroup`). `sequence` covers operations where the *order* and
*per-entity argument* matter. Same dispatch event name can fire both
shapes — they coexist in one reaction list.

**Stale IDs:** an entity destroyed between `registerLevelManifest()` and
dispatch is not a fatal error. Most use cases fire `levelLoad` once on a
freshly populated world; later events may run after deaths. Log and continue
keeps live steps unaffected.

---

## Open questions

- **Per-step `onComplete`:** the tag-based `primitive` shape carries an
  `onComplete` event. `sequence` steps do not carry `onComplete` in this plan.
  Does each step need its own, the whole sequence need one at the end, or both?
  Pick when a sequenced primitive with an asynchronous completion (not
  `setLightAnimation`) is specced.
- **Stagger-helper:** a generic `lightWave` helper was explored and rejected — the phase formula (`i * spacingMs / periodMs`) is specific enough to the arena-wave use case that inlining it in the script is clearer than a helper with bespoke parameters. Future authors with different stagger needs should inline their own formula.
