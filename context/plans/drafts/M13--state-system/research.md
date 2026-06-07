# Research — M13 state system (grounding + decision log)

> **NOTE — this doc was distilled (2026-06).** The monolithic "Goal C" history (the
> pre-split spec where one goal owned both the store and the UI binding) has been
> **removed**. Live, implementer-facing decisions now live in the two specs:
> `drafts/mod-state-store/index.md` (scripting foundation, ships first) and
> `drafts/M13--state-system/index.md` (UI consumption, depends on it). This file is
> **grounding + decision log only** — code anchors (§9) and the rationale behind
> settled decisions (§11). It is not a spec; do not implement from it. If something
> here reads as a live "decision needed," it is stale — check the two specs first.

> **Read this when:** you need the *why* behind a state-system decision, or a code
> anchor to ground an implementation. The *what* lives in the two specs above.

---

## 2. Slot-schema contract — residual nuance

The store spec pins the engine-owned schema (`player.health` / `player.ammo`:
`number`, `readonly`, engine-owned, no default) and Goal C publishes it as the M10
contract. One nuance not fully closed in the specs:

- **`player.ammo` naming.** Superseded `ui-layer.md` §9 wrote `player.ammo.<weapon>`
  (per-weapon) and `player.ammo.current`. The store spec defers per-weapon/nested
  decomposition (flat scalar slots only). With the flat-surface rule, any such name is
  a *single dotted slot name*, not a nested-object access. The store spec ships
  `player.ammo` as one flat scalar; if a future per-weapon set is wanted, each is its
  own flat slot. The proxy/demo pins exactly one ammo slot name — confirm it in the
  Goal C spec when wiring the demo.

(The earlier `[0,100]` range / per-widget `max:` ownership question is **resolved** —
the slot carries the authoritative schema; no fixed range is pinned on `player.health`
in the store spec, and the `bar` fraction math is deferred to F. See §11.8.)

---

## 9. Key code anchors

The single highest-value artifact here. Line numbers drift — re-verify when touching a
listed file; this lives in the research layer precisely so the `lib/` docs stay
durable.

| File / symbol | Role |
|---|---|
| `render/ui/mod.rs:194` `UiReadSnapshot` | The once-per-frame read handle. Goal C extends it to carry slot values. `version_line` (A), `gameplay_tree` (B). |
| `render/mod.rs:959` `ui_snapshot`, `:2887` `set_ui_snapshot` | Snapshot stored on `Renderer`, set by App pre-render. |
| `render/mod.rs:2945` `record_splash_ui`, `:4386` gameplay path | Where the snapshot is read and the tree laid out. Goal C feeds slot-driven content here. |
| `render/ui/tree.rs:93` `UiTree`, `:177` `build_draw_data`, `:187-192` gate, `:106` `recompute_count` | B's retained tree + dirty gate. Goal C plugs value-diffing into this and must **retain the tree across frames** (B follow-up). |
| `render/ui/tree.rs:170-176` | Explicit note: gate never fires in production until the tree is retained — Goal C's job. |
| `render/ui/descriptor.rs` `Widget`, `AnchoredTree` | B's literal descriptor model. Goal C adds `bind` (content binding) on `text`/`panel`; **no new widget** — `bar` stays deferred to F (owner, 2026-06, §11.8). |
| `scripting/primitives/mod.rs:307` `register_all`, `:17` `register_shared_types` | Where a new `defineStore` domain module registers; where `StateValue`/schema types register for typedefs. |
| `scripting/primitives/world.rs:249` `register_world_primitives`, `:292` `gravity.get/set` | Template for a domain primitive module + an engine-owned mutable value (proxy/store-write precedent). |
| `scripting/data_descriptors.rs:88` `LightDescriptor::validate`, `:164` `WeaponDescriptor::validate` | Rust-side range validation precedent (serde can't bound numbers). |
| `scripting/conv.rs:768` `js_to_json`, `:840` `lua_to_json`, `:731` `json_to_js`, `:808` `json_to_lua` | The serde bridge the `defineStore` schema ingests through (both runtimes). |
| `scripting/primitives_registry.rs:99` `TypeShape::Brand`, `typedef.rs:291` | Branded-type emission. `EntityId` precedent. **Non-generic today — `StateValue<T>` needs an extension.** |
| `typedef.rs:294`, `:1119` | `EntityId = number & { readonly __brand: "EntityId" }` — the branded-type pattern to mirror. |
| `scripting/data_registry.rs:16` `DataRegistry`, `:118` `clear` | Engine-global registry pattern (survives level loads; `clear` touches only `reactions`). Model for the slot table's lifetime. |
| `scripting.md` §10.2 / `set_fog_params.rs` | Clamp-and-warn validation convention. |
| `entity_model.md` §9 | "Entity serialization (save/load)" Non-Goal — the tension with the store's slot persistence (narrower: scalar slots, not entities). |
| `sdk/lib/index.ts`, `sdk/lib/data_script.ts` | SDK-lib re-export structure; `sdk/lib/ui/state.{ts,luau}` is where G1 (not C) wraps `defineStore`. |

---

## 10. Web-research notes

- **Pull, not push.** PostRetro is a once-per-frame *pull* model (renderer dereferences
  the read handle each frame after game logic) — not a fine-grained signal/observable
  push system. Value diffing is the pull-side equivalent of fine-grained invalidation:
  compare values frame-over-frame, mark changed bound nodes dirty. No subscription
  graph; fits the "no live VM, Rust owns the loop" invariant.
- **Branded types are zero-cost.** `StateValue<T>` is intersect-with-phantom
  (`T & { readonly __brand }`) — compile-time only, no runtime cost; `EntityId` already
  uses it. The one wrinkle: PostRetro's brand emitter is non-generic, so `StateValue<T>`
  is the first *generic* brand (store spec Task 4).

---

## 11. Decision log

The authoritative record of *why* each settled decision is what it is. The specs carry
the distilled, implementer-facing *what*; this holds the rationale. Tags: **DECIDED**
(owner-settled), **RECOMMEND** (a lean), **OPEN** (genuinely unresolved).

### 11.5 The producer is absent — the proxy is load-bearing; keep M10 out

There is **no health anywhere today.** `EntityRegistry` has no `Health` component
(kinds: `BillboardEmitter` / `FogVolume` / `Light` / `Mesh` / `Particle` /
`PlayerMovement` / `SpriteVisual` / `Weapon`, `registry.rs:78-85`). Damage exists only
as a transient `DamagePayload` / `ActivationOutcome::Hit` the weapon emits
(`weapon/mod.rs:20,27,142`) with **no consumer** — closing the loop is M10 "Entity
health + damage surface" (unbuilt). So Goal C's static proxy is load-bearing: it stands
in for a producer that genuinely does not exist, and the spec must resist letting "show
health" pull entity-health into UI scope. "Static proxy" = stand-in, **not** "constant":
a time-driven proxy (oscillation, decrement, intro flash) is what exercises diffing.

### 11.6 Slot taxonomy — what belongs in the store

Three populations: **(1) engine-owned readonly projections** (`player.health` /
`player.ammo` — the M10 contract); **(2) modder-declared gameplay values** (a mod
resource, objective progress, score / combo / timer); **(3) UI-local / settings**
(menu-open, focus index; `persist:true` config like volume / sensitivity).

- **Belongs:** scalar / small-cardinality, bounded, a summary a player reads off the
  HUD, sampled not streamed.
- **Does NOT belong:** authoritative state (source of truth lives in the entity store —
  a slot is a *read projection*, never the source of truth), high-cardinality
  collections / per-entity data, high-frequency sim data nothing displays,
  structured / nested objects (v1 is scalar `SlotValue`), transient events (M6 / E).
- **Litmus:** "would a player read this off the HUD / would a designer bind a widget to
  it?" → candidate slot. "is it the source of truth, a firehose, or a collection?" →
  not a slot.

### 11.7 Redraw vs. relayout — the key Goal C implementation split

Separate **layout invalidation** (gated, expensive; fires on structural / size change —
text content, image size) from **draw-data refresh** (cheap; reads live slot values
every frame from the *cached* layout — color, fill). B fuses these (its draw list is
derived from layout); **Goal C must split them.** The draw list re-reading the current
slot value under cached taffy layout is the path the color-flash demo proves and the one
most likely to be missed. Conservative default: any bound change dirties the node;
refine later to skip relayout when measured size is unchanged.

The diff itself is trivially cheap (a scalar compare, well under 1% of frame even at
many slots); the real cost is relayout/re-measure, and that cost is identical under push
or pull. Making diffing **subscriber-aware** (only diff slots a widget actually binds —
O(bound slots)) keeps the snapshot coherent with no subscription graph, and renders
"is per-frame diffing sustainable at scale?" a non-question by construction. The store
holds the *displayed projection* (tens of values a player reads), not the entity
firehose — bullets/enemies live in `EntityRegistry` (`registry.rs:424`) + the M6 event
system and never diff through UI.

### 11.8 Goal C's demonstration (DECIDED with owner)

- **Color / fill flash on a `panel`** — appearance-only (size + content unchanged),
  subtle via same-hue RGBA endpoints (no HSL in the wire format; `[f32; 4]` linear RGBA
  per B's Boundary Inventory). The proxy computes the flash color each frame from a spawn
  timer, flashes ~3 s, then settles to solid. Proves `bind` + value-diffing +
  **draw-data refresh under cached layout** (§11.7); the settle-to-solid tail proves the
  no-recompute dirty-gate — the B follow-up Goal C must make fire in production.
- **`player.health` / `player.ammo` as `text`** ("HP 100") — the M10 contract proof;
  `text` needs no deferred widget.
- **Eased "boot-up" health bar — DEFERRED (owner, 2026-06).** The
  display-value-eases-to-authoritative flourish is the seam's best motivating story —
  keep it in the spec's intro — but the literal bar needs F's `bar` widget **and** value
  tweening (roadmap goal **TW**). It lands as a BIS built-in once those exist: model it
  as authoritative `player.health` = target, a separate UI-owned display value eases to
  it, the bar binds the *display* value, never the authoritative slot. This resolves the
  earlier range/`max`-ownership question — `bar` fraction math is F's, not C's.
- **Flash modeling note:** do not push a game-logic-computed color into a slot (couples
  gameplay to presentation). The slot carries a *semantic* value; styling is
  renderer-local. In C, with `styleRanges` (E) absent, bind a field directly to the slot
  as a plumbing demo — the production restyle path is E.

### 11.9 Sequencing — Goal C does not wait on health

UI is **not** gated on health: the roadmap intends concurrency ("the static proxy stands
in… neither blocks the other"). The real residual risk is that the slot *schema* is a
guessed contract until a producer exists. **RECOMMEND:** design `player.health` against
M10's likely `HealthComponent` (bounded `f32`, current + max, 0 = dead), write the
schema *as if M10 exists*, and mark it the published contract M10 must honor — keeps the
seam pure. (Pulling the small, dependency-free M10 health task forward is the alternative
if schema-correctness anxiety dominates.)

### 11.10 Naming (RESOLVED → §11.12)

The store-declaration verb went through `defineState` → `defineValue`/`trackValue`
deliberation (the critique: a verb claiming the whole "state" category for one narrow
mechanism overclaims its noun and corners a future state *store*). **Resolved: owner
picked `defineStore`** — it names a *grouping* of global state values, sidestepping the
overclaim. The type name `StateValue<T>` is unaffected and stays (it is the published
contract M10/SDK build against). See §11.12.

### 11.11 Behavior IR / command buffer (FILED → M14)

The session that surfaced the typed command buffer (`scripting.md` §11) as a
fully-articulated engine principle has been filed into the durable layer and the
roadmap (see §11.13): the noun/verb ownership split, the unified declare-as-data family
(`defineEntity` / `defineStore` / command buffer / reactions: declare at load, Rust owns
at runtime, the VM drops), and **Milestone 14 — Behavior IR (Typed Command Buffer)**.
The store is the connective tissue — the named-input/output namespace the evaluator
binds against and the UI projects — which is the deeper reason it ships first.
Not re-summarized here; it lives in `scripting.md` §11, `entity_model.md` §1,
`index.md` §4, and the roadmap.

### 11.12 Verb resolved — `defineStore` (supersedes §11.10) (2026-06-06)

Owner picked **`defineStore`** for the global store declaration. It declares a *grouping*
of global state values (a store namespace), so the verb names exactly what it returns —
resolving the "verb overclaims its noun" critique. It joins the `define*` family
(`defineEntity`, `defineReaction`) — one definition-context, declare-as-data lifecycle.
The component-local `liveValue()` (G1) is a distinct primitive/lifecycle, so the family
split is principled. The type `StateValue<T>` is unchanged. Follow-up:
`research/ui-layer.md` §9 still carries the older `defineState` term — sync on the next
M13 doc pass.

### 11.13 Placement / provenance of these decisions (2026-06-06)

Where each conclusion was filed, per the context style guide (durable → `lib/`;
sequenced → roadmap; spec detail + rationale → here):

- **Durable → `context/lib/`.** `index.md` §4 (ECS non-goal split: data-oriented
  storage = yes, extensible ECS = no); `entity_model.md` §1 (component ownership =
  hardware + genre vocabulary; representation is internal); `scripting.md` §11 (noun/verb
  ownership split; the named-state surface; the unified four-part declare-as-data family).
- **Sequenced → `roadmap.md`.** New **Milestone 14 — Behavior IR (Typed Command
  Buffer)**; the M10 health task note (published contract; representation fork; policy
  defers); Future/Gameplay **Shields + damage-type system**.
- **Spec → the two `index.md`s + this log.** The store spec and Goal C carry the
  distilled, implementer-facing form; this decision log holds the reasoning.

---

*Two specs exist: `drafts/mod-state-store/` (scripting foundation, ships first) and
`drafts/M13--state-system/` (UI consumption, depends on it). Resolve each spec's open
questions before promotion. Settled: the declaration verb is `defineStore` (§11.12); the
`bar` widget is deferred to F; `liveValue()` component-local state → G1; roadmap goal
**TW** homes value tweening; the behavior-IR foundation is Milestone 14.*
