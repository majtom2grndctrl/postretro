# Emissive Surfaces + Bloom — Optimization Research

Post-implementation audit. Revalidate implementation references before acting.

## Scope

Preserve HDR scene color, additive emissive, the 16/16 material texture budget,
and bloom order: after fog, before capture, before overlays. Emissive never
feeds light buffers.

## Safe Candidates

### Skip zero-strength emissive samples

`forward.wgsl` and `kinematic_brush.wgsl` currently sample the emissive texture
before multiplying it by the prefix-derived strength. Only `neon_` currently has
a nonzero strength. Guard the sample with the material-uniform strength so
zero-strength draws use a zero emissive value without a texture fetch.

The condition is uniform per draw. Keep explicit gradients available before the
branch. This changes no bindings, cache keys, or lighting behavior.

### Upload bloom parameters on create and resize

Bloom has 20 fixed parameter records per frame: extraction, four downsample
passes, ten blur passes, four upsample passes, and composite. Dimensions change
only on resize; threshold, intensity, and directions are otherwise constants.

Build one alignment-padded parameter buffer on bloom creation and rebuild it on
resize. Record passes with fixed dynamic offsets. This removes 20 tiny queue
writes from every bloom-enabled frame. If threshold or intensity becomes live
config, refresh this buffer when that config changes.

Pin parameter slot order with a unit test or assertion. Resize must upload the
new records before the next bloom frame.

### Share the black emissive placeholder

Absent emissive slots currently create a 1×1 black sRGB texture per loaded
material. Create one renderer-lifetime black emissive placeholder and reuse its
view for absent/corrupt world slots and model material fallbacks.

Pixel memory savings are negligible. The win is fewer GPU object allocations,
uploads, and driver bookkeeping at level/model load. This also matches the
documented shared-placeholder contract. Real emissive slots remain per-material.

### Reuse the resolve bind group for capture tonemapping

Capture tonemapping samples the same scene texture, sampler, and effect buffer
as the normal resolve. Reuse that bind group instead of allocating a texture view
and bind group per capture. Preserve the separate LDR capture target and default
capture effect values.

This is small for occasional captures but helps repeated scripted capture runs.

## Profile Before Changing

### Bloom pass count and intermediate bandwidth

The current five-level bloom chain records 20 render passes per enabled frame.
It owns two HDR textures per level. At ideal power-of-two dimensions, this is
roughly 2.33 full-resolution attachment writes and 6.32 full-resolution logical
texture samples per frame. Its intermediate storage is about 5.33 bytes per
full-resolution pixel: about 11 MiB at 1080p and 44 MiB at 4K, excluding driver
metadata.

Use the `bloom` GPU timing bracket at representative 1080p, 1440p, and 4K
before altering the visual algorithm. `POSTRETRO_GPU_TIMING=1` requires the
active adapter to expose `TIMESTAMP_QUERY`. The current Mac adapter lacks that
feature; test on a supported Windows discrete-GPU adapter when available.

If bloom is material to frame time, evaluate in order:

1. Fewer downsample levels while retaining the required halo radius.
2. A compact bloom-only intermediate format.
3. A different blur/composite algorithm only after visual and timing evidence.

### Compact bloom-only intermediate format

Keep full-scene `Rgba16Float`. It carries HDR scene output through tone mapping
and is not the first optimization target.

`Rg11b10Ufloat` is a possible bloom-intermediate format. It is RGB-only,
positive HDR, and four bytes per pixel instead of `Rgba16Float`'s eight. It can
roughly halve bloom intermediate storage and bandwidth. Bloom samples are
nonnegative and do not need alpha, so the representation is semantically
plausible.

Do not adopt it without checking each target adapter for render-attachment,
sampling, filtering, and additive-blend support. Keep an `Rgba16Float` fallback
if support varies. Compare halo smoothness and color banding: 11/11/10-bit
packed float precision differs from half-float RGB.

## Intentional Costs

- HDR `scene_color` and the tone-map resolve are feature requirements.
- Bloom remains after fog and before capture; moving it risks blooming overlays
  or excluding bloom from captured scenes.
- Bloom resources remain allocated while the dev toggle disables recording. Live
  re-enable is more useful than avoiding infrequent allocation cost.
- The shared material bind group remains at 16/16 textures. Do not split it just
  to avoid an emissive binding on non-emissive materials.
- Do not skip an authored emissive upload solely because the current prefix has
  zero strength. That would couple loading policy to mutable material tuning.
- Synchronous capture readback is a capture contract, not a per-frame stall.
- Resize recreates bloom textures. Optimize that only if interactive-resize
  profiling demonstrates a user-visible problem.

## Measurement Checklist

1. Run `RUST_LOG=info POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run` on an
   adapter that reports GPU timing enabled.
2. Record `bloom` and `forward` averages after each 120-frame timing window.
3. Test representative resolutions and an HDR-bright scene.
4. Compare visual halo size, banding, and color before accepting fewer levels or
   a compact format.
