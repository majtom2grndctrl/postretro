# Out-of-scope findings — switch-entity session

Issues surfaced while implementing `switch` that sit outside this spec's scope. None
were acted on. Each entry records how it was verified, so a follow-up agent knows what
is established fact and what still needs confirming.

Provenance shorthand: **[confirmed]** reproduced or read directly in this session ·
**[reported]** observed by an implementation agent, not independently reproduced.

---

## 1. Pre-existing failures on `main`

### Two `#[ignore]`d cold-bake tests fail **[confirmed]**

`cargo test -p postretro-level-compiler -- --ignored` → 4 passed, 2 failed:

| Test | Assertion |
|---|---|
| `mixed_fixture_without_script_membership_matches_pre_feature_golden_prl` (`tests/animated_weight_maps_fixtures.rs:421`) | "an un-targeted static light changed the pre-feature PRL output" — golden PRL byte mismatch |
| `fixture_keeps_script_and_kvp_animated_prl_slots_distinct` (`tests/animated_weight_maps_fixtures.rs:189`) | "animated chunks must contain both the script and KVP light, never the steady control" |

**Not caused by the switch work.** Verified by checking out `crates/level-compiler/src/parse.rs`
from `origin/main` and re-running: the same two tests fail identically.

**Leading hypothesis for the golden mismatch, unconfirmed:** the only golden file
(`tests/fixtures/golden/test_animated_weight_maps_mixed.pre-script-light-membership.prl`)
was last touched by `c8354ed`. `main` has since landed `799dc4a` (emissive material slot),
`e87889e`, and `dab1d30`. A new material slot plausibly changes PRL material bytes, in
which case the golden needs regenerating rather than the compiler fixing. **Do not
regenerate on that assumption** — diff the actual byte delta first and confirm it is
confined to the emissive slot. A golden that absorbs a real regression is worse than a
failing one. The second failure is about animated-chunk membership and is probably a
separate cause.

### Clippy is red on `main` **[confirmed]**

```
error: this function has too many arguments (8/7)
  --> crates/renderer/src/render/renderer_full_init.rs:14:1
```

`build_full_renderer` gained its eighth parameter (`bloom_render_profile`) in `621320a`,
which is an ancestor of `origin/main`. So `cargo clippy -- -D warnings` cannot pass on a
clean checkout. Fix is either `#[allow(clippy::too_many_arguments)]` or collapsing the
params into a struct — a renderer-boundary design call.

---

## 2. Stale test gate

`cap_fixture_every_texel_respects_max_lights_per_chunk`
(`tests/animated_weight_maps_fixtures.rs:442`) carries a bare `#[ignore]` whose comment
says to un-ignore "once the UV packer leaves a…". It **now passes** **[confirmed]** and is
sub-second — nearly free coverage currently switched off. Confirm the packer condition
the comment refers to is genuinely resolved, then drop the attribute.

---

## 3. Dead primitive: `setFogScatter` **[confirmed]**

Two shipping dev content scripts invoke a reaction primitive that Rust never registers:

- `content/dev/scripts/fog-pulse-demo.ts:42`
- `content/dev/scripts/arena-lights.ts:132`

The registry only registers `setFogGlow` (`crates/postretro/src/fx/fog_reactions/mod.rs:24`)
and no alias exists, so at runtime these hit the "primitive is not registered; reaction had
no effect" path in `crates/scripting-core/src/reaction_dispatch.rs`. The canonical fog
reference scene therefore has a silently dead reaction.

The rename to `setFogGlow` reached the TS typedef and `context/lib/scripting.md` §10.2 but
not the Luau typedef (`sdk/lib/data_script.luau`, `SetFogScatterStep`),
`docs/scripting-reference.md:795`, `sdk/lib/entities/fog_volumes.ts:23`, or the content.
Fixing means picking one name and sweeping all six sites.

---

## 4. Type-surface drift

### `SetLightAnimationStep.args` is too narrow **[confirmed]**

`sdk/types/postretro.d.ts:884-888` declares `args: LightAnimation`, but the handler
deserializes `Option<LightAnimation>` (`crates/lighting/src/script_primitives.rs:209-213`)
and `null` is the documented clear-the-animation path. The sibling
`SetFogAnimationStep` gets this right — `args: FogAnimation | null`
(`postretro.d.ts:935-939`). So clearing a light animation works at runtime but does not
type-check in TypeScript.

This is not cosmetic: clearing is the *only* correct way to author an off-then-on light
(see §6), so the type surface currently blocks the sanctioned pattern.

### `build_pipeline.md` light row omits `_animated` **[confirmed]**

`_animated` is a real FGD key (`sdk/TrenchBroom/postretro.fgd:112`), parsed by the
compiler, and the documented fallback for mod-global light animation — but it is absent
from the `light` row's KVP list in the Custom FGD entity table
(`context/lib/build_pipeline.md:63`). It appears only in adjacent comment prose.

---

## 5. Silent data loss: blank line inside a `.map` entity block **[reported]**

A bare blank line within an entity block causes `parse_map_file` to return `Ok` with
**only worldspawn** — the remaining entities vanish with no error and no warning. Hit
while authoring the `switch-demo` fixture; worked around there, and noted in a comment in
the fixture and its test helper.

Pre-existing shambler/shalrath parsing behavior, not introduced by this change. Worth a
diagnostic: hand-authored and generated maps both hit this, and the failure mode is
invisible until something is missing in-engine. Not independently reproduced outside the
fixture-authoring context — reproduce before designing the fix.

---

## 6. Constraints relevant to a `switch` v2 (not defects)

Surfaced while inventorying options for visual feedback on a press. Recorded here because
the spec's own open question (depress animation as a follow-up) depends on them.

- **Mover geometry is invisible to hitscan** **[confirmed]**. Weapons cast against the
  static `CollisionWorld` (`crates/postretro/src/weapon/mod.rs`); only movement uses
  `CombinedCollisionWorld`. So the spec's v2 sketch — `switch` owning a short mover throw
  — would let bullets pass through a solid-looking wall console, on top of losing baked
  lightmap, static BVH, SDF shadow, portal occlusion, and navmesh participation. This is a
  stronger argument for v1's static geometry than the spec assumed.
- **Baked static lights *are* runtime-animatable** **[reported]**. `collect_membership`
  walks every reaction returned from `setupLevel`, not just `levelLoad`, so a light
  animated only by a switch press gets its animated-bake structures reserved
  automatically — no `_animated 1` needed. This makes "console lights up on press" an
  authoring-only change with no engine work, and is the cheapest visual-feedback path.
- **`setLightAnimation` settle is multiplicative** **[reported]**. A finite `playCount`
  settles by writing `intensity *= final_brightness` back as static state. So
  `fade({from:1,to:0})` settles intensity to literal `0`, and a later
  `fade({from:0,to:1})` yields `0 × 1 = 0` — the light can never be re-lit. Off-then-on
  must be authored as `startActive:false` plus a later `setLightAnimation(id, null)`.
  Worth an explicit warning in the authoring docs; it is a trap that reads as a bug.
- **No timed sequencing within one dispatch** **[reported]**. There is no `wait(ms)` step
  in the `SequenceStep` union, so "flash, pause, then open the door" is not expressible in
  a single press — only concurrent effects plus whatever a light curve's own `periodMs`
  provides. Already recorded as WALL #1 in `content/dev/scripts/coop-two-button-puzzles.ts`.
- **Audio is not spatialized** **[reported]**. `playSound` ships and works, but all sounds
  play dry, so a switch click confirms the press without localizing to the button. The
  distance/panning table in `context/lib/audio.md` describes intent, not implementation.
- **Runtime emissive change needs engine work** **[reported]**. Emissive is baked into an
  immutable per-material bind group at load, gated on a `neon_*` name prefix plus an `_e`
  sibling texture, with `emissive_strength` a per-enum constant. Changing it at runtime
  touches `render-data/material.rs`, both `material_plan.rs`, both shaders, and needs a new
  primitive — and static world geometry has no entity to attach the state to.

---

## Fixed in this session (do not re-file)

- **`--lib` silently verified nothing on binary crates.** The recommended focused-test
  invocation assumed unit tests live in a crate's lib target. `postretro-level-compiler`
  exposes only texture helpers from its lib, so its map parsing and entity dispatch live in
  the `prl-build` bin target and `--lib` matched zero tests while printing `0 passed` and
  exiting `ok`. Corrected in `context/lib/testing_guide.md` and the three skills that
  carried the same clause.
- **Cold-bake cost folklore.** The guide claimed ~1 hour and told agents never to run a
  bare `cargo test` on the crate; the gated set measured ~31 min and a bare run is cheap.
  Dropping `occlusion-test` from `GATE_FIXTURES` cut the gates 7× (1812s → 260s), putting
  the gated set at ~5 min. Figures replaced with measured ones.
