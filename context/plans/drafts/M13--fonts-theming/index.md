# M13 Goal D — Fonts + Theming

> Grounding code anchors: `research.md`. Design source: `context/research/ui-layer.md` §8.

## Goal

Add the theme-token table (colors, font keys, spacing constants) and multi-font supply on top of the shipped A–C UI foundation: widgets reference theme tokens by name (`color: "critical"`), the engine registers multiple TTF faces beyond the single embedded default, and a theme override resolves token-by-token against the engine default. The fourth spec of Milestone 13; the token table is what Goal E's `styleRanges` later resolve against.

**Owner decisions (pre-made):** Rust-side only — no script-facing `registerTheme`/font registration; script ingestion stays at G1, following the B/C precedent. Color fields become an untagged string-or-array union, preserving B's locked literal wire contract. Token categories are colors + font keys + spacing constants; panel-sprite tokens are deferred until a textured-panel consumer exists (BIS).

## Scope

### In scope

- **Theme model + wire format.** New `render/ui/theme.rs`: a `UiTheme` with three category maps — colors (name → linear `[f32; 4]`), fonts (name → registered font family), spacing (name → logical px `f32`) — plus an engine default theme and a serde `ThemeDescriptor` wire form with token-by-token override merge (an override replaces only the tokens it names; everything else resolves from the default). The wire format is a locked deliverable (B precedent) even though ingestion is G1.
- **Required token names (the durable contract, research §8):** colors `critical`, `warning`, `ok`, `panel.default`; fonts `body`, `mono`; spacing `xs`, `s`, `m`, `l`. Category-scoped maps mean the research's dotted `font.body` is the `fonts` map's `body` key — the category is carried by the field that references it, not a name prefix. Default values are implementation picks; the names are pinned.
- **Token references on the wire.** Untagged unions on the descriptor model: `ColorValue` (token name string | literal `[f32; 4]`) replacing the raw arrays on `TextWidget.color`, `PanelWidget.fill`, `ContainerWidget.fill`, and `Border.tint`; `SpacingValue` (token name string | literal `f32`) on `gap`/`padding` of `ContainerWidget` and `GridWidget`. New optional `font` field on `TextWidget` (font token name; absent = the `body` token). Every existing literal-only descriptor stays wire-valid and round-trips byte-identically.
- **Resolution at tree build.** `UiTree::from_descriptor` gains a `&UiTheme` parameter; `build_node` resolves every token to its concrete value into `NodeContext` (colors → `[f32; 4]`, spacing → `f32` taffy style values, font → family name). Unknown tokens degrade visibly, never panic: unknown color → opaque magenta `[1, 0, 1, 1]`; unknown font → the `body` family; unknown spacing → `0.0`. Each logs one `log::warn!` per tree build (build-time resolution makes this naturally per-build, not per-frame — dev-guide §6.1).
- **Theme plumbing + retained-tree invalidation.** The `Renderer` owns the active `UiTheme` (default at construction) plus a monotonically bumped theme generation; an engine-side setter installs an override and bumps the generation. `RetainedGameplayTree` records the generation it was built with; the rebuild gate in `UiPass::layout_gameplay_tree` becomes `descriptor != tree || theme_generation changed`. Callers of `from_descriptor`/`layout_tree` (splash record path, gameplay path, tests) thread the renderer's theme; the splash resolves against the same active theme under one rule (its literals resolve to themselves, so output is unchanged).
- **Multi-font supply.** A second committed, redistribution-compatible (OFL or permissive) monospace TTF beside Inter, embedded the same way (`include_bytes!`, license file alongside). `UiTextRenderer`/`build_font_system` register both faces; `UiText` gains a resolved `family` field; `shape_text` selects the per-line family; `measure_run` gains a family parameter so the measure seam shapes with the node's face; `NodeContext::Text` carries the resolved family. The single-family constants generalize into the font category of the default theme.
- **Demo HUD on tokens (the real consumer).** `demo::build_demo_descriptor` switches its text color literal to a color token and the swatch label to the `mono` font token, so token resolution and the second face are exercised on a live screen, not only in fixtures.
- **CPU test gate extensions.** A/B/C's pure-CPU harness covers: per-variant union round-trips, token→value resolution in the draw list, all three unknown-token fallbacks, mono-vs-body measured-width divergence, override merge semantics, and the theme-generation rebuild.

### Out of scope

- **Script-facing theme/font registration** (`registerTheme`, mod TTF loading, `sdk/lib/ui/theme.*`) — G1. D ships the Rust model, wire format, and engine-side setter only.
- **Panel-sprite tokens** (textured 9-slice asset tokens) — deferred until a textured-panel consumer exists (BIS); lands additively as a fourth category map, no wire break.
- **`styleRanges` / value→token mapping** — E (resolves against this table).
- **Runtime theme hot reload / file-watching** — the engine-side setter + generation gate is the full D surface; dev-mode reload rides script hot reload at G1.
- **`fontSize` / envelope `offset` as token fields** — stay literal; widening either to a union later is additive (literals remain valid).
- **Per-widget theme switching** — one active theme; multiple simultaneous themes have no consumer.

## Acceptance criteria

- [ ] A `text` widget with `"color": "critical"` and one with `"color": [1.0, 0.0, 0.0, 1.0]` both deserialize; each re-serializes byte-identically to its input form. The existing B/C fixtures (all-kinds tree, splash, demo) deserialize unchanged and round-trip byte-identically — no wire break.
- [ ] A token color resolves to the active theme's RGBA in the produced draw list; an unknown color token draws opaque magenta and logs exactly one warning per tree build.
- [ ] A spacing token on `gap`/`padding` resolves into layout: child rects sit at the theme-defined spacing (CPU layout assertion), and an unknown spacing token lays out as `0.0` with one warning.
- [ ] A `text` node with `"font": "mono"` measures and shapes with the monospace face — its measured width differs from the same content shaped with `body` — and an unknown font token falls back to `body` with one warning.
- [ ] A `ThemeDescriptor` round-trips through serde JSON, and an override containing only some token names resolves those to the override values and every unnamed token to the engine default (merge is per-token, not per-category).
- [ ] Installing a theme override while a retained gameplay tree is alive rebuilds the tree on the next frame: the new token values appear without any descriptor change (theme-generation gate).
- [ ] The demo HUD renders with a token-resolved text color and a mono-face swatch label; verification reuses A/B/C's approach — pure-CPU draw-list assertions plus a manual run per the project build/run commands.
- [ ] The second TTF is committed under `content/base/fonts/` with its license file, embedded at compile time, and registered so its family resolves (mirrors the Inter build-tests).
- [ ] No QuickJS/Luau bridge code is added; the theme is reachable only through Rust (engine default, engine-side setter, tests).

## Tasks

### Task 1: Theme model + wire format
New `render/ui/theme.rs`: `UiTheme` with the three category maps, the engine default theme carrying the required token names, the serde `ThemeDescriptor` (camelCase wire: `colors` / `fonts` / `spacing`, each an optional map defaulting empty), and the per-token override merge. Token lookup helpers return `Option` so resolution sites own the fallback-and-warn. Round-trip + merge tests. Pure data — no rendering, no taffy. Declare the module in `render/ui/mod.rs`.

### Task 2: Descriptor token unions
In `render/ui/descriptor.rs`: `ColorValue` and `SpacingValue` untagged serde enums (token `String` | literal), swapped into `TextWidget.color`, `PanelWidget.fill`, `ContainerWidget.fill`, `Border.tint`, and the container/grid `gap`/`padding`; optional `font: Option<String>` on `TextWidget` (`skip_serializing_if`, so fontless text keeps its old wire form). Untagged enums try variants in declaration order — declare the literal variant first so existing arrays/numbers never mis-parse as tokens. Round-trip tests per variant plus the no-wire-break fixture assertions. Pure data; resolution lands in Task 4.

### Task 3: Multi-font supply
In `render/ui/text.rs`: commit + embed the second (mono) face alongside Inter; register both in `build_font_system`/`UiTextRenderer::new`; add `family: String` to `UiText` (callers updated: `tree::collect_node`, splash/demo paths via Task 4, tests); `shape_text` selects `Family::Name(&t.family)` per line; `measure_run` gains a `family: &str` parameter (callers: `tree::measure_node`, text tests). The existing single-family constants become the default theme's `body`/`mono` entries (Task 1 references the family-name constants this module exports). Build-tests mirror the Inter magic/family assertions for the new face.

### Task 4: Resolution threading + theme generation
Thread the theme through the build path: `UiTree::from_descriptor(&AnchoredTree, &UiTheme)` resolves every `ColorValue`/`SpacingValue`/`font` token in `build_node` (spacing into the taffy `Style`, colors/family into `NodeContext`), with the three fallback-and-warn behaviors. `Renderer` owns `ui_theme` + generation and the engine-side setter; `RetainedGameplayTree` records its build generation; `UiPass::layout_tree` / `layout_gameplay_tree` take the theme (callers: `record_splash_ui` and the gameplay record block in `render/mod.rs`, plus the gate tests), and the gameplay rebuild gate ORs in the generation change. Depends on Tasks 1–3.

### Task 5: Demo-on-tokens + CPU gate
Switch `demo::build_demo_descriptor`'s text color to a color token and the swatch label's font to `mono`. Extend the CPU harness (`splash_layout_test` / `demo_ui_gate_test` patterns) to cover the AC set: resolution, fallbacks, mono measurement, merge, and the theme-change rebuild. Splash output stays byte-identical (literal colors resolve to themselves).

## Sequencing

**Phase 1 (concurrent):** Task 1 (theme model — new file) and Task 2 (descriptor unions) — independent files, no shared types yet.
**Phase 2 (sequential):** Task 3 — multi-font mechanics in `text.rs`; consumes Task 1's family-name constants only.
**Phase 3 (sequential):** Task 4 — resolution threading; consumes Tasks 1, 2, and 3.
**Phase 4 (sequential):** Task 5 — demo + gate; consumes Task 4.

## Concurrency note (D ‖ TW orchestrate wave)

D and `M13--ui-value-tweening` are planned as one concurrent wave. Their unavoidable shared files: `render/ui/descriptor.rs` (D edits widget field types; TW edits the bind structs), `render/ui/tree.rs` (D adds resolved theme values to `NodeContext` and `build_node`; TW adds tween state to `NodeContext` and rewrites `resolve_bindings`), and `render/ui/mod.rs` (D widens `layout_tree`/`layout_gameplay_tree` with the theme; TW widens them with time). All conflicts are additive — new fields, new parameters — not semantic. Run the two in isolated worktrees and merge-coordinate those three files at integration; whichever lands second rebases its signature changes mechanically.

## Boundary inventory

The theme wire format and the token-bearing descriptor fields cross Rust ↔ wire (JSON); JS/Luau ingestion lands at G1 with the casing locked here. Rust snake_case; wire camelCase.

| Name | Rust | Wire / serde | JS / TS (G1) | Luau (G1) |
|---|---|---|---|---|
| color field | `ColorValue` (`Literal([f32; 4])` \| `Token(String)`) | array `[r,g,b,a]` OR token string, untagged | same | same |
| spacing field | `SpacingValue` (`Literal(f32)` \| `Token(String)`) | number OR token string, untagged | same | same |
| text font | `font: Option<String>` | `font` (token string; omitted when absent) | `font` | `font` |
| theme doc | `ThemeDescriptor` | `{ "colors": {…}, "fonts": {…}, "spacing": {…} }` | same | same |
| theme color entry | `[f32; 4]` linear RGBA | `[r,g,b,a]` array | same | same |
| theme font entry | `String` (registered family name) | family string | same | same |
| theme spacing entry | `f32` logical px | number | same | same |
| required tokens | — | colors: `critical`/`warning`/`ok`/`panel.default` · fonts: `body`/`mono` · spacing: `xs`/`s`/`m`/`l` | same | same |

## Open questions

- **Mono face pick.** Exact monospace TTF (OFL or permissive) is an implementation pick, mirroring A's Inter decision. Constraint: redistribution-compatible license committed alongside.
- **Default palette values.** The required token *names* are pinned; their default RGBA/px values are the implementer's call (cyberpunk-consistent with the existing splash/demo constants).
- **`Border.texture` under tokens.** The 9-slice `texture` key stays a literal asset string — sprite tokens are the deferred fourth category. Confirm at review that no consumer expects otherwise (the tree path currently draws borders via margin + fill only).
