# Effect Sequencing Primitive

## Goal

Extend the data-script effect system with ordered, per-entity sequenced
operations. Add a `sequence` effect shape that targets specific entity IDs
computed inside `setup()`, and ship `setLightAnimation` as the first concrete
sequenced primitive. Migrate the existing light-wave behavior scripts onto data
scripts and retire the behavior-script implementation.

---

## Scope

### In scope

- A `sequence` effect descriptor shape: an ordered list of per-entity
  operations, each carrying an `EntityId` and primitive-specific arguments
- ID-based targeting on the new `sequence` shape — pure declarative payload,
  no tag writebacks from `setup()`
- `setLightAnimation` as the first sequenced primitive: applies a
  `LightAnimation` to a single entity ID
- `setup()` ergonomics: `world.query()` already exposes `EntityId` handles;
  spec confirms this is the supported sort+order pathway
- SDK type declarations for the `sequence` shape and `setLightAnimation` arg
  payload in `postretro.d.ts` and `postretro.d.luau`
- Migration of the two existing arena light-wave scripts
  (`assets/scripts/arena-wave.ts`) to data-script descriptors
- Retirement of the behavior-script light-wave path: delete
  `arena-wave.ts`/`.js` and confirm the data-script path produces the same
  visual outcome

### Out of scope

- Other sequenced primitives (`moveGeometry` ordered, ordered geometry
  reveals, sequenced spawns) — add when authored maps need them
- Hot reload of data scripts — separate plan
- Per-entity targeting on the existing `primitive` shape — `primitive` stays
  tag-only by design
- Tag writeback API from `setup()` — explicitly rejected (see *Open
  questions* in `data-script-setup`)
- Runtime-computed sequences (sequences derived from gameplay state after
  level load) — `setup()` is the only producer
- Time-ordered fire-event chains beyond the existing `onComplete` field

---

## Acceptance criteria

- [ ] A data script that returns `registerEffect("levelLoad", { sequence:
  [...] })` with a list of `{ id, primitive: "setLightAnimation", args: {...} }`
  entries fires each operation in list order when the named event dispatches
- [ ] `world.query({ component: "light", tag: ... })` inside `setup()` returns
  handles whose `id` field can be embedded directly into a `sequence` step
- [ ] `setLightAnimation` dispatches against an `EntityId`, applies the
  supplied `LightAnimation`, and reports the same errors as the
  `set_light_animation` primitive (zero-length direction, color on
  non-dynamic light)
- [ ] A stale `EntityId` (entity destroyed between `setup()` and event
  dispatch) logs a warning and continues with remaining steps — not fatal
- [ ] The arena 1 and arena 2 light waves render identically before and after
  migration: same period, same pulse shape, same per-light phase ordering
- [ ] `assets/scripts/arena-wave.ts` and its compiled `.js` are deleted; the
  test map's `worldspawn` references the new data script via `data_script`
- [ ] `postretro.d.ts` and `postretro.d.luau` declare the `sequence` shape on
  the effect descriptor union and the `setLightAnimation` step payload
- [ ] After `setup()` returns, the data-script VM context is released — no
  per-frame VM cost for the wave (validated by absence of a behavior handler
  on `levelLoad` for arena lights)

---

## Tasks

### Task 1: Sequence descriptor type and dispatch

Extend the effect descriptor union in Rust with a `sequence` variant — an
ordered `Vec` of step records. Each step carries an `EntityId`, a primitive
name, and a primitive-specific argument payload. Dispatch evaluates steps in
order, resolves each ID against the entity registry, and invokes the named
primitive. Stale-ID warnings are logged and non-fatal. Unknown primitive
names are reported at `setup()` time, not at dispatch.

### Task 2: setLightAnimation sequenced primitive

Register `setLightAnimation` in the sequenced-primitive table. Argument
payload mirrors the existing `LightAnimation` shape. Implementation reuses
the same code path as the existing `set_light_animation` primitive — same
validation, same errors. Difference is only the registration surface.

### Task 3: SDK types and authoring helpers

Add the `sequence` shape and `setLightAnimation` step payload to the SDK
type declarations. Update the `registerEffect` overload set so authors get
type checking on `sequence` step entries. Provide a thin authoring helper
(matching the `light_animation.ts` style) for building a step list from a
sorted `LightEntity[]` and a stagger function — encapsulates the
"phase = i / N" pattern from the existing scripts.

### Task 4: Migrate arena light waves

Rewrite both arena waves as a data script returning two `sequence` effects
keyed to `levelLoad`. Each effect's steps come from `world.query()` →
sort by position → map to `{ id, primitive: "setLightAnimation", args: {
periodMs, phase, brightness, ... } }`. Delete the behavior-script
implementation (`assets/scripts/arena-wave.ts` and the compiled `.js`).
Wire the new data script into the test map's `worldspawn`.

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
import { registerEffect } from "postretro";
import { world } from "./sdk/world";

export function setup() {
    const arena1 = world
        .query({ component: "light", tag: "arena_1_light" })
        .sort(/* angle around centroid, anchored NW */);

    return {
        effects: [
            registerEffect("levelLoad", {
                sequence: arena1.map((light, i) => ({
                    id: light.id,
                    primitive: "setLightAnimation",
                    args: { periodMs, phase: stagger(i, arena1.length),
                            brightness, /* ... */ },
                })),
            }),
        ],
    };
}
```

**Why ID-based, not tag writeback:** `world.query()` already returns
entity handles with stable `EntityId`s. Embedding IDs in descriptors keeps
the `setup()` return value pure data — no side-effecting tag writes from
script. The dispatch path resolves each ID at fire time, exactly as
`set_light_animation` does today. Tag writeback would require a writable
entity API in the data context (`world.tag(id, "...")`), expanding the
data-script surface and breaking the "descriptors only" boundary.

**Group vs. sequence boundary:** the existing tag-only `primitive` shape
covers operations that act uniformly on a group (`moveGeometry`,
`activateGroup`). `sequence` covers operations where the *order* and
*per-entity argument* matter. Same dispatch event name can fire both
shapes — they coexist in one effect list.

**Stale IDs:** an entity destroyed between `setup()` and dispatch is not a
fatal error. Most use cases fire `levelLoad` once on a freshly populated
world; later events may run after deaths. Log and continue keeps live
steps unaffected.

---

## Open questions

- **Single-step ergonomics:** when a `sequence` has one step, authors could
  write a flatter `{ id, primitive, args }` form instead of a one-element
  list. Worth a sugar shape, or does the consistent list form win? Resolve
  before Task 3 freezes the SDK signature.
- **Per-step `onComplete`:** the tag-based `primitive` shape carries an
  `onComplete` event. Does each `sequence` step need its own, the whole
  sequence need one at the end, or both? Pick when a sequenced primitive
  with an asynchronous completion (not `setLightAnimation`) is specced.
