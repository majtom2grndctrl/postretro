# UI Layer

> **Read this when:** working on the UI layer (`render/ui/`) — widgets, theming, HUD state binding, UI animation.
> **Key invariant:** scripts declare widget trees and state values; Rust owns the live UI. Authoritative values live in the state store; anything the UI animates or displays is renderer-local presentation state that never writes back.
> **Related:** `context/research/ui-layer.md` (design exploration) · `scripting.md` (state store) · `rendering_pipeline.md` (frame structure) · plans under `context/plans/` `done/M13--*` and `ready/M13--*`.

---

## 1. Layer Shape

UI is a renderer-owned sibling pass to scene rendering, recorded after the world passes, before present, beneath the egui debug overlay. Authoring model: serde descriptor trees (discriminated union per widget kind, camelCase wire) → a Rust-retained layout tree (taffy flex/grid, glyphon-measured text) → a device-pixel draw list. Layout happens in a 1280×720 logical reference space scaled to the native backbuffer; quads and panels snap to integer device pixels, glyphs stay anti-aliased.

Game logic and the renderer meet at exactly one point: a read-only snapshot published once per frame after game logic (descriptor tree, resolved slot values, frame time). The renderer never reads the live slot table. The retained gameplay tree diffs only bound slots frame-over-frame and splits invalidation: content changes relayout, appearance changes only redraw, a settled frame rebuilds nothing.

Gameplay UI is a **modal stack** of trees: top tree's capture mode decides capture vs. passthrough (lower trees freeze; HUD passes through), and behavior-relevant properties — capture mode, text-entry target, initial focus — live on the declared `AnchoredTree` envelope, so a JSON-authored tree declares its own behavior. Trees register by name in an app-side registry (engine built-ins at boot; script registration arrives with the UI SDK). The boot splash stays on its own pre-gameplay path, outside the stack.

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
- `styleRanges` (continuous value→style) evaluate the value the widget renders — the display value mid-tween; state crossings (`onStateCrossing`) watch the **authoritative** slot, engine-side, after game-logic writes. The two may diverge mid-tween by design.

Engine-owned UI slots: `screen.flash` (RGBA, engine-decayed flash surface; the post-UI-effects goal consumes it later), `input.mode` (`pointer` / `focus`, app-written in the input phase), `ui.textEntry` (writable string — the text-entry target). Writability, not ownership, gates event-time writes: readonly slots warn and no-op; engine-owned writable slots are valid targets.

## 4. Interaction

UI input rides one queued seam (`input/ui_dispatch.rs`) with kinded intents (nav / pointer-click / text): an event a capturing tree consumes on frame N reaches game logic no earlier than frame N+1 — the system's defining ordering contract. The focus engine is **app-side** (frame-order rule: it consumes intents, fires reactions, and writes slots — game-logic work; the renderer only displays). The renderer publishes a flat hit-test rect list once per frame (the reverse twin of the snapshot); the focused-node id rides the next snapshot, so the focus ring may trail a focus change by one frame, the same latency every UI event carries. Activation resolves to named, pre-registered reactions — never a script callback. Hover is tracked cursor state, never a queued event.

## 5. Non-Goals

- A live scripting VM driving UI logic, or per-frame script callbacks.
- Direct UI-module writes to authoritative state — event-time writes go through the `setState` reaction (and the text-edit reactions built on its gate), applied at the game-logic stage.
- DOM/CSS-style markup or cascade — descriptor objects only.
