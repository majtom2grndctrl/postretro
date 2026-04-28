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
- Add a `build.rs` to the `postretro` crate that regenerates
  `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` during `cargo
  build`, replacing the debug-only runtime emission. Type files are always
  current after any build.

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
- [ ] `cargo build` (any profile) regenerates `sdk/types/postretro.d.ts` and
  `sdk/types/postretro.d.luau` with the new camelCase names. Verified by
  deleting the files, running `cargo build`, and confirming they reappear with
  correct content.
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
across `primitives.rs`, `primitives_light.rs`, `event_dispatch.rs`, `quickjs.rs`,
`luau.rs`, `pool.rs`, and `runtime.rs` test suites — all of which contain inline
JS/Lua eval strings calling snake_case primitive names.

Also update `typedef.rs` — specifically the `EXPECTED_TS`/`EXPECTED_LUAU`
snapshot string constants and the `day_one_primitives_all_appear_in_both_outputs`
test, which loop over snake_case names. The mini-registry fixture registers
primitives by name and the snapshot strings embed the old names.

Update error strings that reference snake_case primitive names (e.g.
`"world_query: unknown component"` in `parse_query_filter`, `"set_component:"`
etc.) to use the new camelCase name so script authors see the correct primitive
name in console errors.

Underscore-prefixed engine-internal names (e.g. `__collect_definitions` if any)
stay unchanged — they are not script-visible.

`sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` are auto-generated
and will pick up new names automatically if type generation runs. The task is to
ensure they are regenerated and committed — not hand-edited.

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
serde round-trips through JSON with no key rewriting needed. Note: the
`FromJs`/`FromLua` impls keep the `js_to_json` / `lua_to_json` bridge step;
only the `rename_json_keys(json, camel_to_snake)` line is removed.

**`primitives_light.rs` cleanup:**

Remove `serialize_light_component_camel` and `rename_animation_keys`. The
`handles_to_json` function can call `serde_json::to_value(light_component)`
directly and get correctly-cased keys.

Update tests that assert on raw JSON wire keys (e.g. any test that checks for
`period_ms` or `play_count` in a serialized value) to expect the camelCase
spelling. This includes `crates/postretro/src/scripting/components/light.rs` —
specifically the `light_animation_defaults_accept_missing_optional_fields` test,
whose JSON literal uses `"period_ms"` and will fail after `rename_all` lands.

Add a test that reads `h.component.lightType` (or `falloffModel`) from a light
entity handle in both QuickJS and Luau and asserts the value is correct
(non-undefined/non-nil). This is the crux of the latent wire-format bug being
fixed.

### Task 3: Add build.rs for compile-time type generation

Add `build.rs` at the root of the `postretro` crate. Mirror the `#[path]`
include pattern already used by `src/bin/gen_script_types.rs`:

```rust
#[path = "src/scripting/mod.rs"]
mod scripting;
```

Build the primitive registry via `register_all`, call `write_type_definitions`
targeting `concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/types")`.

Add `cargo:rerun-if-changed` entries for every source file that contributes to
primitive registration: `src/scripting/primitives.rs`,
`src/scripting/primitives_light.rs`, `src/scripting/event_dispatch.rs`,
`src/scripting/primitives_registry.rs`, `src/scripting/typedef.rs`, and
`build.rs` itself.

Add the scripting module's transitive dependencies to `[build-dependencies]` in
`Cargo.toml` — these are the same workspace crates the scripting module
imports: `mlua`, `rquickjs`, `serde`, `serde_json`, `glam`, `log`,
`thiserror`, `anyhow`. Because they are workspace deps compiled for the host
platform, Cargo shares artifacts with the main crate on native (non-cross)
builds; no significant compile-time overhead is introduced.

Remove `emit_sdk_types_in_debug` from `typedef.rs` and its call site in
`runtime.rs` — the build-time path supersedes it. The `gen-script-types`
binary remains; it provides a manual escape hatch for CI or tooling that needs
an explicit invocation.

### Task 4: Update SDK library call sites

Grep `sdk/lib/` for every snake_case primitive name (`get_component`,
`set_light_animation`, `world_query`, `set_component`, `entity_exists`,
`spawn_entity`, `despawn_entity`, `emit_event`, `send_event`) and update all
call sites found. Do not assume `world.ts` and `world.luau` are the only
consumers.

Regenerate `sdk/lib/prelude.js` from the updated TypeScript SDK:

```bash
cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js
```

Also rebuild `content/tests/scripts/arena-wave.js` from the updated
`arena-wave.ts` via the script compiler and commit the updated `.js` alongside
the source.

## Sequencing

**Phase 1 (sequential):** Task 2 — serde rename_all lands first. No scripting
surface change; purely internal. Confirms tests pass before touching names.

**Phase 2 (concurrent):** Task 1 (primitive renames) and Task 3 (build.rs) —
independent changes, can land in either order. SDK library call sites are
broken after Task 1 until Task 4 lands.

**Phase 3 (sequential):** Task 4 — SDK library call sites updated, prelude
regenerated. Restores end-to-end tests. Run `cargo build` to confirm
`sdk/types/` is regenerated with the new names.

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
the string key `"isDynamic"` — that stays unchanged. To be explicit: the
top-level `isDynamic` on the handle is duplicated by design and MUST remain
hand-inserted; the change only removes the camel-rename pass on the nested
`component` value. An implementer must not remove it.

## Open questions

None — scope is fully resolved.
