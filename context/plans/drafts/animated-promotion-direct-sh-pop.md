# Follow-up — promoted animated light pops brighter than its baked direct-SH delta

> **Status:** deferred bug note, not a spec. Recorded so a lighting pass can root-cause it.
>
> **Origin:** surfaced while repairing the `capture_frame` GPU goldens on `fix/capture-frame-failures`. The forced-`w` promotion stills for spawner-test's alarm light showed the closet door brightening as `w` rises, where the promotion contract expects lit texels to hold constant radiance.

## The problem

A promoted animated-baked light's receiver term is `(1 − w) × section-45 delta + w × runtime shadowed term` (`rendering_pipeline.md` §4, promoted animated lights). The no-pop guarantee needs the two arms to agree on lit texels. On spawner-test's closed closet door they do not. The runtime term is well above the baked direct-SH delta, so promotion visibly brightens the door.

- End-face crop mean (red channel, sRGB): **145 at `w=0` versus 186 at `w=1`**.
- **13451 of 17791** door pixels brighten by more than 12 between `w=0` and `w=1`. None darken.
- The door is a convex brush, and both faces the capture camera sees face `alarm_light`. The brighter runtime term looks physically plausible: at `w=1` the door's end face matches the adjacent lightmapped wall. That points at the delta reading low, not the runtime reading high. This is unconfirmed.

## Ruled out

Each of these moved the door's delta arm by a few levels at most. None closed the gap:

- `--sh-density-force-level 0` (forced L0 base density): output bytes identical to the default bake.
- `--release` (exact, uncached bake): output bytes identical to the default bake.
- `--sh-probe-spacing 0.5`: end-face delta rose from 145 to 152; the runtime arm rose too.
- Prop occlusion: removing the prop_mesh left every door pixel unchanged at the original prop position.
- Streaming mode: `off` and `sync-proof` agree once the weight-compounding fix below is in place.

## Candidate commits

The capture golden was added at `10ae2150a`. These commits after it change the SH sampler, the delta encoding, or the promoted runtime term:

- `239ecb8ae` drop alpha from SH delta tiles
- `7409f57fa` support scaled SH runtime indirection
- `f18827300` generalize adaptive SH L1 sampling
- `d628878f6` … `9db72e005`: the same-day animated promotion review fixes

It is also possible that the gap existed when the golden was authored and the golden's old self-shadow population only masked it.

## Reproduce

The measurement is renderer-only and needs no VM:

1. Compile `content/dev/maps/spawner-test.map` to a PRL under `content/dev/maps/` so capture finds the material tree.
2. Write two capture scenes using the golden's camera: position `[6.1, 2.2, -2.5]`, yaw 77, pitch −12, fov 90, 640×480. Both use `force_active: [{tag: "alarm_light", radiance: [4, 0, 0]}]`. Add `force_promotion: [{tag: "alarm_light", weight: 0.0}]` to one scene and weight `1.0` to the other.
3. Run `POSTRETRO_SH_STREAMING=sync-proof target/debug/postretro --capture <scene>` for each scene.
4. Compare the red-channel mean of the door end-face crop `30x150+392+180`, for example with `magick <png> -crop 30x150+392+180 -format "%[fx:mean.r*255]" info:`. A fixed bake should hold the two values close; today they read 145 and 186.

Measurement depends on `6e9e5ba42` (promoted tail weight no longer compounds across frames). Without it, a streamed capture's preload frames decay the runtime arm and hide the pop.

## Acceptance sketch

On the reproduce scene, `w=0` and `w=1` agree within the golden's `LIT_ENDPOINT_DRIFT` (12, summed RGB) on the door's lit texels. The prop-shadow population keeps darkening with `w`.

## Separate item — stale mixed-fixture golden PRL

`crates/level-compiler/tests/fixtures/golden/test_animated_weight_maps_mixed.pre-script-light-membership.prl` still carries an SH volume v9 section, and the runtime loader now requires v11. Its only remaining consumer is the ignored level-compiler test `mixed_fixture_without_script_membership_matches_pre_feature_golden_prl`. That test fails on `804351717` with "an un-targeted static light changed the pre-feature PRL output". The fresh bake has 26 container sections; the golden has 21. The capture test no longer reads this golden.

Do not re-baseline on that assumption alone. As in the last regeneration (`54919d7cf`), first diff the fresh bake against the golden section by section. Confirm that Lightmap (22), AnimatedLightChunks (24), and AnimatedLightWeightMaps (25) are still byte-identical, and that the delta is confined to newer sections and format bumps. Only then regenerate the golden and update the test's pinned commit and SHA-256.
