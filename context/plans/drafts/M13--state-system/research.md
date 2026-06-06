# Research — M13 Goal C (State system: the decoupling seam)

Investigation notes for the Goal C draft. Decisions live in the spec; this is the
grounding. Every claim anchored to a real `path:symbol` where possible.

> **Read this when:** drafting the Goal C spec. Goal C is the third spec of
> Milestone 13 (UI), the spine's last sequential link (A → B → C).
> **Sources:** roadmap M13 · `context/research/ui-layer.md` §9 + neighbours ·
> shipped Goal A (`done/M13--ui-render-pass-slice/`) + Goal B
> (`done/M13--descriptor-tree-layout/`) · scripting layer + `scripting.md` ·
> `entity_model.md`.

---

## 1. Scope (in / out)

### In scope (Goal C ships)

Roadmap line (verbatim): *"C — State system (the decoupling seam). `defineState`,
branded `StateValue<T>`, the slot table, engine-owned-readonly vs. modder-declared,
the once-per-frame published read handle, value diffing for layout invalidation,
clamp / validate (research §9). Ships the static proxy populating `player.health` /
`player.ammo`. **Publishes the engine-owned slot schema as the contract Milestone 10
honors.** Owns the persisted-slot save **wire format** (`persist: true` slots,
research §9) — a real format, not a one-word mention. Depends on B."*

Concretely, C delivers:

- **`defineState` primitive** — a new scripting primitive (camelCase, `scripting.md`
  §4) that ingests a namespaced slot schema. Installs in both runtimes via the
  existing registry (one registration → QuickJS + Luau, `scripting.md` §1).
- **The slot table** — a Rust-owned runtime structure holding every declared slot
  by dotted name (`player.health`, `audio.master`), its type, range, readonly flag,
  persist flag, current value, and previous-frame value (for diffing).
- **Branded `StateValue<T>`** — the per-slot SDK handle type. Brand pattern already
  in-tree: `EntityId = number & { readonly __brand: "EntityId" }`
  (`typedef.rs:294`, `:1119`). `StateValue<T>` mirrors this with a phantom `T`.
- **The once-per-frame published read handle** — extends `UiReadSnapshot`
  (`render/ui/mod.rs:194`) so the UI pass dereferences live slot values when it
  records, after game logic, before render (research §4 frame order).
- **Value diffing → B's dirty relayout** — per-frame compare of slot values;
  changed slots invalidate the dependent retained tree so taffy recomputes
  (`tree.rs:177` `build_draw_data` gate).
- **Clamp / validate** — `range: [min, max]` clamping and type/enum validation on
  write, following the `setFog*` clamp-and-warn precedent (`scripting.md` §10.2).
- **The static proxy** — minimal Rust that writes `player.health` / `player.ammo`
  with no real game logic (the M10 shootable-static-proxy pattern applied to UI).
- **Persisted-slot save wire format** — a real serialized format for `persist: true`
  slots (§5 below). No save infra exists today.
- **Published engine-owned slot schema** — `player.health` etc. as a typed, ranged,
  readonly contract Milestone 10's health/damage task honors (§2 below).

### Out of scope (boundary against neighbours)

| Goal | C must NOT do |
|---|---|
| **B (done)** | No new widgets, no layout, no taffy mapping. B's descriptors are literal-only (`descriptor.rs` carries no `bind` field). C adds the *binding* surface and the slot-driven content path, but reuses B's tree/layout/dirty machinery unchanged. |
| **D (fonts/theming)** | No theme tokens, no multi-font. C uses literal values like B. |
| **E (HUD dynamics)** | **No `styleRanges`, no `onStateCrossing`.** These read slot values per frame (E) — C ships the slot *table and read path* they consume, but neither the value→style mechanism nor threshold-crossing dispatch. This is the sharpest boundary: C builds the slot substrate E sits on. |
| **F (input)** | No hit-testing, focus, nav intents, `setState`-from-event. C's writable modder slots exist, but the *event→write* path (`setState` reaction) is F/E territory. C may need the slot-write primitive itself; see §8 open question. |
| **G1 (SDK core)** | **No script-ingestion factory, no SDK sugar.** But C *does* define `defineState` as a primitive and the `StateValue<T>` branded-type contract. The line: the **primitive** (`defineState` Rust registration + FFI deserialization of the schema) is C; the **SDK ergonomics** (`sdk/lib/ui/state.{ts,luau}` wrappers, the namespaced-export sugar in research §9, `onStateCrossing` helper) are G1/E. C registers the primitive and emits the branded `StateValue<T>` typedef; G1 wraps it. |

**`defineState` primitive vs. SDK-sugar line (ambiguity resolved).** Research §9 shows
`defineState("audio", {...})` returning a namespace object whose properties are
`StateValue<T>` handles (`audio.master.get()` / `.set()`). That return-object-with-
methods shape is **SDK sugar (G1)**. The C deliverable is the underlying **primitive**:
a registered Rust function that takes `(namespace, schema)` as data, validates and
populates the slot table, and whose typedef declares `StateValue<T>`. C does the
hardest, most-expensive-to-reverse half: the slot-table contract, the schema wire
shape, the branded-handle typedef, the readonly/persist/range semantics. G1 later
adds the property-access ergonomics on top — same split as `defineReaction` (primitive
+ data) vs. the `sdk/lib/data_script.ts` wrapper.

---

## 2. The published slot-schema contract (the cross-milestone touchpoint)

This is the one real cross-milestone obligation. The roadmap M13 preamble:

> *"decoupling breaks the *code* dependency but couples the *slot schema*. The
> State-system goal publishes the engine-owned slot schema (`player.health` as a
> typed, ranged contract) as a **published contract** that Milestone 10's entity
> health/damage task is on the hook to honor — the schema is the coordination point,
> not a shared module."*

The M10 task (roadmap, "Entity health + damage surface"):

> *"minimal health/damage primitive on the Milestone 6 entity model: an entity
> carries HP, consumes a `DamagePayload`, dies at zero HP. … **Shootable as a static
> proxy, so it gives the shipped weapon a target the day it lands.**"*

**The coordination, precisely.** C publishes `player.health` (and `player.ammo`) as
engine-owned, `readonly: true`, typed-and-ranged slots. C feeds them from a *static
proxy* (no real game logic). M10's health/damage task, when it lands, must write the
*same* slot names with the *same* type and range — replacing C's proxy writer with
real entity-HP-derived writes. Neither blocks the other (roadmap: "neither blocks the
other"); the schema is the merge contract. This mirrors exactly how M10's own
health/damage surface is "shootable as a static proxy" before real AI drives it — C
applies that pattern one layer up, to UI.

**What a typed, ranged, engine-owned slot must specify** (the contract surface the
spec must pin):

- **Dotted name** — `player.health`, `player.ammo` (research §9 names
  `player.health`, `player.armor`, `player.ammo.<weapon>`). Names are the durable
  contract — "Stable names; survive refactor" (research §9 table).
- **Type** — `number` for both. (Research §9 modder types: number, boolean, string,
  enum, array. No nested objects — flat surface.)
- **Range** — `player.health` as `[0, 100]` (research §3's `max: 100` HUD example).
  The range is part of the contract because the HUD `bar` widget (E) computes
  `value / max` for `styleRanges` fractions. **Decision needed:** is the max a slot
  property C publishes, or a per-widget `max:` field (research §3 shows `max: 100`
  on the widget)? Recommend: the slot carries the authoritative range; the widget
  may still echo a display max. Flag for owner.
- **`readonly: true`** — scripts read, never write. Game logic (or C's proxy) writes.
  Research §9: *"`readonly: true` — scripts read, cannot write."*
- **Engine-owned vs. modder-declared** — engine-owned slots are pre-registered by
  the engine (not via `defineState` from a mod); modder slots arrive via
  `defineState`. The slot table must distinguish them (an `owner: Engine | Mod`
  discriminant or equivalent), because readonly enforcement and the
  "survives-refactor stable name" guarantee differ.

**`player.ammo` nuance.** Research §9 writes `player.ammo.<weapon>` (per-weapon) and
`player.ammo.current` (research §3 `bind: "player.ammo.current"`, `format: "{}/{max}"`).
The flat-surface rule (no nested objects) means `player.ammo.current` is a *single
dotted slot name*, not a nested object access. C ships a static proxy for a small
fixed set; the spec should pin exactly which ammo slot name(s) the proxy publishes.

---

## 3. Runtime mechanism (anchored to real code)

### 3.1 `defineState` as a primitive

Registration mechanism (`scripting.md` §4, `primitives/mod.rs:307` `register_all`):
each domain module registers via `registry.register("name", closure).scope(...)
.param(...).finish()`. One registration installs in **both** QuickJS and Luau
(`scripting.md` §1; `primitives/mod.rs` tests call both `p.quickjs_installer` and
`p.luau_installer`). `defineState` slots in as a new domain module — likely
`scripting/primitives/state.rs` — called from `register_all`.

**Context: definition vs. behavior.** Two scopes exist: `DefinitionOnly` and `Both`
(`scripting.md` §3; `primitives_registry::ContextScope`). State is *declared at load
time* (research §9: "State is declared in namespaced modules using `defineState`").
Per `scripting.md` §2, cross-script data declarations belong to the **Definition
context** (engine lifetime). But §3 notes `DefinitionOnly` has *no in-tree consumer*
today (`registerEntity` was removed) and is untested; `Both` is the only active path.
**Recommendation:** register `defineState` as `Both` (matches every shipping
primitive) OR revive `DefinitionOnly` for it — `defineState` is the natural first
real consumer of `DefinitionOnly`, since slot schemas are definition-context data
that must outlive level loads (cf. mod-init lifecycle, research §18). Flag for owner:
this decides whether C revives the dormant scope. Note scope is *advisory metadata*
only (drives typedefs), not call-time enforcement (`scripting.md` §3).

### 3.2 Schema ingestion: follow the descriptor pattern, not the reaction pattern

Two FFI-ingestion precedents exist:

- **Reactions** (`data_descriptors.rs`): hand-rolled `from_js` / `from_lua` walkers,
  manual `contains_key` discrimination, duplicated per runtime (`named_reaction_from_js`
  ~L457, `named_reaction_from_lua` ~L1131). Verbose.
- **UI descriptors** (Goal B, `descriptor.rs`): serde internally-tagged enums,
  ingested via `conv.rs` `js_to_json` (`conv.rs:768`) / `lua_to_json` (`conv.rs:840`)
  → `serde_json::from_value`. Less code. B *deliberately* diverged from reactions
  toward serde tagging (`descriptor.rs:29` comment; Goal B research §"Wire-format").

**Recommendation for C:** follow B's serde path. A slot schema is a map of
`{ name: { type, default?, range?, readonly?, persist?, values? } }`. Define a serde
struct (e.g. `SlotSchema` / `SlotDecl`), ingest the whole `defineState` payload
through `js_to_json`/`lua_to_json` → `serde_json::from_value`, then validate ranges
in Rust (serde can't enforce numeric bounds — cf. `LightDescriptor::validate`
`data_descriptors.rs:88`, `WeaponDescriptor::validate` `:164`). The `type` field is
a string-enum discriminant (`number`/`boolean`/`string`/`enum`/`array`) — serde
internally-tagged or a manual match. This reuses the exact bridge G1 will feed
VM-produced JSON through.

### 3.3 The slot table

No such structure exists yet. New, Rust-owned, engine-global (survives level loads,
like the entity-type registry — `DataRegistry`, `scripting.md` §2). Holds per slot:
dotted name, value type, current value, **previous-frame value** (diffing), range,
readonly flag, persist flag, owner (engine vs mod), default. Writes clamp/validate;
readonly slots reject mod writes. Where it lives is an open question (§8) — game-logic
side (it's authoritative state, like `ScriptCtx.gravity` a `Cell<f32>`,
`world.rs:292`) vs. a dedicated module. The renderer only *reads* it via the snapshot.

**Value representation.** Slots are number/boolean/string/enum/array (research §9).
A `serde_json::Value` per slot is the path-of-least-resistance (matches `args:
serde_json::Value` on `PrimitiveDescriptor`, `data_descriptors.rs:56`), but a typed
enum (`SlotValue::Number(f32) | Bool(bool) | ...`) gives cheaper diffing and clamping.
Recommend a typed enum; flag.

### 3.4 The once-per-frame published read handle (extends `UiReadSnapshot`)

`UiReadSnapshot` (`render/ui/mod.rs:194`) today carries `version_line: String` (A)
and `gameplay_tree: Option<AnchoredTree>` (B widened it). It is **stored on the
`Renderer`** (`render/mod.rs:959` `ui_snapshot`), set via `set_ui_snapshot`
(`render/mod.rs:2887`) by the App just before each render — *not* a render parameter,
so render signatures stay stable (B research §"Render handle"; A index Task 4). The
renderer reads it when recording (`record_splash_ui` `render/mod.rs:2945` splash path;
gameplay path `render/mod.rs:4386`).

**C's extension.** The snapshot must carry the frame's resolved slot values (or a
read handle into the slot table) alongside `gameplay_tree`, so the renderer can
dereference `bind`-ed slots when it lays out / builds the draw list. Per research §2:
*"Renderer dereferences `StateValue<T>` handles each frame."* Critical ordering
(research §4): the snapshot is published **once per frame, after game logic completes**
— so the renderer sees post-tick slot values. The handle is the game-logic→render
contract A established and B preserved. Whether the snapshot carries a *clone* of slot
values or a borrowed read view is an open question (§8) — a clone is simplest and
matches the `Clone`-derived snapshot, but couples cost to slot count.

### 3.5 Value diffing → B's dirty relayout

This is where C plugs into B's machinery. Today (Goal B, `tree.rs:177-219`):
`UiTree::build_draw_data` gates `compute_layout_with_measure` on
`viewport_changed || structural_change` where `structural_change =
self.taffy.dirty(self.root)`. Layout recomputes only on tree-rebuild or resize. There
is **no value-change trigger** because B has no slot values — descriptors are literal
(`descriptor.rs`).

C introduces per-frame slot values that change. The diffing logic: compare each slot's
current vs. previous-frame value; for a changed slot, mark the *dependent* tree nodes
dirty so taffy recomputes only those. The relevant taffy hook is `mark_dirty` (named
in roadmap Goal B + B research §taffy). Two mechanisms the spec must choose between:

- **Content-only change** (e.g. a `text` node's bound content string changes but its
  measured width is identical) — may not need relayout at all, only a draw-list
  content update. But text width usually *does* change with content, so a bound text
  node generally needs re-measure → relayout.
- **Layout-affecting change** (bound text width, bar fill fraction if it drives
  sizing) — must `mark_dirty` the node so taffy re-measures.

The conservative correct default: any changed slot value bound to a node marks that
node dirty. Refinement (skip relayout when measured size is unchanged) is an
optimization the spec can defer.

### 3.6 The retained-tree dependency (see §6 — C is on the hook for it)

Diffing only *pays off* if the `UiTree` survives across frames. It does not today.
This is a hard C prerequisite, detailed in §6.

### 3.7 Clamp / validate

Precedent: the `setFog*` primitives clamp-and-warn (`scripting.md` §10.2 table;
`set_fog_params.rs` et al.). `range: [min, max]` on a numeric slot clamps writes to
the range; out-of-range writes log a `log::warn!` then take the clamped value (the
established convention — "All invalid inputs emit `log::warn!` before taking effect",
`scripting.md` §10.2). Type mismatches (writing a string to a number slot) and
invalid enum values are rejected. NaN/non-finite handling follows `worldSetGravity`
(`world.rs:301` — silently ignore non-finite, warn). Validation that serde can't do
(numeric bounds) runs in a Rust `validate` pass, like `LightDescriptor::validate`.

---

## 4. The static proxy

C ships *no real game logic*. The proxy is the minimal thing that writes
`player.health` / `player.ammo` into the slot table each frame (or on a timer), so the
HUD has live values to bind without M10. Precedent and framing:

- **M10's own pattern** — the health/damage surface is "shootable as a static proxy"
  before AI drives it. C applies this one level up.
- **`world.gravity`** (`world.rs:292`, `ScriptCtx.gravity: Cell<f32>`) — a single
  engine-owned mutable value the proxy can model: a `Cell`/field the proxy writes,
  the snapshot reads.

**Minimal proxy behavior (spec to pin):** a fixed or slowly-varying value is enough to
prove the bind path and exercise diffing (a value that *changes* over frames proves
the dirty-relayout path actually fires). Recommend the proxy *animate* health/ammo
(e.g. a slow oscillation or a decrement-on-timer) so the value-diffing → relayout path
runs for real, not just structurally. This is the analogue of A's splash carrying a
real version string ("the once-per-frame contract is exercised with a real value, not
an empty handle", A index). The proxy lives behind one named seam M10 replaces in
place — same discipline as A's splash-descriptor seam B replaced.

---

## 5. Persisted-slot save wire format

### What exists today: nothing

No save/load infrastructure exists. Searched: no `save`/`load_save`/`SaveFile`
symbols, no `std::fs::write` to a save path (only typedef emit + test scaffolding),
no `dirs`/`directories` crate in any `Cargo.toml`. **`entity_model.md` §9 explicitly
lists "Entity serialization (save/load)" as a Non-Goal** — note the tension: C
introduces *slot* persistence, which is narrower than entity save/load but is the
engine's first persistence of any kind. The spec should call this out (C is not
violating the entity-save non-goal; it persists declared scalar slots, not entities).

### The decision space

Research §9: *"`persist: true` survives map transitions. Rust serializes and restores
on engine start."* So persisted slots must (a) survive level transitions in-memory and
(b) round-trip to disk across engine restarts.

- **Serialization format.** `serde_json` is the established, in-tree, ubiquitous
  serializer (every descriptor, `conv.rs`, `data_descriptors.rs`). **Recommend JSON**
  for the save file — human-readable, debuggable, matches the modder-facing wire
  conventions, zero new dependency. (PRL's binary format is for baked spatial data,
  not mutable runtime state — wrong tool here.)
- **Casing.** Per `scripting.md` §4 / Boundary Inventory convention: wire/JS/Luau is
  camelCase, Rust snake_case via `#[serde(rename_all = "camelCase")]`. Slot *names*
  are already dotted strings (`audio.master`) — they are keys, not field idents, so
  they serialize verbatim. The save file is a flat `{ "audio.master": 80, "a11y.subtitles":
  false, ... }` map (only `persist: true` slots), or a namespaced nesting. Recommend
  flat dotted-key map — matches the slot table's keying.
- **Where the file lives.** No precedent. Options: (a) next to the mod/content root
  (the mod root is already resolved from the map path, `scripting.md` §2); (b) a
  per-user data dir (would need a `dirs`-style crate — new dependency, cross-platform
  concern). **Recommend** the spec pick the simplest that works for a single-player
  engine; a content-relative or working-dir-relative `save.json` is enough for C's
  scope (the static proxy + audio/a11y settings slots). Flag the dependency question.
- **What persists.** Only `persist: true` slots (research §9 examples: `audio.*`,
  `a11y.*`). Engine-owned `player.health` is **not** persisted (transient gameplay).
  So the persisted set is a *subset* of the slot table, filtered by the persist flag.
- **Restore timing.** "restores on engine start" (research §9). Hooks into boot —
  see `boot_sequence.md` (not read here; spec should consult it for the exact
  restore-before-first-frame point). Restore must apply *after* `defineState`
  declarations register the slots (you can't restore into a slot that isn't declared)
  but before the first frame reads them.
- **Missing/extra keys.** A persisted key for a slot no longer declared → drop with
  warn. A declared persisted slot absent from the save → use its `default`. (Mirrors
  the descriptor optional-field-default discipline, `data_descriptors.rs` `get_optional_*`.)

### IR/versioning caution

`scripting.md` §11 flags an open obligation: *"A serialized command buffer baked into
a mod must survive engine-version changes."* The same applies to the save format — a
save file must survive schema evolution (slots added/removed, ranges changed). The
spec should at minimum stamp a format version into the save file so a future migration
path exists.

---

## 6. The retained-tree-across-frames concern (C is on the hook)

This is a **major C concern**, deferred explicitly in Goal B. Goal B `index.md`
Follow-ups (verbatim):

> *"**Retain `UiTree` across frames so dirty-gating fires in production.** Task 4's
> dirty-gating is real and tested at the tree level (`tree.rs` recompute-counter
> tests), but `UiPass::layout_tree` rebuilds a fresh `UiTree` every frame, so a
> fresh-always-dirty tree never short-circuits the recompute in production today. When
> persistent gameplay screens land (the goal introducing retained-across-frames UI —
> C/F), hold the `UiTree` on the `Renderer` and rebuild it only on descriptor change,
> so the no-recompute path runs for real. Deferred deliberately in B (owner decision):
> B has no persistent screen to retain, and the splash re-derives its descriptor each
> frame."*

Confirmed in code: `tree.rs:170-176` — *"The gate only pays off when the same `UiTree`
survives across frames. The renderer rebuilds a fresh `UiTree` every frame today (see
`UiPass::layout_tree`), so a fresh tree is always dirty and the gate never short-
circuits in production yet."* And `render/mod.rs:4386` clones `gameplay_tree` and
calls `self.ui.layout_tree(&tree, ...)` every frame — which (per `tree.rs`) builds a
fresh `UiTree` from the descriptor each call.

**What this means concretely for C.** C's value-diffing → dirty-relayout is *useless*
unless the `UiTree` persists. So C must:

1. **Hold the `UiTree` on the `Renderer`** (not rebuild per frame). Rebuild only when
   the *descriptor* changes (structural change). Today the descriptor lives in
   `UiReadSnapshot.gameplay_tree` and is cloned every frame.
2. **Separate descriptor-change (rebuild tree) from value-change (mark nodes dirty).**
   A bound slot value changing must *not* rebuild the whole tree — it marks the bound
   nodes dirty so taffy re-measures just them. This is the whole point of diffing.
3. **Reconcile** the retained tree against an incoming descriptor: if the descriptor
   is unchanged, keep the tree and only update slot-driven content; if changed, rebuild.

This couples C to the renderer's UI ownership model. It is the single largest piece of
net-new plumbing in C beyond the slot table itself. The spec must treat "retain the
tree" as a first-class task, not a footnote — B handed it to C by name.

### Goal B open question: "pre-bake a typed screen/slot handle to de-risk C"

Goal B `index.md` Open questions, resolved (verbatim): *"`UiReadSnapshot` shape.
Decided: carries the descriptor tree; the renderer lays it out (renderer-owns-GPU).
Residual: whether to pre-bake a typed screen/slot handle to de-risk C. **Recommendation:
defer — C is next and sequential, so the handle is best shaped against C's content
contract.**"* So B deliberately did **not** pre-bake a slot handle; C designs it fresh
against C's own contract. No constraint inherited; full design latitude.

---

## 7. Cross-boundary / casing inventory (mirrors B's Boundary Inventory style)

The new state surface crosses Rust ↔ wire (JSON) ↔ JS/TS ↔ Luau. `defineState`
schema ingestion lands as a primitive in C (the bridge is C's, unlike B which deferred
ingestion to G1 — but C must ingest the schema to populate the slot table). The save
file is a new on-disk wire format C owns. Rust snake_case ↔ wire camelCase via serde
rename, per convention (`scripting.md` §4).

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| primitive | `defineState` registration | n/a (call name) | `defineState(ns, schema)` | `defineState(ns, schema)` |
| namespace | `&str` / `String` | string key | `"audio"` | `"audio"` |
| slot decl: type | `enum SlotType` | `"type"`: `"number"`/`"boolean"`/`"string"`/`"enum"`/`"array"` | same | same |
| slot decl: default | `SlotValue` | `default` | `default` | `default` |
| slot decl: range | `[f32; 2]` | `range` (`[min,max]` array) | `range` | `range` |
| slot decl: readonly | `bool` | `readonly` | `readonly` | `readonly` |
| slot decl: persist | `bool` | `persist` | `persist` | `persist` |
| slot decl: enum values | `Vec<String>` | `values` | `values` | `values` |
| handle type | (typedef only) | n/a | `StateValue<T>` (branded) | `StateValue<T>` (Luau alias) |
| slot name (key) | dotted `String` | dotted key verbatim (`"player.health"`) | dotted access `player.health` | dotted access `player.health` |
| save file | (new) | flat `{ "audio.master": 80, ... }` JSON, `persist:true` subset + format version stamp | n/a | n/a |
| readonly engine slot | `player.health` etc. | engine-pre-registered | read-only handle | read-only handle |

**Branded handle emission.** `StateValue<T>` is emitted by the typedef generator like
`EntityId` (`typedef.rs:291` `TypeShape::Brand` → TS `& { readonly __brand: ... }`;
Luau alias). But `StateValue<T>` is *generic* over `T` — the existing `Brand` shape
(`primitives_registry.rs:99`) is non-generic (`EntityId = number`). **C must extend
the typedef generator** to emit a generic branded type, or hand-author the
`StateValue<T>` declaration in the SDK-lib block (`TS_SDK_LIB_BLOCK` / `LUAU_SDK_LIB_BLOCK`,
referenced `sdk/lib/index.ts:6`). This is a real, named gap — the brand mechanism
today does not do generics. Flag for the spec.

---

## 8. Open questions & risks

**Genuine unknowns for the spec author:**

1. **Where does the slot table live?** Game-logic side (authoritative state, like
   `ScriptCtx`), or a new module? The renderer only reads it (via snapshot). Frame
   order (research §4) wants writes settled *before* the snapshot publishes. Likely
   game-logic-adjacent, read-only-borrowed by the renderer. Owner input useful.

2. **Snapshot carries cloned values vs. a borrowed read view?** `UiReadSnapshot`
   derives `Clone` (`ui/mod.rs:193`). Cloning all slot values per frame is simple but
   scales with slot count. A borrowed view complicates lifetimes. Recommend clone for
   C's small slot set; revisit if slots proliferate.

3. **`defineState` scope: `Both` or revive `DefinitionOnly`?** §3.1. `DefinitionOnly`
   is dormant and untested (`scripting.md` §3). `defineState` is its natural first
   consumer (slot schemas are definition-context, engine-lifetime data). Decide
   whether C revives the scope or takes the well-trodden `Both` path.

4. **Save file location + dependency.** §5. Content-relative `save.json` (no new dep)
   vs. a per-user data dir (`dirs`-style crate, new dependency). For a single-player
   engine the former likely suffices for C's scope.

5. **Generic branded type in the typedef generator.** §7. The `Brand` shape is
   non-generic today. `StateValue<T>` needs generics — extend the generator or
   hand-author in the SDK-lib block. Real gap.

6. **Range/`max` ownership.** §2. Does the slot own the authoritative range, or the
   widget echo `max:`? Research shows `max:` on the widget (§3) *and* `range:` on the
   slot (§9). Pin which is authoritative for `bar` fraction computation.

7. **Does C need a slot-*write* primitive, or only the readonly-engine path + proxy?**
   Writable modder slots exist in the model (research §9 `audio.master.set`), but the
   *write mechanism* is `setState` — a reaction (research §11, §15), which is E/F
   territory. C ships the slot table that *can* hold writable modder slots and the
   engine-write path the proxy uses, but the spec must decide whether C includes any
   mod-facing write at all, or defers every write to E/F and ships read-only + proxy
   only. **Recommend:** C ships the slot table with writable-slot *storage and
   clamping*, the engine-side write the proxy uses, but **no** mod-facing `setState`
   (defer to E/F). This keeps C's surface the read path + schema + proxy.

**Scope-creep traps:**

- **Pulling `styleRanges`/`onStateCrossing` forward.** They *read* slots (E). The
  temptation is "the slot table is right here." Resist — C ships the substrate, E the
  consumers. (Mirrors B's "keep bindings out" highest-risk note.)
- **Building the `setState` reaction / event-write path.** That's the F/E write
  surface. C's writes are engine-side (proxy) + readonly-engine; mod writes defer.
- **Over-building the save format.** A versioned flat JSON map is enough. Don't build
  a migration framework, slot-history, or multi-profile save slots in C.
- **The `bind` field on descriptors.** B's `descriptor.rs` has no `bind`. C adds it —
  but that addition is *content binding*, not a new widget. Keep it minimal: a string
  slot-name reference on `text` (and a `panel` `fill` for the color-flash demo).
  **DECIDED (owner, 2026-06): C does *not* add the `bar` widget** — it stays deferred
  to F, and the eased health-bar built-in lands in BIS once F's `bar` + value-tweening
  (roadmap goal **TW**) exist. See §11.8. No slot-bound widget beyond `text`/`panel` is
  needed for C's demo.

**Needs owner input:** items 1, 3, 4, 7. (The `bar`-widget question is resolved — §11.8.)

---

## 9. Key code anchors

| File / symbol | Role for C |
|---|---|
| `render/ui/mod.rs:194` `UiReadSnapshot` | The once-per-frame read handle. C extends it to carry slot values. `version_line` (A), `gameplay_tree` (B). |
| `render/mod.rs:959` `ui_snapshot`, `:2887` `set_ui_snapshot` | Snapshot stored on `Renderer`, set by App pre-render. |
| `render/mod.rs:2945` `record_splash_ui`, `:4386` gameplay path | Where the snapshot is read and the tree laid out. C feeds slot-driven content here. |
| `render/ui/tree.rs:93` `UiTree`, `:177` `build_draw_data`, `:187-192` gate, `:106` `recompute_count` | B's retained tree + dirty gate. C plugs value-diffing into this and must **retain the tree across frames** (B follow-up). |
| `render/ui/tree.rs:170-176` | Explicit note: gate never fires in production until the tree is retained — C's job. |
| `render/ui/descriptor.rs` `Widget`, `AnchoredTree` | B's literal descriptor model. C adds `bind` (content binding) on `text`/`panel`; **no new widget** — `bar` stays deferred to F (owner, 2026-06, §11.8). |
| `scripting/primitives/mod.rs:307` `register_all`, `:17` `register_shared_types` | Where a new `defineState` domain module registers; where `StateValue`/schema types register for typedefs. |
| `scripting/primitives/world.rs:249` `register_world_primitives`, `:292` `gravity.get/set` | Template for a domain primitive module + an engine-owned mutable value (proxy precedent). |
| `scripting/data_descriptors.rs:88` `LightDescriptor::validate`, `:164` `WeaponDescriptor::validate` | Rust-side range validation precedent (serde can't bound numbers). |
| `scripting/conv.rs:768` `js_to_json`, `:840` `lua_to_json`, `:731` `json_to_js`, `:808` `json_to_lua` | The serde bridge C ingests the `defineState` schema through (both runtimes). |
| `scripting/primitives_registry.rs:99` `TypeShape::Brand`, `typedef.rs:291` | Branded-type emission. `EntityId` precedent. **Non-generic today — `StateValue<T>` needs an extension.** |
| `typedef.rs:294`, `:1119` | `EntityId = number & { readonly __brand: "EntityId" }` — the branded-type pattern to mirror. |
| `scripting/data_registry.rs:16` `DataRegistry`, `:118` `clear` | Engine-global registry pattern (survives level loads). Model for the slot table's lifetime. |
| `scripting.md` §10.2 / `set_fog_params.rs` | Clamp-and-warn validation convention. |
| `entity_model.md` §9 | "Entity serialization (save/load)" Non-Goal — note the tension with C's slot persistence. |
| `sdk/lib/index.ts`, `sdk/lib/data_script.ts` | SDK-lib re-export structure; `sdk/lib/ui/state.{ts,luau}` is where G1 (not C) wraps `defineState`. |

---

## 10. Web-research notes (tied to the spec)

- **Branded `StateValue<T>`.** The idiomatic TS pattern is intersect-with-phantom:
  `type StateValue<T> = T & { readonly __brand: "StateValue" }` (or carry the slot
  type as the phantom param). Zero runtime cost; compile-time only. PostRetro already
  uses exactly this for `EntityId`. The one wrinkle: PostRetro's brand emitter is
  non-generic — `StateValue<T>` is the first *generic* brand, so the typedef generator
  needs the `T` parameter (§7). Sources: [Learning TypeScript — Branded Types](https://www.learningtypescript.com/articles/branded-types),
  [Phantom Types in TypeScript](https://dev.to/gabrielanhaia/phantom-types-in-typescript-stop-mixing-kilograms-and-pounds-at-compile-time-iem).

- **Reactivity model: pull, not push.** PostRetro is explicitly a **once-per-frame
  pull** model — "the renderer dereferences `StateValue<T>` handles each frame"
  (research §2), "Engine state reaches UI via a read handle published once per frame"
  (research §4). This is the opposite of fine-grained signal/observable push systems
  (Solid-style signals, Bevy `haalka`/`quill`) where a write propagates immediately
  to subscribers. C's "value diffing" is the pull-model equivalent of fine-grained
  invalidation: instead of dependency tracking at write time, the engine compares
  values frame-over-frame and invalidates the dependent retained-tree nodes. This is
  simpler and fits the "no live VM, Rust owns the loop" invariant — no subscription
  graph, no push propagation. The spec should frame diffing as *pull-side
  invalidation*, not as a reactive signal system. Sources:
  [Retained and immediate mode (Glazkov)](https://glazkov.com/2021/11/25/retained-and-immediate-mode/),
  [Signals and Retained UI (wtrclred)](https://wtrclred.io/en/posts/12).

- **Value-diffing for retained-mode invalidation.** Standard retained-UI practice:
  cache prior values, diff per frame, mark only changed subtrees dirty for relayout.
  taffy's `mark_dirty` + cached subtree layout (already in B's `tree.rs`) is exactly
  the substrate. The key correctness rule: a bound value that affects *measured size*
  (text content, image) must dirty the node; a value affecting only *draw content*
  (color, fill fraction not driving layout) need not relayout, only rebuild the draw
  list. C can start conservative (any bound change dirties the node) and refine later.

---

## 11. Owner ideation — accumulated design decisions (2026-06-05)

A working session with the owner refined several C decisions and surfaced new ones.
Captured here so the spec author inherits the reasoning instead of re-deriving it.
Tags: **DECIDED** (owner-settled), **RECOMMEND** (a lean, owner to confirm),
**OPEN** (genuinely unresolved).

### 11.1 Pull is the substrate; push is a projection — a fit, not a constraint

Frame-over-frame diffing is the *pull-model equivalent* of fine-grained invalidation,
not a workaround we hit a wall into. The engine is already a once-per-frame pull model
with no live VM (frame order Input → … → Render; the read handle is published after
game logic). Push/signals need a live subscription graph *between* frames, which
contradicts the no-live-VM + pooled-context + hot-reload lifecycle and would let UI
read torn mid-frame state. Push semantics already have a home — M6 events + E's
`onStateCrossing`/reactions — derived *on top of* the sampled slot (push-on-pull).
Asymmetry that settles the substrate choice: push derives cleanly from pull
(edge-detect a sampled level → crossings); pull does **not** derive from pure push
without rebuilding the level into state anyway.

- **OPEN / future hook:** a `notify: true` slot flag could have the engine emit a
  changed/crossing event for that slot — additive, no second mechanism, no torn reads.
  **RECOMMEND:** keep the door open, land it in **E** alongside `onStateCrossing`, not C.

### 11.2 Scale — diff the displayed projection, not the simulation

The slot table holds the *displayed projection* — tens of values a player reads — not
the entity firehose. Hundreds of bullets / dozens of enemies live in the entity store
(`EntityRegistry`, `registry.rs:424`) and the M6 event system, and never diff through
UI state. A boss bar is **one** projected slot, not 50 enemy slots. The diff itself is
trivially cheap (a scalar compare; well under 1% of frame even at 10k slots); the real
cost is relayout/re-measure, and that cost is identical under push or pull (see §11.7).

- **RECOMMEND (scale insurance):** make diffing **subscriber-aware** — only diff slots
  a widget actually binds. O(bound slots), keeps the once-per-frame snapshot coherent,
  needs no subscription graph. Cheap insurance even if the slot count grows past
  expectation. Frame "is per-frame diffing sustainable at scale?" as a non-question by
  construction.

### 11.3 Naming — name the primitive after the value it returns

`defineState()` returns a `StateValue<T>`: the verb claims the whole *category*, the
return type is *one value*. The verb overclaims relative to its own noun. "State" is
the broadest word in the domain; spending it on the first narrow mechanism risks
cornering a future state *store* / *machine* / derived-state graph, and cuts against
PostRetro's narrow-primitive, additive-growth ethos.

- **RECOMMEND:** name it for the unit — `defineValue` / `trackValue`, or a
  `state(initial, opts)` factory — returning `StateValue<T>`. **Keep the type name**
  `StateValue<T>` (it is correct, specific, and is the published contract M10/SDK build
  against). Reserve "state" as an umbrella. Keep value-shape (scalar now; structured /
  list later) and derived / computed values as *additive* options or siblings so the
  name stays accurate as it grows.
- Counterweight: if modder familiarity (React/Svelte "declare state") is the priority
  *and* you commit to "exactly one kind of state, forever," `defineState` is
  defensible. **OPEN** — owner picks the verb. Cheap pre-ship; the contract-bearing
  name (`StateValue<T>`) is unaffected either way. **(Resolved → §11.12: `defineStore`
  — it names a *grouping*, sidestepping the overclaim critique above.)**

### 11.4 Value vs. store — the slot is a projection, not the source of truth

Two stores: the **entity store** (`EntityRegistry`, authoritative game state) and the
**UI slot table** (displayed projection). `player.health` the slot is a *read
projection* of the authoritative entity `HealthComponent`; **never make the UI slot the
source of truth.** Engine-owned readonly slots are projections; modders `defineValue`
their own; UI-local/settings are the third population (§11.6).

### 11.5 The producer is absent — the proxy is load-bearing; keep M10 out of C

There is **no health anywhere today.** `EntityRegistry` has no `Health` component
(kinds: `BillboardEmitter` / `FogVolume` / `Light` / `Mesh` / `Particle` /
`PlayerMovement` / `SpriteVisual` / `Weapon`, `registry.rs:78-85`). Damage exists only
as a transient `DamagePayload` / `ActivationOutcome::Hit` the weapon emits
(`weapon/mod.rs:20,27,142`) with **no consumer** — the loop is open; closing it is M10
"Entity health + damage surface" (`[ ]`, unbuilt). So C's static proxy is *load-bearing*
(it stands in for a producer that genuinely does not exist), and the spec must resist
letting "show health" pull entity-health into C's scope.

- "Static proxy" = stand-in for real game logic, **not** "constant." A time-driven
  proxy (oscillation, decrement, intro flash) is what exercises diffing — preferred.

### 11.6 Slot taxonomy — what belongs in UI state

Three populations: **(1) engine-owned readonly projections** (`player.health` /
`player.ammo` — the M10 contract); **(2) modder-declared gameplay values** (a mod
resource, objective progress, score / combo / timer); **(3) UI-local / settings**
(menu-open, focus index; `persist:true` config like volume / sensitivity).

- **Belongs:** scalar / small-cardinality, bounded, a summary a player reads off the
  HUD, sampled not streamed.
- **Does NOT belong:** authoritative state (source of truth lives in the entity store),
  high-cardinality collections / per-entity data, high-frequency sim data nothing
  displays, structured / nested objects (v1 is scalar `SlotValue`), transient events
  (those are M6 / E).
- **Litmus:** "would a player read this off the HUD / would a designer bind a widget to
  it?" → candidate slot. "is it the source of truth, a firehose, or a collection?" →
  not a slot.

### 11.7 Redraw vs. relayout — the key C implementation split

Separate **layout invalidation** (gated, expensive; fires on structural / size change —
text content, image size) from **draw-data refresh** (cheap; reads live slot values
every frame from the *cached* layout — color, fill). B fuses these (its draw list is
derived from layout); **C must split them.**

- **Put in C scope explicitly:** the draw list re-reads the current slot value even when
  taffy layout is cached. This is the path the color-flash demo (§11.8) proves and the
  one most likely to be missed.
- Conservative default (§3.5): any bound change dirties the node; refine later to skip
  relayout when measured size is unchanged.

### 11.8 C's demonstration (DECIDED with owner)

- **Color / fill flash on a `panel`** — appearance-only (size + content unchanged),
  subtle via same-hue RGBA endpoints (no HSL in the wire format; `[f32; 4]` linear RGBA
  per B's Boundary Inventory). The proxy computes the flash color each frame from a
  spawn timer, flashes for ~3 s, then settles to solid. Proves `bind` + value-diffing +
  **draw-data refresh under cached layout** (§11.7); the settle-to-solid tail also
  proves the no-recompute dirty-gate — C's signature, and the B follow-up C must make
  fire in production.
- **`player.health` / `player.ammo` as `text`** ("HP 100") — the M10 contract proof;
  `text` needs no deferred widget.
- **Eased "boot-up" health bar — DEFERRED (owner, 2026-06).** The cosmetic
  display-value-eases-to-authoritative flourish is the seam's best motivating story —
  keep it in the spec's intro — but the literal bar needs F's `bar` widget **and** value
  tweening (new roadmap goal **TW**). It lands as a BIS built-in once those exist. When
  it lands, model it as: authoritative `player.health` is the target; a separate
  UI-owned display value eases to it; the bar binds the *display* value, never the
  authoritative slot.
- **Flash modeling note:** do not push a game-logic-computed color into a slot (that
  couples gameplay to presentation). The slot carries a *semantic* value; styling is
  renderer-local. In C, with `styleRanges` (E) absent, bind a field directly to the slot
  as a plumbing demo — the production restyle path is E.

### 11.9 Sequencing — C does not wait on health

The roadmap intends concurrency: M13's prerequisite line says C "does not require the
entity health/damage surface; the static proxy stands in… neither blocks the other." UI
is **not** gated on health. The real residual risk is that the slot *schema* is a
**guessed contract** until a producer exists.

- **(a)** Design `player.health` against M10's likely `HealthComponent` (bounded `f32`,
  current + max, 0 = dead) and mark it the published contract M10 must honor.
- **(b)** Pull the small, dependency-free M10 "Entity health + damage surface" task
  forward so C binds *real* health (that task is "Pure Milestone 6 — no render / nav /
  AI dependency").
- **RECOMMEND (a):** write the schema *as if M10 exists*, mark it the published
  contract; keeps the seam pure. (b) is viable if schema-correctness anxiety dominates.

### 11.10 The store/UI split — three state concepts (owner decision, 2026-06-06)

A follow-up session split "state" into three distinct concepts (the §11.3 instinct,
applied to the architecture):

1. **Engine-owned global slots** — `player.health` / `player.ammo`. Engine writes,
   readonly to scripts, the M10 contract.
2. **Modder-defined global store** — declared at mod level, shared, persisted; game
   logic owns values the HUD merely displays (score, objective progress, custom
   resources). Game logic writes, UI reads.
3. **Component-local state** — `liveValue()`, per-component-instance, ephemeral.

**Decisions:**

- **The store (#1 + #2 mechanism) is extracted to its own scripting-foundation spec,
  `drafts/mod-state-store/`** — not UI-only, so it sits *below* the UI milestone. It
  owns the slot table, `defineState`, validation, ownership/readonly, the persist wire
  format, the branded `StateValue<T>`, and a behavior-context read/write API (game logic
  reads and writes it). Independently testable; no render.
- **Goal C is now the UI *consumption* layer** — the once-per-frame published snapshot,
  bind-by-slot-name, value diffing → relayout/redraw, the retained tree, the static
  proxy, and publishing the `player.*` schema. C *consumes* the store. Retitled "State
  binding." (Earlier §1–§10 here describe the pre-split monolithic C; the store half of
  that content now lives in the store spec. §11 is the authoritative decision log.)
- **`liveValue()` (component-local) → G1.** It needs a component instance + lifecycle to
  scope to, and script-authored components are G1. Distinct primitive from the global
  store's declaration verb.
- **Naming.** `liveValue()` = component-local (G1). Global store declaration verb is
  `defineState` (working name) or `liveState` (live-family parallel) — **OPEN**, owned
  by the store spec. Type `StateValue<T>` stays. **(Resolved → §11.12: `defineStore`.)**
- **Writes.** Behavior-context direct read/write (game logic, like `world.setGravity`)
  lives in the store spec. The UI-reaction `setState` (writing a slot from a UI
  event/reaction as serializable IR) stays deferred to **E / F**.

### 11.11 Engine ownership, the command buffer, and the unified model (2026-06-06)

A session on store foundations, after the owner discovered the typed command buffer
(`scripting.md` §11) is a fully-articulated engine principle. Reshaped the ownership
story and surfaced a new milestone.

- **Two senses of "engine owns," both legitimate.** (a) *hardware-/loop-layer
  primitives* (storage layout, per-tick systems a script VM can't drive at scale);
  (b) the *opinionated genre vocabulary* — a retro shooter owns health, shields, ammo
  as first-class nouns. Health is (b), **not** (a): the earlier "the loop forces health
  engine-side" argument was overstated — health is a scalar, and the firehose is
  projectiles, not health components. Genre opinion, not hardware, is why it lives in
  core.
- **ECS is two orthogonal axes.** Data layout (dense per-kind component columns —
  already in `registry.rs`, gives projectile scale) vs. who owns the component
  *vocabulary* (engine-closed vs. modder-extensible). Projectile performance pushes only
  on the first; "should the modder own health" only on the second. The index §4
  non-goal was blunted to distinguish them (data-oriented storage = yes; general
  extensible ECS framework = no).
- **The command buffer dissolves the noun/verb tension.** Engine owns the nouns
  (components, store slots) and the total evaluator; the author owns the verbs as IR.
  Shield recharge (`recharge = f(timeSinceDamage, element, damageType)`) is §11's own
  `boost = f(...)` shape — elemental shields and Halo-vs-Borderlands recharge need no new
  mechanism, just §11 bound to a Shield component. Authored *policy* lives in the command
  buffer; the engine ships the component + per-tick system. No live VM.
- **`Stat` vs. dedicated kind — invisible at the syntax layer.** Whether Rust stores a
  dedicated `Health` component or a generic scalar-stat parameterized by name is an
  internal choice; the author writes `health({ max, regen: <policy> })` either way. The
  fork is deferrable and changeable without touching modder code. Lean: a generic
  `Stat`/`Resource` kind serving health + shields, but unforced.
- **The declare-time lifecycle is the same everywhere.** `defineEntity` (→ component
  struct), `defineStore` (→ slot table), the command buffer (→ evaluated IR), reactions
  (→ one-instruction buffer) are one family: **declare as data at load, Rust owns at
  runtime, the VM drops.** No ECS *runtime* API (no `entity.add(Health)`); the model is
  declarative composition, not imperative mutation. Runtime structural change (grant a
  shield to a live entity) is the open edge — lean (a) component-always-present, a
  reaction flips it active; (b) a `grantComponent` reaction opcode only if a case
  demands it.
- **The store is the connective tissue.** Entity components / store slots / command
  buffers / reactions are one architecture; the store slots are the named-input/output
  namespace the evaluator binds against and the UI projects. Deeper reason the store
  ships first.
- **The primitives predate the command buffer.** Reactions, animation channels, and
  behavioral descriptor fields are pre-IR special cases (§11 already calls reactions
  "a one-instruction command buffer"). Consolidating them onto one IR substrate is
  *demand-driven refactoring*, not a teardown — only computed/conditional/derived fields
  migrate; static config stays plain data.

### 11.12 Verb resolved — `defineStore` (supersedes §11.3, §11.10) (2026-06-06)

Owner picked **`defineStore`** for the global store declaration. It declares a *grouping*
of global state values (a store namespace), so the verb names exactly what it returns —
resolving §11.3's "verb overclaims its noun" critique, which the working name
`defineState` triggered. It joins the `define*` family (`defineEntity`, `defineReaction`)
— one definition-context, declare-as-data lifecycle. The component-local `liveValue()`
(G1) is a distinct primitive/lifecycle, so the family split is principled. The type
`StateValue<T>` is unchanged. Follow-up: `research/ui-layer.md` §9 still carries the older
`defineState` term — sync on the next M13 doc pass.

### 11.13 Placement of these decisions (2026-06-06)

Where each conclusion was filed, per the context style guide (durable → `lib/`;
sequenced → roadmap; spec detail + rationale → here):

- **Durable → `context/lib/`.** `index.md` §4 (ECS non-goal split); `entity_model.md`
  §1 (component ownership = hardware + genre vocabulary; representation is internal);
  `scripting.md` §11 (noun/verb ownership split; the named-state surface; the unified
  four-part family).
- **Sequenced → `roadmap.md`.** New **Milestone 14 — Behavior IR (Typed Command
  Buffer)** (substrate + evaluator → IR versioning → first adopter → primitive
  consolidation); M10 health task note (published contract; representation fork; policy
  defers); Future/Gameplay **Shields + damage-type system**.
- **Spec → here + the two `index.md`s.** This decision log holds the reasoning; the
  store spec and Goal C carry only the distilled, implementer-facing form.

---

*End of research. Two specs now exist: `drafts/mod-state-store/` (scripting foundation,
ships first) and `drafts/M13--state-system/` (UI consumption, depends on it). Resolve
each spec's remaining open questions before promotion — the read/write ergonomic shape
and the generic-brand typedef gap (store spec); the snapshot value carrier and format
scope (Goal C). Resolved this session: the declaration verb is `defineStore` (§11.12).
The `bar` widget is deferred to F (owner, 2026-06); `liveValue()` component-local state →
G1; roadmap goal **TW** homes value tweening; the behavior-IR foundation is Milestone 14.*
