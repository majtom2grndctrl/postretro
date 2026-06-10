# Goal D research notes — code anchors

Grounding for the spec, confirmed against source at draft time (2026-06). Ephemeral; do not maintain after ship.

## Current state (post-C)

- **Descriptor model** — `crates/postretro/src/render/ui/descriptor.rs`. Internally-tagged `Widget` enum (`#[serde(tag = "kind", rename_all = "camelCase")]`), struct variants only. Color fields today: `TextWidget.color: [f32; 4]`, `PanelWidget.fill: [f32; 4]`, `ContainerWidget.fill: Option<[f32; 4]>`, `Border.tint: [f32; 4]`. Spacing: `gap: f32` / `padding: f32` on `ContainerWidget` and `GridWidget`. Round-trip identity is test-pinned byte-for-byte (`ALL_KINDS_JSON` etc.) — the no-wire-break AC asserts against these fixtures.
- **Text engine** — `render/ui/text.rs`. Single embedded face: `UI_FONT_TTF` (`include_bytes!` of `content/base/fonts/Inter-Regular.ttf`), `UI_FONT_FAMILY = "Inter"`, license at `content/base/fonts/Inter-OFL.txt`. `shape_text` and `measure_run` both hardcode `Family::Name(UI_FONT_FAMILY)`. `measure_run(font_system, content, font_size)` is the taffy measure seam (pure CPU). `UiText { content, position, font_size, color: [u8; 4] }` — no family field. `build_font_system()` registers the one face; cosmic-text's DB supports multiple `load_font_data` calls.
- **Tree / resolution** — `render/ui/tree.rs`. `UiTree::from_descriptor(&AnchoredTree)` → `build_node` maps widgets to taffy nodes + `NodeContext` (`Text { content, font_size, color, bind, last_resolved }`, `Panel { fill, border, bind, last_resolved }`, `Image { asset }`). Spacing goes into taffy `Style` at build time via `container_base_style(gap, padding, align)` — so spacing tokens must resolve at build, not draw. Colors are carried in `NodeContext` and consumed in `collect_node` (`linear_rgba_to_srgb_u8` for text, raw linear for quads).
- **Retained gate** — `render/ui/mod.rs`. `RetainedGameplayTree { descriptor, tree }`; rebuild gate is `retained.descriptor != *tree` in `UiPass::layout_gameplay_tree`. A theme change with an identical descriptor would keep stale resolved values — hence the theme-generation field in the spec.
- **Call sites for signature widening** — `UiPass::layout_tree` (splash: `render/mod.rs` `record_splash_ui`, ~line 3232) and `layout_gameplay_tree` (gameplay record block, `render/mod.rs` ~line 4743). Tests: `splash_layout_test.rs`, `gameplay_ui_gate_test.rs`, `demo_ui_gate_test.rs`, `multi_batch_test.rs`, plus `tree.rs` inline tests construct trees directly.
- **Demo consumer** — `render/ui/demo.rs` `build_demo_descriptor`: `HUD_TEXT_COLOR` literal, swatch label text node — the two spots Task 5 switches to tokens.
- **Border.tint** is wire-present but the tree draw path ignores it (`project_quad` uses `fill` + margin only); it participates in the union change for wire consistency, not behavior.

## Decisions captured from the owner (pre-draft)

1. Rust-side only; script registration at G1.
2. String-or-array untagged union for color fields (literal variant declared first so arrays never mis-parse).
3. Categories: colors + fonts + spacing. Panel sprites deferred (additive fourth map later).

## Untagged-union serde note

`#[serde(untagged)]` tries variants in declaration order; `Literal` first guarantees `[1.0, 0.0, 1.0, 1.0]` parses as a literal and `"critical"` as a token — no ambiguity since JSON arrays/numbers and strings are disjoint. Round-trip identity holds per variant (a token re-serializes as a string, a literal as an array).
