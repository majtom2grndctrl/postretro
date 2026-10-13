# fluid-surface-motion — research

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

## Dev content
- Fluids declared in `content/dev/start-script.ts` and `scripts/fluids-swim.ts` (with its Luau mirror): `water` and `sludge`. No lava or slime fluid exists yet.
- Maps naming them: `maps/stress-env-volumes.map`, `maps/e24-environment-test.map`. The current-vs-still manual capture needs a pool with a coincident push volume in one of these maps, or a new fixture.
