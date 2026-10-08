# Mod Loading Screen — Contract

Every track reads this file before its own brief. Amended by the integrator only.

## Goal

Level loads show a mod-authored UI tree instead of the bare boot splash. Mods declare which tree(s) to show — mod-wide and per map, with an optional random pool — and supply their own imagery. The engine publishes load progress and the loading level's name as readonly UI state, so a loading bar is an ordinary `Bar` bound to an engine slot; no loading-specific widget exists. The engine ships a branded fallback loading screen (PostRetro logo + bar). Separately, the boot splash logo now spans the minor golden section of the window width, `1 - 1/φ` ≈ 0.382 (landed by the integrator; amended from one fifth after an owner test run).

## Decisions

1. **Imagery is a general UI-image contract, not loading-specific.** New manifest field `uiImages: { [name]: path }` — `path` is a PNG relative to the mod root. Each entry loads into the UI image registry under key `name`, at mod init and again after a committed staged reload, the way glyph art does. Any tree (HUD, menus, loading) references it with `Image({ asset: name })`. *Consequence:* the loading screen needs no image plumbing of its own.
2. **`Image` gains optional `width` / `height`** (logical-reference px, positive). Neither → natural size (today's behavior). One → the other follows the source aspect. Both → exactly that box. *Consequence:* without it, no authored art larger than the 1280×720 reference canvas can be laid out, including the engine logo.
3. **Tree selection.** Manifest `loading: { tree: string | string[] }`; map catalog entry `loadingTree?: string | string[]`. Resolution per load: the catalog entry's pool → the mod's `loading.tree` pool → the registry name `loadingScreen` (engine fallback, shadowable by a mod tree of the same name through normal tier precedence). A pool picks uniformly at random among its **registered** names, once per load, when the load begins. Raw-path loads (CLI map, dev cycle) have no catalog entry and start at the mod pool.
4. **Progress is honest and coarse.** The level-loader publishes a thread-safe progress counter (done/total units) the worker advances as the PRL load consumes sections; the bar maps that fraction onto `[0, LOAD_PARSE_SHARE]`, `LOAD_PARSE_SHARE = 0.85`. Main-thread install stays one blocking frame; when the payload arrives the app paints one more loading frame at `LOAD_PARSE_SHARE`, then installs on the next frame. *Consequence:* one extra frame of latency per load, in exchange for a bar that visibly arrives before the install hitch.
5. **Two engine-owned readonly UI slots**, declared in the engine state catalog (so the SDK `getGameState()` types regenerate): `loading.progress` (number, `[0, 1]`, default 0) and `loading.levelName` (string, default ""; the catalog entry's `name`, else the map file stem). Client-local: not replicated, not persisted. Set when a load begins, updated every loading frame, reset to defaults when the load ends (success or failure).
6. **The boot splash is unchanged** — renderer-owned, pre-UI. The loading screen takes over on Loading frames once the session and full renderer exist (they always do by then). Loading frames clear to the splash background color (sRGB 8-bit `(28, 33, 39)`), so the splash→loading handoff has no color step.
7. **Engine fallback** `core/ui/loadingScreen.json`, engine tier, name `loadingScreen`: the splash logo (engine image key `engine/splashLogo`, the same PNG the boot splash draws) at width 489 logical px (0.382 × 1280, matching the boot splash), centered as the splash centers it, with a `Bar` bound to `loading.progress` beneath it. Only the bar appears at handoff.
8. **The loading screen is display-only.** UI input stays dropped on Loading frames exactly as today. Interactive widgets in a loading tree still render; they are never reachable.
9. **Dev mod proof of concept**: the dev mod declares `uiImages`, a mod-wide loading pool of at least two trees, and one per-map `loadingTree` override, each showing imagery plus a bar and the level name.

## Invariants

- **Wire names** (camelCase, exact): manifest `uiImages`, `loading`, `loading.tree`; catalog entry `loadingTree`; widget props `width`, `height` on `image`; slots `loading.progress`, `loading.levelName`; registry name `loadingScreen`; engine image key `engine/splashLogo`.
- **Reserved image prefix:** a `uiImages` name beginning `engine/` is rejected with a load-time warning (entry skipped). Mod images load before glyph art; a name that collides with a glyph key warns once and glyph art wins.
- **Paths:** `uiImages` paths are mod-relative (no `..`, not absolute — same rule as `input.glyphs` directories); a bad path, missing file, or undecodable PNG warns naming the entry and skips it. Never fatal.
- **Malformed manifest parts degrade, never abort:** a non-string / non-string-array `tree` or `loadingTree` warns and is treated as absent; an empty array is absent.
- **Round-trip identity:** `image` descriptors without `width`/`height` serialize byte-identically to today (`skip_serializing_if`). Non-positive or non-finite sizes are rejected at the bridge like `Bar` sizes.
- **TS / Luau parity:** every new manifest field and widget prop lands in both SDKs, both type files, and both manifest drains in the same pass.
- **Progress monotonicity:** `loading.progress` never decreases within one load. The counter reaches done == total when the worker returns successfully.
- **Layering:** the progress counter lives in `postretro-level-loader` (or below it); no new upward edges. `layering_invariants_hold` must stay green. The renderer owns every wgpu call.
- **Staged reload:** `loading` commits with the same successful-generation boundary as `frontend`; a failed or stale reload leaves it alone. Level-tier trees are never loading candidates (the outgoing level's tier is cleared before Loading starts).
- **Randomness** is client-local presentation; it needs no determinism or replication.
- **UI time advances on Loading frames**, so authored tweens and fades on a loading tree run.

## Tracks and file ownership

**Track 1 — data surface** (scripting-core, entities, ui, sdk; sequential, on branch):
manifest parse of `uiImages` / `loading` / `loadingTree` in both JS and Luau drains and their result types; staged-manifest commit of the new fields; `image` `width`/`height` in the descriptor, bridge validation and `ui` crate layout; SDK factories, `postretro.d.ts` / `.d.luau` types and their generation; the two engine-state catalog entries. Owns `crates/scripting-core/`, `crates/entities/src/engine_state_catalog.rs`, `crates/ui/`, `sdk/`.

**Track 2 — engine integration** (postretro, level-loader, core, content, docs; after Track 1):
progress counter in the level-loader and its worker plumbing; app-side `uiImages` loading; engine logo image registration; loading-tree resolution and random pick; Loading-frame UI render, clear color, slot writes, deferred install frame; `core/ui/loadingScreen.json`; dev mod proof of concept; author docs in `docs/`. Owns `crates/postretro/`, `crates/level-loader/`, `crates/renderer/` (only if a render seam is needed), `core/ui/`, `content/dev/`, `docs/`.

The integrator owns `context/lib/` and this file. Neither track edits `context/lib/`.

## Acceptance

Track 1:
- `cargo test -p postretro-scripting-core <filters>` covering: `uiImages` drained in JS and Luau; `loading.tree` string and array; `loadingTree` string, array, malformed → absent; `engine/` image name rejected. Report the test count.
- `cargo test -p postretro-ui <filter>` covering image sizing: natural, width-only (aspect), both.
- Image round-trip: a pre-existing `image` JSON without sizes round-trips byte-identically.
- SDK type files regenerate with no diff after generation (whatever check the repo uses for `postretro.d.ts` freshness passes).

Track 2:
- `cargo test -p postretro-level-loader <filter>`: progress is monotonic and ends at done == total on a fixture load.
- `cargo test -p postretro --bin postretro <filters>` (or the crate's actual test target) covering: pool resolution order (catalog → mod → fallback), unregistered names skipped, raw-path load uses mod pool; slots set at load begin and reset at load end; the delivered payload installs one frame after delivery.
- `cargo run -p postretro-tool -- run --install-root . maps/campaign-test.prl` (or the dev equivalent) shows the mod loading screen with a moving bar; a frame capture of a Loading frame is produced and inspected.
- `cargo test -p xtask layering_invariants_hold` passes.

## Amendments after Track 1

- Pools are a normalized `Vec<String>` (empty = absent), deduplicated in authored order: `ModLoading { tree }` on `ModManifestResult.loading` / `StagedManifest.loading` (not an `Option`), and `ModMapEntry.loading_tree`. Commit `loading` wherever `frontend` is committed today.
- `ui_images` is a `BTreeMap<String, String>` on both result types. Path shape (relative, no `..`) is validated at parse; file existence and PNG decode are load-time checks.
- SDK manifest types register in `crates/sim`; regenerate with `cargo run -p postretro-sim --bin gen-script-types`; freshness test `committed_sdk_types_match_current_registry`.

## Outcome

- Landed on `feat/mod-loading-screen`; `context/lib/` absorbed the durable facts (`boot_sequence.md` §1 Loading screen, §2; `ui.md` §1, §3, §5; `rendering_pipeline.md`).
- Accepted for now: on windows wider than 16:9 the fallback logo is smaller than the splash logo at handoff (the splash sizes by window width, UI layout by `min(w/1280, h/720)`). Fitting the splash logo to the 16:9 reference box would remove the step at no boot cost.
- Unverified at landing: a live windowed load (handoff, bar motion, held frame), ultrawide, DX12, and `sdk/type-tests/loading-screen.ts` under `tsc`.
