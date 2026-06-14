# UI Layer

> **Read this when:** working on the UI layer (`render/ui/`) — widgets, theming, HUD state binding, UI animation.
> **Key invariant:** scripts declare widget trees and state values; Rust owns the live UI. Authoritative values live in the state store; anything the UI animates or displays is renderer-local presentation state that never writes back.
> **Related:** `context/research/ui-layer.md` (design exploration) · `scripting.md` (state store) · `rendering_pipeline.md` (frame structure) · plans under `context/plans/` `done/M13--*` and `ready/M13--*`.

---

## 1. Layer Shape

UI is a renderer-owned sibling pass to scene rendering, recorded after the world passes, before present, beneath the egui debug overlay. Authoring model: serde descriptor trees (discriminated union per widget kind, camelCase wire) → a Rust-retained layout tree (taffy flex/grid, glyphon-measured text) → a device-pixel draw list. Layout happens in a 1280×720 logical reference space scaled to the native backbuffer; quads and panels snap to integer device pixels, glyphs stay anti-aliased.

Game logic and the renderer meet at exactly one point: a read-only snapshot published once per frame after game logic (descriptor tree, resolved slot values, frame time). The renderer never reads the live slot table. The retained gameplay tree diffs only bound slots frame-over-frame and splits invalidation: content changes relayout, appearance changes only redraw, a settled frame rebuilds nothing.

Gameplay UI is a **modal stack** of trees: top tree's capture mode decides capture vs. passthrough (lower trees freeze; HUD passes through), and behavior-relevant properties — capture mode, text-entry target, initial focus — live on the declared `AnchoredTree` envelope, so a JSON-authored tree declares its own behavior. Trees register by name in an app-side registry, and the render path resolves them **by name** — it never calls a layout builder, so content authoring is decoupled from the frame loop. Engine built-ins (HUD, pause menu, on-screen keyboard) are JSON (`content/base/ui/*.json`) loaded at boot through one shared load-and-register path; script registration arrives with the UI SDK. The registry's name→tree lookup is the seam the per-frame snapshot reads through (a `&self` accessor; the mutating handle stays private). The boot splash is the deliberate exception — JSON-authored too, but it keeps its own pre-gameplay path and **never** enters the registry or modal stack.

### 1.1 Script authoring model

Scripts author UI as the same descriptor trees, built by SDK factory functions (one per widget kind; props-first, container children positional) and registered by name. The factory output is wire-identical to JSON — the bridge converts a VM value to the descriptor and the wire round-trips byte-identically, so factory-built and JSON-built trees are indistinguishable downstream. The same "scripts declare, Rust executes; the VM drops after load" contract holds (`scripting.md` §1): registration is the only crossing, and the VM drops after each registration pass.

**Modder components are plain functions.** A reusable component is a function returning a descriptor subtree — no component registry, decorator, or inheritance. It takes the same props-first(-then-children) shape as a factory and nests inside SDK containers; the bridge sees no difference between a factory call and a component call. A component that uses presentation cells (`§3` / `createLocalState`) declares its scope on the container it returns, since cell scope resolves to the nearest declaring ancestor — so each component instance owns an independent scope.

**Registration lifecycle.** `setupMod` / `setupLevel` returns carry UI registrations (named trees, theme tokens, font assets) alongside their other fields. The engine drains them into the registry, the live theme, and the font system **before** the authoring VM context drops — no new lifecycle stage, the same register→VM-drop boundary entity types use. A registered tree marked always-on composes as a per-frame base layer (the HUD case); otherwise it shows only when pushed. Tree precedence is **engine < mod** (per-level tier deferred until runtime level unload exists): a mod registration under an engine built-in's name shadows it (last-wins + a one-line warning — the reskin path). Malformed registrations are contained: a single malformed tree is logged and skipped; a structurally broken theme/font field surfaces a named load-time diagnostic the caller logs before continuing — a bad UI registration never aborts boot or level load.

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

## 5. Render Path & Asset Loading

- **One `prepare`/vertex-buffer fill per surface composition.** The whole frame's UI — every modal-stack layer's quads and text — is encoded as a **single composition**, not one encode per layer. glyphon's `prepare()` overwrites a single internal vertex buffer at offset 0, and `queue.write_buffer` resolves on the **queue timeline, not the command-recording timeline**, so recording a draw between two writes does not snapshot the buffer. A per-layer encode loop therefore makes the *last* layer's glyphs win for *every* layer (the historical modal-stack text clobber). The quad path obeys the same rule by giving each batch a disjoint instance-buffer region. `encode` takes the whole composition by type, so the per-layer loop is unrepresentable; a debug-only guard additionally fires on a second `prepare` within one composition. Do not reintroduce a per-layer encode.

- **Asset paths: cwd-relative at runtime, `CARGO_MANIFEST_DIR` only under `#[cfg(test)]`.** The engine runs from the workspace root, so content loaders resolve `content/base/...` relative to the current directory. Tests anchor to `CARGO_MANIFEST_DIR` (`../..` to the workspace root) because `cargo test`'s working directory is the *crate* dir — gate that anchor behind `#[cfg(test)]`. **Never bake `CARGO_MANIFEST_DIR` into a production loader:** it embeds the build machine's absolute path into the shipped binary. Every asset loader (UI JSON, keyboard, splash, maps, textures) follows this. Missing/malformed engine-shipped content warns once and degrades (the screen is absent, or a minimal in-code fallback) — it never aborts boot.

## 6. Non-Goals

- A live scripting VM driving UI logic, or per-frame script callbacks.
- Direct UI-module writes to authoritative state — event-time writes go through the `setState` reaction (and the text-edit reactions built on its gate), applied at the game-logic stage.
- DOM/CSS-style markup or cascade — descriptor objects only.
