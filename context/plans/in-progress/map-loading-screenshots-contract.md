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

## Tracks and file ownership

| Track | Owner | Files |
|---|---|---|
| A. Engine `Tree` background | agent, isolated worktree | `crates/scripting-core/**`, `crates/ui/**`, `crates/sim/**` (typedef fixtures, gen output), `sdk/types/**`, compile-forced spillover elsewhere (`AnchoredTree` struct literals), `context/lib/ui.md`, `docs/scripting-reference.md` |
| B. Screenshots | orchestrator, main checkout | `content/dev/ui/loading/**` |
| C. Content wiring | orchestrator, after A merges | `content/dev/scripts/loading-screens.ts`, `content/dev/scripts/frontend-menu.ts`, `content/dev/start-script.ts`, `content/dev/start-script.js` if regenerated |

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

## Open questions

None.
