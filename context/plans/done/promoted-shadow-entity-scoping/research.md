# Promoted-Shadow Entity Scoping — Research

Investigation notes behind the spec. Decisions live in `index.md`; this file
holds the measurements and the discarded branches.

---

## What produced this plan

A mode-5 capture (`SdfShadowMode::ShadowmaskUnion`, `forward.wgsl` `fs_main`
returns `shadowmask_union` directly) on `combat-demo` showed regular striping
across open floor under a promoted light, with no dynamic occluder near the
striped area. Cross-map: the striping lands on floors when the light aims
horizontally and on walls when it aims down — the surfaces the light *rakes*,
where the surface is near edge-on in light space.

Regular stripes at the shadow-texel pitch on raking surfaces is depth-compare
acne. The world depth pass carries `DepthBiasState { constant: 2, slope_scale:
1.5 }` (`renderer_init_pipelines.rs`, "Spot Shadow Depth Pipeline") and the
receiver adds a two-texel normal offset along the geometric normal
(`WORLD_RECEIVER_BIAS_SCALE` in `shadow_sample.wgsl`, applied in
`shadowmask_sample_spot_shadow_wide`). Both are calibrated face-on. The
`SHADOWMASK_SPOT_VISIBILITY_DEAD_ZONE` of `1.0/25.0` is one tap of a 25-tap
kernel and cannot absorb a stripe where several taps fail.

Acne raises the subtraction because the union takes `max(baked_vis -
shadow_map_vis, 0.0)`: a spuriously *dark* runtime compare against a correctly
*lit* bake yields a large positive difference. Peter-panning has the opposite
sign and clamps to zero, which is why receiver bias reads as a weak candidate
on paper and a strong one in the capture.

## Secondary mechanism — penumbra-model mismatch

`baked_vis` is an area-light visibility: the unoccluded fraction of rays to an
emitter disk of radius `light.light_size` (`soft_visibility` in
`lightmap_bake.rs`), defaulting to `DEFAULT_LIGHT_SIZE` = 0.25 m when
`_light_size` is absent. `light_size` is documented bake-only and is never
serialized to a runtime PRL section, so the runtime cannot reproduce the baked
penumbra even in principle.

Measured with the compiler's own `soft_visibility` (light 3 m above the
receiver plane, half-plane occluder between, no entity), against a runtime
model of the 5×5 PCF at two-texel spacing over a 1024² map:

| x from edge (m) | `baked_vis` | pool vis | `union_difference` |
|---|---|---|---|
| −0.08 | 0.1875 | 0.0 | 0.1536 |
| −0.05 | 0.3125 | 0.0 | 0.2839 |
| −0.02 | 0.4688 | 0.2 | 0.2383 |
| −0.01 | 0.5000 | 0.4 | 0.0625 |
| +0.00 | 0.4688 | 0.6 | 0.0000 |

Peak 0.28 of the direct term over a 7 cm band for that geometry; moving the
occluder toward the light widens it (penumbra half-width scales as
`light_size × d(occluder→receiver) / d(light→occluder)`). The lit side is
clean — the `max(…, 0.0)` clamps it.

This is real but is **not** what the capture shows: it produces a smooth band
hugging a shadow edge, not stripes across open floor. Recorded here so a later
reader does not re-derive it. It is also bounded by `baked_vis` under linear
falloff, so it hardens an edge rather than driving a surface below its
neighbours.

## Why this is redundant work, not just a bias defect

The lightmap already stores `contribution × visibility` (`lightmap_bake.rs`,
`light_texel_contribution_and_visibility`), and the same `v` feeds the
shadowmask. The union re-derives that occlusion at runtime from a depth
compare, gets it wrong at rake angles, and subtracts the difference. Fixing
bias treats the error; scoping the evaluation to where a dynamic occluder can
actually cast removes the class.

## Bandwidth arithmetic

`copy_spot_to_pool` runs for every promoted spot slot every frame, outside the
`needs_world_render` branch and before the entity-eligibility check.

| Copy | Per frame | At 60 fps |
|---|---|---|
| Spot — `MAX_PROMOTED_SPOT` × 1024² × 4 B | 32 MiB | ≈1.9 GiB/s |
| Cube — `MAX_PROMOTED_CUBE` × 6 faces × 512² × 4 B | 12 MiB | ≈0.7 GiB/s |
| Total | 44 MiB | ≈2.6 GiB/s |

Pure bandwidth, no ALU component, on hardware the technique survey
characterises as bandwidth-expensive and ALU-cheap.

## Alternatives not taken

**Static-only reference** — bind the promoted depth cache and compute
`static_pool_vis − combined_pool_vis` instead of `baked_vis − pool_vis`. Zero
by construction with no occluder, and it cancels acne because both compares
share a kernel and bias. Rejected as the *first* move: it doubles taps in the
region the plan is trying to make cheaper, needs `TEXTURE_BINDING` on a cache
currently declared `RENDER_ATTACHMENT | COPY_SRC`, needs a story for adapters
without `CUBE_ARRAY_TEXTURES` (`strip_point_shadow_cube`), needs
`min(difference, baked_vis)` to avoid over-subtracting where an entity shadow
lands on baked-shadowed texels, and does nothing for the copy cost. Entity
scoping makes it strictly cheaper if it is still wanted afterward, because it
would then run only inside entity shadow volumes.

**Retune bias / raise the dead zone.** Treats the symptom. Raising the dead
zone erodes real entity-shadow contrast linearly, and the two pool dead zones
are pinned against their kernels by
`forward_shader_shadowmask_dead_zone_matches_each_pool_kernel`, so any change
drags a recalibration behind it.

**Drop world-receiver pool consumption entirely.** Entity shadows would stop
appearing on world surfaces. Feature regression.

**Right-size the shadow pools.** `SHADOW_POOL_SIZE` = 96 slots at 1024²
`Depth32Float` is 384 MiB allocated eagerly, against a ceiling of roughly 19
occupied slots on `campaign-test` (11 `light_dynamic_spot` plus the promotion
cap). Real, and worth its own change — but allocation is not per-frame
traffic, so it does not serve a bandwidth goal and is out of scope here.
