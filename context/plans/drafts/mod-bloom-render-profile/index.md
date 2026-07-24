# Mod Bloom Render Profile

## Goal

Let a mod choose its bloom resolution and a pixelated bloom style once in its
manifest. Keep the current half-resolution smooth bloom when a mod does not
opt in. Keep player preferences for a later feature.

## Scope

### In scope

- Optional static `ModManifest.render.bloom` configuration in TypeScript and
  Luau.
- `half`, `quarter`, and `eighth` base resolutions.
- Pixelated bloom upsample and final composite mode.
- Initial mod-init, debug staged-reload, resize, and renderer-rebuild lifecycle.
- SDK typedefs, parser parity, renderer regression coverage, and manual GPU
  timing guidance.

### Out of scope

- Player settings, TOML persistence, menus, and command-line overrides.
- Per-material bloom tiers, masks, or multiple concurrent bloom chains.
- Per-level or reaction-time bloom changes.
- Non-power-of-two base divisors, including thirds and golden-ratio scaling.
- Changing bloom threshold, intensity, level count, or the `POSTRETRO_BLOOM`
  diagnostic override.
- Mod-aware `--capture`; it continues to use the renderer default profile.

## Boundary inventory

| Name | Rust | JS / TS | Luau | Default |
|---|---|---|---|---|
| render envelope | `ModRenderProfile` | `ModManifest.render` | `ModManifest.render` | bloom default |
| bloom profile | `ModBloomProfile` | `RenderProfile.bloom` | `RenderProfile.bloom` | half, smooth |
| base resolution | `ModBloomResolution` | `BloomRenderProfile.resolution` | `BloomRenderProfile.resolution` | `"half"` |
| pixelated mode | `pixelated` | `BloomRenderProfile.pixelated` | `BloomRenderProfile.pixelated` | `false` |
| renderer profile | `BloomRenderProfile` | n/a | n/a | half, smooth |

Script wire:

```ts
// Proposed authoring surface
export default defineMod({
  name: "Neon demo",
  render: {
    bloom: {
      resolution: "quarter",
      pixelated: true,
    },
  },
});
```

`render` and `bloom` are optional. A non-object `render` or `bloom` logs a
warning and uses the complete default profile. An absent field defaults only
that field. An invalid `resolution` logs a warning and uses `"half"`; a
non-boolean `pixelated` logs a warning and uses `false`. QuickJS and Luau have
the same degradation behavior. These malformed optional fields do not abort an
otherwise valid manifest.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Omitted configuration exactly preserves half-resolution smooth bloom. | Tasks 4, 5 | Initial startup, no-start-script reload, renderer rebuild | AC 1, 3, 4 |
| A profile changes only after a successful manifest commit. | Tasks 4, 6 | Staged build failure, rejection, and stale result | AC 2, 3 |
| The renderer is the sole owner of bloom GPU state. | Task 5 | App mapping, resize, full renderer rebuild | AC 3, 4 |
| Every source texel has one bright-pass contribution at every supported base divisor. | Task 5 | Odd dimensions and edge blocks | AC 4 |
| Pixelation never changes downsample or blur filtering. | Task 5 | Pixelated upsample/composite selection | AC 5 |

## Acceptance criteria

- [ ] TypeScript and Luau authors can declare the optional profile shown above.
  `half`, `quarter`, and `eighth` are the only accepted resolution values.
  Omission produces the current half-resolution smooth result.
- [ ] Both script runtimes warn and degrade invalid optional bloom fields as
  specified in the boundary inventory, without rejecting an otherwise valid
  manifest. Generated SDK declarations document the same shape.
- [ ] Initial mod init and a committed debug staged reload apply the profile
  before the next scene frame. Failed, rejected, and stale reloads retain the
  active profile. A no-start-script reload restores the default profile.
- [ ] Renderer target dimensions equal `ceil(surface / (base_divisor * 2^level))`,
  clamped to one, for all three profiles and after odd-size resize. The bright
  pass covers every source texel once, including odd right/bottom edges.
- [ ] Pixelated mode has stable block replication in bloom upsample and final
  composite while downsample and Gaussian blur stay linear. Smooth mode remains
  visually unchanged from the current path.
- [ ] A mod profile survives map loads, unloads, and full-renderer recreation.
  It does not affect `POSTRETRO_BLOOM=0` or dev-tools bloom enable state.
- [ ] Focused unit, parser, staged-lifecycle, and generated-SDK tests pass.
  Manual GPU timing at a fixed scene can show quarter/eighth bloom costs against
  the default profile without adding a second timing bracket.

## Tasks

### Task 1: Extract the SDK manifest registration seam

Move existing `ModManifest` type registration and its parity guard from
`crates/postretro/src/scripting/primitives/mod.rs` into a focused sibling
registrar. Keep generated type output unchanged. This is behavior-preserving
and must land before the profile adds nested SDK types to the extracted seam.

### Task 2: Extract staged-manifest snapshot types

Move `StagedManifest` transfer types from
`crates/scripting-core/src/staged_manifest.rs` into a focused child module and
re-export them without behavior changes. Keep worker-to-main ownership,
diagnostics, and Send guarantees unchanged. This split creates the owned
snapshot seam needed for the parsed render profile.

### Task 3: Extract staged-manifest app lifecycle

Move `App::poll_staged_manifest_results` from `crates/postretro/src/main.rs`
to a focused lifecycle module without changing commit order or existing UI and
reaction behavior. Keep the existing call sites and tests passing. Later work
extends this smaller module rather than the oversized application root.

### Task 4: Add the mod-manifest render contract

Add CPU-only `ModRenderProfile` and `ModBloomProfile` parsing to scripting-core.
Register the nested SDK types and `ModManifest.render` field in the extracted
registrar, regenerate committed TypeScript and Luau declarations/fixtures, and
update the parity guard. Thread the profile through cold QuickJS/Luau mod-init
and the extracted staged snapshot, including parser and snapshot tests. Do not
introduce a renderer dependency in scripting-core.

### Task 5: Make bloom profile-driven and preserve bright sources

Add renderer-owned `BloomRenderProfile` and resolution enum, with current
behavior as `Default`. Cache it on `Renderer`, pass it through full-renderer
construction, and expose a public setter that safely reconfigures a live pass
or a later full renderer. Make `BloomPass` rebuild profile-dependent target
resources and parameter data on profile change and resize.

Generalize bright extraction to each supported divisor with per-source-texel
thresholding and correct edge normalization. Add pixelated upsample/composite
shader entry points using `textureLoad`; preserve linear downsample and blur.
Cover sizing, source coverage, shader source, default, and resize behavior in
renderer tests.

### Task 6: Commit profile changes at the app boundary

Map `ModBloomProfile` to the renderer profile in the app, after full renderer
initialization and successful initial mod init. Extend the extracted staged
lifecycle to apply built snapshots only after `Committed`, restore defaults for
`NoStartScript`, and leave the active profile untouched for failed, rejected,
or stale results. Keep the profile out of `DataRegistry`, level install/unload,
and per-frame script execution. Add lifecycle tests for cold boot, staged
outcomes, and full-renderer recreation.

### Task 7: Verify author-visible behavior and performance

Run the focused automated gates, regenerate and verify SDK types, then perform
manual visual checks on half/quarter/eighth profiles, pixelated toggle, thin
edge emitters, and odd-size resize. With `POSTRETRO_GPU_TIMING=1`, compare the
existing `bloom` timing bracket at a fixed scene; record observed values without
asserting hardware-specific frame-time targets.

## Sequencing

**Phase 1 (concurrent):** Tasks 1, 2, 3, and 5 — separate seam extractions; Task 5 has no script dependency.

**Phase 2 (sequential):** Task 4 — consumes the extracted SDK and staged-snapshot seams from Tasks 1 and 2.

**Phase 3 (sequential):** Task 6 — maps Task 4's parsed profile into Task 5's renderer API through Task 3's lifecycle seam.

**Phase 4 (sequential):** Task 7 — verifies the integrated profile lifecycle and visual result.

## Rough sketch

The app maps the scripting-core profile enum to renderer-owned profile values.
`Renderer` retains that value in boot-phase state so `finish_full_init` rebuilds
with the last committed profile. `BloomPass` retains it through resize. This
keeps all wgpu allocation and shader selection inside the renderer.

The extraction parameter carries the active base divisor. Each output pixel
iterates its corresponding 2x2, 4x4, or 8x8 source block with `textureLoad`.
Only valid source coordinates count. Every source texel is thresholded before
the block average, preserving thin emissive surfaces.

Pixelated mode selects shader pipeline variants for bloom upsample and final
scene composite. Those variants use texel-addressed reads. Filtering passes use
the existing linear sampling path.

## Open questions

None. Player maximum-resolution caps and material lower-resolution opt-ins are
separate follow-up work after this static mod profile is proven.
