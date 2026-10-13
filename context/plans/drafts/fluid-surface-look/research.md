# fluid-surface-look — research

Read at e8f607b94. Findings that inform the brief without deciding it.

## Why the water looks smooth
- `fluid_warp_uv` adds `0.125 * sin(uv.yx * 8 + t)` per fragment. The offset varies continuously across screen pixels, so the nearest-texel lookup in `sample_post_retro` lands on a texel boundary that curves through the face.
- `sample_post_retro` is nearest-texel sampling with an anti-aliased band about one screen pixel wide at each seam (`floor(uv*dims + 0.5)` plus a clamped linear blend). Its seam width and mip come from the unwarped derivatives.
- Pinning: `fluid_post_retro_sampling_matches_the_mover_shader` compares the fluid and kinematic bodies textually. `kinematic_brush_shader_matches_forward_post_retro_sampling` compares only a call fingerprint against forward.

## Texel density
- Face UVs are in texels: `brush_side_texel_uv`, where 1 map unit is 0.0254 m. At face scale 1.0 that is about 39 texels/m.
- UVs are normalized by the texture's own size at install (`pack_fluid_surfaces` for fluids).
- No project-, mod- or map-level texel density exists. The only worldspawn density key is `_lightmap_density`.
- `fog_pixel_scale` is the precedent for a map-owned render parameter, and it needed its own PRL section field. That cost is one reason the brief keeps the grid on the texture.

## Currents
- `resolve_environment_volume` emits exactly one of gravity, push or fluid per entity. The FGD documents a current as a fluid volume with a coincident push volume.
- `EnvironmentVolumeRecord.push` is m/s² in engine Y-up, and is `None` on every fluid record.
- `resolve_environment` resolves push at a body's position and fluid along its capsule. Push wins by priority, then the smaller volume, then map order.
- Push volumes are part of `level_content_digest`, are never script-toggled, and are not replicated per volume. Only the player's drift layer is predicted.

## SSR plumbing
- Depth is `Depth32Float` with `TEXTURE_BINDING` (`create_depth_texture`). Fog compute, SDF shadow compute and forward already read it.
- At fluid time, depth holds world, movers and skinned meshes. The viewmodel pass clears depth after bloom.
- `scene_color` is `Rgba16Float`; `COPY_SRC`/`COPY_DST` are added only under `cfg(test)` (`TEST_COPY_USAGE`).
- Reduced-resolution precedents: SDF shadow (half), fog scatter (`fog_pixel_scale`) and the bloom chain. The brief keeps the copy at scene extent. Pixel mode's per-cell rays already cut march work, and a lower-resolution copy is a measured follow-up if the 1660 numbers demand it.
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
