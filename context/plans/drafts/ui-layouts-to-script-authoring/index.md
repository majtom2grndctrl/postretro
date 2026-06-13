# UI layouts → script/JSON authoring

## Goal

Move the UI layouts currently hardcoded as Rust descriptor builders — the demo HUD, the pause menu, and the boot splash — off engine-side authoring and onto JSON-authored `AnchoredTree` descriptors. The HUD and pause menu register by name into the modal-stack registry, the path `content/base/ui/keyboard.json` already follows. The boot splash migrates its authoring source only (Rust builder → JSON) and stays on its own pre-gameplay path outside the modal stack, keeping the `SplashDescriptor` newtype and call sites stable. Engine keeps layout COMPUTATION (taffy in `render/ui/tree.rs`) and the descriptor wire model; it stops carrying layout AUTHORING.

## Why

- Keep engine code free of content/layout authoring. Which screen holds which buttons/sliders/text is content, not engine logic.
- Modder-facing UI authoring: a mod author edits a JSON tree (later an SDK factory tree) and reloads — no Rust change, no recompile.
- The precedent already works: the on-screen keyboard ships as `content/base/ui/keyboard.json`, loaded from disk via `load_keyboard_descriptor` and registered by name into `UiTreeRegistry`. Same wire format, same registry, same modal stack — the path the HUD and pause menu follow. The splash reuses the wire model and serde loader but keeps its own pre-gameplay call sites rather than the registry-by-name path.

## Phasing and dependency

Two phases. The split is dictated by what exists today versus what G1 (SDK core + lifecycle, `context/plans/roadmap.md`) must deliver first.

- **Phase JSON (buildable today, NOT G1-gated).** Migrate the three hardcoded builders to disk JSON loaded through the existing serde path. The loader (`load_keyboard_descriptor` pattern), the wire format (`descriptor.rs`), and the registry (`UiTreeRegistry`/`ModalStack::push_named`) already exist and ship the keyboard this way. No script VM, no SDK. Land this ahead of G1.
- **Phase SDK (gated on G1).** Author the same trees as SDK factory functions (script-side registration, register → VM-drop lifecycle) instead of static JSON. G1 owns script-side tree registration and factory functions; do NOT start any Phase SDK task before G1 ships.

Gating rule for executors: every Phase JSON task is buildable now. No Phase JSON task may depend on a Phase SDK deliverable, the SDK surface, or a script VM. Any task that touches script-side registration or SDK factories is Phase SDK and is blocked on G1.

## Scope

### In scope

- **Phase JSON.**
  - Author `content/base/ui/hud.json` and `content/base/ui/pauseMenu.json` as `AnchoredTree` JSON that deserializes (via `serde_json::from_str::<AnchoredTree>`) to a value `PartialEq` to what `build_demo_descriptor` / `build_pause_menu_descriptor` produce today — descriptor equality, not byte equality.
  - A reusable disk-load + registry-register path generalizing the keyboard's boot wiring (load JSON → `Option<AnchoredTree>` → `registry.register(name, tree)`), so the HUD and pause menu register by name like the keyboard. Graceful degradation on missing/malformed file, matching `load_keyboard_descriptor`.
  - Author `content/base/ui/splash.json` and migrate `build_splash_descriptor` to load + deserialize it into the existing `SplashDescriptor` newtype, keeping the newtype and all call sites stable. Splash stays OUTSIDE the registry/modal stack.
  - Delete the migrated Rust builder bodies once the JSON path is the source of truth (keep the function/newtype seams the splash and call sites need; replace builder bodies, do not leave dead hardcoded trees).
  - Round-trip fixtures proving the authored JSON deserializes to the same `AnchoredTree` the old builder produced (descriptor-level equality, the seam the existing `demo.rs`/`splash.rs` tests already assert against).

### Out of scope

- **Phase SDK is specced here only as the gated follow-on** — its tasks are named and sequenced, but their detailed AC land when G1's SDK surface is concrete. This plan does not define the SDK factory API.
- Changing layout COMPUTATION — taffy stays in `render/ui/tree.rs`, engine-owned. JSON authors the tree; the engine still lays it out.
- Changing the descriptor wire format — migration reuses `descriptor.rs` as-is. No new widget kinds, no serde-shape change.
- Hot-reload / file-watching of UI JSON — load-at-boot only, matching the keyboard. A reload story is a separate follow-up (noted under Related work).
- Theme authoring migration — theme tokens stay engine-side (see `ui.md` §2); only widget trees migrate.
- Registering the splash by name — the splash keeps its pre-gameplay call sites and never enters `UiTreeRegistry`/`ModalStack`.

## Acceptance criteria

- [ ] `content/base/ui/hud.json` and `content/base/ui/pauseMenu.json` exist and each deserializes via the standard serde path (`serde_json::from_str::<AnchoredTree>`) to an `AnchoredTree` descriptor-equal to what the corresponding Rust builder produced before migration. A fixture test asserts the equality at the descriptor level. *(Task 1)*
- [ ] The HUD and pause-menu descriptors load from disk and register by name into `UiTreeRegistry` at boot through one shared load-and-register path; a missing or malformed file warns once and degrades (that screen is unavailable, the engine still boots) exactly as the keyboard does today. *(Task 2)*
- [ ] The pause menu remains pushable/poppable by its registered name through `ModalStack::push_named` (the `nav.menu` toggle path), and the HUD still composes as the bottom passthrough layer — both behaviors unchanged from the hardcoded version, now sourced from JSON. *(Task 2)*
- [ ] The per-frame snapshot resolves the HUD by name through the new public registry accessor — no `build_demo_descriptor()` call remains on the render path — and the HUD layer's capture mode is read from the resolved `AnchoredTree` envelope (converted to `input::UiCaptureMode`), not a hardcoded `Passthrough` literal. *(Task 2)*
- [ ] `content/base/ui/splash.json` exists; `build_splash_descriptor` loads and deserializes it into a `SplashDescriptor`, the newtype and every call site (the per-frame `record_splash_ui` site and the tests) compile and behave unchanged; the splash never registers into `UiTreeRegistry`/`ModalStack`. A missing/malformed splash JSON degrades to a stated fallback rather than panicking on the boot path. *(Task 3)*
- [ ] After migration, no `Widget`/`AnchoredTree` tree is assembled by hand in engine Rust for the HUD, pause menu, or splash — the builder bodies are replaced by the load path. The keyboard's existing JSON path is unchanged. *(Task 2, Task 3)*
- [ ] The descriptor wire format (`descriptor.rs`) and taffy layout (`tree.rs`) are unchanged by this work — no new widget kinds, no serde-shape change, no layout-computation move. *(all tasks)*
- [ ] **(Phase SDK, gated on G1)** The HUD and pause menu can be authored and registered as SDK factory functions through G1's register → VM-drop lifecycle, producing the same `AnchoredTree` the JSON path produces; the splash's `SplashDescriptor` body is sourced from the SDK while its newtype + call sites stay stable. *(Task 4 — blocked until G1)*

## Tasks

### Task 1: Author HUD + pause-menu + splash JSON fixtures

Produce `content/base/ui/hud.json`, `content/base/ui/pauseMenu.json`, and `content/base/ui/splash.json` as `AnchoredTree` JSON matching the current builders' output. Note the splash tree is not text-only: `build_splash_descriptor` emits a logo `image` node (`asset: "splash/logo"`, the `SPLASH_LOGO_ASSET` const, referenced by string id in the wire model) alongside the version `text` node, so `splash.json` must reproduce the logo image node and its asset string. Author against the locked wire model in `descriptor.rs`: `Widget` is internally tagged on `kind` (camelCase variants, except `vstack`/`hstack` pinned lowercase — see Boundary inventory), `AnchoredTree` fields are camelCase (`captureMode`, `initialFocus`, `textEntryTarget`, skip-serialized when default/absent). Each JSON file must deserialize to the descriptor the matching `build_*` function returns today. Add a fixture test per file that round-trips JSON → `AnchoredTree` and asserts descriptor equality against the current builder (the builders stay available as the equality oracle until Task 2/3 replace their bodies); for the splash, the oracle is `build_splash_descriptor("{version}")`, matching the `"{version}"` sentinel the JSON carries. This is the authoring deliverable; wiring is Task 2/3.

### Task 2: Load-and-register path for HUD + pause menu

Generalize the keyboard's boot wiring into one reusable load-and-register helper: read a named JSON file, `serde_json::from_str::<AnchoredTree>`, on `Ok` `registry.register(name, tree)`, on `Err`/missing warn once and skip — the exact degradation `load_keyboard_descriptor` already implements. Wire the HUD and pause menu through it at boot (the same place the keyboard registers via `registry_mut().register(...)`): the pause menu under `"pauseMenu"` (the name the `nav.menu` toggle already pushes via `push_named`), the HUD under a new HUD name constant. Replace `build_demo_descriptor` / `build_pause_menu_descriptor` bodies with the load path once the JSON is the source of truth — do not leave the hardcoded trees behind as dead code. The helper needs the registry handle (`ModalStack::registry_mut()`) and the content path; both are reachable at the boot site that already registers the keyboard.

Decouple the engine frame loop from the demo — this is the shortcoming the migration fixes. Today the per-frame snapshot composes the HUD by calling `build_demo_descriptor()` directly (`main.rs` ~2203–2208), and the boot-time `register("hud", …)` is dead because the snapshot never reads it. Make the registry the single seam: register the HUD by name like every other tree, and have the snapshot site resolve it by name through a new public read accessor — make the currently-private `UiTreeRegistry::resolve` public as `get(&self, name: &str) -> Option<&AnchoredTree>` (a borrow), and the snapshot site clones the borrowed tree into its owned `UiTreeEntry` as it does today — prepending the resolved tree as the bottom layer. The HUD entry's capture mode comes from the loaded `AnchoredTree` envelope (per `ui.md` §1 the declared envelope is the source of truth), replacing the hardcoded `Passthrough` literal: the envelope's `descriptor::CaptureMode` converts to the entry's `input::UiCaptureMode` via the existing `.into()` mapping that `ModalStack::entries()` / `top_capture_mode()` already use. Update the stale snapshot-site comment (which still narrates "the demo HUD ... calling `build_demo_descriptor()` directly") to describe the registry-resolved HUD layer. This removes the demo-builder call from the render path and gives Phase SDK one registry seam to register into rather than a bespoke HUD path. (A missing `hud.json` resolves to `None` — the HUD is absent that frame, the engine still boots; no in-code HUD fallback is needed, unlike the splash.)

### Task 3: Migrate the splash authoring source to JSON

Rewrite `build_splash_descriptor`'s body to load + deserialize `content/base/ui/splash.json` (authored in Task 1) into the existing `SplashDescriptor` newtype. The newtype, the function signature (`build_splash_descriptor(version_line: &str) -> SplashDescriptor`), and every call site stay stable — the per-frame `record_splash_ui` site and the splash tests must compile and behave unchanged. The splash stays OUTSIDE the registry and modal stack; it does NOT register by name. Version-line injection is a **templated placeholder**: `splash.json` authors the version `text` node with a sentinel content string (`"{version}"`), and the builder substitutes `version_line` into that node's content at load. This keeps the whole tree (logo `image` + version `text`) in JSON with no wire-format change — a patched-node-id would need an `id` field the wire model does not carry (out of scope per AC#6), and composing the version text in Rust would leave a hand-built node behind (the coupling this migration removes). The Task 1 round-trip fixture deserializes `splash.json` (sentinel intact) and asserts descriptor-equality against `build_splash_descriptor("{version}")` — the `{version}` sentinel on both sides — since live substitution is runtime-only (`build_splash_descriptor` sets the version node content to exactly its argument). On missing/malformed `splash.json`, degrade to a minimal in-code fallback splash tree and warn once — matching `load_keyboard_descriptor`'s graceful degradation — so the boot path never panics; do not treat a missing engine-shipped asset as a fatal boot error.

### Task 4: SDK factory authoring (Phase SDK — gated on G1)

**Blocked on G1.** Author the HUD and pause menu as SDK factory functions registered through G1's register → VM-drop lifecycle, producing the same `AnchoredTree` the JSON path produces; source the splash's `SplashDescriptor` body from the SDK while keeping the newtype + call sites stable. Detailed AC for this task land when G1's SDK surface is concrete — this task is named and sequenced now so no executor starts G1-blocked work as if it were buildable, and so the JSON phase is structured to converge with it (same wire model, same registry-by-name path for HUD/pause-menu, same stable splash seam).

## Sequencing

**Phase 1 (sequential):** Task 1 — authors the JSON the load paths consume; blocks Task 2 and Task 3.
**Phase 2 (concurrent):** Task 2, Task 3 — independent (HUD/pause-menu registry path vs. splash newtype path; different call sites, no shared contract beyond the wire model). Both consume Task 1's JSON.
**Phase 3 (gated, post-G1):** Task 4 — Phase SDK. Does not start until G1 ships; consumes the Phase JSON wire model and registry path as its convergence target.

## Rough sketch

- **Task 1.** JSON authored by hand (or generated once from the builder via `serde_json::to_string_pretty` of the built `AnchoredTree`, then committed and the generator discarded). Fixture tests live next to the existing `demo.rs`/`splash.rs` descriptor tests; they `serde_json::from_str::<AnchoredTree>` the committed file and `assert_eq!` against `build_*_descriptor()` (the wire model derives `PartialEq`, so descriptor equality is a direct compare).
- **Task 2.** One helper near the keyboard's boot wiring (`main.rs`): `load_named_tree(path) -> Option<AnchoredTree>` mirroring `load_keyboard_descriptor`, then `registry.register(name, tree)`. The keyboard's own wiring can fold into this helper or stand beside it. The pause-menu name is the existing `"pauseMenu"` constant; the HUD gets a name constant. `ModalStack::push_named` and the `nav.menu` toggle are unchanged — they already resolve by name. The HUD's snapshot site (`main.rs` ~2203) swaps its direct `build_demo_descriptor()` call for a `registry.get("hud")` borrow (the now-public accessor), cloning the tree into the `UiTreeEntry` and converting the envelope's `descriptor::CaptureMode` to the entry's `input::UiCaptureMode` via `.into()`.
- **Task 3.** `build_splash_descriptor` body becomes load + deserialize + `"{version}"` substitution, returning `SplashDescriptor { tree }`. Keep the minimal in-code fallback so the boot path is panic-free. Splash call sites (`record_splash_ui`, tests) are untouched.
- **Task 4.** Deferred; converges the Phase JSON wire model onto G1's factory API.

## Boundary inventory

This plan crosses a content/asset authoring boundary (JSON wire → serde → `AnchoredTree`) and, in Phase SDK, a Rust ↔ script boundary. Casing is pinned by `descriptor.rs`'s serde attributes (`rename_all = "camelCase"`, internally tagged on `kind`).

| Name | Rust | Wire / serde (JSON) | Luau / JS (Phase SDK) | FGD KVP |
|---|---|---|---|---|
| Widget discriminator | `Widget` enum (tagged on `kind`) | `"kind"` field | `kind` (SDK factory maps to it) | n/a |
| Text widget | `Widget::Text` | `"text"` | `"text"` | n/a |
| Panel widget | `Widget::Panel` | `"panel"` | `"panel"` | n/a |
| Image widget | `Widget::Image` | `"image"` | `"image"` | n/a |
| Vertical stack | `Widget::VStack` | `"vstack"` (pinned lowercase, not `"vStack"`) | `"vstack"` | n/a |
| Horizontal stack | `Widget::HStack` | `"hstack"` (pinned lowercase) | `"hstack"` | n/a |
| Grid / Spacer / Button / Slider / Bar | `Widget::{Grid,Spacer,Button,Slider,Bar}` | `"grid"`/`"spacer"`/`"button"`/`"slider"`/`"bar"` | same | n/a |
| Tree envelope | `AnchoredTree` | object with `anchor`, `offset`, `root`, `captureMode`, `initialFocus`, `textEntryTarget` | same | n/a |
| Capture mode | `CaptureMode::{Capture,Passthrough}` | `"capture"` / `"passthrough"` (camelCase → lowercase) | same | n/a |
| Registry name (pause menu) | `PAUSE_MENU_NAME` (`"pauseMenu"`) | n/a (registry key, not wire) | `pushTree("pauseMenu")` | n/a |
| Registry name (HUD) | new HUD name constant | n/a | n/a | n/a |
| Keyboard tree name | `KEYBOARD_TREE_NAME` (`"keyboard"`) | n/a | `showDialog { tree: "keyboard" }` | n/a |

The registry names are app-side keys (`UiTreeRegistry`'s `HashMap<String, AnchoredTree>`), not serialized into the tree JSON — they are the boot-registration identity, fixed in Rust constants and referenced by `PushTree { tree, .. }` / `push_named`. The splash carries no registry name (it never registers).

## Related work

- `ui-render-path-robustness-text-shaping` (this plan's sibling): its Task C cache threshold cites G1's script-authored UI as a label-count multiplier — script-defined screens author more text nodes than the hardcoded builders, which is the convergence Phase SDK delivers.
- G1 — SDK core + lifecycle (`roadmap.md`): owns the script-side registration and factory API Phase SDK builds on. Phase JSON is explicitly buildable ahead of it. Only Phase SDK (Task 4) is the G1 convergence; where the sibling render-path spec's "(G1)" shorthand labels this whole migration, it means the Phase SDK end state, not the pre-G1 Phase JSON work.
- UI JSON hot-reload / file-watching: a reload-on-edit story for the migrated trees — separate follow-up, not specced here (load-at-boot only).

## Open questions

- **HUD JSON envelope.** Resolved: register by name. The HUD registers into `UiTreeRegistry` like every other tree, and the snapshot site resolves it by name through a new public registry accessor — replacing the direct `build_demo_descriptor()` call at `main.rs` ~2203 — with capture mode read from the loaded envelope. This decouples the render path from the demo builder and gives Phase SDK a single registry seam, rather than a bespoke direct-load path at the snapshot site.
- **Splash version-line injection.** Resolved: templated placeholder. `splash.json` carries a `"{version}"` sentinel in the version `text` node; the builder substitutes `version_line` at load. Chosen over patched-node-id (would add an `id` field the wire model lacks, out of scope per AC#6) and builder-composes (would leave a hand-built node in Rust).
- **Splash fallback shape.** Resolved: minimal in-code fallback splash tree plus a one-time warn, matching `load_keyboard_descriptor`'s graceful degradation — the boot path never panics. A missing engine-shipped splash asset degrades rather than aborting boot.
- **Generated vs. hand-authored JSON.** Resolved: generate once from the builders (`to_string_pretty`) to seed the committed files, then treat the JSON as the source of truth (hand-authored thereafter). Generation guarantees descriptor-equality on day one; the committed JSON is the modder-facing artifact from then on.
