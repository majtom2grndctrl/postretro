# Handoff — Animated Lightmap Normal-Map Correction

Working note. Ephemeral. Delete on ship with `research.md`.

## Where you are

You orchestrated the `animated-lightmap-normal-correction` plan end to end.
All three tasks are implemented, reviewed, and pushed. A launch panic surfaced
on the user's Mac during local testing — you root-caused and fixed it. The plan
is **not landed yet**: it waits on the user's manual visual verification.

Branch: `claude/great-carson-OWRAx`. Everything below is committed and pushed.

## What the feature does

Animated (`style`) lights now get the same bumped-Lambert normal-map correction
the static lightmap already had. Before, `style=2` lights read up to 4× dimmer
than an equivalent no-style light on normal-mapped surfaces — only the static
term responded to bump detail. The per-texel light direction needed was already
baked (`direction_oct`, section v2) and on the GPU, left unread since an SDF
trace was removed. This plan re-consumes it. No PRL format, bake, or version
change.

Data path: compose pass fuses per-texel-light baked directions into one
per-texel dominant direction (luminance-weighted by each light's current
radiance) → new `Rgba16Float` runtime atlas → group-4 binding 5 → forward pass
applies `scale_anim` to `lm_anim`, mirroring the static path's `NDOTL_EPS` floor
and 4.0 cap.

## Commit map

| Commit | What |
|--------|------|
| `8869ac0` | Move plan to `in-progress/` |
| `8f329cf` | Task 1 — emit dominant-direction atlas in compose pass (storage binding 8) |
| `5d978c0` | Task 2 — wire atlas through group-4 BGL slot 5 (non-filterable float, nearest sampler) |
| `c30332a` | Task 3 — correct `lm_anim` in `forward.wgsl` |
| `1dd705f` | Review-panel comment fixes (doc only) |
| `fc1840b` | Raise sampled-texture limit 11→12 (fixes launch panic) |

## The launch panic (fixed — keep it in mind)

Symptom: `create_pipeline_layout` validation error at launch — "Too many
bindings of type SampledTextures in Stage FRAGMENT, limit is 11, count was 12".

Cause: `REQUIRED_SAMPLED_TEXTURES` in `render/mod.rs` was a hand-rolled exact
request of 11, sized to the old forward texture count. Task 3 added the 12th
sampled texture (`animated_lm_direction`) without bumping it. wgpu grants a
device *exactly* the requested limit, never the adapter's full max — so the
device stayed capped at 11.

Fix: bump to 12. Still under the WebGPU spec floor of 16, so no
supported-hardware floor moves. The adapter pre-check below the constant still
fails fast with a `[Renderer]` error if any adapter reports < 12.

**Lesson worth carrying:** any new sampled or storage texture in a pipeline
stage must be matched against `REQUIRED_SAMPLED_TEXTURES` /
`REQUIRED_STORAGE_TEXTURES` in `render/mod.rs`. CI can't catch this — pipeline
validation needs a real GPU; the no-GPU `cargo test` path never exercises it.
Storage textures were checked and fine: compose uses 2 (bindings 6, 8) vs. a
limit of 4.

## Outstanding — blocks landing

The visual acceptance criteria are run-the-engine checks on the user's Mac, not
CI. Still unverified:

- Animated term varies with normal-map detail (bump-corrected).
- Direction-sense: bump highlight tracks an off-axis `style=2` light as it moves
  (proves the fused direction is correct, not merely non-zero).
- A/B parity: `style=2` vs. no-style of equal color/intensity within ~15% peak;
  styled light still pulses.
- No-op regression: a map with no animated weight maps renders identically to
  before.

User re-pulls, runs `cargo run -p postretro -- content/dev/maps/campaign-test.prl`,
exercises a `style=2` light over a normal-mapped surface A/B'd against a no-style
light.

## Landing the plane (when the user confirms visuals)

1. `git mv context/plans/in-progress/animated-lightmap-normal-correction context/plans/done/...`
2. Delete `research.md` and this `handoff.md` — both marked delete-on-ship.
3. No durable `context/lib/` update is required: the change reuses existing
   architecture (lightmap BGL, compose pass) and adds no new subsystem contract.
   The sampled-texture-budget gotcha lives in the `render/mod.rs` comment, where
   it belongs.
4. Commit and push. No PR has been opened — ask the user before creating one.

## Review state

Panel verdict: approve, no correctness blockers. You applied 6 of 7 doc
findings. One nit left **deliberately** — the redundant second `dummy_direction_view`
(1×1) in `animated_lightmap.rs`. Both reviewers called it cosmetic and noted it
intentionally mirrors the irradiance atlas's `dummy_view`. Don't re-flag it.

## Technical facts to keep top of mind

- **Two binding-number spaces, same atlas:** group-4 binding 5 (forward read)
  and compose storage binding 8 (compute write) are independent. Not a conflict.
- **Raw vec3, not octahedral:** the animated direction atlas stores a normalized
  vec3 directly in `.rgb` (compute-written at runtime). Forward reads `.xyz` and
  re-normalizes — it does NOT route through `decode_lightmap_direction` (that oct
  decode is static-atlas only). The compose-side `decode_packed_direction` helper
  decodes the *baked* oct input, a separate thing.
- **NaN safety:** compose stores `vec3(0)` for uncovered/canceling texels;
  `normalize(vec3(0))` in forward can yield NaN. The `use_correction_anim` gate
  catches it (`NaN > NDOTL_EPS` is false → `scale_anim = 1.0`). Documented inline
  at the `normalize` call. Don't remove that gate without removing the hazard.
- **Test invocation:** `postretro` is a binary crate, no lib target. Use
  `cargo test -p postretro` or `--bin postretro`, never `--lib` (fails with exit
  101 and no useful message).

## Preflight (last run, all green)

`cargo fmt --check` · `cargo clippy -- -D warnings` · `cargo test` (1143 pass).
