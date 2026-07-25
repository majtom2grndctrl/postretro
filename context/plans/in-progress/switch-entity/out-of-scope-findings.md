# Out-of-scope findings — switch-entity session

Issues surfaced while implementing `switch` that sit outside this spec's scope. None
were acted on. Each entry records how it was verified, so a follow-up agent knows what
is established fact and what still needs confirming.

Provenance shorthand: **[confirmed]** reproduced or read directly in this session ·
**[reported]** observed by an implementation agent, not independently reproduced.

---

## 1. Pre-existing failures on `main`

### Two `#[ignore]`d cold-bake tests fail **[confirmed]**

**Repro:** `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler -- --ignored`
→ 4 passed, 2 failed. Both live in the `tests/` integration suite, which runs in
well under a minute, so reproducing these does not cost the full gated set. They are
untouched by this session's `GATE_FIXTURES` trim — that trim only affects the three
SH/lightmap determinism gates in the bin target, which pass 3/0.

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

**Repro:** `cargo clippy --target-dir target/preflight-clippy --all-targets -- -D warnings`,
exit 101. Re-confirmed at the end of this session, after every switch change landed —
this branch neither causes nor fixes it, and `crates/renderer/` is absent from the
branch diff.

**Consequence worth knowing before the next session:** this is the *third* clause of the
standard preflight (`cargo fmt --check && cargo clippy … && cargo test`), so with `&&`
chaining it currently blocks `cargo test` from running at all. Anyone treating a red
preflight as "my change broke something" will lose time here. Run the clauses separately,
capturing each exit code, until this is resolved.

**Environment note, unrelated to the lint but hit on the way to it:** clippy and
`cargo test` both need system libraries the container may lack — `libasound2-dev` (ALSA,
via kira) and `libudev-dev` (via gilrs). Without them the failure is a build error in
`alsa-sys`/`libudev-sys` that looks nothing like a lint or test failure. `apt-get update`
first; a stale index 404s on the `.deb`.

---

## 2. Stale test gate

`cap_fixture_every_texel_respects_max_lights_per_chunk`
(`tests/animated_weight_maps_fixtures.rs:442`) carries a bare `#[ignore]` whose comment
says to un-ignore "once the UV packer leaves a…". It **now passes** **[confirmed]** and is
sub-second — nearly free coverage currently switched off. Confirm the packer condition
the comment refers to is genuinely resolved, then drop the attribute.

---

## 3. Unregistered primitive: `setFogScatter` **[confirmed]**

> **Correction (superseding this entry's original framing).** This was first written as the
> reason campaign-test showed no pulsing fog. That diagnosis was wrong. The fog was missing
> because `campaign-test.map` had no `fog_volume` tagged `pulse_fog`; the map author fixed
> that in `7ed99d07`. `arena-lights.ts:127` guards the whole fog block with
> `if (fogs.length > 0)`, so with an empty query **neither** fog reaction was defined — the
> unregistered primitive was unreachable code, not the cause. Pulsing itself comes from
> `fog.pulse(...)` → `setFogAnimation`, which **is** registered, which is why the pulse
> returned with the entity alone.
>
> The defect below is real but was masked. `7ed99d07` unmasks it: campaign-test now takes the
> `fogs.length > 0` branch, so the tag-targeted `setFogScatter` reaction is defined for the
> first time and will log `[Scripting] primitive 'setFogScatter' is not registered; reaction
> had no effect` at every level load. Consequence: the fog pulses, but the `0.4` baseline
> never applies.

Two shipping dev content scripts invoke a reaction primitive that Rust never registers:

- `content/dev/scripts/fog-pulse-demo.ts:42`
- `content/dev/scripts/arena-lights.ts:132`

`register_fog_reaction_primitives` (`crates/postretro/src/fx/fog_reactions/mod.rs:17-60`)
registers `setFogDensity`, `setFogGlow`, `setFogEdgeSoftness`, `setFogFalloff`,
`setFogParams`, and `setFogAnimation`. There is no `setFogScatter` and no alias, so these
calls hit the not-registered path at `crates/scripting-core/src/reaction_dispatch.rs:392`.

**The fix is a rename plus an arg-key change — not just a rename.** `SetFogGlowArgs`
(`set_fog_glow.rs:14`) has one field, `glow`, and the struct is
`#[serde(rename_all = "camelCase")]` with no alias. Both content scripts pass
`args: { scatter: 0.4 }`, so renaming the primitive alone converts a not-registered warning
into a deserialization error. Both keys have to move together.

`setFogGlow` is the current truth on the authoritative surface — the generated typedefs
(`sdk/types/postretro.d.ts:900`, `sdk/types/postretro.d.luau:979`) and their templates
(`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts:153`, `sdk_lib.luau:233`) all say
`setFogGlow`, and `committed_sdk_types_match_current_registry` guards them. The old name
survives in hand-maintained files the generator does not own:

- `sdk/lib/data_script.luau:106,108,168` (`SetFogScatterStep`)
- `sdk/lib/data_script.ts:62,80` and `sdk/lib/index.ts:55` (re-exporting a
  `SetFogScatterStep` that `postretro.d.ts` no longer declares)
- `sdk/lib/entities/fog_volumes.ts:23` (doc comment)
- `docs/scripting-reference.md:798`
- the two content scripts above

**Not affected by `7ed99d07`:** `campaign-test` is not in `GATE_FIXTURES`
(`fixture_pipeline.rs:48-55`) and the golden-PRL test pins the animated-weight-maps `mixed`
fixture, not campaign-test — so the map edit does not touch either §1 failure.

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

### An sdk_lib doc sentence has four hand-maintained copies **[confirmed]**

Editing a doc comment in the scripting type surface means editing it in **four** places,
and only one of them is discoverable from the others:

1. `crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` (and `.luau`) — the template
2. `sdk/types/postretro.d.ts` (and `.d.luau`) — the committed generated output
3. `crates/postretro/src/scripting/typedef/tests/fixtures/expected.d.ts` (and `.d.luau`) —
   committed snapshot fixtures that embed the same sdk_lib block

Nothing in 1 or 2 points at 3. Verifying that the template and the generated file agree —
the obvious check, and a real one — still leaves the snapshot fixtures stale, and the only
thing that catches it is running `typescript_snapshot_matches_full_registry` /
`luau_snapshot_matches_full_registry`. This session hit exactly that: a one-sentence doc
addition passed a template-vs-generated diff and still failed the full suite.

Not a defect in the generator; the snapshot fixtures are doing their job. But the
enforcement is discoverable only by failing, so a comment in the templates pointing at the
fixture path would save the next person a confusing red gate.

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

## 7. Deferred by scope during the review-findings pass

Both were found by the review panel, judged real, and left unfixed because the fix is
wider than this feature. Recorded so they are decisions, not oversights.

- **One `use` press fires every switch whose volume the player overlaps** **[confirmed]**.
  `trigger_system.rs` iterates triggers independently and each evaluates
  `overlapping && use_pressed`; there is no nearest-trigger arbitration, no
  consume-the-press, and no facing tie-break. Two console switches ~1.5 m apart both
  fire from one press at the default reach, as does a switch co-located with a legacy
  `use` `trigger_volume`. Newly *reachable* because the press margin is now compulsory
  and its size is a default a mapper may never touch — before, the mapper sized the
  volume by hand and could keep volumes disjoint. **Why deferred:** arbitration lives in
  the shipped `trigger_system` and would change behavior for every existing `use`
  trigger, and switch-vs-`trigger_volume` provenance is deliberately not on the wire, so
  it cannot be scoped to switches without a PRL change. Both are outside this feature's
  stated boundary (FGD + level compiler + tests). Workaround today: keep switches more
  than `2 × use_reach` apart, or narrow `use_reach` on dense banks.
- **Animated lightmaps cover atlas layer 0 only** **[reported]**. `forward.wgsl`
  documents that faces with `lightmap_layer >= 1` receive no animated lighting at all.
  The `switch-demo` room is far too small to spill past layer 0, so the light-on-press
  fixture is safe — but anyone porting that pattern into a large map gets a light that
  silently never animates on the overflow faces. A false-negative mode with no
  diagnostic; worth a compile-time or load-time warning when an animated light's
  receivers land beyond layer 0.

The switch's own **probe residual** — a switch floated more than one map unit off its
mount still grows a margin across that gap — is *not* listed here because it is
documented as an accepted limit in both `sdk/TrenchBroom/postretro.fgd` (the `switch`
comment block) and the spec's Decisions section, where an author will actually meet it.

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
- **Switches were pressable through walls.** The spec's uniform all-axis inflation omitted
  the player capsule radius, so effective reach was ~31 map units against 16-unit walls.
  Now grows per face, only where the adjacent space is open. Spec revised rather than
  shipped; rationale and the generalizable insight recorded in its Decisions section.
- **`use_reach` range, discarded `activation`, and switch diagnostics.** `use_reach <= 0`
  and an absurd upper value are now compile errors; an authored `activation` warns instead
  of vanishing; `resolve_trigger_volume` takes the classname, so a switch's errors no
  longer report themselves as `trigger_volume`. That last one supersedes the "accepted for
  v1" note the spec used to carry.
- **`trigger_volume` queries silently included switches.** Documented on all three
  author-facing surfaces (`docs/scripting-reference.md` and both type surfaces) plus the
  two generator templates, so the committed snapshot and the templates stay in step.
