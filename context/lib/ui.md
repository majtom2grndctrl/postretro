# UI Layer

> **Read this when:** working on the UI layer (`render/ui/`) — widgets, theming, HUD state binding, UI animation.
> **Key invariant:** scripts declare widget trees and state values; Rust owns the live UI. Authoritative values live in the state store; anything the UI animates or displays is renderer-local presentation state that never writes back.
> **Related:** `context/research/ui-layer.md` (design exploration) · `scripting.md` (state store) · `rendering_pipeline.md` (frame structure) · plans under `context/plans/` `done/M13--*` and `ready/M13--*`.

---

## 1. Layer Shape

UI is a renderer-owned sibling pass to scene rendering, recorded after the world passes, before present, beneath the egui debug overlay. Authoring model: serde descriptor trees (discriminated union per widget kind, camelCase wire) → a Rust-retained layout tree (taffy flex/grid, glyphon-measured text) → a device-pixel draw list. Layout happens in a 1280×720 logical reference space scaled to the native backbuffer; quads and panels snap to integer device pixels, glyphs stay anti-aliased.

Game logic and the renderer meet at exactly one point: a read-only snapshot published once per frame after game logic (descriptor tree, resolved slot values, frame time). The renderer never reads the live slot table. The retained gameplay tree diffs only bound slots frame-over-frame and splits invalidation: content changes relayout, appearance changes only redraw, a settled frame rebuilds nothing.

Gameplay UI is a **modal stack** of trees: top tree's capture mode decides capture vs. passthrough (lower trees freeze; HUD passes through), and behavior-relevant properties — capture mode, text-entry target, initial focus — live on the declared `AnchoredTree` envelope, so a tree declares its own behavior. Trees register by name in an app-side registry, and the render path resolves them **by name** — it never calls a layout builder, so content authoring is decoupled from the frame loop. Engine fallbacks and script registrations share the same registry seam. JSON assets cover layout-only engine screens such as the on-screen keyboard; SDK-authored mod trees are the production path for HUD and pause menu. The registry's name→tree lookup is the seam the per-frame snapshot reads through (a `&self` accessor; the mutating handle stays private). The boot splash is the deliberate exception — JSON-authored too, but it keeps its own pre-gameplay path and **never** enters the registry or modal stack.

### 1.1 Script authoring model

Scripts author UI as the same descriptor trees, built by SDK factory functions (one per widget kind; props-first, container children positional) and registered by name. `Tree(...)` builds the placement envelope; `defineUiTree({ name, tree, alwaysOn? })` builds the returned registration entry while preserving the manifest wire shape. The factory output is wire-identical to JSON — the bridge converts a VM value to the descriptor and the wire round-trips byte-identically, so factory-built and JSON-built trees are indistinguishable downstream. The same "scripts declare, Rust executes; the VM drops after load" contract holds (`scripting.md` §1): registration is the only crossing, and the VM drops after each registration pass.

**Modder components are plain functions.** A reusable component is a function returning a descriptor subtree — no component registry, decorator, or inheritance. It takes the same props-first(-then-children) shape as a factory and nests inside SDK containers; the bridge sees no difference between a factory call and a component call. A component that uses presentation cells (via `ui.createLocalState`) declares its scope on the container it returns, since cell scope resolves to the nearest declaring ancestor — so each component instance owns an independent scope.

**Registration lifecycle.** `setupMod` / `setupLevel` returns carry UI registrations (named trees, theme tokens, font assets) alongside their other fields. The engine drains them into the registry, the live theme, and the font system **before** the short-lived authoring VM context drops — no new lifecycle stage, the same return→VM-drop boundary entity types use. A registered tree marked always-on composes as a per-frame base layer (the HUD case); otherwise it shows only when pushed. Tree precedence is **engine < mod** (per-level tier deferred until runtime level unload exists): a mod registration under an engine built-in's name shadows it. Engine `hud` and `pauseMenu` entries are minimal fallbacks. A mod `hud` or `pauseMenu` shadows the matching fallback, and `hud.reticle` is a separate always-on mod tree for the centered reticle. The registry retains tiers rather than overwriting lower-tier entries, so removing a mod tree reveals the engine fallback with the same name on the next resolve. Malformed registrations are contained: a single malformed tree is logged and skipped; a structurally broken theme/font field surfaces a named load-time diagnostic the caller logs before continuing — a bad UI registration never aborts boot or level load.

**Staged replacement.** Staged mod init commits UI trees and theme with the same successful-generation boundary as returned stores. A successful current staged result replaces returned stores, the complete mod tree tier, and the complete mod theme override together. Omitted mod trees are removed from the mod tier, revealing engine fallbacks with the same name on the next open or always-on resolve. Omitted theme tokens revert to engine defaults. Failed or stale staged results preserve the current stores, registry, and theme. Always-on layers resolve the updated registry on the next frame. Already-pushed modal instances keep their cloned descriptor until closed, even if reload replaces or removes the registered tree. Reopened modals resolve the updated registry entry.

### 1.2 HUD Authoring Contract

The production HUD is authored through the SDK, returned from `setupMod()`, and retained by Rust after mod init drops. Durable contract points are:

- HUD authors import SDK factories, `bindState`, and `getGameState` from `"postretro"`.
- A HUD module obtains `const { player } = getGameState()` when constructing its returned UI-tree registration.
- `bindState(player.health, options)` decorates the readonly ref for display. It does not read HP during authoring.
- The health bar uses `player.maxHealth` as a direct readonly max reference. There is no `player.healthFraction` slot; UI derives the displayed fill from `player.health / player.maxHealth`.
- Bar `styleRanges` evaluate the normalized displayed fill, so health bands use thresholds in `[0, 1]` with `styleRanges.max = 1.0`.
- The reticle is a separate always-on tree from the status HUD because one anchored tree has one viewport anchor.

Tree anchors and offsets are literal placement data. Theme tokens drive styling only: colors, fonts, spacing. Do not route placement through theme tokens.

## 2. Theme Tokens

Theme is three category-scoped maps — colors (linear RGBA), fonts (registered family), spacing (logical px). The category is carried by the referencing field, not a name prefix.

- **Semantic required set — the engine contract.** Colors `critical` / `warning` / `ok` / `panel.default` (a literal flat key, dot and all), fonts `body` / `mono`, spacing `xs` / `s` / `m` / `l`. Built-in screens and `styleRanges` resolve these names; a mod rethemes built-ins by overriding them.
- **Open key space — the primitive tier.** Maps accept arbitrary additional keys and widgets may reference them; a mod-defined primitive palette (`cyan.500`) is supported, not just tolerated.
- **Literal escape hatch.** Every token-capable field is an untagged union: a token name or an inline literal value. One-off treatments need no theme entry.
- **Override merge** is per-token after the complete mod theme override is chosen: the current override replaces only the names it ships; everything else resolves from the engine default.
- **Unknown tokens degrade visibly, never panic:** unknown color → opaque magenta, unknown font → `body`, unknown spacing → zero, each with a warning per tree build.
- **Token aliasing** (semantic → primitive references) is deferred; it widens theme entries additively when a consumer justifies it.

Theme registration arrives through returned manifest data. Custom font asset replacement/removal is not hot-reloadable until the font system has an explicit replacement contract. Staged reload may replace the theme token override, but changing custom font declarations, replacing custom font assets, or removing them requires an engine restart.

The TypeScript/Luau SDK ships `defineTheme(theme)` as authoring sugar for custom themes. It returns the same flat `colors` / `fonts` / `spacing` object that `setupMod().theme` already accepts, plus `tokens.color(...)`, `tokens.font(...)`, and `tokens.spacing(...)` helpers. The helpers capture category keys when `defineTheme` runs, return the token string, and warn on unknown or invalid names so UI resolution can degrade visibly instead of aborting mod init. The helper metadata is non-enumerable/non-retained: generic theme iteration and serialization see only the three category maps. In TypeScript, helpers are keyed from the concrete theme object, so editors autocomplete valid dotted token names at widget call sites. `defineTheme` is not a new runtime theme format; descriptors still carry plain strings, and the Rust theme drain still consumes only the three category maps.

## 3. Display vs. Authoritative Values

Widgets bind authoritative store slots by state reference at the SDK layer and by dotted slot name on the retained wire. The renderer may hold a per-node **display value** that eases toward the authoritative target over a declared duration and curve (tweening). Contract:

- The authoritative slot is always the target; the widget renders the display value, never the slot directly.
- Display state is presentation-only and renderer-local — no store write ever originates in the UI module.
- Retargeting is continuous: a target change mid-flight eases from the current display value, never snaps.
- Bar widgets may resolve `max` from either a literal number or a readonly numeric state reference. The bar fill normalizes the displayed value against that resolved max. `styleRanges` on a bar evaluate the normalized displayed fill; health bars use thresholds in `[0, 1]` and `styleRanges.max = 1.0`.
- UI time is dt-accumulated game time, never wall clock — pausing game logic pauses presentation.
- Structural tree rebuilds discard display state (in-flight values snap to target); rebuilds are rare, authored events.
- `styleRanges` (continuous value→style) evaluate the value the widget renders — the display value mid-tween; state crossings (`onStateCrossing`) watch the **authoritative** slot, engine-side, after game-logic writes. The two may diverge mid-tween by design.
- Diagnostics fire at tree build, never per frame. Unknown tokens (§2) and malformed binds (orphan `{local}`, unknown slot) warn once when the tree is built; the per-frame resolve path stays log- and allocation-free.

Engine-owned UI slots: `screen.flash` (RGBA, engine-decayed flash surface; the screen-effects resolve pass in `render/screen_effects.rs` consumes it), `screen.vignette` (RGBA — rgb tint, a = strength; mod-readonly), `screen.shake` ([dx, dy] offset; mod-readonly), `input.mode` (`pointer` / `focus`, app-written in the input phase), `ui.textEntry` (writable string — the text-entry target). Writability, not ownership, gates event-time writes: readonly slots warn and no-op; engine-owned writable slots are valid targets.

## 4. Interaction

UI input rides one queued seam (`input/ui_dispatch.rs`) with kinded intents (nav / pointer-click / text): an event a capturing tree consumes on frame N reaches game logic no earlier than frame N+1 — the system's defining ordering contract. The focus engine is **app-side** (frame-order rule: it consumes intents, fires reactions, and writes slots — game-logic work; the renderer only displays). The renderer publishes a flat hit-test rect list once per frame (the reverse twin of the snapshot); the focused-node id rides the next snapshot, so the focus ring may trail a focus change by one frame, the same latency every UI event carries. Activation resolves to named, pre-registered reactions or closed engine-reserved UI actions — never a script callback. Hover is tracked cursor state, never a queued event.

Reserved UI actions live in the `ui.*` namespace. They are button `onPress` values that the App intercepts before named-reaction dispatch. `ui.commitTextEntry` commits the active text-entry modal. `ui.closeDialog` pops the active modal. Other `onPress` names keep the normal named-reaction path.

The production pause menu is a capturing modal registered as `pauseMenu`. The mod-authored SDK tree is the normal production entry; the engine keeps a minimal fallback under the same name so Escape / Start still open and close a menu when no mod tree is registered. `nav.menu` opens `pauseMenu` only when the modal stack is empty, closes it when it is active, and is ignored while another modal is active. `nav.cancel` closes only an active `pauseMenu`; other modal types own their own cancel policy. The pause menu is not a true simulation pause: while it is active, UI capture suppresses player controls and releases the cursor, but game simulation, animation clocks, particles, audio, and UI time continue unless a separate true-pause system is added.

## 5. Render Path & Asset Loading

- **One `prepare`/vertex-buffer fill per surface composition.** The whole frame's UI — every modal-stack layer's quads and text — is encoded as a **single composition**, not one encode per layer. glyphon's `prepare()` overwrites a single internal vertex buffer at offset 0, and `queue.write_buffer` resolves on the **queue timeline, not the command-recording timeline**, so recording a draw between two writes does not snapshot the buffer. A per-layer encode loop therefore makes the *last* layer's glyphs win for *every* layer (the historical modal-stack text clobber). The quad path obeys the same rule by giving each batch a disjoint instance-buffer region. `encode` takes the whole composition by type, so the per-layer loop is unrepresentable; a debug-only guard additionally fires on a second `prepare` within one composition. Do not reintroduce a per-layer encode.

- **Asset paths: cwd-relative at runtime, `CARGO_MANIFEST_DIR` only under `#[cfg(test)]`.** The engine runs from the workspace root, so content loaders resolve `content/base/...` relative to the current directory. Tests anchor to `CARGO_MANIFEST_DIR` (`../..` to the workspace root) because `cargo test`'s working directory is the *crate* dir — gate that anchor behind `#[cfg(test)]`. **Never bake `CARGO_MANIFEST_DIR` into a production loader:** it embeds the build machine's absolute path into the shipped binary. Every asset loader (UI JSON, keyboard, splash, maps, textures) follows this. Missing/malformed engine-shipped content warns once and degrades (the screen is absent, or a minimal in-code fallback) — it never aborts boot.

## 6. Non-Goals

- A live scripting VM driving UI logic, or per-frame script callbacks.
- Direct UI-module writes to authoritative state — event-time writes go through the `setState` reaction (and the text-edit reactions built on its gate), applied at the game-logic stage.
- DOM/CSS-style markup or cascade — descriptor objects only.
