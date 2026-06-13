# UI layouts → script/JSON authoring (draft placeholder)

> **Status:** Draft placeholder. NOT a spec — awaits a `/draft-plan` pass to expand
> into tasks + acceptance criteria. Captures the seam and anchors only.

## Goal

Migrate the UI layouts currently hardcoded as Rust descriptor builders — the demo
HUD, the pause menu, and the boot splash — to script/JSON-authored `AnchoredTree`
descriptors. The HUD and pause menu register by name into the modal-stack registry,
matching how `content/base/ui/keyboard.json` is already authored. The boot splash
migrates its authoring source only: it stays on its own pre-gameplay path OUTSIDE
the modal stack and keeps the `SplashDescriptor` newtype (which wraps an
`AnchoredTree`) and its call sites stable. Engine code keeps layout COMPUTATION
(taffy in `render/ui/tree.rs`) and the descriptor wire model; it stops carrying
layout AUTHORING.

## Why

- Keep engine code free of content/layout authoring. Which screen holds which
  buttons/sliders/text is content, not engine logic.
- Modder-facing UI authoring: a mod author edits a JSON tree (or, later, an SDK
  factory tree) and reloads — no Rust change, no recompile.
- The precedent already works: the on-screen keyboard ships as
  `content/base/ui/keyboard.json`, loaded from disk and registered by name. Same
  wire format, same registry, same modal stack — the path the HUD and pause menu
  follow. The splash reuses the wire model and serde loader but keeps its own
  pre-gameplay call sites rather than the registry-by-name path.

## Concrete anchors

Hardcoded Rust descriptor builders to migrate:

- `crates/postretro/src/render/ui/demo.rs` — `build_demo_descriptor` (gameplay HUD)
  and `build_pause_menu_descriptor` (centered capturing pause modal). Both assemble
  `Widget`/`AnchoredTree` structs by hand.
- `crates/postretro/src/render/ui/splash.rs` — `build_splash_descriptor` /
  `SplashDescriptor`. Already self-describes as the seam G1 replaces: "script
  ingestion (G1) will replace the body while keeping the `SplashDescriptor` shape
  and call sites stable."

Existing seam that already supports asset-authored trees (the migration target shape):

- `crates/postretro/src/render/ui/keyboard_asset.rs` — `load_keyboard_descriptor`
  reads `content/base/ui/keyboard.json`, deserializes to `AnchoredTree` via the
  standard serde wire path, degrades gracefully on missing/malformed file. The
  working loader precedent.
- `crates/postretro/src/render/ui/descriptor.rs` — the serde `Widget` /
  `AnchoredTree` wire model (camelCase, internally-tagged on `kind`). Locked wire
  format; both Rust-built and JSON-authored trees flow through it identically.
- `crates/postretro/src/render/ui/modal_stack.rs` — `UiTreeRegistry` (`name →
  AnchoredTree`) + `ModalStack`. Engine built-ins register by name at boot;
  `push_named` resolves a `PushTree` by name. The named-tree registry both
  hardcoded and asset-authored trees register into.

## Dependency

Blocked on the SDK UI-authoring surface: **G1 — SDK core + lifecycle**
(`context/plans/roadmap.md`). G1 owns script-side tree registration and factory
functions (register → VM-drop lifecycle) — the "script registration arrives with
the UI SDK" the modal-stack registry and `ui.md` both note as deferred. JSON-only migration (keyboard.json style) can land ahead of full G1
script ingestion, since the loader + wire format + registry already exist; SDK
factory-authored trees need G1 proper.

## Non-goals (for the eventual spec to confirm)

- Changing layout COMPUTATION — taffy stays in `render/ui/tree.rs`, engine-owned.
- Changing the descriptor wire format — migration reuses it as-is.
