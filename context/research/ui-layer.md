# UI Layer — Design Exploration

**Date investigated:** 2026-05-11
**Status:** Pre-spec exploration. Not yet a draft plan.

> **Read this when:** thinking about how PostRetro will render HUDs, menus,
> dialogs, minigame chrome, and how modders author the same. Captures the
> architectural shape before any code lands.
> **Key invariant:** scripts declare widget trees and state values; Rust owns
> the live UI. No script VM runs continuously to drive UI.
> **Related:** `context/lib/scripting.md` · `context/lib/entity_model.md` ·
> `context/lib/rendering_pipeline.md` · `context/lib/index.md`

---

## 1. Problem

PostRetro needs UI for built-in concerns (HUD, damage feedback, level load,
pause menu, dialogs) and for a modder SDK rich enough to author looter-shooter
inventories, immersive-sim journals, and minigame chrome. The architectural
constraint: same model as entities and reactions — scripts declare at load
time, the VM drops, Rust executes thereafter. No background scripting VM, no
mid-frame script calls, no per-tick author code.

The bet: a retained widget tree with state-slot bindings, evaluated entirely
in Rust, covers the surface area without a live VM.

---

## 2. Core Design Principles

| Principle | Invariant |
|-----------|-----------|
| **Declare, don't drive.** | Scripts emit widget descriptors at load time. Rust owns the live tree, layout, and dispatch. |
| **State values are the only dynamic surface.** | Engine-owned values (e.g. `player.health`) and modder-declared values are what widgets bind to. Renderer dereferences `StateValue<T>` handles each frame. |
| **Reactions are the only event surface.** | UI events (input, focus, threshold crossings) fire pre-registered reactions from the engine-global reaction registry. |
| **Renderer owns GPU.** | UI is one more pass in the renderer module. Layout produces draw commands; submission stays inside renderer. |
| **Retro by default, capable underneath.** | Bitmap-grid layout, blocky panels, pixel snapping. Underlying stack supports shaped text and flex for SDK richness. |

---

## 3. The Model

Three concepts cover the surface:

- **Descriptors** — object literals registered once at level/mod load.
- **State** — named, typed values owned by Rust. Declared via `defineState`; widgets bind via `StateValue<T>` handles.
- **Reactions** — pre-registered tag-targeted Rust handlers. UI events resolve
  to reactions; no script callback fires at runtime.

```ts
// Proposed design — registered at level load, VM drops after.
registerHud({
  layout: "vstack", gap: 4, anchor: "bottomLeft",
  children: [
    { kind: "text", text: "HP" },
    { kind: "bar", bind: "player.health", max: 100,
      styleRanges: [
        { upTo: 0.20, color: "critical", pulse: { periodMs: 400 } },
        { upTo: 0.50, color: "warning" },
        {             color: "ok" },
      ],
    },
    { kind: "text", bind: "player.ammo.current", format: "{}/{max}" },
  ],
})

onStateCrossing("player.health", { below: 0.20 }, [
  flashScreen({ tag: "damage-vignette", color: "red", durationMs: 200 }),
  playSound("hud/critical"),
  rumble({ strong: 0.5, durationMs: 150 }),
])
```

The descriptor outlives the VM. Rust walks it every frame.

---

## 4. Subsystem Boundary and Frame Order

UI sits inside the renderer module as a sibling pass to scene rendering. It
consumes engine state through narrow read-only handles; it does not own
gameplay state.

```
Input → Game logic → Audio → Render
                              ├── Scene pass
                              └── UI pass   (layout → draw → submit)
                            → Present
```

Input dispatch to UI runs in the **Input** stage, not the render stage.
Layout and drawing run in the render stage because they read final
post-game-logic state. Order matters: a UI click resolved on frame N must be
visible to game logic on frame N+1, like any other input.

**Module boundary.** UI is a peer of `render/scene/`, `audio/`, `input/`.
Widgets, state slots, layout, draw lists, and input dispatch live inside the
UI module. Engine state reaches UI via a read handle published once per frame
after game logic completes.

---

## 5. Crate Dependencies

Take libraries per layer; do not take a UI framework. Frameworks own loops,
schedulers, or state models we already own.

| Layer | Crate | Role |
|-------|-------|------|
| Layout | `taffy` | Flexbox/grid/block math on a descriptor tree. No rendering, no input. |
| Text | `glyphon` | wgpu-native shaped text + glyph atlas. Built on `cosmic-text`. |
| Image | `image` (existing) | Decode for sprite/icon assets. |
| Animation | inline or `keyframe` | Easing curves. Likely inlined. |
| Quads / 9-slice | hand-rolled wgpu pipeline | One shader, vertex buffer, instanced draws. |
| Input | `winit` + `gilrs` (existing) | Raw events. Hit-testing and focus are in-house. |

Frameworks evaluated and rejected: Iced (owns message loop), Slint (parallel
DSL/scheduler), Dioxus (VDOM diffing assumes live driver), egui (immediate
mode — wrong shape for retained descriptors), Bevy UI (ECS-coupled). None map
cleanly onto the declare-then-drop lifecycle.

---

## 6. Widget Vocabulary

Small core; presets in SDK library. Each widget is a serde struct on the wire,
a Rust enum variant in the engine, a TS/Luau type in the SDK.

| Widget | Purpose |
|--------|---------|
| `text` | Static or bound text. Optional `format` template. |
| `bar` | Progress bar bound to numeric slot. `styleRanges` for color/effect tiers. |
| `image` | Sprite or icon by asset key. |
| `panel` | Background rect with optional 9-slice sprite. |
| `vstack` / `hstack` | Linear layout container. `gap`, `align`, `padding`. |
| `grid` | 2D grid container. |
| `spacer` | Flex spacer. |
| `button` | Focusable, fires a tagged reaction on activate. |
| `input` | Text entry, fires reactions on commit. Optional. |
| `list` | Vertical list of items from a bound array slot. |
| `viewport` | World-space passthrough for in-world UI (terminals, signs). |

`button` and `input` are the only widgets that introduce script-author event
surface — both route to pre-registered reactions, not callbacks.

Containers compose. Visual leaves do not nest children.

---

## 7. Layout Model

Flexbox-style: parents distribute, children fill. `taffy` runs the math.

- **Anchor + offset.** Top-level descriptors anchor to one of nine screen
  positions, with pixel offset. Anchors are aspect-ratio-stable.
- **Pixel snapping.** All computed rects snap to integer pixels in the design
  resolution before draw. Preserves retro aesthetic; avoids subpixel text
  blur.
- **Design resolution.** UI authors against a fixed grid (e.g. 320×240 base).
  Renderer upscales the final UI render target with nearest-neighbor.
- **Safe area.** Gamepad-controlled platforms reserve a configurable safe
  area; built-in HUD respects it by default.

---

## 8. Style and Theming

Theme is a named table of colors, font keys, panel sprites, and spacing
constants. Widgets reference theme tokens by name (`color: "critical"`), not
literal values. A mod can ship its own theme; built-ins resolve against the
default theme.

- **Tokens** are the durable contract: `critical`, `warning`, `ok`,
  `panel.default`, `font.body`, `font.mono`.
- **Fonts** are registered like other assets. Default is a single bitmap
  font baked into the engine. Mods register additional fonts (TTF) and
  `glyphon` handles shaping.

---

## 9. State API

Rust owns all authoritative state. Because of that, the script-side API sheds the entire Redux ceremony — no actions, reducers, selectors, middleware, derived state, or async wiring in script. Rust handles coalescing writes within a frame, persistence for marked values, schema validation and clamping, change broadcast, read-only enforcement for engine-owned values, defaults, and reset.

State is declared in namespaced modules using `defineState`. The namespace lives in the export name, not in a string key. Property access is real property access, not bracket-with-dotted-string.

```ts
// Proposed design — content/scripts/state.ts
import { defineState } from "postretro/state"

export const audio = defineState("audio", {
  master: { type: "number", default: 80, range: [0, 100], persist: true },
  music:  { type: "number", default: 70, range: [0, 100], persist: true },
  sfx:    { type: "number", default: 90, range: [0, 100], persist: true },
})

export const a11y = defineState("a11y", {
  subtitles: { type: "boolean", default: false, persist: true },
  captions:  { type: "boolean", default: true,  persist: true },
})

export const menu = defineState("menu", {
  currentTab: { type: "enum", values: ["audio", "video", "input", "a11y"], default: "audio" },
})

// Engine-owned: scripts can read, cannot write.
export const player = defineState("player", {
  health: { type: "number", readonly: true },
  armor:  { type: "number", readonly: true },
})
```

`StateValue<T>` is the per-value handle. Three operations:

- `audio.master` — the handle. Pass to descriptors as a binding.
- `audio.master.get()` — read current value. Legal only inside handlers.
- `audio.master.set(v)` — write. Legal only inside handlers.
- `audio.master.reset()` — restore declared default.

**Handles vs. imperative I/O.** Descriptors take handles; the engine dereferences them at render time. Handlers do imperative I/O via `.get()` and `.set()`. Calling `.get()` during descriptor build captures a stale snapshot — exactly the bug the model prevents.

| Aspect | Behavior |
|--------|----------|
| **Engine-owned values** | `player.health`, `player.armor`, `player.ammo.<weapon>`, `player.weapon.current`, `level.name`, `hud.objective`. Stable names; survive refactor. `readonly: true` — scripts read, cannot write. |
| **Modder-declared values** | `defineState(namespace, schema)` at load time. Types: number, boolean, string, enum, array. No nested objects. Flat surface. |
| **Persistence** | `persist: true` survives map transitions. Rust serializes and restores on engine start. |
| **Update path** | Game logic writes engine-owned values. Handlers write modder-declared values. No writes from descriptor build time. |
| **Diffing** | Renderer compares values across frames to invalidate cached layout regions. |

---

## 10. Continuous Styling — `styleRanges`

Renderer-local mechanism for value→style mapping. No VM, no event dispatch.
Evaluated every frame on the bound slot's current value.

```ts
// Proposed design
{ kind: "bar", bind: "player.health", max: 100,
  styleRanges: [
    { upTo: 0.20, color: "critical", pulse: { periodMs: 400 } },
    { upTo: 0.50, color: "warning" },
    {             color: "ok" },
  ],
}
```

- Entries are ordered by `upTo` (fraction of max). First match wins; trailing
  entry is the default.
- `color` resolves to a theme token. `pulse` overlays a sinusoidal alpha
  modulation; `flash` overlays a one-shot. Effect set is finite and known to
  the renderer.
- Applies to `bar`, `text`, `panel`. Not all widgets.

---

## 11. Discrete Events — Crossings and Input

`onStateCrossing` fires when a slot value crosses a threshold. The handler
list is a sequence of pre-registered reactions; no script code runs at fire
time.

```ts
// Proposed design
onStateCrossing("player.health", { below: 0.20 }, [
  flashScreen({ tag: "damage-vignette", color: "red", durationMs: 200 }),
  playSound("hud/critical"),
  rumble({ strong: 0.5, durationMs: 150 }),
])
```

Crossings are computed in the engine slot table after game logic writes. Each
crossing dispatches the reaction list synchronously inside the engine.
Reactions are tag-targeted, same registry as entity reactions.

**Input events** route through the same shape: a focused widget consumes
input, resolves it to a tagged reaction, the engine fires the reaction.
`button` activation, `input` commit, navigation keys, and gamepad
confirm/cancel are all reactions.

---

## 12. Input Dispatch

| Concern | Approach |
|---------|----------|
| **Hit-testing** | Layout produces a flat list of rects with widget IDs. Pointer hits do binary search on z-ordered rects. |
| **Focus** | Single focused widget. Navigation keys / gamepad d-pad move focus through a layout-derived focus ring. |
| **Modal stacks** | UI is a stack of trees. Top tree captures input; lower trees freeze. Pause menu, dialog, minigame chrome push onto the stack. |
| **Capture vs. passthrough** | Each top-level descriptor declares whether it captures pointer/keyboard or passes through to game logic. HUD passes through; menu captures. |
| **Gamepad** | `gilrs` events route through the same focus/activation path as keyboard. Analog stick navigation has a configurable deadzone and repeat rate. |

---

## 13. Rendering Pipeline Placement

UI render runs after scene render, before tonemap/present. It writes into the
same final color target with alpha blending. Depth test disabled inside UI;
draw order is descriptor-tree order with per-tree z-bias.

| Pass step | Owner |
|-----------|-------|
| Layout (taffy) | UI module — runs on dirty trees only. |
| Text shaping & atlas churn | `glyphon` — UI module orchestrates. |
| Quad/9-slice draw list assembly | UI module. |
| GPU submission | Renderer (per the renderer-owns-GPU invariant). |
| Post-UI effects (vignette, flash, screen shake) | Scene compositor — driven by reactions on slots like `screen.flash`. |

Screen-space effects authored as widgets (flash, vignette) are layered as
full-screen quads in the UI pass. Reactions that fire them set a slot; the
widget reads the slot.

---

## 14. Built-in Screens

These ship with the engine and cannot be uninstalled (a mod may override
their theme tokens or replace the descriptor entirely).

| Screen | Trigger | Notes |
|--------|---------|-------|
| **HUD** | Always active during gameplay. | Default descriptor bound to player slots. |
| **Damage vignette** | `onStateCrossing("player.health", { decreasedBy: ... })`. | Reaction-driven full-screen flash. |
| **Level load** | Engine event when map streams in. | Static panel with bound `level.name`, progress slot. |
| **Pause** | Input event captured by engine. | Modal; pushes onto UI stack. |
| **Dialog** | Reaction `showDialog({ ... })`. | Modal; emits a reaction on dismiss. |
| **Death / respawn** | `onStateCrossing("player.health", { reaches: 0 })`. | Modal. |

Every built-in resolves through the same descriptor + slot + reaction model
as mod-authored UI. No private path.

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
  `showDialog`, `openMenu`.

---

## 16. Input Model

Mouse-first click-event models break when a player picks up a controller. Game
UI input is a small state machine, not a sequence of pointer events. Three
things real game UIs handle that a click model elides:

### Navigation intents vs. widget-internal intents

D-pad Left on a focused list navigates to the previous sibling. D-pad Left on a
focused slider adjusts its value. Widgets get first refusal on directional
intents via a `capturesNav` prop:

```ts
// Proposed design
Slider({
  label: "Master volume",
  value: audio.master,
  min: 0, max: 100, step: 5,
  capturesNav: ["nav.left", "nav.right"],   // focused slider eats horizontal nav
})
```

When `capturesNav` is set, the listed intents are consumed by the widget before
the container's navigation policy sees them. Widgets without `capturesNav` let
all directional intents fall through to the container.

Nav intents are template-literal-typed to catch typos at compile time:

```ts
// Proposed design
export type NavIntent =
  | "nav.up" | "nav.down" | "nav.left" | "nav.right"
  | "nav.next" | "nav.prev"
  | "nav.confirm" | "nav.cancel"
  | "nav.menu" | "nav.options"
```

### Hold-to-repeat

Declared at the container's `focus` policy, not on individual handlers. Holding
D-pad Down walks a list at roughly 6 steps/sec, not 60. The rate is
configurable:

```ts
// Proposed design
VStack({
  focus: { mode: "linear", repeat: { initialDelayMs: 350, intervalMs: 90 } },
}, ...)
```

Omitting `repeat` gives a single-fire response per press. Repeat only applies
to navigation intents; confirm/cancel never repeat.

### Input-mode switching

Mouse motion → pointer mode (cursor visible, focus ring hidden). Stick or D-pad
press → focus mode (cursor hidden, focus ring visible). The engine manages the
transition; the current mode is a `StateValue<InputMode>` that any descriptor
can read:

```ts
// Proposed design
import { input } from "postretro/state"

Cursor({ visibleWhen: input.mode, equals: "pointer" })
FocusRing({ visibleWhen: input.mode, equals: "focus" })
```

Scripts do not manage input mode. The engine detects the triggering event and
updates `input.mode`. Descriptors bound to it rerender when the value changes.

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

## 18. Lifecycle

| Stage | What happens |
|-------|--------------|
| **Mod init** | `start-script` and imports run. UI helpers register theme tokens, fonts, custom state slots, and reaction handlers into engine-global registries. VM context drops. |
| **Level load** | Data script runs. May register level-scoped HUD overrides, dialog descriptors, minigame configurations, and reactions. VM context drops. |
| **Gameplay** | Engine reads descriptors and slots every frame. Reactions fire on input and crossings. No VM. |
| **Level unload** | Level-scoped descriptors and reactions clear. Mod-init descriptors and engine-global registries survive. |

Mirrors the entity-type/reaction split in `scripting.md` §2 — engine-global
vs. per-level registries are the same shape, separate Rust structures.

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
- Subpixel-positioned anti-aliased text (retro aesthetic).
- DOM-style mutation API for widgets after registration. Re-register the
  subtree instead.
- Web/browser-compatible markup (HTML, CSS) — descriptor objects, not
  parsed markup.
- Built-in accessibility tooling (deferred).
- Cross-VM communication for UI events (QuickJS and Luau remain
  independent).
- Imperative drawing API for mod-authored custom widgets. New widget kinds
  ship in Rust.
