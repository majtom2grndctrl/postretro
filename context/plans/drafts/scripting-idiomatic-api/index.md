# Scripting Idiomatic API

## Goal

Make the scripting API idiomatic in every language it targets. Today, most
primitive function names use `snake_case` — idiomatic in Rust but not in
TypeScript, JavaScript, or Luau. The primitive names are the modder's primary
vocabulary; they should feel native to the language they write in.

A second, related issue: the `LightComponent` struct serializes to the wire via
serde's default naming (snake_case), but the declared type definitions already
advertise camelCase field names. The mismatch is a latent bug for any script
that accesses `component.lightType` or `component.falloffModel` directly.

## Scope

### In scope

- Rename all snake_case primitive function names to camelCase on the scripting
  surface (registered string, type declarations, SDK library call sites).
- Apply serde `rename_all = "camelCase"` to `LightAnimation` and
  `LightComponent` so the wire format matches the declared type definitions
  without the manual rename pipeline in `conv.rs` / `primitives_light.rs`.
- Update Luau SDK library (`world.luau`) and TypeScript SDK library
  (`world.ts`) to call the new primitive names.
- Update all Rust tests referencing the old primitive names or the old wire
  shape.
- Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` (or
  update their hand-written declarations) to reflect the new function names.

### Out of scope

- Event name strings (`"levelLoad"`, `"tick"`) — these are already camelCase.
- Adding new primitives or component types.
- Luau-specific style changes beyond function name and serde wire alignment.
- Changing how the prelude is compiled or embedded.

## Acceptance criteria

- [ ] All primitive function names visible to scripts are camelCase. Verified by
  running the existing primitive-name assertion test (updated to expect the new
  names) and by manually checking the generated `postretro.d.ts` exports.
- [ ] The Luau equivalent `postretro.d.luau` declares the same camelCase names.
- [ ] `world.luau` and `world.ts` build without errors and call only the new
  camelCase primitive names.
- [ ] `arena-wave.ts` compiles and loads without errors on a running engine.
- [ ] Accessing `component.lightType`, `component.falloffModel`,
  `component.castShadows`, etc. on a light entity handle returned by
  `worldQuery` returns the correct value in both QuickJS and Luau.
- [ ] `cargo test` passes across all affected crates.

## Tasks

### Task 1: Rename primitive function names

Rename every `snake_case` primitive at the registration call site in Rust.
Each `.register("old_name", ...)` call becomes `.register("newName", ...)`.

Affected registrations:

| Old name             | New name            | File                              |
|----------------------|---------------------|-----------------------------------|
| `entity_exists`      | `entityExists`      | `primitives.rs`                   |
| `spawn_entity`       | `spawnEntity`       | `primitives.rs`                   |
| `despawn_entity`     | `despawnEntity`     | `primitives.rs`                   |
| `get_component`      | `getComponent`      | `primitives.rs`                   |
| `set_component`      | `setComponent`      | `primitives.rs`                   |
| `emit_event`         | `emitEvent`         | `primitives.rs`                   |
| `send_event`         | `sendEvent`         | `primitives.rs`                   |
| `world_query`        | `worldQuery`        | `primitives.rs`                   |
| `set_light_animation`| `setLightAnimation` | `primitives_light.rs`             |

`registerHandler` is already camelCase — no change.

Update the assertion in `primitives.rs::tests::register_all_installs_expected_primitives`
to use the new names. Update any other tests that call primitives by name
(e.g. JS/Lua `eval` strings that invoke `entity_exists`, `world_query`, etc.)
across `primitives.rs`, `primitives_light.rs`, and `event_dispatch.rs` test
suites.

Update `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` — the
`declare function` lines for each renamed primitive. (If these files are
auto-generated at runtime from the registry, just confirm the generator picks
up the new names; if they are hand-maintained, update them directly.)

### Task 2: Fix serde wire format for LightAnimation and LightComponent

Replace the manual camel↔snake rename pipeline with serde attributes.

**`LightAnimation` (`components/light.rs`):**

Add `#[serde(rename_all = "camelCase")]` to the struct. The field names
(`period_ms`, `play_count`, `start_active`) stay snake_case in Rust; serde
emits `periodMs`, `playCount`, `startActive` on the wire automatically.

**`LightComponent` (`components/light.rs`):**

Add `#[serde(rename_all = "camelCase")]` to the struct. Serde then emits
`lightType`, `falloffModel`, `falloffRange`, `coneAngleInner`, `coneAngleOuter`,
`coneDirection`, `castShadows`, `isDynamic` on the wire — matching the type
declarations that were already promised to modders.

**`conv.rs` cleanup:**

Remove `camel_to_snake`, `snake_to_camel`, and `rename_json_keys`. Remove
the rename wrapper calls from `FromJs`/`IntoJs`/`FromLua`/`IntoLua` impls for
`LightAnimation` and `LightComponent`. Those impls become straightforward
serde round-trips through JSON with no key rewriting needed.

**`primitives_light.rs` cleanup:**

Remove `serialize_light_component_camel` and `rename_animation_keys`. The
`handles_to_json` function can call `serde_json::to_value(light_component)`
directly and get correctly-cased keys.

Update tests that assert on raw JSON wire keys (e.g. any test that checks for
`period_ms` or `play_count` in a serialized value) to expect the camelCase
spelling.

### Task 3: Update SDK library call sites

**`sdk/lib/world.ts`:**

Change imports from `get_component`, `set_light_animation`, `world_query` to
`getComponent`, `setLightAnimation`, `worldQuery`. Update every call site.

**`sdk/lib/world.luau`:**

Change calls from `get_component(...)`, `set_light_animation(...)`,
`world_query(...)` to `getComponent(...)`, `setLightAnimation(...)`,
`worldQuery(...)`. Update the file-level comment that names the primitives.

Regenerate `sdk/lib/prelude.js` from the updated TypeScript SDK:

```bash
cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js
```

## Sequencing

**Phase 1 (sequential):** Task 2 — serde rename_all lands first. No scripting
surface change; purely internal. Confirms tests pass before touching names.

**Phase 2 (sequential):** Task 1 — primitive renames. These change the
scripting surface; SDK library call sites are broken until Task 3 lands.

**Phase 3 (sequential):** Task 3 — SDK library call sites updated, prelude
regenerated. Restores end-to-end tests.

## Rough sketch

The core of Task 1 is mechanical: nine string literals in two `.register()`
files, plus test strings. The largest churn is in `primitives_light.rs` tests,
which call primitives by name from inline JS/Lua `eval` strings.

For Task 2, serde's `rename_all = "camelCase"` applies the transform uniformly.
One subtlety: the existing `handles_to_json` in `primitives_light.rs` manually
constructs some fields (`id`, `isDynamic`, `transform`, `tag`) rather than
serializing the full struct. The `component` sub-object is the only field that
goes through `serde_json::to_value`; once `LightComponent` carries
`rename_all = "camelCase"`, that call already produces the right wire keys.

The manual field for `isDynamic` in the top-level object (not the component
sub-object) is built from `h.component.is_dynamic` directly and inserted with
the string key `"isDynamic"` — that stays unchanged.

## Open questions

- The type definition files (`postretro.d.ts`, `postretro.d.luau`) in `sdk/types/`
  carry a "Generated by gen-script-types. Do not edit by hand." header. If
  the generator runs at engine startup (debug builds), the files should update
  automatically once the primitive names change. Confirm whether the files are
  checked in as source-of-truth or generated artifacts; update accordingly.
