# Specular Falloff Parity — Test Coverage

## Goal

Close the verification gap left when static-light specular was made to evaluate the authored falloff
model. The *implementation* shipped — `pack_spec_lights` packs `falloff_model_code` into
`SpecLight.cone_cos.w` (`spec_buffer.rs`), and the forward specular loop and SDF selection evaluate it
through the shared `light_eval_falloff` (`light_falloff.wgsl`), whose curve matches the compiler bake
`lightmap_bake::falloff`. But the tests that would *hold* that parity are partial or absent, so the
guarantee "static specular fades on the same curve as its baked diffuse, for every falloff model" is
currently unpinned and could regress silently. This follow-up adds the missing coverage; it changes no
shipped behavior.

## Scope

### In scope

- Extend the `SpecLight` pack test to cover all three falloff models.
- A distance-sweep test pinning the WGSL `light_eval_falloff` curve against the compiler-bake falloff
  shape, for all three models, across `0 → beyond range`.

### Out of scope

- Any change to the shipped packing, shader, or bake logic — this is test-only.
- Unifying the several hand-maintained falloff copies (`lightmap_bake::falloff`, `sh_bake::falloff`,
  `light_falloff.wgsl`, `billboard.wgsl::falloff`) into one source. That is a real single-source
  cleanup, but larger than this coverage patch and tracked separately if pursued.

## Direction

**Problem.** The falloff-model plumbing is verified only by a one-model pack assertion and by
spot-check reimplementations that never compare against the actual bake curve, so a future edit to
either the WGSL helper or the bake could break diffuse/specular parity without failing a test.

**Prior commitments.** The repo's WGSL↔Rust parity pattern is the headless-wgpu mirror in
`sdf_light_select_test.rs` (precedent `curve_eval_test.rs`): run the real WGSL helper on a GPU harness,
assert against a Rust reference. AC 2 below does not need the GPU — the WGSL `light_eval_falloff` is a
pure per-distance function, so a Rust mirror of its branches compared to the documented bake shape
across a sweep pins the same guarantee cheaply and deterministically.

## Acceptance criteria

- [ ] A pack test asserts `SpecLight` bytes 60..64 decode to `0.0` / `1.0` / `2.0` for a `Linear` /
      `InverseDistance` / `InverseSquared` light respectively — all three models, not only
      `InverseSquared`. (Today: `packs_falloff_model_code_into_cone_cos_w`, `spec_buffer.rs`, covers
      only `InverseSquared` because its `sample()` fixture hardcodes that model.)
- [ ] For each of the three models, a host-side test asserts the `light_eval_falloff`
      (`light_falloff.wgsl`) curve equals the compiler-bake falloff shape across a distance sweep from
      `0` to beyond `range`, within float tolerance: `Linear` reaches `0` continuously at `range`;
      `InverseDistance` / `InverseSquared` follow `1/d` / `1/d²` inside `range` and cut to `0` beyond
      it. This strengthens the existing spot-check
      `shared_falloff_matches_baked_model_shapes_and_cutoffs` (`shader_tests.rs`) from ~8 hard-coded
      points to a full sweep over all three models.

## Rough sketch

**AC 1** is a two-line extension: pack a `Linear` and an `InverseDistance` `MapLight` alongside the
existing `InverseSquared` fixture and assert byte-60 decodes to `0.0` / `1.0` / `2.0`.

**AC 2** lives in the renderer crate, where the WGSL and the existing string/spot-check parity test
already sit. The bake `lightmap_bake::falloff` is in `level-compiler` and not callable cross-crate, so
the test asserts the WGSL-mirrored Rust closure against the *documented* bake formula (the same
formula both `lightmap_bake::falloff` and `light_eval_falloff` implement), swept rather than
spot-checked. Keep the existing structural string-match on the WGSL source as-is; add the sweep beside
it.

**Deferred — AC "no over-reach" headless capture.** A headless capture asserting an `InverseSquared`
static light's specular highlight stops where its diffuse falls off would exercise the full path
end-to-end, but the only capture suite (`capture_frame.rs`, GPU-gated, `#[ignore]`) currently asserts
shadowmask darkening and PNG dimensions, not a specular-vs-diffuse extent relationship. Adding that
assertion is higher-cost and lower-marginal-value than AC 1–2 (which pin the curve directly); left for
the owner to schedule if the direct-curve tests prove insufficient.
