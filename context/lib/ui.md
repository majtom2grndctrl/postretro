# UI Layer

> **Read this when:** working on the UI layer (`render/ui/`) — widgets, theming, HUD state binding, UI animation.
> **Key invariant:** scripts declare widget trees and state values; Rust owns the live UI. Authoritative values live in the state store; anything the UI animates or displays is renderer-local presentation state that never writes back.
> **Related:** `context/research/ui-layer.md` (design exploration) · `scripting.md` (state store) · `rendering_pipeline.md` (frame structure) · plans under `context/plans/` `done/M13--*` and `ready/M13--*`.

---

## 1. Layer Shape

UI is a renderer-owned sibling pass to scene rendering, recorded after the world passes, before present, beneath the egui debug overlay. Authoring model: serde descriptor trees (discriminated union per widget kind, camelCase wire) → a Rust-retained layout tree (taffy flex/grid, glyphon-measured text) → a device-pixel draw list. Layout happens in a 1280×720 logical reference space scaled to the native backbuffer; quads and panels snap to integer device pixels, glyphs stay anti-aliased.

Game logic and the renderer meet at exactly one point: a read-only snapshot published once per frame after game logic (descriptor tree, resolved slot values, frame time). The renderer never reads the live slot table. The retained gameplay tree diffs only bound slots frame-over-frame and splits invalidation: content changes relayout, appearance changes only redraw, a settled frame rebuilds nothing.

## 2. Theme Tokens

Theme is three category-scoped maps — colors (linear RGBA), fonts (registered family), spacing (logical px). The category is carried by the referencing field, not a name prefix.

- **Semantic required set — the engine contract.** Colors `critical` / `warning` / `ok` / `panel.default` (a literal flat key, dot and all), fonts `body` / `mono`, spacing `xs` / `s` / `m` / `l`. Built-in screens and `styleRanges` resolve these names; a mod rethemes built-ins by overriding them.
- **Open key space — the primitive tier.** Maps accept arbitrary additional keys and widgets may reference them; a mod-defined primitive palette (`cyan.500`) is supported, not just tolerated.
- **Literal escape hatch.** Every token-capable field is an untagged union: a token name or an inline literal value. One-off treatments need no theme entry.
- **Override merge** is per-token: an override replaces only the names it ships; everything else resolves from the engine default.
- **Unknown tokens degrade visibly, never panic:** unknown color → opaque magenta, unknown font → `body`, unknown spacing → zero, each with a warning per tree build.
- **Token aliasing** (semantic → primitive references) is deferred; it widens theme entries additively when a consumer justifies it.

Theme registration is engine-side today; script-facing registration arrives with the UI SDK.

## 3. Display vs. Authoritative Values

Widgets bind authoritative store slots by dotted name. The renderer may hold a per-node **display value** that eases toward the authoritative target over a declared duration and curve (tweening). Contract:

- The authoritative slot is always the target; the widget renders the display value, never the slot directly.
- Display state is presentation-only and renderer-local — no store write ever originates in the UI module.
- Retargeting is continuous: a target change mid-flight eases from the current display value, never snaps.
- UI time is dt-accumulated game time, never wall clock — pausing game logic pauses presentation.
- Structural tree rebuilds discard display state (in-flight values snap to target); rebuilds are rare, authored events.

## 4. Non-Goals

- A live scripting VM driving UI logic, or per-frame script callbacks.
- UI writes to authoritative state (the reaction-driven `setState` path is a separate, deferred decision).
- DOM/CSS-style markup or cascade — descriptor objects only.
