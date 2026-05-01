# Descriptor-Spawned Light Culling and Shadow Support

## Goal

Descriptor-spawned lights (via `registerEntity`) currently receive `leaf_index: u32::MAX`
in `absorb_dynamic_lights`, so the existing `leaf_visible` check in
`update_dynamic_light_slots` always excludes them — they never win shadow slots even when
fully visible. This plan assigns a real BSP leaf to each descriptor-spawned light at absorb
time, lets PVS culling flow through automatically, and lifts the `cast_shadows: false`
hard-code that blocked shadow slot eligibility.

**Depends on:** `entity-model-foundation` (in-progress).

---

## Scope

### In scope

- Runtime BSP leaf lookup in `absorb_dynamic_lights` for newly-enrolled lights.
- `warn!` for lights whose origin lands in a solid leaf.
- Confirming the existing `leaf_visible` path in `update_dynamic_light_slots` is reachable for descriptor-spawned lights; no renderer changes beyond confirming wiring is correct.
- Exposing `cast_shadows` on `LightDescriptor` in `data_descriptors.rs` and removing the hard-coded `cast_shadows: false` in `data_archetype.rs`.
- `cast_shadows` added to `LightDescriptor` in the SDK type definitions (`sdk/types/postretro.d.ts`, `sdk/types/postretro.d.luau`) and scripting reference.
- Unit tests covering leaf assignment and visibility gating.

### Out of scope

- Format changes, compiler changes, or new PRL sections.
- Renderer changes beyond confirming the existing wiring is correct.
- Moving lights at runtime (leaf re-lookup on position change is a future entity-system concern).
- Influence-bound culling (light whose origin is in an invisible leaf but whose influence sphere reaches a visible cell).
- Non-point light types for descriptor-spawned lights (currently always `Point`).

---

## Acceptance Criteria

- [ ] A descriptor-spawned light with a valid leaf receives the correct leaf index (not `u32::MAX`) after `absorb_dynamic_lights` completes.
- [ ] A descriptor-spawned light whose origin falls in a solid leaf receives `u32::MAX` and a `warn!` is emitted at absorb time.
- [ ] When the light's leaf is absent from the current visible cell set, the light is excluded from shadow slot ranking (same behavior as FGD lights in invisible cells).
- [ ] When the light's leaf is in the visible cell set, the light is eligible for shadow slots.
- [ ] A descriptor-spawned light with `cast_shadows: true` in its `LightDescriptor` can win a shadow slot once it has a valid leaf index.
- [ ] A descriptor-spawned light with `cast_shadows: false` (or the new default `false`) is excluded from shadow slot ranking regardless of visibility — matching existing `SpotShadowPool::rank_lights` behavior.
- [ ] `LightDescriptor` in `data_descriptors.rs` has a `cast_shadows: bool` field.
- [ ] `data_archetype.rs` no longer hard-codes `cast_shadows: false`; the value comes from the descriptor.
- [ ] SDK types (`postretro.d.ts`, `postretro.d.luau`) include `cast_shadows` on `LightDescriptor`.
- [ ] `docs/scripting-reference.md` documents `components.light.cast_shadows`.
- [ ] `cargo test --workspace` passes; `cargo clippy --workspace -- -D warnings` clean.

---

## Tasks

### Task A: Runtime BSP leaf lookup on absorb

Extend `LightBridge::absorb_dynamic_lights` (in `light_bridge.rs`) to walk the runtime BSP
when enrolling a new light. The `LevelWorld` is held in `App.level` and is available at the
`absorb_dynamic_lights` call site in `main.rs` (line 785); pass a reference to `LevelWorld`
into `absorb_dynamic_lights` as a new parameter. Inside the function, for each newly-enrolled
light, call `world.find_leaf(Vec3::from(origin_f64))` to get the leaf index. If the returned
leaf is `world.leaves[idx].is_solid`, emit `log::warn!` naming the entity ID and origin, and
store `ALPHA_LIGHT_LEAF_UNASSIGNED` (`u32::MAX`). Otherwise store `idx as u32`. Replace the
unconditional `leaf_index: u32::MAX` assignment in the existing `MapLightShape` push. The
`find_leaf` call is O(log n) and runs once per newly-spawned light — no per-frame cost.

When no level is loaded (`App.level` is `None`), pass a `None` reference; keep `u32::MAX` as
the fallback. This handles the edge case of a light enrolled before a level is present.

### Task B: Confirm PVS culling path and add tests

The existing `leaf_visible` check in `update_dynamic_light_slots` (`render/mod.rs` ~line 1674)
already handles valid leaf indices correctly: `ALPHA_LIGHT_LEAF_UNASSIGNED` → cull,
non-empty `visible_leaf_mask` → check the index, empty mask → pass through. No renderer
code changes are needed. Task B is verification and tests.

Add a unit test to `light_bridge.rs` that:
1. Enrolls a descriptor-spawned light (via `absorb_dynamic_lights`) into a bridge backed by a
   minimal `LevelWorld` with a known non-solid leaf.
2. Asserts the enrolled light's `leaf_index` is the expected leaf index (not `u32::MAX`).
3. Asserts a second call to `absorb_dynamic_lights` (idempotent) does not change the count.

Add a unit test verifying that a light with leaf index pointing at a solid leaf results in
`ALPHA_LIGHT_LEAF_UNASSIGNED` being stored.

### Task C: Expose `cast_shadows` on `LightDescriptor`

Add `pub(crate) cast_shadows: bool` to `LightDescriptor` in `data_descriptors.rs`. Default
is `false` — descriptor-spawned lights opt in explicitly. Update `LightDescriptor::validate`
(no additional numeric checks needed for bool). Update both FFI deserializers:
`entity_descriptor_from_js` and `entity_descriptor_from_lua` — treat the field as optional,
defaulting to `false` when absent so existing scripts remain valid.

In `data_archetype.rs::apply_data_archetype_dispatch`, replace the hard-coded
`cast_shadows: false` in the `LightComponent` construction with `light_desc.cast_shadows`.
Add `cast_shadows` to `apply_light_kvp_overrides` under the `initial_cast_shadows` key,
parsing via `str::parse::<bool>()`.

Update the SDK types:
- `sdk/types/postretro.d.ts` — add `cast_shadows: boolean` to `LightDescriptor`.
- `sdk/types/postretro.d.luau` — add `cast_shadows: boolean` to `LightDescriptor`.

Update `docs/scripting-reference.md` — add `cast_shadows` row to the `components.light`
table in the `registerEntity` section.

---

## Sequencing

**Phase 1 (sequential):** Task A — BSP leaf assignment in `absorb_dynamic_lights`. Unblocks B and C.

**Phase 2 (sequential):** Task B — verification tests confirming the PVS path is reachable. Depends on A.

**Phase 3 (parallel with B):** Task C — `cast_shadows` field wiring. Depends on A landing so the full
descriptor-spawned path is exercised; B and C can be developed together once A is in.

---

## Rough Sketch

**`absorb_dynamic_lights` signature change:**

```rust
// Proposed design
pub(crate) fn absorb_dynamic_lights(
    &mut self,
    registry: &EntityRegistry,
    world: Option<&prl::LevelWorld>,
) { ... }
```

The call site in `main.rs` (line 785) passes `self.level.as_ref()`.

**Leaf lookup per new light:**

```rust
// Proposed design
let leaf_index = match world {
    Some(w) => {
        let origin = Vec3::new(
            origin_f64[0] as f32,
            origin_f64[1] as f32,
            origin_f64[2] as f32,
        );
        let idx = w.find_leaf(origin);
        if w.leaves[idx].is_solid {
            log::warn!("[LightBridge] descriptor-spawned light {id:?} origin {:?} \
                        lands in a solid leaf; culled on all paths", origin_f64);
            ALPHA_LIGHT_LEAF_UNASSIGNED
        } else {
            idx as u32
        }
    }
    None => ALPHA_LIGHT_LEAF_UNASSIGNED,
};
```

**`cast_shadows` field addition:**

`LightDescriptor` gains `pub(crate) cast_shadows: bool`. Serde `#[serde(default)]` attribute
(already used on the struct via `Serialize, Deserialize` derivation) handles absent-field
defaulting for both JS and Lua paths — verify or add explicit `None`-check in each
deserializer since `LightDescriptor` currently uses `serde_json::from_value` internally.
The `apply_light_kvp_overrides` match arm for `"cast_shadows"` parses `"true"`/`"false"`.

**`data_archetype.rs` change:**

`cast_shadows: false` → `cast_shadows: light_desc.cast_shadows` in the `LightComponent`
construction block at line ~220.

**Files to modify:**

| File | Task | Change |
|------|------|--------|
| `crates/postretro/src/scripting/systems/light_bridge.rs` | A, B | Add `world: Option<&prl::LevelWorld>` param to `absorb_dynamic_lights`; BSP leaf lookup; tests |
| `crates/postretro/src/main.rs` | A | Pass `self.level.as_ref()` to `absorb_dynamic_lights` |
| `crates/postretro/src/scripting/data_descriptors.rs` | C | Add `cast_shadows: bool` field; update both deserializers |
| `crates/postretro/src/scripting/builtins/data_archetype.rs` | C | Remove `cast_shadows: false` hard-code; pass `light_desc.cast_shadows`; add `initial_cast_shadows` KVP override |
| `sdk/types/postretro.d.ts` | C | Add `cast_shadows: boolean` to `LightDescriptor` |
| `sdk/types/postretro.d.luau` | C | Add `cast_shadows: boolean` to `LightDescriptor` |
| `docs/scripting-reference.md` | C | Document `components.light.cast_shadows` |

---

## Open Questions

None. Resolved decisions:

- **`cast_shadows` default:** `false`. Opt-in nudges modders to make a deliberate choice; no existing map regresses.
- **`LightDescriptor` serde default:** use `#[serde(default)]` on the new field, consistent with the existing pattern on the struct. Both the JS and Lua FFI paths go through `serde_json::from_value` after `js_to_json` / `lua_to_json` conversion — same mechanism used for other optional fields.
