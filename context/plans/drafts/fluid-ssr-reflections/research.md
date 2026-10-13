# fluid-ssr-reflections — research

Read at e8f607b94. Findings that inform the brief without deciding it.

## SSR plumbing
- Depth is `Depth32Float` with `TEXTURE_BINDING` (`create_depth_texture`). Fog compute, SDF shadow compute and forward already read it.
- At fluid time, depth holds world, movers and skinned meshes. The viewmodel pass clears depth after bloom.
- `scene_color` is `Rgba16Float`; `COPY_SRC`/`COPY_DST` are added only under `cfg(test)` (`TEST_COPY_USAGE`).
- Reduced-resolution precedents: SDF shadow (half), fog scatter (`fog_pixel_scale`) and the bloom chain. The brief's quality tiers follow these: the reflection target resolves at scene, half or quarter extent.
- Budgets (pipeline_budget_tests.rs):
  - The fluid fragment stage uses 7 sampled textures across groups 0–4 against a requested 16.
  - Fragment storage is 8/8.
  - `max_bind_groups` is 8.
- No SSR, Hi-Z or runtime cubemap exists. `env_cubemap` appears only in docs.

## Options precedent
- Every graphics option is its own `PlayerOptions` field. No preset exists, and none is planned in `context/plans`.
- `lighting--per-term-isolation` rejected presets for a debug panel only, which does not bind player options.
- `render.bloom.pixelated` is the existing author-owned pixel style switch. It resolves in `renderer_bloom_profile` (startup/render_profile.rs).

## Dev content
- Fluids declared in `content/dev/start-script.ts` and `scripts/fluids-swim.ts` (with its Luau mirror): `water` and `sludge`. No lava or slime fluid exists yet.
- Maps naming them: `maps/stress-env-volumes.map`, `maps/e24-environment-test.map`. The current-vs-still manual capture needs a pool with a coincident push volume in one of these maps, or a new fixture.

## Reflection pass reads scene color without a copy
- Before the fluid pass, `scene_color` holds the opaque scene and is not attached by any pass. A pass rendering into a separate reflection target can sample it and scene depth directly. Both carry `TEXTURE_BINDING` in production.
- The earlier in-fluid-pass design needed a full-screen copy because the fluid pass renders into `scene_color`. Resolving into a separate target removes that copy.

## Existing reflection commitments
- `resource_management.md` §5 defines `env_cubemap`, with the bake tool deferred, and its feature table marks environment-mapped reflections as Planned. Nothing in `crates/` implements it.
- `rendering_pipeline.md` §13 excludes image-based specular from the closed world material vocabulary.
- `index.md` §1.1 admits runtime ray work only against baked structures.
