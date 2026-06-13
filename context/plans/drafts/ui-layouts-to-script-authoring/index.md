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
- [ ] `content/base/ui/splash.json` exists; `build_splash_descriptor` loads and deserializes it into a `SplashDescriptor`, the newtype and every call site (the per-frame `record_splash_ui` site and the tests) compile and behave unchanged; the splash never registers into `UiTreeRegistry`/`ModalStack`. A missing/malformed splash JSON degrades to a stated fallback rather than panicking on the boot path. *(Task 3)*
- [ ] After migration, no `Widget`/`AnchoredTree` tree is assembled by hand in engine Rust for the HUD, pause menu, or splash — the builder bodies are replaced by the load path. The keyboard's existing JSON path is unchanged. *(Task 2, Task 3)*
- [ ] The descriptor wire format (`descriptor.rs`) and taffy layout (`tree.rs`) are unchanged by this work — no new widget kinds, no serde-shape change, no layout-computation move. *(all tasks)*
- [ ] **(Phase SDK, gated on G1)** The HUD and pause menu can be authored and registered as SDK factory functions through G1's register → VM-drop lifecycle, producing the same `AnchoredTree` the JSON path produces; the splash's `SplashDescriptor` body is sourced from the SDK while its newtype + call sites stay stable. *(Task 4 — blocked until G1)*

## Tasks

### Task 1: Author HUD + pause-menu + splash JSON fixtures

Produce `content/base/ui/hud.json`, `content/base/ui/pauseMenu.json`, and `content/base/ui/splash.json` as `AnchoredTree` JSON matching the current builders' output. Note the splash tree is not text-only: `build_splash_descriptor` emits a logo `image` node (`asset: "splash/logo"`, the `SPLASH_LOGO_ASSET` const, referenced by string id in the wire model) alongside the version `text` node, so `splash.json` must reproduce the logo image node and its asset string. Author against the locked wire model in `descriptor.rs`: `Widget` is internally tagged on `kind` (camelCase variants, except `vstack`/`hstack` pinned lowercase — see Boundary inventory), `AnchoredTree` fields are camelCase (`captureMode`, `initialFocus`, `textEntryTarget`, skip-serialized when default/absent). Each JSON file must deserialize to the descriptor the matching `build_*` function returns today. Add a fixture test per file that round-trips JSON → `AnchoredTree` and asserts descriptor equality against the current builder (the builders stay available as the equality oracle until Task 2/3 replace their bodies). This is the authoring deliverable; wiring is Task 2/3.

### Task 2: Load-and-register path for HUD + pause menu

Generalize the keyboard's boot wiring into one reusable load-and-register helper: read a named JSON file, `serde_json::from_str::<AnchoredTree>`, on `Ok` `registry.register(name, tree)`, on `Err`/missing warn once and skip — the exact degradation `load_keyboard_descriptor` already implements. Wire the HUD and pause menu through it at boot (the same place the keyboard registers via `registry_mut().register(...)`). The HUD registers under its name and composes as the bottom passthrough layer; the pause menu registers under `"pauseMenu"` (the name the `nav.menu` toggle already pushes via `push_named`). Replace `build_demo_descriptor` / `build_pause_menu_descriptor` bodies with the load path once the JSON is the source of truth — do not leave the hardcoded trees behind as dead code. The plumbing: the helper needs the registry handle (`ModalStack::registry_mut()`) and the content path; both are reachable at the boot site that already registers the keyboard.

The HUD's current call site publishes its tree on the once-per-frame snapshot (it is the first gameplay UI producer); confirm the HUD's registration/lookup matches how the snapshot path consumes it, and route the HUD through the same name-based resolution rather than a direct builder call.

### Task 3: Migrate the splash authoring source to JSON

Rewrite `build_splash_descriptor`'s body to load + deserialize `content/base/ui/splash.json` (authored in Task 1) into the existing `SplashDescriptor` newtype. The newtype, the function signature (`build_splash_descriptor(version_line: &str) -> SplashDescriptor`), and every call site stay stable — the per-frame `record_splash_ui` site and the splash tests must compile and behave unchanged. The splash stays OUTSIDE the registry and modal stack; it does NOT register by name. The `version_line` argument is injected into the deserialized tree at load (the version `text` node's content), so the JSON carries a placeholder the builder fills — state in the spec how the version line reaches the text node (e.g. a known node id the loader patches, or the builder composes the version `text` around the JSON-authored frame). On missing/malformed `splash.json`, degrade to a stated fallback (a minimal in-code splash tree, since the splash runs before gameplay and must never panic the boot path) — name the fallback explicitly rather than leaving it to the implementer.

### Task 4: SDK factory authoring (Phase SDK — gated on G1)

**Blocked on G1.** Author the HUD and pause menu as SDK factory functions registered through G1's register → VM-drop lifecycle, producing the same `AnchoredTree` the JSON path produces; source the splash's `SplashDescriptor` body from the SDK while keeping the newtype + call sites stable. Detailed AC for this task land when G1's SDK surface is concrete — this task is named and sequenced now so no executor starts G1-blocked work as if it were buildable, and so the JSON phase is structured to converge with it (same wire model, same registry-by-name path for HUD/pause-menu, same stable splash seam).

## Sequencing

**Phase 1 (sequential):** Task 1 — authors the JSON the load paths consume; blocks Task 2 and Task 3.
**Phase 2 (concurrent):** Task 2, Task 3 — independent (HUD/pause-menu registry path vs. splash newtype path; different call sites, no shared contract beyond the wire model). Both consume Task 1's JSON.
**Phase 3 (gated, post-G1):** Task 4 — Phase SDK. Does not start until G1 ships; consumes the Phase JSON wire model and registry path as its convergence target.

## Rough sketch

- **Task 1.** JSON authored by hand (or generated once from the builder via `serde_json::to_string_pretty` of the built `AnchoredTree`, then committed and the generator discarded). Fixture tests live next to the existing `demo.rs`/`splash.rs` descriptor tests; they `serde_json::from_str::<AnchoredTree>` the committed file and `assert_eq!` against `build_*_descriptor()` (the wire model derives `PartialEq`, so descriptor equality is a direct compare).
- **Task 2.** One helper near the keyboard's boot wiring (`main.rs`): `load_named_tree(path) -> Option<AnchoredTree>` mirroring `load_keyboard_descriptor`, then `registry.register(name, tree)`. The keyboard's own wiring can fold into this helper or stand beside it. The pause-menu name is the existing `"pauseMenu"` constant; the HUD gets a name constant. `ModalStack::push_named` and the `nav.menu` toggle are unchanged — they already resolve by name.
- **Task 3.** `build_splash_descriptor` body becomes load + deserialize + version-line injection, returning `SplashDescriptor { tree }`. Keep the fallback in-code so the boot path is panic-free. Splash call sites (`record_splash_ui`, tests) are untouched.
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

- **HUD JSON envelope.** The demo HUD publishes its tree on the once-per-frame snapshot as the first gameplay UI producer, distinct from the registry-push path the pause menu and keyboard take. Confirm the HUD registers by name and is resolved through the registry for snapshot publication (the brief's "register by name" framing), versus loaded directly at the snapshot site — both reach JSON authoring, but the registry-by-name path is the stated target. Pin which before Task 2.
- **Splash version-line injection.** The splash's version `text` is runtime data (`version_line: &str`), not static JSON. Confirm the injection mechanism: a known node id the loader patches, a templated placeholder substituted at load, or the builder composing the version `text` around a JSON-authored frame. Task 3 states the constraint; the mechanism choice is open.
- **Splash fallback shape.** On missing/malformed `splash.json`, the boot path must not panic. Confirm the fallback is a minimal in-code splash tree (proposed) versus a hard boot error with a clear message — the splash is pre-gameplay, so a degraded-but-present splash is preferred, but a missing engine-shipped asset is arguably a fatal install error. Pin the policy.
- **Generated vs. hand-authored JSON.** Whether the committed JSON is hand-written or generated once from the builders (`to_string_pretty`) and committed. Generation guarantees descriptor-equality on day one; hand-authoring is the modder-facing reality. Default: generate once to seed, then treat the JSON as source of truth. Confirm.
