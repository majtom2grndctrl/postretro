# Changed Files — entity-model-foundation

All files modified as part of this plan's implementation (relative to HEAD at plan start).

## Content / test assets

- `content/tests/maps/campaign-test.map`
- `content/tests/scripts/arena-lights.js`
- `content/tests/scripts/arena-lights.ts`

## Context / docs

- `context/lib/build_pipeline.md`
- `context/lib/scripting.md`
- `context/plans/in-progress/entity-model-foundation/index.md`
- `docs/scripting-reference.md`

## level-compiler crate

- `crates/level-compiler/src/chunk_light_list_bake.rs`
- `crates/level-compiler/src/format/quake_map.rs`
- `crates/level-compiler/src/main.rs`
- `crates/level-compiler/src/map_data.rs`
- `crates/level-compiler/src/pack.rs`
- `crates/level-compiler/src/parse.rs`
- `crates/level-compiler/src/partition/brush_bsp.rs`
- `crates/level-compiler/src/portals.rs`

## level-format crate

- `crates/level-format/src/lib.rs`

## postretro crate — engine

- `crates/postretro/src/main.rs`
- `crates/postretro/src/prl.rs`
- `crates/postretro/src/render/mod.rs`

## postretro crate — scripting

- `crates/postretro/src/scripting/builtins/billboard_emitter.rs`
- `crates/postretro/src/scripting/builtins/mod.rs`
- `crates/postretro/src/scripting/call_context.rs`
- `crates/postretro/src/scripting/components/billboard_emitter.rs`
- `crates/postretro/src/scripting/components/particle.rs`
- `crates/postretro/src/scripting/components/sprite_visual.rs`
- `crates/postretro/src/scripting/conv.rs`
- `crates/postretro/src/scripting/ctx.rs`
- `crates/postretro/src/scripting/data_descriptors.rs`
- `crates/postretro/src/scripting/data_registry.rs`
- `crates/postretro/src/scripting/event_dispatch.rs`
- `crates/postretro/src/scripting/luau.rs`
- `crates/postretro/src/scripting/mod.rs`
- `crates/postretro/src/scripting/pool.rs`
- `crates/postretro/src/scripting/primitives.rs`
- `crates/postretro/src/scripting/primitives_light.rs`
- `crates/postretro/src/scripting/primitives_registry.rs`
- `crates/postretro/src/scripting/quickjs.rs`
- `crates/postretro/src/scripting/reaction_dispatch.rs`
- `crates/postretro/src/scripting/reactions/log_capture.rs`
- `crates/postretro/src/scripting/reactions/set_emitter_rate.rs`
- `crates/postretro/src/scripting/reactions/set_spin_rate.rs`
- `crates/postretro/src/scripting/registry.rs`
- `crates/postretro/src/scripting/runtime.rs`
- `crates/postretro/src/scripting/systems/emitter_bridge.rs`
- `crates/postretro/src/scripting/systems/light_bridge.rs`
- `crates/postretro/src/scripting/typedef.rs`

## SDK

- `sdk/lib/data_script.luau`
- `sdk/lib/data_script.ts`
- `sdk/lib/entities/emitters.luau`
- `sdk/lib/entities/emitters.ts`
- `sdk/lib/entities/lights.luau`
- `sdk/lib/entities/lights.ts`
- `sdk/lib/index.ts`
- `sdk/lib/prelude.js`
- `sdk/lib/world.luau`
- `sdk/lib/world.ts`
- `sdk/types/postretro.d.luau`
- `sdk/types/postretro.d.ts`
