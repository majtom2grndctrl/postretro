# Map loading screenshots — contract

## Goal

Each dev catalog map's loading screen shows a screenshot of that map as a full-window background, posed on a landmark. Loading screens stop drawing world textures. The engine gains one general UI capability to make this possible: an optional full-window background image on any UI tree.

## Decisions

- **`background` lives on `Tree` (the `AnchoredTree` envelope), not on the map catalog.** Any registered tree can use it — loading, frontend, pause, HUD. Consequence: one loading tree per map, each naming its own image; the engine needs no catalog-side image field.
- **Cover fit, full window.** The background fills the whole device backbuffer, not the letterboxed 1280×720 canvas, and crops to keep the image's aspect. Consequence: no splash-color bands at 16:10, 4:3 or ultrawide.
- **Screenshots come from the engine's headless capture mode** (`--features capture`, scene JSON). Capture runs no scripts. Consequence: shots show architecture, lighting and map-placed props at rest, never enemies or script-spawned entities.
- **Splash damage test leaves the frontend catalog.** The map stays on disk and loadable by path. Five catalog maps remain.
- **World textures leave loading screens.** No loading tree references anything under `content/dev/textures/`.

## Invariants

### Engine: `background` field

- Rust: `AnchoredTree.background: Option<TreeBackground>`, `pub struct TreeBackground { pub image: String }`, both `rename_all = "camelCase"`, `deny_unknown_fields`. `background` is skip-serialized when `None`, so a tree without it round-trips byte-identically.
- Wire: `"background": { "image": "<ui image key>" }`. An object, not a bare string, so later fields (tint, fit) extend it without a wire break. `image` is a UI image registry key — the same namespace as `Image({ asset })`.
- Script surface (JS and Luau): `Tree({ anchor, offset, background: { image } }, root)`. An empty `image` or an unknown key inside `background` is rejected the way the bridge rejects other malformed `Tree` props.
- Typedefs: `TreeProps` and `AnchoredTreeDescriptor` gain `background?: TreeBackground`; `TreeBackground = { image: string }`, with a doc comment stating full-window cover fit beneath the tree. Regenerate via `cargo run -p postretro-sim --bin gen-script-types`; update typedef fixtures to match.
- Rect: device pixels `[0, 0, device_w, device_h]`. The origin is the backbuffer's top-left, not the canvas origin. Independent of `anchor`, `offset` and root size.
- Crop: `s = max(device_w / image_w, device_h / image_h)`. Visible texture extent is `(device_w / s, device_h / s)` texels, centered. `uv_rect = [u0, v0, uw, vh]` in normalized `[0, 1]` texture space (the existing `UiInstance.uv_rect` convention): `uw = device_w / (s * image_w)`, `u0 = (1 - uw) / 2`, and likewise for v. Exactly one of `uw`/`vh` is 1 unless the aspects match, then both are.
- Paint order: the background is the tree's first paint op, beneath every root widget. It has no focus, hit-test, accessibility node or `visibleWhen`; it is decorative.
- Color: untinted white `[1, 1, 1, 1]`, no 9-slice margin.
- Image size unknown (unregistered key, or not yet uploaded): emit no background quad, no panic. The tree's widgets still draw.
- Retained path: a settled frame's cached draw list includes the background. A viewport change recomputes the crop. An image-size generation change that affects the background key rebuilds the list.
- Layering: no upward crate edges. `layering_invariants_hold` (`crates/xtask/src/crate_graph.rs`) enforces it. All wgpu stays in the renderer. The renderer's UI pass already samples `uv_rect`, so no shader change is expected.

### Content

- Images: `content/dev/ui/loading/<map-id>.png`, 1920×1080 RGBA PNG, one per catalog map. UI image key `dev/loading/<map-id>`.
- Capture specs: `content/dev/ui/loading/scenes/<map-id>.scene.json`, resolution `[1920, 1080]`, `output` pointing at the PNG, so a shot can be re-taken after a map edit.
- Loading trees: `dev.loading.<map-id>`, one per catalog entry, set as that entry's `loadingTree`. Each draws its map's background, a level-name line and the progress bar on a dark translucent panel near the bottom.
- Mod pool (`loading.tree`): a single `dev.loading.plain` tree — the level name and bar, no imagery — for path loads with no catalog entry.
- Catalog ids (unchanged): `campaign-test`, `kinematic-platform`, `movement-feel`, `stress-warren-hallway-inspection`, `combat-demo`.

### Engine: loading-only images load lazily (amendment, owner decision)

Five eager 1920×1080 screenshots cost ~41 MB of resident textures plus a synchronous decode at mod init and staged reload. The owner chose to load only the chosen loading tree's images.

- **Loading-only images.** A mod `uiImages` entry is loading-only when some loading-candidate tree references it, as `background.image` or as an `Image` widget's `asset`, and no other registered mod tree or presentation template references it. Loading candidates are every name in any catalog entry's `loadingTree` pool, every name in the mod's `loading.tree` pool, and a mod-registered `loadingScreen`. Loading-only images are not decoded at mod init or staged reload. Every other entry stays eager, exactly as today. Consequence: no manifest or SDK change; authors write the same `uiImages`.
- **Decode off the main thread, on choice.** When a load chooses its tree (`begin_loading_screen`), that tree's loading-only images not already registered start decoding on a worker thread. A Loading or Settling frame polls; once decoded, the main thread uploads through the existing registration path. Until then the background draws nothing (the unknown-size rule) and the tree's widgets draw normally. Consequence: no main-thread stall; on a very short load the shot may never appear.
- **Release when the loading screen ends.** Every path through `end_loading_screen` (reveal, failure, abandon, network relevel) and platform suspend unregisters the images that load uploaded. A decode still in flight at that point is discarded when it arrives. Consequence: zero resident cost outside a load; each load re-decodes its shot.
- **Registry removal.** `UiImageRegistry` gains removal. It drops the entry and its natural size and bumps the image-size generation, so a retained tree rebuilds and stops emitting the quad. All wgpu stays in the renderer.
- **Reload safety.** A staged reload that changes the committed `uiImages` or the loading pools recomputes the loading-only set. An in-flight decode whose key or path no longer matches is discarded on arrival.
- **Logging.** Mod init's info line counts eager images and names how many were deferred. Each lazy upload logs one info line with key, size and decode milliseconds. Failures warn naming the `uiImages` entry, as eager loads do.

## Tracks and file ownership

| Track | Owner | Files |
|---|---|---|
| A. Engine `Tree` background | agent, isolated worktree | `crates/scripting-core/**`, `crates/ui/**`, `crates/sim/**` (typedef fixtures, gen output), `sdk/types/**`, compile-forced spillover elsewhere (`AnchoredTree` struct literals), `context/lib/ui.md`, `docs/scripting-reference.md` |
| B. Screenshots | orchestrator, main checkout | `content/dev/ui/loading/**` |
| C. Content wiring | orchestrator, after A merges | `content/dev/scripts/loading-screens.ts`, `content/dev/scripts/frontend-menu.ts`, `content/dev/start-script.ts`, `content/dev/start-script.js` if regenerated |
| D. Lazy loading-only images | agent, on the branch after review round 2 | `crates/postretro/src/app/ui_images.rs`, `crates/postretro/src/startup/loading_screen*.rs`, suspend path, `crates/renderer/src/render/ui/image_registry.rs` and its renderer API, a pure image-key walk in `postretro-scripting-core` if needed, `context/lib/ui.md` §5, `context/lib/boot_sequence.md` §1 Loading screen |

## Acceptance

**A**
- `cargo test -p postretro-scripting-core --lib background` → nonzero passed, 0 failed. Covers: round-trip with `background`; a tree without it serializes with no `background` key; unknown key inside `background` rejected; JS and Luau bridges accept `background: { image }` and reject an empty `image`.
- `cargo test -p postretro-ui --lib background` → nonzero passed, 0 failed. Covers: crop for a wider-than-image device (2560×1080 over 1920×1080), a taller one (1280×1024), and an equal aspect (3840×2160); rect equals the full device rect at a letterboxed size; background is paint op 0 ahead of root content; unknown image size emits no background quad; a retained settled frame still carries it.
- Typedef fixture tests pass: `cargo test -p postretro-sim --lib typedef` → nonzero passed, 0 failed. `git diff sdk/types` shows `TreeBackground`.
- `cargo test -p xtask layering_invariants_hold` passes.
- `cargo clippy -p postretro-ui -p postretro-scripting-core -p postretro-sim --all-targets -- -D warnings` clean.

**B**
- Five PNGs exist at 1920×1080, each reviewed by eye for a readable landmark, and shown to the owner.

**C**
- `grep -n "textures/" content/dev/scripts/loading-screens.ts` → empty.
- `grep -n "splash-damage-demo" content/dev/scripts/frontend-menu.ts` → empty.
- A live run shows a screenshot loading screen for a catalog load and the frontend backdrop.

**D**
- Unit tests prove: an image referenced only by a loading candidate is deferred; one also referenced by a non-loading tree or a presentation template is eager; begin → decode → upload registers it; end unregisters it and a retained tree stops emitting its quad; end before the decode lands discards it; a reload that changes the entry discards the stale decode.
- `cargo test -p postretro --bin postretro -- startup:: app::ui_images` and `cargo test -p postretro-renderer --lib image_registry` → nonzero passed, 0 failed.
- A live boot logs mod init with 0 eager and 5 deferred images, then one lazy-upload line for `dev/loading/combat-demo` at the frontend-backdrop load.

## Open questions

None.
