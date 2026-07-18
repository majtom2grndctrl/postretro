# Script-Driven Light Bake

> **Status:** draft — substantively rewritten 2026-07-18 against the shipped reaction/interactive
> substrate. The original 2026-07-16 draft assumed the pre-reaction scripting model
> (`registerHandler`, imperative capture at a synthetic `levelLoad`); that world no longer exists.

## Goal

Close the authoring gap between runtime script-driven light animation and the compile-time
animated-light bake by evaluating the map's data script inside `prl-build` and deriving the
**animated-baked membership set** from the returned reaction data.

Today an author animates a baked-tier light from script in two places: the script builds
`setLightAnimation` sequence steps (`sdk/lib/entities/lights.ts` handle methods or raw steps),
and the map must independently flag each targeted light `_animated 1` so the compiler reserves
its weight map, animated-chunk entry, SH-delta tiles, and compose descriptor slot. Forgetting
the flag fails silently: the runtime admits the call but routes it down the legacy forward
path — the light's baked lightmap diffuse and SH indirect stay frozen while only
forward-evaluated terms respond. This plan makes the script the single source of membership:
`prl-build` evaluates `setupLevel` in a sandboxed compile-time context, walks the returned
`LevelManifest`, collects every `setLightAnimation` step, and enrolls each targeted static
baked-tier light in the animated-baked namespace exactly as `_animated 1` does — union with
explicitly flagged lights.

**The durable division of labor.** Bake reserves structure; runtime supplies curves. Every
baked animated-light structure is curve-independent — weight maps bake per-texel unit-radiance
weights (`crates/level-compiler/src/animated_light_weight_maps.rs`), SH delta tiles bake
indirect at brightness 1.0 (`crates/level-compiler/src/delta_sh_bake.rs`), descriptor slots are
reserved empty. Curves arrive at runtime from the same script whose compiled bytes ride in the
PRL (`DataScript`, section 28): the levelLoad-addressed reactions fire after level install and
the light bridge writes each descriptor into the compose buffer
(`crates/postretro/src/scripting/systems/light_bridge.rs`). Compile time never bakes a curve,
so compile-time and runtime cannot disagree about one.

**Double-count invariant** (`context/lib/index.md` §2): a physical light's contribution must
never be double-counted on a given receiver. Membership derivation preserves it by routing: a
light entering the animated-baked set leaves `StaticBakedLights`
(`crates/level-compiler/src/light_namespaces.rs`), so its world-receiver direct lives in the
weight-map compose path XOR the static lightmap — never both. Curves are single-sourced from
the runtime descriptor and modulate only the compose/forward paths. There is no baked curve for
a runtime curve to diverge from; the only thing the two sides can disagree about is
*membership*, which this plan makes derived, logged, and (residually) loud at runtime.

## Current substrate (what already shipped)

- **Reactions are data; the VM drops.** `setupLevel` returns a `LevelManifest`
  (`sdk/lib/data_script.ts`); light animation is `SetLightAnimationStep` entries in `sequence`
  reaction bodies with entity ids baked at install (setup-id resolution, `scripting.md` §12).
  There is no tag-targeted `setLightAnimation`, so every light-animation step the level can
  fire is materialized in the manifest at `setupLevel` time — regardless of dispatch address
  (levelLoad, trigger event, crossing).
- **`_animated` FGD flag** (`sdf-static-occluder-shadows` Task 2c): reserves weight map,
  animated-chunk entry, descriptor slot, and delta tiles for a static-geometry light whose
  intensity arrives from script. The parser synthesizes a placeholder `LightAnimation` so the
  light enters `AnimatedBakedLights` (`crates/level-compiler/src/format/quake_map.rs`).
- **Runtime bridge**: `setLightAnimation` is admitted on static lights; a cached
  `animated_slot` routes the descriptor to the animated-compose buffer. No slot → legacy
  forward path only (`light_bridge.rs`).
- **`prl-build` compiles and embeds the data script** (worldspawn `data_script` KVP → PRL
  section 28), locating `scripts-build` beside the binary or on PATH
  (`crates/level-compiler/src/main.rs`). The original draft's dependency on
  scripting-compile-pipeline Tasks 1 and 3 has shipped and been superseded by this.
- **Carrier sections need no change**: `AnimatedLightChunks` (24), `AnimatedLightWeightMaps`
  (25), `DeltaShVolumes` (27), and the SH-volume descriptor table carry the output.
- **Reference content**: `content/dev/scripts/arena-lights.ts` + `campaign-test.map`. Both
  arena wave groups currently use dynamic-tier lights (`light_dynamic*`), burning the rationed
  runtime budget for a fixed-geometry ambient effect that belongs on the baked tier. This plan
  is what makes retagging them to baked-tier lights a one-line map edit with no flag
  bookkeeping.

## Scope

### In scope

- Compile-time script evaluation in `prl-build`: after parse, before the lightmap/SH bakes,
  evaluate the compiled data script (QuickJS) in a sandboxed context — SDK prelude, a
  `world.query` implementation backed by parsed `MapData` (lights fully; other map-derived
  components best-effort), non-throwing warn-and-degrade stubs for runtime-only primitives.
- Manifest walk: collect `setLightAnimation` steps from **all** returned reactions (every
  dispatch address, since interactive-fired sequences also target install-time light ids), map
  compile-time entity ids back to map-light indices.
- Membership derivation: each targeted static baked-tier (`!is_dynamic`) light enters
  `AnimatedBakedLights` via the same placeholder-animation synthesis `_animated` uses today.
  Union with explicit `_animated` flags; the flag remains valid and is never required for
  script-targeted lights.
- Determinism: the compile-time context pins wall-clock and RNG (fixed `Date`, seeded
  `Math.random`) so repeated builds are byte-identical (Build Cache determinism invariant,
  `build_pipeline.md`).
- Build log: derived lights (with tags), flag-only lights, dynamic-tier targets (info — normal
  runtime path, no bake), stubbed primitives hit during evaluation.
- Runtime diagnostic: `log::warn!` when `setLightAnimation` targets a static light with no
  compose slot — turns any residual membership divergence from silent into loud.
- Doc updates: `build_pipeline.md` (compiler pipeline stage), `scripting.md` (compile-time
  evaluation note), `docs/scripting-reference.md` (author-facing: no flag needed for
  script-animated static lights).

### Out of scope

- **Baking curves.** No curve data from script reaches the PRL. Runtime stays the sole curve
  authority; the `_animated` contract ("curves stay empty until the bridge writes them") is
  unchanged.
- **Luau compile-time evaluation.** `.luau` data-script maps skip derivation with a clear
  warning; `_animated` remains their authoring surface. See Open questions — the
  QuickJS/Luau behavioral-twin principle (`scripting.md` §1) pressures this.
- **Mod-global reactions.** `ModManifest.reactions` (with `levels` selectors) live outside the
  map and are invisible to `prl-build`. Lights animated only by mod-global reactions still
  need `_animated`. Documented limitation.
- **Removing `_animated`.** It stays: as the surface for mod-global/interactive-only cases
  above, and as an explicit reservation that needs no script.
- **Dynamic-tier lights.** Unaffected; script animation of them is the existing runtime-only
  path.
- **PRL format changes.** None; existing sections carry everything.
- **Compile-time uses of the manifest beyond light membership** (fog, spawn, nav validation).
  Extension point, not this plan.

## Acceptance criteria

- [ ] `prl-build` on a map with a `data_script` KVP evaluates `setupLevel` in the compile-time
  context before the lightmap bake. Genuine script errors fail the build with the script path
  and exception; primitive stubs themselves never throw (parity: a script that evaluates at
  runtime evaluates at compile time).
- [ ] `world.query({ component: "light", tag })` in the compile-time context returns handles
  for all map lights with that tag, snapshot-shaped like the runtime (`id`,
  `transform.position`, `tags`, `isDynamic`, component fields) so `wrapLightEntity` and the
  handle methods (`pulse`, `fade`, `flicker`, `colorShift`, `sweep`) work unchanged.
- [ ] A static baked-tier light targeted by any returned reaction's `setLightAnimation` step
  produces baked output identical to authoring `_animated 1` on that light: excluded from the
  static lightmap, present in `AnimatedLightChunks`/`AnimatedLightWeightMaps`, descriptor slot
  reserved, delta tiles baked.
- [ ] A static light neither targeted nor flagged produces byte-identical output to today.
- [ ] A `setLightAnimation` step targeting a dynamic-tier light changes no baked output and is
  logged at info level, not as a warning.
- [ ] Two consecutive builds of the same inputs are byte-identical, including for a script that
  calls `Math.random()` or `Date.now()` (pinned context).
- [ ] A `.luau` data script logs a warning that membership derivation is skipped; the build
  succeeds with flag-only membership.
- [ ] Runtime: `setLightAnimation` on a static light with no `animated_slot` logs a warning
  naming the light (tag/index) and the fix (script-derived membership or `_animated 1`).
- [ ] Integration test: fixture map + data script animating tagged static lights compiles to a
  PRL whose animated sections cover exactly the targeted lights.
- [ ] `cargo test --workspace` passes.

## Dependencies

All hard dependencies have shipped:

- `plans/done/scripting-compile-pipeline` — shipped; partly superseded (data scripts now
  compile into PRL section 28 rather than to loose `.js` siblings).
- `plans/done/sdf-static-occluder-shadows` Task 2c — the `_animated` reservation substrate and
  compose descriptor routing this plan derives membership for.
- `plans/done/lighting-animated-sh`, `plans/done/scripted-light-color-curve-intensity` — the
  animated compose + delta pipeline.

Related, orthogonal drafts: `remove-style-key` (retires Quake `style`; touches the same parser
region), `single-source-animated-light-brightness` (forward-path brightness single-sourcing;
does not touch the compose path or membership).

## Tasks

### Task 1: Compile-time evaluation context in `prl-build`

Give the level compiler a sandboxed QuickJS context that can evaluate a compiled data script
and hand back the `LevelManifest` as plain data.

- Evaluate the generated SDK prelude first (embed via the same build-time generation the engine
  uses — a `postretro-script-compiler` build-dependency writing `prelude.js` to `OUT_DIR`; do
  not walk the filesystem for it).
- `world_query` backed by parsed `MapData`: lights fully faithful (compile-time entity id ↔
  map-light index mapping retained; ids are build-local and never serialized); movers, trigger
  volumes, fog volumes best-effort from parsed map data; runtime-only kinds (enemies,
  spawner-spawned) return empty with a warning.
- All other primitives install as non-throwing stubs that log once per name. Store reads return
  declared defaults where visible, else neutral values (see Open questions). `getGameState()`
  is FFI-free and works as-is once the generated tree is installed.
- Pin `Date`/`Math.random` for determinism.
- Prefer reusing `postretro-scripting-core` VM machinery over hand-rolling rquickjs in the
  compiler — one query implementation, behavioral parity with the runtime data context. Verify
  the dependency direction against `crate-graph.md` first; if the layering cost is too high, a
  bespoke minimal context is the fallback (see Open questions).

### Task 2: Membership derivation and bake wiring

Walk the returned manifest: every `sequence` body, every step with
`primitive == "setLightAnimation"`. Map each target id to its map light. For static baked-tier
targets with `animation == None` and `is_animated == false`, synthesize the same placeholder
`LightAnimation` the `_animated` parser path emits (empty channels, `start_active` from the
step's `startActive` when present, else the FGD `_start_inactive` default). Downstream stages
key on `animation.is_some()` and need no change — `AnimatedBakedLights` picks the lights up,
and the entity-shadow selector already excludes both `is_animated` and `animation.is_some()`
lights (`crates/level-compiler/src/entity_shadow_select.rs`), so derived membership drops out
of promotion by construction. Pin that with a test rather than new code.

Emit the build-log inventory (derived / flagged / dynamic-target / stubs-hit). Run the
evaluation step between parse and the lightmap bake in `pipeline.rs`; fold the script bytes
into the affected stages' cache keys if they are not already part of the input hash.

### Task 3: Runtime slotless-target diagnostic

In the light bridge (or the `setLightAnimation` handler seam in
`crates/lighting/src/script_primitives.rs`), warn when a static light without an
`animated_slot` receives an animation: name the light and state that its baked contribution
will not animate. Cheap, independent of Tasks 1–2, and valuable even alone — it converts
today's silent failure into a diagnosable one for maps built before this plan lands.

### Task 4: Documentation

`build_pipeline.md`: add the evaluation stage to the compiler pipeline list and the membership
rule to the PRL notes. `scripting.md`: one paragraph — compile-time evaluation exists, what it
derives, what it stubs. `docs/scripting-reference.md`: author guidance — script-animated static
lights need no `_animated` flag when animated from the map's data script; flag still required
for mod-global reactions and Luau maps.

## Sequencing

**Phase 1 (concurrent):** Task 1 (context) and Task 3 (runtime diagnostic) — independent.

**Phase 2 (sequential):** Task 2 — consumes Task 1's manifest output.

**Phase 3:** Task 4 — after behavior settles.

## Open questions

- **Derive vs. validate-only.** `sdf-static-occluder-shadows` settled on "explicit, not
  auto-detected — modder-friendly" when it chose the `_animated` flag. This plan's derivation
  reverses that: the script *is* the explicit declaration, and dual authoring is the bug. The
  conservative alternative is validate-only: evaluate at compile time, hard-error when a script
  targets an unflagged static light, keep the flag mandatory. Failure asymmetry favors
  derivation (over-reservation wastes a weight map; under-reservation silently breaks
  lighting), and derivation is the recommendation here — but it overturns a settled decision
  and needs an owner call.
- **Luau parity.** QuickJS-only derivation makes `.ts` and `.luau` maps behave differently at
  compile time, cutting against the behavioral-twin principle (`scripting.md` §1). mlua is
  already in the workspace; the cost is the Luau prelude + `require` resolver in the compiler
  context. Decide whether twin parity is a landing requirement or a fast-follow.
- **Store-conditional membership.** A script that gates its light queries on store reads
  derives membership under stubbed defaults; a runtime with different persisted store state may
  animate a different set. The Task 3 diagnostic makes under-reservation loud, and author docs
  should advise against store-gating light setup. Is that enough, or should compile time warn
  whenever a store read occurs during `setupLevel`?
- **Crate layering.** Reusing `postretro-scripting-core` (and possibly `postretro-entities`)
  in `postretro-level-compiler` pulls VM crates into the compiler's dependency graph. Check
  blast radius (`cargo run -p xtask -- crate-graph --rdeps`) before Task 1; if unacceptable, a
  bespoke minimal context duplicates the query/marshalling surface and must be pinned to the
  runtime shape by tests.
- **Evaluation-failure policy.** Recommended: a script that throws fails the build (it would
  fail at level load too, and stubs are non-throwing so compile-time-only failures should be
  rare). Alternative: warn-and-skip to flag-only membership — but that reintroduces a silent
  path. Confirm before Task 1.
- **`start_active` fidelity.** Deriving `start_active` from the step's `startActive` assumes
  one step per light; a light targeted by multiple reactions (e.g. a levelLoad idle flicker
  plus a trigger-fired surge) has no single authoritative initial state. Placeholder default
  (FGD `_start_inactive`) with a log line when steps disagree is the likely answer; confirm at
  implementation.
