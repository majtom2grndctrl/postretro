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
  existing subprocess seam rather than opening a new one: two new optional flags on the same
  invocation (`--light-table`, `--manifest-out`; see Task 1's CLI surface) produce the
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
  backed by the map's light table (tags/ids/positions/isDynamic), which `prl-build` passes in
  via the `--light-table` flag (Task 1); non-throwing warn-and-degrade stubs for runtime-only
  primitives. Covers both `.ts`/QuickJS and `.luau`/mlua data scripts.
- Manifest walk (in `scripts-build`): collect `setLightAnimation` steps from **all** returned
  reactions (every dispatch address, since interactive-fired sequences also target install-time
  light ids), resolve targets against the passed-in light table, emit the resolved membership
  set as a sidecar manifest at the `--manifest-out` path. `scripts-build` also resolves
  `start_active` here (see Task 1's Wire formats) — membership and `start_active` are both
  resolved upstream; `prl-build` reads, it does not derive.
- Membership derivation (in `prl-build`): read the sidecar manifest; each targeted static
  baked-tier (`!is_dynamic`) light enters `AnimatedBakedLights` via the same
  placeholder-animation synthesis `_animated` uses today. Union with explicit `_animated` flags;
  the flag remains valid and is never required for script-targeted lights. For a light that is
  both `_animated`-flagged and targeted by a `levelLoad` step, the sidecar's non-null
  `startActive` wins over the FGD `_start_inactive` default (mirroring the runtime's in-place
  overwrite).
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
  tag, snapshot-shaped like the runtime (`id`, `position`, `tags`, `isDynamic`, a nested
  `component` (`LightComponent`) snapshot) so `wrapLightEntity` and the handle methods (`pulse`,
  `fade`, `flicker`, `colorShift`, `sweep`) work unchanged.
- [ ] A static baked-tier light targeted by any returned reaction's `setLightAnimation` step
  produces baked output structurally identical to authoring `_animated 1` on that light:
  excluded from the static lightmap, present in `AnimatedLightChunks`/`AnimatedLightWeightMaps`,
  descriptor slot reserved, delta tiles baked. `start_active` is governed separately by the
  `start_active` acceptance criterion below, so the two paths may differ there without violating
  this one.
- [ ] A static light neither targeted nor flagged produces byte-identical output to today (a
  golden-PRL compare — a committed golden fixture and review gate, not a self-checking unit test,
  since the harness holds no "today" bytes on its own).
- [ ] A `setLightAnimation` step targeting a dynamic-tier light changes no baked output and is
  logged at info level, not as a warning.
- [ ] A light targeted by both a `levelLoad`-addressed step and a trigger-addressed step bakes
  `start_active` from the `levelLoad` step only. Two `levelLoad`-addressed steps disagreeing on
  `startActive` for the same light take the last one in manifest order, and `scripts-build` sets
  the sidecar's `startActiveConflict` true; `prl-build` is the sole owner of the resulting
  build-log line — it logs the single warning, `scripts-build` does not separately warn.
- [ ] A `_animated 1` light also targeted by a `levelLoad` step bakes the step's resolved
  `start_active`, overwriting the existing placeholder in place, not the FGD `_start_inactive`
  default.
- [ ] Two consecutive builds of the same inputs are byte-identical, including for a script that
  calls `Math.random()` or `Date.now()` (QuickJS) or `math.random`, `os.time`, `os.clock`
  (Luau) — all pinned in the manifest emitter.
- [ ] A `.luau` data script derives identical membership to the equivalent `.ts` script: the same
  `setLightAnimation` targets produce the same `AnimatedBakedLights` membership set.
- [ ] Runtime: `setLightAnimation` on a static light with no `animated_slot` logs a warning
  naming the light by its map-light index and tag (the build-local runtime entity id is not
  stable, so it is not used) and stating the fix (script-derived membership or `_animated 1`).
- [ ] The sidecar manifest carries `stubbedPrimitives` (names hit during evaluation), and
  `prl-build`'s build log reports the full inventory: derived / flag-only / dynamic-target /
  conflict / stubbed.
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
returned `LevelManifest`, and emits a light-membership manifest as a sidecar output at the path
`prl-build` requests via `--manifest-out`. This is the decided approach: full-fidelity
`world.query` resolution, and the VM stays out of `postretro-level-compiler`'s dependency graph
entirely. That premise is about `postretro-level-compiler` (prl-build) specifically;
`script-compiler` necessarily embeds both the QuickJS and mlua VMs to evaluate `setupLevel` —
that is the intended home for them.

**CLI surface.** The existing invocation `scripts-build --in <source> --out <js>` is unchanged.
Two new optional flags: `--light-table <path.json>` (input: the map's light table) and
`--manifest-out <path.json>` (output: the light-membership sidecar). Manifest emission requires
BOTH `--light-table` and `--manifest-out` together; supplying one without the other is a usage
error. Absent both (the live-reload watcher's path), behavior is exactly today's compile-only —
no manifest, no light-table read. `prl-build` writes the light table to a temp path, passes
`--light-table`/`--manifest-out`, and reads the sidecar back from the path it chose — there is no
implicit "next to the compiled bytes" naming convention to guess. Both flags compose with the
existing `--in`/`--out` compile mode and are mutually exclusive with `--prelude` mode, consistent
with the hand-rolled parser's existing mode-compat checks (e.g. `--dep-json` is incompatible with
`--prelude`).

- `prl-build` passes its parsed light table (tags, ids, positions, `isDynamic`) to
  `scripts-build` via `--light-table` (schema below), so `world.query({component: "light",
  tag})` resolves against real map lights rather than a stub.
- For TS/QuickJS, `scripts-build` (`postretro-script-compiler`) reuses its own existing
  prelude-assembly machinery — `bundle_prelude`/`write_prelude`
  (`crates/script-compiler/src/lib.rs`) — but only for the prelude SOURCE STRING, assembled from
  the SDK sources it already compiles. The QuickJS host itself is net-new work here:
  `scripts-build` must stand up its own rquickjs VM, install the primitive stubs, invoke
  `setupLevel`, and extract the `reactions[].sequence[]` subset it needs directly — it does not
  need to port the full `LevelManifest::from_js_value` deserializer. This host cannot be borrowed:
  rquickjs VM construction, primitive-stub install, `setupLevel` invocation, and `LevelManifest`
  extraction live in `scripting-core` today, but the dependency runs `scripting-core` →
  `postretro-script-compiler` (for prelude assembly), not the reverse, so `script-compiler` cannot
  link `scripting-core`. The primitive stubs to install are the globals the assembled prelude
  references (`worldQuery`, `setLightAnimation`, `getGameState`, and the
  `wrapLightEntity`/mover/trigger/fog bridges) — discoverable from `sdk/types/postretro.d.ts` plus
  the prelude itself.
- For Luau there is no reusable generator: the ordered `.luau` embedding, `require`
  virtual-module wiring, and `wrapLightEntity` upvalue capture live in `evaluate_prelude`
  (`crates/scripting-core/src/luau_prelude.rs`), which evaluates live in an mlua VM inside the
  engine runtime crate (`scripting-core`) — and `scripting-core` depends on
  `postretro-script-compiler`, not the reverse, so `scripts-build` cannot call into it.
  Landing Luau parity (a landing requirement, not a fast-follow — see Status note above) is
  real new work in this task: porting the ordered-embedding + `require` + wrapper-capture logic,
  plus an mlua VM, into `script-compiler` itself.
- `world_query` backed by the passed-in light table: lights fully faithful to the light-table
  schema (`index`, `tags`, `position`, `isDynamic`, nested `component`); movers, trigger volumes,
  fog volumes best-effort from parsed map data if available; runtime-only kinds (enemies,
  spawner-spawned) return empty with a warning.
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

**Wire formats.**

The JSON below is the illustrative shape; the canonical contract is a pair of versioned `serde`
structs — the light-table struct and the sidecar-manifest struct — defined in
`postretro-level-format`, a leaf crate (no `postretro-*` deps) both binaries link:
`level-compiler` already depends on it (with the `serde` feature), and `script-compiler` adds the
dependency (it currently pulls no `postretro-*` crate and already uses `serde_json`, so this
introduces no cycle). Both binaries (de)serialize through these structs rather than hand-rolling
JSON on either side.

The prl-build → scripts-build light table (`--light-table`):

```json
{
  "version": 1,
  "lights": [
    {
      "index": 0,
      "tags": ["arena_1"],
      "position": [0.0, 0.0, 0.0],
      "isDynamic": false,
      "component": {
        "origin": [0.0, 0.0, 0.0],
        "lightType": "Point",
        "intensity": 1.0,
        "color": [1.0, 1.0, 1.0],
        "falloffModel": "InverseSquared",
        "falloffRange": 512.0,
        "coneAngleInner": null,
        "coneAngleOuter": null,
        "coneDirection": null,
        "isDynamic": false,
        "animation": null
      }
    }
  ]
}
```

`index` is the stable `MapData.lights` vec index — the identity the sidecar (below) keys on.
`position` is engine-space `Vec3` as `[f32; 3]`. `isDynamic` is camelCase to match the handle
surface. `component` carries the full `LightComponent` snapshot (`sdk/types/postretro.d.ts`) —
not flattened `color`/`intensity` — neutral-defaulted when the map omits fields, so `world.query`
handles are faithful; without it the reconstructed handle's `component` is nil. `scripts-build`
assigns build-local entity ids internally for `world.query`; those ids are never serialized — the
JSON carries `index` (the map-light index), not runtime ids. The light table's `position` is
`[f32; 3]`, while the runtime handle snapshot uses `{x, y, z}`; `scripts-build` reshapes
`[f32; 3]` → `{x, y, z}` when constructing the `world.query` handles it hands to the script, so
`wrapLightEntity` sees the same shape it would at runtime.

The scripts-build → prl-build sidecar manifest (`--manifest-out`):

```json
{
  "version": 1,
  "lights": [
    {
      "index": 0,
      "isDynamic": false,
      "startActive": true,
      "startActiveConflict": false
    }
  ],
  "stubbedPrimitives": ["fireTick", "getPlayerTransform"]
}
```

The sidecar lists every targeted light (static and dynamic), each carrying its map-light
`index`, `isDynamic` (echoed so `prl-build` logs+skips dynamic-tier targets without
re-deriving), the resolved `startActive` (`null` when no `levelLoad`-addressed step targets the
light), and `startActiveConflict` (true when 2+ `levelLoad` steps disagreed). `stubbedPrimitives`
is the forwarded list for `prl-build`'s build-log inventory.

`scripts-build` resolves `startActive`: `levelLoad`-addressed steps only, last-write-wins in
manifest order. It owns this because only it holds the `LevelManifest` (reaction dispatch
addresses, step order, `startActive`). A step is `levelLoad`-addressed iff its containing
reaction's dispatch address (`defineReaction` name) is the reserved `"levelLoad"` auto-fire
address; every other named or handle-referenced reaction is non-`levelLoad` and contributes to
membership but not to initial `start_active`. `prl-build` is a pure reader: it maps each
static-target record to its map light, uses `startActive` when non-null else the FGD
`_start_inactive` default, and emits the build-log inventory (derived / flagged / dynamic /
conflict / stubbed).

### Task 2: `prl-build` consumes the manifest and wires bake membership

First, invoke `scripts-build`: `prl-build` serializes the light table (the shared
`postretro-level-format` light-table struct) from the parsed `MapData`, invokes `scripts-build`
with `--light-table <temp>` and `--manifest-out <temp>`, and reads the sidecar back, deserializing
it through the shared `postretro-level-format` sidecar-manifest struct. This lands in
`compile_worldspawn_data_script` (`crates/level-compiler/src/main.rs`) — the sole `scripts-build`
invocation site, called from `pipeline.rs` — alongside the existing `--in`/`--out` call that
already lives there.

Then read the sidecar manifest: a resolved per-light record (the shared `postretro-level-format`
sidecar-manifest struct) for each targeted map-light index. For static baked-tier targets with
`animation == None` and `is_animated == false`, synthesize the same placeholder
`map_data::LightAnimation` the `_animated` parser path emits (mirroring the existing placeholder
emission in `crates/level-compiler/src/format/quake_map.rs`) — empty channels; `start_active`
taken from the record's resolved value per the rule below. This is the compiler-side placeholder
type (`start_active: bool`, field `period`) — distinct from the runtime `entities::LightAnimation`
(`start_active: Option<bool>`, `period_ms`), which Task 2 does not touch. The sidecar's `Option`
`startActive` maps to the placeholder's `bool` `start_active` by: non-null → that value; null →
the FGD `_start_inactive` default. For targets with `is_animated == true` (an existing
`_animated`-flagged placeholder), `prl-build` does not synthesize a new placeholder — instead it
overwrites the existing placeholder's `start_active` with the sidecar's resolved `startActive`
when non-null, mirroring the runtime's in-place overwrite (`light_bridge.rs`).

**Membership vs. `start_active`.** Membership is the union of `setLightAnimation` targets across
all dispatch addresses (unchanged from the In-scope rule above — the light needs reserved
structure because it will be animated whenever its reaction fires, regardless of when that
reaction fires). Baked `start_active`, by contrast, is resolved upstream in `scripts-build`
(Task 1): `levelLoad`-addressed steps only, last-write-wins in manifest order, delivered via the
sidecar's `startActive`/`startActiveConflict` fields — `scripts-build` is the stage that holds
the `LevelManifest` this filtering needs, so it owns the resolution. `prl-build` does not
re-derive it; it applies the resolved value and logs. This mirrors the runtime exactly: the
light bridge writes each reaction's `start_active` into the descriptor in place when the
reaction fires (`light_bridge.rs`), so the last `levelLoad`-addressed write is what the runtime
has settled to by the time the level finishes installing, and the baked pre-script value cannot
diverge from it. Trigger- and crossing-addressed steps do not contribute to the initial baked
state — they are later transitions, not install-time state. When the sidecar's `startActive` is
`null` (no `levelLoad`-addressed step targeted the light), `prl-build` falls back to the FGD
`_start_inactive` default (active), which correctly yields "looks normal until triggered." When
the sidecar's `startActiveConflict` is `true` (two `levelLoad`-addressed steps disagreed on
`startActive` for the same light), `prl-build` emits a build-log line — two boot handlers
targeting the same light's initial state is an authoring smell, surfaced without inventing a
precedence mechanic. `prl-build` is the sole owner of this log line: `scripts-build` only sets
`startActiveConflict` on the sidecar record, it does not itself log a warning.

Downstream stages key on `animation.is_some()` and need no change —
`AnimatedBakedLights` picks the lights up, and the entity-shadow selector already excludes both
`is_animated` and `animation.is_some()` lights (`crates/level-compiler/src/entity_shadow_select.rs`),
so derived membership drops out of promotion by construction. Pin that with a test rather than
new code.

Emit the build-log inventory (derived / flagged / dynamic-target / stubs-hit, the last forwarded
from `scripts-build`'s own log). `run_after_parsing` (`pipeline.rs`) currently takes `map_data` by
value (not `mut`); the manifest-read + membership-injection step needs `mut map_data` and must
land after the `scripts-build` call that produces the sidecar and before the light-namespace
construction. Fold the script bytes and the manifest into the affected stages' cache keys if they
are not already part of the input hash.

### Task 3: Runtime slotless-target diagnostic

In `LightBridge::update` (`crates/postretro/src/scripting/systems/light_bridge.rs`) — the seam
that holds the map-light index (`entity_ids` position), tags (`registry.get_tags(id)`),
`animated_slot`, and `is_dynamic`; the `setLightAnimation` handler seam in
`crates/lighting/src/script_primitives.rs` sees only an `EntityId` and cannot name the map-light
index this diagnostic requires, so it is not the right seam — warn when a static light without an
`animated_slot` receives an animation: name the light by its map-light index and tag (the
build-local runtime entity id is not stable, so it is not used) and state that its baked
contribution will not animate. Gate the warning on `!is_dynamic && animated_slot.is_none() &&
animation.is_some()`, so script-spawned dynamic lights (also slotless) do not warn. `update` runs
every dirty frame, so the warning must be warn-once (per light, not re-logged on every dirty
frame the animation stays active). Cheap, independent of Tasks 1–2, and valuable even alone — it
converts today's silent failure into a diagnosable one for maps built before this plan lands.

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
- **`start_active` fidelity → dispatch-semantic last-write-wins.** Baked `start_active` is
  resolved in `scripts-build` (Task 1) from `levelLoad`-addressed steps only, last-write-wins in
  manifest order, and delivered via the sidecar; `prl-build` (Task 2) applies it rather than
  re-deriving it — this is what the runtime settles to at install, so the baked value cannot
  diverge from it. See Task 1's Wire formats, Task 2, and the matching acceptance criterion.
