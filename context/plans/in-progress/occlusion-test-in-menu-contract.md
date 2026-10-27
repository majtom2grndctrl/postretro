# Occlusion test in the dev menu — contract

## Goal

Offer `occlusion-test` from the dev mod's level select under Development Tests, and ship it baked in both distributions.

## Decisions

- **Catalog entry is the whole shipping change.** `dist` and `sdk-dist` ship the `maps/<name>.prl` literals in the emitted entry script (`build_pipeline.md` §Shipped level set). A literal `path` in `mapCatalog` ships the level; no recipe, since the default bake infers `content/dev/maps/occlusion-test.map`.
- **Name "Occlusion Test", tags `occlusion`, `lighting`, `test`.** `test` places it under Development Tests; no `recommended`.
- **It gets a loading screenshot.** `dev_mod_loading_trees_*` requires every catalog map to name `dev.loading.<id>` drawing `ui/loading/<id>.png`. Captured from the player spawn.
- **Cost accepted.** Every payload and SDK bundle bakes and carries the map (~139 MB `.prl` locally).

## Invariants

- Catalog `id` = `occlusion-test`; `path` = `maps/occlusion-test.prl` as a string literal.
- `loadingTree` = `loadingTreeName("occlusion-test")`; the id appears in `SCREENSHOT_MAPS`.
- Scene at `content/dev/ui/loading/scenes/occlusion-test.scene.json`, 1920×1080, writing `content/dev/ui/loading/occlusion-test.png`.

## Acceptance

- `cargo test -p postretro --lib loading` passes, count > 0.
- The PNG shows the map, looked at.
- `postretro-tool dist --install-root .` reports `maps/occlusion-test.prl` among shipped levels, and the payload holds it.
