# UI Layer — Design Exploration

**Date investigated:** 2026-05-11
**Updated:** 2026-08 (trimmed — shipped UI layer moved to `context/lib/ui.md`)
**Status:** §1–§14, §16, and §18 shipped — `context/lib/ui.md` is a
near-total supersede; read it for the live UI layer. This doc now holds only
what's still unbuilt: §15 (Modder SDK Shape), §17 (minigames), and §19 (open
questions).

> **Read this when:** scoping minigame chrome (`spawnMinigame`, §17) or the
> still-open UI questions (§19) — localization, in-world `viewport` widgets,
> per-pixel hit precision, gamepad text input. For the shipped UI layer
> (descriptor tree, state API, theming, input dispatch, lifecycle), read
> `context/lib/ui.md` instead.
> **Key invariant:** scripts declare widget trees and state values; Rust owns
> the live UI. No script VM runs continuously to drive UI.
> **Related:** `context/lib/ui.md` · `context/lib/scripting.md` ·
> `context/lib/entity_model.md` · `context/lib/rendering_pipeline.md` ·
> `context/lib/index.md`

---

## Shipped — see `context/lib/ui.md`

§1–§14, §16, and §18 below (descriptor tree, state API, `styleRanges`,
`onStateCrossing`, theming, input dispatch, staged/hot reload) shipped.
`context/lib/ui.md` is a near-total supersede of this research — read it
instead of what follows. Old section numbers are kept here as one-line stubs
only, since some `done/` plans cite them by number:

- §1 Problem — shipped; the descriptor-tree/state/reactions model is
  `context/lib/ui.md` §1 (Layer Shape).
- §2 Core Design Principles — shipped; declare-then-drop and
  renderer-owns-GPU hold as built. See `context/lib/ui.md`.
- §3 The Model (descriptors, state, reactions) — shipped; see
  `context/lib/ui.md` §1.1–§1.2.
- §4 Subsystem Boundary and Frame Order — shipped; see
  `context/lib/rendering_pipeline.md` and `context/lib/ui.md` §1.
- §5 Crate Dependencies — shipped; `taffy`/`glyphon` choices are in code
  (`postretro-ui` crate, `render/ui/`).
- §6 Widget Vocabulary — shipped; see `context/lib/ui.md` and the SDK widget
  factories.
- §7 Layout Model — shipped; see `context/lib/ui.md` §1 (reference space,
  pixel snapping).
- §8 (fonts/theming) — shipped; see `context/lib/ui.md` §2.
- §9 State API — shipped, then superseded again: `defineState` was renamed
  `defineStore` in `context/plans/done/mod-state-store/`. See
  `context/lib/scripting.md` (state store) and `context/lib/ui.md` §3.
- §10 Continuous Styling (`styleRanges`) — shipped; see
  `context/lib/ui.md` §3.
- §11 Discrete Events (crossings and input) — shipped; see
  `context/lib/ui.md` §3–§4.
- §12 Input Dispatch — shipped; see `context/lib/ui.md` §4.
- §13 (text/SSE runtime) — shipped; see `context/lib/ui.md`.
- §14 Built-in Screens — shipped; see `context/lib/ui.md` (HUD, pause,
  dialog, frontend tree registrations).
- §16 Input Model (nav intents, hold-to-repeat, input-mode switching) —
  shipped; see `context/lib/ui.md` §4 (Interaction).
- §18 Lifecycle — shipped; see `context/lib/ui.md` §1.1 (registration
  lifecycle, staged replacement).

---

## 15. Modder SDK Shape

The SDK exposes a thin TS/Luau vocabulary that produces descriptor object
literals. Same pattern as `flicker` and `pulse` for lights — helpers return
plain data; registration primitives consume the data.

### Authoring surface

**Canonical form: factory functions with positional children.**
Capitalized component names. The lineage is Compose and SwiftUI, not React or
HTML. That framing is deliberate: there is no CSS cascade, no event bubbling,
no document flow, no `querySelector`. Modders who arrive expecting React
conventions will misread the model. Modders who arrive expecting Compose or
SwiftUI will not.

TS form:

```ts
// Proposed design
const HealthHud = () => VStack({ spacing: 8 },
  Text("HP"),
  HealthBar({ max: 100 }),
)
```

Luau form:

```lua
-- Proposed design
local HealthHud = function() return VStack({ spacing = 8 },
  Text "HP",
  HealthBar { max = 100 }
) end
```

Props object first, then positional children. Capitalized names follow the
Compose/SwiftUI convention. The two forms are mechanically translatable — same
tree shape, same argument order, same component names.

**JSX is available** as an optional alternative. SWC transforms it to
identical factory calls at build time; no React runtime, no live VM. Lead with
factory calls in examples and docs; JSX is an escape hatch for modders who
prefer it.

Both forms compile down to the same descriptor object the engine ingests.
Helpers exist for ergonomics and IDE completion.

### Modder-defined components

Components are plain functions. No `defineComponent` wrapper, no decorator, no
inheritance. Same call shape as SDK built-ins — props object first, children
positional after. A modder-defined component is indistinguishable from an SDK
widget at the call site.

```ts
// Proposed design
type PanelProps = {
  title: string                 // required
  hint?: string                 // optional
  background?: "panel" | "panelMuted"
}

export const Panel = (props: PanelProps, ...children: Node[]) =>
  VStack({ gap: 8, padding: 12, background: props.background ?? "panel" },
    Text({ text: props.title, style: "panelTitle" }),
    props.hint ? Text({ text: props.hint, style: "panelHint" }) : null,
    ...children,
  )
```

Prop conventions: required props bare, optional with `?:`, constrained string
unions for enum-like values, `StateValue<T>` for reactive values, `HandlerRef`
for callbacks (never a raw closure), children as `...children: Node[]`.

### Accessibility via the type system

Interactive widget types require either `label: string` or `labelledBy: NodeId`.
The descriptor won't construct without one. Same applies to inputs, toggles,
and sliders. Accessibility stops being a checklist and becomes a precondition
for compilation.

`Announce` is a first-class node type, not an attribute:

```ts
// Proposed design
Announce({ priority: "polite" }, "Picked up shotgun shells")
```

The type system enforces several additional classes of correctness. Branded
`StateValue<T>` prevents binding a boolean state handle to a numeric widget.
Template-literal-typed intent names prevent typos in event wiring:

```ts
// Proposed design
type Intent<S extends string> = ...
// "menu.confirm" ✓    "menu.confrim" — type error
```

Discriminated unions per descriptor kind narrow props per kind, so
widget-specific props can't leak across widget types.

The framework's job: keep modders inside the lines without making the lines
feel restrictive. Whole classes of bugs — unlabeled interactive elements,
dangling intents, mistyped state bindings — become impossible to author.

**i18n note.** Accessible names take plain `string` for now. `LocalizedText`
is a planned future tightening: swap the type alias, regenerate, fix the
resulting type errors. The structural accessibility obligations survive that
change because they constrain shape, not content.

### Focus model

Focus comes from tree order by default. Containers declare navigation policy
via a `focus` prop:

- `focus: "linear"` — top-to-bottom for `VStack`, left-to-right for `HStack`.
  Respects RTL when layout direction is set.
- `focus: "spatial"` — grids and free layouts. Nearest-neighbor on D-pad /
  stick direction.
- `focus: { mode: "linear", wrap: true, initial: "resume", repeat: { initialDelayMs: 350, intervalMs: 90 } }` — richer policy with wrapping, initial focus hint, and hold-to-repeat rate.

Per-node override for cases where the spatial guess would be wrong:
`focusNeighbors: { right: "rifle" }`.

`initialFocus` is a screen-level concern, not a per-node concern.
`restoreOnReturn: true` on a container makes focus sticky across navigations.

```ts
// Proposed design
VStack({ focus: "linear" },
  Button({ id: "resume", label: "Resume", onPress: resume }),
  Button({ id: "settings", label: "Settings", onPress: openSettings }),
  Button({ id: "quit", label: "Quit to menu", onPress: quit }),
)

Grid({ cols: 3, focus: "spatial" }, ...)

HStack({ focus: "spatial" },
  VStack({ id: "tabs", focus: { mode: "linear", onCommit: "panel" } }, ...),
  VStack({ id: "panel", focus: { mode: "linear", restoreOnReturn: true } }, ...),
)

Screen({ initialFocus: "resume" },
  // ...
)
```

There is no `focusOrder: number` field. Tree order is the default order;
policy props override at the container boundary.

### Deviations from Compose / SwiftUI

The lineage is Compose and SwiftUI. Where we deviate:

- **Declare-then-drop, no recomposition.** Descriptors are declared once and
  handed to Rust. No `remember {}`, no recomposition keys, no diffing in
  script.
- **Named state, not implicit reactivity.** No `@State` / `remember`. Values
  come from `defineState` namespaces and are explicitly bound via
  `StateValue<T>` handles.
- **Property objects, not modifier chains.** `Box({ padding: 8, background: "panel" }, …)` instead of `.padding(8).background(…)`. Less elegant on one axis; serializable, inspectable, and mechanically translatable to Luau on every other axis.
- **Named handler references, not closures.** `onPress: handlerRef` resolves
  to a registered handler. Forced by the JSON-as-descriptor model; the side
  benefit is mod-overridable, discoverable, analyzable handlers.
- **Input intents over pointer events.** Game UI is gamepad-first. See §16.
- **`styleRanges` and `onStateCrossing` as first-class primitives.**
  Continuous-value → style interpolation and discrete-threshold callbacks are
  central HUD primitives, not afterthoughts.

The unifying theme: where Compose and SwiftUI rely on host-language ergonomics
and runtime reactivity to make UI pleasant, this model uses the type system and
an engine-managed state model to make whole classes of UI bugs impossible to
author.

### SDK file layout

Mirrors the entity-domain convention from `scripting.md` §7:

- `sdk/lib/ui/widgets.{ts,luau}` — widget constructors.
- `sdk/lib/ui/layout.{ts,luau}` — `VStack`, `HStack`, `Grid`, `Spacer`.
- `sdk/lib/ui/theme.{ts,luau}` — theme registration helpers.
- `sdk/lib/ui/state.{ts,luau}` — `defineState`, `onStateCrossing`.
- `sdk/lib/ui/reactions.{ts,luau}` — `flashScreen`, `playSound`, `rumble`,
  `showDialog`, `openMenu`, `setState`. `setState(slot, value)` emits a
  state-write reaction targeting a writable modder-declared slot — the
  on-principle way to mutate a slot at event time without a live VM. Engine-owned
  `readonly` slots (`player.health`) are written by game logic, not by
  `setState`.

---

## 17. Minigames as Built-in Entity Types

Novel simulations (lockpick, hacking puzzle, dialogue tree) live as built-in
ECS entity types in Rust, configured declaratively. Same shape as
`billboard_emitter` from `scripting.md` §10.1: the simulation runs in Rust;
scripts configure it.

```ts
// Proposed design
spawnMinigame({
  kind: "tumbler_lock",
  pins: 5, tolerance: 0.08, pickDurability: 3,
  visual: { tag: "lockpick-skin-rusty" },
  onSuccess: openDoor({ tag: "vault-door" }),
  onFail:    alertGuards({ radius: 800 }),
})
```

UI chrome (lockpick visuals, tumbler positions, pick angle) is authored as a
descriptor tree bound to slots the minigame entity publishes. Engine
publishes `minigame.lockpick.pin[i].position`, `.pin[i].set`,
`.pickAngle`. The chrome reads these slots like any other UI.

Adding a novel minigame the engine doesn't ship requires Rust mod code. This
is a deliberate boundary: scripts do not simulate.

---

## 19. Open Questions

- **Localization.** String IDs vs. inline literals. How modders ship
  translations. Likely a string table per locale registered like theme tokens.
- **Animation richness.** Beyond `pulse`/`flash`/`fade`, do we want timeline
  curves on UI properties? Probably yes; `sdk/lib/util/keyframes` already
  exists.
- **In-world UI** (`viewport` widget). Resolving 3D placement, depth
  interaction, and input picking against a world surface. Defer until a
  concrete use case lands.
- **Accessibility.** Screen reader support is not free with a custom UI
  stack. Out of scope for v1; revisit before public release.
- **Hot reload.** Dev-mode descriptor reload on file change. Slot bindings
  must survive re-registration. Likely shares plumbing with script hot
  reload.
- **Per-pixel hit precision for non-rectangular widgets.** Probably skip;
  retro aesthetic tolerates rect hit zones.
- **Text input on gamepad.** Virtual keyboard descriptor, or punt to platform
  IME. Lean toward a built-in modal descriptor.

---

## 20. Non-Goals

- Live scripting VM driving UI logic.
- Per-frame script callbacks.
- Vector graphics (SVG, splines, beziers).
- DOM-style mutation API for widgets after registration. Re-register the
  subtree instead.
- Web/browser-compatible markup (HTML, CSS) — descriptor objects, not
  parsed markup.
- Built-in accessibility tooling (deferred).
- Cross-VM communication for UI events (QuickJS and Luau remain
  independent).
- Imperative drawing API for mod-authored custom widgets. New widget kinds
  ship in Rust.
