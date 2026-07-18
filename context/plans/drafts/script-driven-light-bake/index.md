# Script-Driven Light Bake

> **Status:** draft — substantively rewritten 2026-07-18 against the shipped reaction/interactive
> substrate. The original 2026-07-16 draft assumed the pre-reaction scripting model
> (`registerHandler`, imperative capture at a synthetic `levelLoad`); that world no longer exists.
> Later the same day, four decisions were folded in: derive (not validate-only); Luau parity is a
> landing requirement, not a fast-follow; store-conditional membership resolves to a runtime-only
> warning, no compile-time store-read check; and the mechanism pivoted from an in-process VM in
> `prl-build` to a sidecar manifest emitted by `scripts-build`. Later the same day, the three
> remaining open questions closed: manifest resolution is resolved membership (not predicates);
> a throwing script fails the build; `start_active` bakes last-write-wins from
> `levelLoad`-addressed steps only. See *Resolved decisions* below.

## Goal

Close the authoring gap between runtime script-driven light animation and the compile-time
animated-light bake by evaluating the map's data script at compile time and deriving the
**animated-baked membership set** from the returned reaction data.

Today an author animates a baked-tier light from script in two places: the script builds
`setLightAnimation` sequence steps (`sdk/lib/entities/lights.ts` handle methods or raw steps),
and the map must independently flag each targeted light `_animated 1` so the compiler reserves
its weight map, animated-chunk entry, SH-delta tiles, and compose descriptor slot. Forgetting
the flag fails silently: the runtime admits the call but routes it down the legacy forward
path — the light's baked lightmap diffuse and SH indirect stay frozen while only
forward-evaluated terms respond. This plan makes the script the single source of membership:
`scripts-build` evaluates `setupLevel`, walks the returned `LevelManifest`, collects every
`setLightAnimation` step, and emits a light-membership manifest as a sidecar alongside the
compiled script bytes; `prl-build` reads that manifest and enrolls each targeted static
baked-tier light in the animated-baked namespace exactly as `_animated 1` does — union with
explicitly flagged lights.

Deriving membership from script deliberately overrides the "explicit flag, not auto-detected"
call in `sdf-static-occluder-shadows`: failure asymmetry favors derivation, since
over-reservation is benign (wastes a weight map) but under-reservation silently breaks lighting.

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
  section 28) by invoking `scripts-build` (`postretro-script-compiler`) as a subprocess —
  `--in <source> --out <js>`, located beside the binary or on PATH (`find_scripts_build`,
  `compile_worldspawn_data_script`, `crates/level-compiler/src/main.rs`). This plan extends that
  existing subprocess seam rather than opening a new one: the same invocation also produces the
  light-membership manifest as a sidecar output. The original draft's dependency on
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

- Light-membership manifest emission in `scripts-build`: after compiling the data script,
  evaluate `setupLevel` in a sandboxed context — SDK prelude, a `world.query` implementation
  backed by the map's light table (tags/ids/positions/isDynamic), which `prl-build` passes in as
  JSON over the existing subprocess boundary; non-throwing warn-and-degrade stubs for
  runtime-only primitives. Covers both `.ts`/QuickJS and `.luau`/mlua data scripts.
- Manifest walk (in `scripts-build`): collect `setLightAnimation` steps from **all** returned
  reactions (every dispatch address, since interactive-fired sequences also target install-time
  light ids), resolve targets against the passed-in light table, emit the resolved membership
  set as a sidecar manifest beside the compiled bytes.
- Membership derivation (in `prl-build`): read the sidecar manifest; each targeted static
  baked-tier (`!is_dynamic`) light enters `AnimatedBakedLights` via the same
  placeholder-animation synthesis `_animated` uses today. Union with explicit `_animated` flags;
  the flag remains valid and is never required for script-targeted lights.
- Determinism: the manifest emitter (in `scripts-build`, not `prl-build`) pins wall-clock and
  RNG (fixed `Date`, seeded `Math.random`, and Luau equivalents) so repeated builds are
  byte-identical (Build Cache determinism invariant, `build_pipeline.md`).
- Build log: derived lights (with tags), flag-only lights, dynamic-tier targets (info — normal
  runtime path, no bake), stubbed primitives hit during evaluation.
- Runtime diagnostic: `log::warn!` when `setLightAnimation` targets a static light with no
  compose slot — turns any residual membership divergence from silent into loud.
- Doc updates: `build_pipeline.md` (manifest-emission and manifest-read pipeline stages),
  `scripting.md` (manifest-emission note, QuickJS and Luau), `docs/scripting-reference.md`
  (author-facing: no flag needed for script-animated static lights, either language).

### Out of scope

- **Baking curves.** No curve data from script reaches the PRL. Runtime stays the sole curve
  authority; the `_animated` contract ("curves stay empty until the bridge writes them") is
  unchanged.
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

- [ ] `scripts-build`, given a `data_script` KVP's compiled source, evaluates `setupLevel` and
  emits a light-membership manifest sidecar; `prl-build` reads it before the lightmap bake.
  Genuine script errors fail the build with the script path and exception; primitive stubs
  themselves never throw (parity: a script that evaluates at runtime evaluates at compile time).
- [ ] `world.query({ component: "light", tag })` in the manifest emitter's context resolves
  against the light table `prl-build` passes in, returning handles for all map lights with that
  tag, snapshot-shaped like the runtime (`id`, `transform.position`, `tags`, `isDynamic`,
  component fields) so `wrapLightEntity` and the handle methods (`pulse`, `fade`, `flicker`,
  `colorShift`, `sweep`) work unchanged.
- [ ] A static baked-tier light targeted by any returned reaction's `setLightAnimation` step
  produces baked output identical to authoring `_animated 1` on that light: excluded from the
  static lightmap, present in `AnimatedLightChunks`/`AnimatedLightWeightMaps`, descriptor slot
  reserved, delta tiles baked.
- [ ] A static light neither targeted nor flagged produces byte-identical output to today.
- [ ] A `setLightAnimation` step targeting a dynamic-tier light changes no baked output and is
  logged at info level, not as a warning.
- [ ] A light targeted by both a `levelLoad`-addressed step and a trigger-addressed step bakes
  `start_active` from the `levelLoad` step only. Two `levelLoad`-addressed steps disagreeing on
  `startActive` for the same light take the last one in manifest order and log a warning.
- [ ] Two consecutive builds of the same inputs are byte-identical, including for a script that
  calls `Math.random()` or `Date.now()` (pinned in the manifest emitter).
- [ ] A `.luau` data script derives identical membership to the equivalent `.ts` script: the same
  `setLightAnimation` targets produce the same `AnimatedBakedLights` membership set.
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

### Task 1: Light-membership manifest emitter in `scripts-build`

Give `scripts-build` (`postretro-script-compiler`) a sandboxed evaluation context — QuickJS for
`.ts`, mlua for `.luau` — that evaluates a compiled data script's `setupLevel`, walks the
returned `LevelManifest`, and emits a light-membership manifest as a sidecar output next to the
compiled bytes. This is the decided approach: full-fidelity `world.query` resolution, and the VM
stays out of `postretro-level-compiler`'s dependency graph entirely.

- `prl-build` passes its parsed light table (tags, ids, positions, `isDynamic`) to
  `scripts-build` as JSON alongside the existing `--in`/`--out` invocation, so
  `world.query({component: "light", tag})` resolves against real map lights rather than a stub.
- Evaluate the generated SDK prelude first (embed via the same build-time generation the engine
  uses; do not walk the filesystem for it). For Luau, resolve the prelude/`require` surface with
  mlua the same way.
- `world_query` backed by the passed-in light table: lights fully faithful (entity id ↔
  map-light index mapping retained; ids are build-local and never serialized); movers, trigger
  volumes, fog volumes best-effort from parsed map data if available; runtime-only kinds
  (enemies, spawner-spawned) return empty with a warning.
- All other primitives install as non-throwing stubs that log once per name. Store reads return
  declared defaults where visible, else neutral values. `getGameState()` is FFI-free and works
  as-is once the generated tree is installed.
- Pin `Date`/`Math.random` (QuickJS) and the Luau equivalents for determinism — this pinning
  lives in the manifest emitter, not in `prl-build`.
- Collect every `setLightAnimation` step from **all** returned reactions (every dispatch
  address, since interactive-fired sequences also target install-time light ids); resolve
  targets against the light table; emit the resolved membership set as the sidecar manifest.
- Both language paths (QuickJS and Luau) must derive identical membership for equivalent
  scripts — pin this with a test.

### Task 2: `prl-build` consumes the manifest and wires bake membership

Read the sidecar manifest `scripts-build` emitted (Task 1): the resolved set of targeted
map-light indices. For static baked-tier targets with `animation == None` and `is_animated ==
false`, synthesize the same placeholder `LightAnimation` the `_animated` parser path emits
(empty channels; `start_active` derived per the rule below).

**Membership vs. `start_active`.** Membership is the union of `setLightAnimation` targets across
all dispatch addresses (unchanged from the In-scope rule above — the light needs reserved
structure because it will be animated whenever its reaction fires, regardless of when that
reaction fires). Baked `start_active`, by contrast, derives from `levelLoad`-addressed steps
only, last-write-wins in manifest order. This mirrors the runtime exactly: the light bridge
writes each reaction's `start_active` into the descriptor in place when the reaction fires
(`light_bridge.rs`), so the last `levelLoad`-addressed write is what the runtime has settled to
by the time the level finishes installing, and the baked pre-script value cannot diverge from
it. Trigger- and crossing-addressed steps do not contribute to the initial baked state — they
are later transitions, not install-time state. If no `levelLoad`-addressed step targets the
light, `start_active` falls back to the FGD `_start_inactive` default (active), which correctly
yields "looks normal until triggered." When two `levelLoad`-addressed steps disagree on
`startActive` for the same light, emit a build-log line — two boot handlers targeting the same
light's initial state is an authoring smell, surfaced without inventing a precedence mechanic.

Downstream stages key on `animation.is_some()` and need no change —
`AnimatedBakedLights` picks the lights up, and the entity-shadow selector already excludes both
`is_animated` and `animation.is_some()` lights (`crates/level-compiler/src/entity_shadow_select.rs`),
so derived membership drops out of promotion by construction. Pin that with a test rather than
new code.

Emit the build-log inventory (derived / flagged / dynamic-target / stubs-hit, the last forwarded
from `scripts-build`'s own log). Run the manifest-read step between parse and the lightmap bake
in `pipeline.rs`; fold the script bytes and the manifest into the affected stages' cache keys if
they are not already part of the input hash.

### Task 3: Runtime slotless-target diagnostic

In the light bridge (or the `setLightAnimation` handler seam in
`crates/lighting/src/script_primitives.rs`), warn when a static light without an
`animated_slot` receives an animation: name the light and state that its baked contribution
will not animate. Cheap, independent of Tasks 1–2, and valuable even alone — it converts
today's silent failure into a diagnosable one for maps built before this plan lands.

This diagnostic is also the resolution for store-conditional membership (a script that gates
its light queries on store reads derives membership under stubbed compile-time defaults, which
can diverge from a runtime with different persisted state): the warning is the safety net.
Compile time does not additionally warn on store reads during `setupLevel`; author docs (Task 4)
advise against store-gating light setup instead.

### Task 4: Documentation

`build_pipeline.md`: add the manifest-emission stage (in `scripts-build`) and the manifest-read
stage (in `prl-build`) to the compiler pipeline list, and the membership rule to the PRL notes.
`scripting.md`: one paragraph — manifest emission exists for both QuickJS and Luau, what it
derives, what it stubs. `docs/scripting-reference.md`: author guidance — script-animated static
lights need no `_animated` flag when animated from the map's data script, in either language;
flag still required for mod-global reactions; advise against gating light setup on store reads.

## Sequencing

**Phase 1 (concurrent):** Task 1 (manifest emitter in `scripts-build`) and Task 3 (runtime
diagnostic) — independent.

**Phase 2 (sequential):** Task 2 — consumes Task 1's sidecar manifest.

**Phase 3:** Task 4 — after behavior settles.

## Resolved decisions

- **Manifest resolution granularity → resolved membership.** `scripts-build` receives the full
  parsed light table and returns *resolved* membership, not query predicates for `prl-build` to
  re-resolve in Rust: full fidelity, handles data-dependent `world.query` logic correctly. See
  In scope and Task 1.
- **Evaluation-failure policy → a throwing script fails the build.** A genuine script exception
  fails the build with the script path and exception; primitive stubs are non-throwing by
  design, so compile-time-only failures should be rare. See the first acceptance criterion.
- **`start_active` fidelity → dispatch-semantic last-write-wins.** Baked `start_active` derives
  from `levelLoad`-addressed steps only, last-write-wins in manifest order — this is what the
  runtime settles to at install, so the baked value cannot diverge from it. See Task 2 and its
  matching acceptance criterion.
