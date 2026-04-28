# Script-Driven Light Bake

## Goal

Run behavior scripts at compile time inside `prl-build` using a sandboxed QuickJS context with
a compile-time primitive surface. `world.query` returns lights from the parsed `.map`; captured
`set_light_animation` calls populate `MapLight.animation` on matching static lights. These
lights flow into the existing `AnimatedBakedLights` pipeline — animated-light chunks and
per-texel weight maps — without any new bake infrastructure. Authors write one script that works
at both compile time and runtime.

## Scope

### In scope

- Compile-time QuickJS context in `prl-build`: `registerHandler`, `world_query`,
  `get_component`, `set_light_animation` implemented against parsed `MapData`.
- SDK prelude evaluated in the compile-time context (same `sdk/lib/prelude.js` the engine uses).
- Firing a synthetic `levelLoad` event after scripts are evaluated, capturing `set_light_animation`
  calls targeting static (`!is_dynamic`) lights.
- Populating `MapLight.animation` from captured calls; lights then enter `AnimatedBakedLights`
  automatically and flow through the existing chunk and weight-map bake.
- Clear build log output: which scripts ran, which lights received baked animations, which
  primitives were called but unavailable (logged as warnings, not errors).

### Out of scope

- Luau compile-time execution. QuickJS only; Luau baked animations remain FGD-only for now.
- Compile-time execution of scripts other than the `levelLoad` handler.
- Baking animations for dynamic lights (`is_dynamic = true`). These are runtime-only; the
  compile-time context ignores `set_light_animation` calls targeting them with a warning.
- Detecting or preventing the script from also running at runtime. Runtime behavior is unchanged:
  the script fires at level load and `set_light_animation` sets the same animation on the same
  static lights. The result is idempotent for scripts that use only compile-time-safe data
  (light positions, tags). See Open questions for the edge case.
- Any new PRL section or format change. The existing `AnimatedLightChunks` (section 23) and
  `AnimatedLightWeightMaps` (section 25) carry the output.

## Acceptance criteria

- [ ] `prl-build` with a `scripts_dir` KVP executes behavior scripts in a compile-time QuickJS
  context before the lightmap bake step.
- [ ] `world.query({ component: "light", tag: "arena_1_light" })` in a compiled script returns
  handle objects for all static lights in the map with that tag.
- [ ] `light.setAnimation(...)` on a returned handle populates `MapLight.animation` for that
  light; the light appears in `AnimatedBakedLights` and receives a weight-map bake.
- [ ] A static light whose tag is never queried by any script is unaffected — no animation, no
  weight map, no change to baked output.
- [ ] A `set_light_animation` call targeting a dynamic light is logged as a warning and ignored;
  build succeeds.
- [ ] Primitives not available at compile time (`fire_tick`, `get_player_transform`, etc.) are
  no-ops that log a warning; the build does not fail.
- [ ] `arena-wave.ts` compiled against the test map produces the correct per-light phase offsets
  in the baked `AnimatedLightChunks` section (verifiable by reading the packed PRL output or an
  integration test).
- [ ] `cargo test --workspace` passes after the change.

## Dependencies

This plan **requires** scripting-compile-pipeline Tasks 1 and 3 to land first:
- Task 1 (SDK prelude) — the compile-time context evaluates `sdk/lib/prelude.js` before scripts;
  without it, `world` is undefined and every script that imports it fails.
- Task 3 (level compiler script compilation) — `prl-build` must already locate and compile `.ts`
  → `.js` before this plan can execute the compiled output.

## Tasks

### Task 1: Compile-time QuickJS context

Add `rquickjs` to `postretro-level-compiler`'s `Cargo.toml`. Build a minimal compile-time
scripting context in a new `crates/level-compiler/src/compile_time_scripts.rs` module.

The context provides:

| Primitive | Compile-time behavior |
|-----------|----------------------|
| `registerHandler(event, fn)` | Stores the handler; fires it when `event = "levelLoad"` |
| `world_query(filter)` | Filters `MapData.lights` by tag/component; returns entity handle objects with id, transform, tag |
| `get_component(id, "Light")` | Returns intensity, color, isDynamic from the matching `MapLight` |
| `set_light_animation(id, anim)` | Records `(light_index, LightAnimation)` in a capture table |
| Any other primitive | No-op; logs `[prl-build] compile-time: unsupported primitive '{name}' ignored` |

The context evaluates the SDK prelude (`sdk/lib/prelude.js`, loaded from disk relative to the
compiler binary or via `include_str!`) before any user scripts. Then it evaluates each compiled
`.js` behavior script. After all scripts are evaluated, it fires `"levelLoad"`. The capture
table is the output.

Entity handles returned by `world_query` use the `MapLight` vec index as the compile-time entity
ID. The handle shape matches the runtime `LightEntity` shape (`id`, `transform.position`,
`tag`, `isDynamic`) so `wrapLightEntity` in the SDK prelude works unchanged.

### Task 2: Wire captures into MapData and bake

After the compile-time context runs, apply captured animations to `MapData`:

```
for (light_index, animation) in captures {
    if map_data.lights[light_index].is_dynamic {
        log::warn!("[prl-build] compile-time: set_light_animation on dynamic light ignored");
        continue;
    }
    map_data.lights[light_index].animation = Some(animation);
}
```

No other bake code changes. The `AnimatedBakedLights` namespace filter
(`!is_dynamic && animation.is_some()`) automatically picks up newly-animated lights. The
animated-light chunk builder, weight-map baker, and SH-volume baker all consume
`AnimatedBakedLights` already — they see script-driven animations the same as FGD `style`
animations.

Add the compile-time execution step to `main.rs` between script compilation (Task 3 of
scripting-compile-pipeline) and the lightmap bake, so baked animations are present when the
baker runs.

## Sequencing

**Phase 1 (sequential):** Task 1 — the context must exist before captures can be wired.

**Phase 2 (sequential):** Task 2 — applies captures to `MapData` and wires into the bake call
chain.

## Rough sketch

### Translate `LightAnimation` from JS to Rust

The JS `LightAnimation` object shape (`periodMs`, `brightness`, `color`, `direction`,
`playCount`, `phase`, `startActive`) must be deserialized from a `rquickjs::Value` into the
Rust `LightAnimation` struct. The mapping already exists for the runtime (the `set_light_animation`
primitive in `primitives.rs` does this translation). The compile-time context can reuse the same
deserialization logic by extracting it to a shared helper in `postretro-level-format` or by
duplicating it in the compiler (the runtime crate is not a dep of the compiler crate).

`postretro-level-format` already defines `LightAnimation` as a shared type. Moving the
JS-to-Rust deserialization helper there (or to a small `scripting_bridge` module in the compiler)
keeps the logic in one place.

### Prelude path in the compiler

The compiler binary can locate `sdk/lib/prelude.js` using the same walking logic as
`scripts_build_binary()` in `watcher.rs`: walk upward from `CARGO_MANIFEST_DIR` until a
`sdk/lib/prelude.js` is found. In distribution, it sits beside the compiler binary.

### Idempotency at runtime

For static lights receiving script-driven baked animations, the runtime script also fires
`set_light_animation` at `levelLoad`. The runtime call updates the `AnimationDescriptor` the
compose pass reads — to the same values the bake used (same positions, same tags, same script
logic). The result is idempotent as long as the script uses only data available at both compile
time and runtime (light positions and tags). The baked weight maps remain valid.

## Open questions

- **Non-idempotent scripts:** If a script uses data only available at runtime (e.g. a counter
  that increments each load, a time-based seed, player-proximity logic), compile-time and
  runtime executions produce different animations. The weight maps are baked against the
  compile-time result, but the runtime descriptor is overwritten. The compose pass uses the
  runtime descriptor to index into the weight maps, so divergence between compile-time and
  runtime animations produces incorrect lighting. Document this constraint, or detect it (scripts
  that call unavailable primitives are flagged as non-bake-safe).

- **LightAnimation deserialization:** The cleanest home for the JS→Rust translation helper is
  `postretro-level-format`. Adding `rquickjs` as an optional dep of the format crate is possible
  but adds complexity. Alternative: duplicate the small deserialization into the compiler. Decide
  before Task 1 starts.

- **Prelude availability:** The compiler binary needs `sdk/lib/prelude.js` at runtime (not
  embedded via `include_str!` as the engine does, since the compiler already runs offline from
  disk). Confirm the distribution packaging includes the prelude alongside `prl-build`.
