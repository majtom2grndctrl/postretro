# M13 G1-infra — UI registration + render-path spine

> Wave plan 2 of 3 (concurrent with **G1a**; both precede **G1b**; all ship in one /orchestrate).
> Prereqs: A–F/TW shipped (`done/M13--*`). Grounding: `lib/ui.md` §1/§2/§3, `render/ui/modal_stack.rs`, `render/ui/theme.rs`, `render/ui/tree.rs`, `main.rs` (frame loop), `done/M13--ui-value-tweening/`.

## Goal

Build the Rust-side UI runtime spine that script-facing registration (G1b) binds to but that does not exist yet. Three gaps surfaced in review: (1) there is one flat `UiTreeRegistry` (`modal_stack.rs`), boot-populated, silent-insert, never cleared — no engine-global vs per-level split and no scope tiers, so the override-precedence contract can't be expressed; (2) no owned override-merged `UiTheme` persists across frames — `UiTheme::engine_default()` is rebuilt per call site and `with_override` has no production caller; (3) the gameplay retained-tree render path (`layout_gameplay_tree`, which owns `NodeContext`, the recompute counter, and would own a `localState` cell) runs only from `demo.rs`/tests, not the production frame loop. This plan closes all three: a tiered tree registry with clear-on-unload, an owned merged theme with a `theme_generation` bump, and the gameplay tree wired into `main.rs`.

Pure engine work — no SDK surface, no script ingestion (→ G1b), no new widget kinds. It makes the existing retained-tree/theme/registry machinery production-resident so G1b's registration and `ui.localState()` render end-to-end.

## Scope

### In scope

- **Tiered named-tree registry.** Extend the tree registry so each entry carries a scope tier (engine / mod / level) and name resolution returns the highest-precedence entry (engine < mod < level), warning at registration when a lower tier is shadowed. Add a per-level tier cleared on level unload; engine and mod tiers survive. The mutating handle stays private; resolution stays a `&self` accessor (`ModalStack::tree`). Owner and storage assigned explicitly (a tier field per entry, or separate maps, on `ModalStack`/`App`).
- **Owned override-merged theme.** A single `UiTheme` owned across the process (on `App`/renderer), initialized to `engine_default()`, replaced via `with_override` at registration time, with a monotonically bumped `theme_generation` feeding the retained-tree reuse gate. The install seam (`Renderer::set_ui_theme`, currently dead-code) becomes live. Per-token override-merge and `tree.rs`'s unknown-token degradation (magenta/`body`/zero, warn-once) are reused unchanged.
- **Gameplay render-path wiring.** Call `layout_gameplay_tree`/`layout_tree` from the production frame loop in `main.rs`, threading the owned theme + `theme_generation`, so the retained gameplay tree (with `NodeContext`, the recompute/dirty gate, and per-node cells) runs every frame. The per-frame snapshot composes the resolved gameplay/HUD tree plus the pushed modal-stack entries through the existing single-composition encode (`ui.md` §5).
- **Render-time consumption seam for registered trees.** The snapshot's compose step resolves trees through the tiered registry by name (HUD plus any auto-composed/pushed names), so a G1b-registered tree of a known role renders without a bespoke special-case per name.

### Out of scope

- Script-facing registration (manifest UI fields, `setupMod`/`setupLevel` drains, named-tree/theme/font *script* registration), `ui.localState()` the SDK primitive. → **G1b**.
- Factories, the bridge, typed handles, the text alias. → **G1a**.
- New widget kinds, screen-space effects, built-in screen authoring, egui retirement. → **SE / BIS**.
- The modal-stack capture/focus semantics themselves (F shipped these) — this plan only resolves and composes trees, it does not change input capture.

## Acceptance criteria

- [ ] The tree registry resolves a name to the highest-precedence entry across tiers: a level-tier tree shadows a mod-tier tree of the same name, which shadows an engine built-in, and registering a shadowing entry emits a one-line warning naming the shadowed tier. Resolution is a `&self` accessor; the mutating handle is not publicly exposed.
- [ ] After a level unload the level tier is empty (a level-registered name no longer resolves) while engine and mod tiers still resolve — verified the same way `DataRegistry`'s per-level clear is (the level clear runs at the same unload site).
- [ ] A `with_override` applied to the owned theme changes a token's resolved value in a rendered widget and bumps `theme_generation`; an unknown token still degrades via `tree.rs` (magenta / `body` / zero, warn-once) without panic. The owned theme persists across frames (not rebuilt per call).
- [ ] `layout_gameplay_tree` runs from the production frame loop: a gameplay tree renders on a normal launch (not only in `demo.rs`/tests). On a settled frame the retained-tree recompute counter does not increment; an appearance-only bound change redraws without relayout; a content change relayouts (counter increments) — the `done/M13--ui-value-tweening` invalidation contract, now exercised in production.
- [ ] The per-frame snapshot is a single composition (one `prepare`/vertex-buffer fill) covering the resolved gameplay tree plus pushed modal entries — the `ui.md` §5 per-layer-clobber guard still holds (no second `prepare` per composition).
- [ ] A tree resolved through the tiered registry and composed into the snapshot renders at its anchored placement; removing it from its tier removes it from the next frame's composition.

## Tasks

### Task 1: Tiered tree registry + clear-on-unload
Add a scope-tier dimension to the registry (`modal_stack.rs` `UiTreeRegistry`): per-entry tier, highest-precedence resolution, registration-time shadow warning, and a per-level tier wiped at the level-unload site (beside the `DataRegistry` clear in `main.rs`). Keep `ModalStack::tree` the `&self` read seam. `Depends on` nothing.

### Task 2: Owned override-merged theme + generation
Give `App`/renderer a persistent `UiTheme` (init `engine_default()`), apply `with_override` to it via the now-live `Renderer::set_ui_theme`, and bump `theme_generation` on change so the retained-tree reuse gate sees it. Reuse `tree.rs` degradation unchanged. `Depends on` nothing.

### Task 3: Wire the gameplay tree into the frame loop
Call `layout_gameplay_tree`/`layout_tree` from the `main.rs` frame loop, threading the owned theme + `theme_generation`, and compose the resolved tree + pushed modal entries into the single-composition snapshot. `Depends on` Tasks 1, 2 (it resolves through the registry and reads the owned theme).

### Task 4: Render-consumption seam + tests
Make the compose step resolve trees by name through the tiered registry (HUD + pushed/auto-composed names), and add the production-path tests: tier resolution/shadowing, level-clear, theme override + generation bump, settled-frame recompute counter, single-composition guard. `Depends on` Task 3.

## Sequencing

**Prereq:** A–F/TW shipped.
**Phase 1 (concurrent):** Task 1 (registry), Task 2 (theme) — independent.
**Phase 2 (sequential):** Task 3 (frame-loop wiring) consumes both.
**Phase 3 (sequential):** Task 4 (consumption seam + tests).

## Rough sketch

**Registry tiers.** The current registry is a flat `HashMap<String, AnchoredTree>` with silent insert. Tag each entry with a tier and resolve level→mod→engine; the shadow warning fires at registration (the only point with both old and new identities). The per-level tier clears at the same `main.rs` unload sweep that clears `DataRegistry` (reactions + crossings). **Owned theme.** `theme_generation: u64` already exists in the retained-tree reuse gate but nothing owns/bumps it; this plan gives it an owner. **Frame-loop wiring** is the highest-risk task — `layout_gameplay_tree` exists and is unit-tested, so this is integration, not new layout code; the risk is composition correctness (`ui.md` §5's single-`prepare` rule) and threading the theme/generation, both already proven in `demo.rs`.

**Key files:** `render/ui/modal_stack.rs` (registry + tiers), `render/ui/theme.rs` (`with_override`, `set_ui_theme`), `render/ui/tree.rs` (`layout_gameplay_tree`, `theme_generation`, degradation — consumed, not changed), `main.rs` (frame loop, unload sweep, ownership), `render/ui/mod.rs` (compose seam).

## Decisions

- **Scope tiers live on the registry, not in separate ad-hoc maps per call site.** One registry with a tier field expresses engine<mod<level precedence and the shadow warning in one place; the per-level tier is the only cleared partition. Rejected: a second standalone registry struct (duplicates resolution logic and the `&self` seam).
- **One owned `UiTheme`, generation-bumped, replaces per-call `engine_default()`.** A persistent merged theme is the only way a mod-registered token survives from registration to every frame; `theme_generation` already gates retained-tree reuse. Rejected: re-resolving the theme each frame (loses overrides; defeats the reuse gate).
- **Render-path wiring lands here, not in G1b.** Per owner direction, `localState` must render end-to-end; the production frame loop is engine work, kept out of the SDK-facing G1b. Rejected: leaving `layout_gameplay_tree` demo/test-only and scoping G1b's localState ACs to the unit path.
- **Composition stays single-`prepare`.** The snapshot composes all layers in one encode (`ui.md` §5); the per-layer-clobber guard is preserved. Rejected: per-layer encode (the historical text-clobber bug).

## Open questions

None. The three gaps were confirmed against source; ownership and clear-sites are assigned in the tasks.
