## Goal

Backport the `sdk/lib/entities/<name>.{ts,luau}` structural convention — established when `BillboardEmitter` shipped — to the pre-existing lights vocabulary. Today, light-specific concerns are split across flat top-level files (`sdk/lib/world.{ts,luau}`, `sdk/lib/light_animation.{ts,luau}`) and `world` mixes a generic query primitive with light-only handle methods and animation math. After this change, light vocabulary lives under `sdk/lib/entities/lights.{ts,luau}`, mirroring the emitter layout; `world.{ts,luau}` becomes a thin generic query module; and `timeline` / `sequence` / `Keyframe` — structurally generic keyframe utilities — move to `sdk/lib/util/keyframes.{ts,luau}`. Also fixes a gap from the emitter work: `entities/emitters.luau` exists but was never wired into the Luau prelude; this plan adds the missing wiring.

## Scope

### In scope

- Move the `LightEntity` handle wrapper, easing curves, and one-cycle animation builders out of `sdk/lib/world.{ts,luau}` into `sdk/lib/entities/lights.{ts,luau}`.
- Move the light-vocabulary helpers (`flicker`, `pulse`, `colorShift`, `sweep`) out of `sdk/lib/light_animation.{ts,luau}` into the same `sdk/lib/entities/lights.{ts,luau}` file.
- Move `timeline`, `sequence`, and the `Keyframe` type out of `sdk/lib/light_animation.{ts,luau}` into a new `sdk/lib/util/keyframes.{ts,luau}` — they are structurally generic and not light-specific.
- Reduce `sdk/lib/world.{ts,luau}` to the generic `world.query` typed wrapper. It returns base `Entity` handles for non-light components and delegates light-handle wrapping to a function imported from `entities/lights`.
- Update `sdk/lib/index.ts` re-exports to source light symbols from `./entities/lights` (matching `./entities/emitters`) and keyframe utilities from `./util/keyframes`.
- Update the Luau prelude wiring in `crates/postretro/src/scripting/luau.rs`: replace the `LIGHT_ANIMATION_LUAU_SRC` `include_str!` and field list with separate constants for `LIGHTS_LUAU_SRC` and `KEYFRAMES_LUAU_SRC`; wire `entities/emitters.luau` (missing from the current prelude) into the same pass.
- Regenerate `sdk/lib/prelude.js` and the `.d.ts` / `.d.luau` files so committed artifacts match the new source layout.
- Update doc references in `crates/postretro/src/scripting/typedef.rs`, `primitives.rs`, `primitives_light.rs`, `components/light.rs`, and any other places naming the old paths.
- Delete `sdk/lib/world.{ts,luau}` light-only sections, and delete `sdk/lib/light_animation.{ts,luau}` once their contents have moved.
- Update `context/lib/scripting.md` — particularly §7 (SDK library globals) and §11 (which already names `sdk/lib/entities/emitters.{ts,luau}`) — so the lights file, the emitters wiring fix, and the `util/keyframes` module are reflected.

### Out of scope

- Naming or behavior changes to any primitive (`worldQuery`, `setLightAnimation`, `getComponent`).
- Naming or behavior changes to author-facing symbols (`world.query`, `flicker`, `pulse`, `colorShift`, `sweep`, `timeline`, `sequence`, `LightEntity`, `EasingCurve`).
- Changes to the prelude-bundling mechanism for QuickJS (`scripts-build --prelude`).
- A general per-component-type plugin loader for Luau prelude files. The scope is "load one more file" and reuse the existing `include_str!` + destructure pattern.
- `data_script.{ts,luau}` reorganization. It is context-lifecycle vocabulary (describes the `registerLevelManifest` return shape), not per-entity-type vocabulary, and stays at `sdk/lib/data_script.{ts,luau}`.

## Acceptance criteria

- [ ] `sdk/lib/entities/lights.ts` and `sdk/lib/entities/lights.luau` exist; `sdk/lib/light_animation.{ts,luau}` no longer exist.
- [ ] `sdk/lib/world.ts` and `sdk/lib/world.luau` contain only the generic `world.query` wrapper plus the typed dispatch hook for light handles. They reference no easing math, no animation builders, and no `flicker`/`pulse`/`colorShift`/`sweep` definitions. Importing `LightEntity` (type) and `wrapLightEntity` (function) from `./entities/lights` is required for the `component === "light"` branch and is not a violation; what is prohibited is defining or inlining easing math, animation builders, or vocabulary helpers in `world.{ts,luau}` itself.
- [ ] `sdk/lib/index.ts` re-exports the light entity symbols — `EasingCurve`, `LightEntity`, `flicker`, `pulse`, `colorShift`, `sweep` — from `./entities/lights`; re-exports keyframe utilities — `Keyframe`, `timeline`, `sequence` — from `./util/keyframes`; re-exports emitter symbols from `./entities/emitters` (unchanged). `world` and `World` continue to come from `./world`. `EntityForComponent` remains the dispatch type.
- [ ] Author scripts importing any of the moved symbols by bare specifier (`import { flicker } from "postretro"`) continue to compile via `scripts-build` and run without changes.
- [ ] Author scripts that import the type re-export by sub-path (`import type { LightEntity } from "postretro/entities/lights"`) compile.
- [ ] The committed `sdk/lib/prelude.js` and `sdk/types/postretro.d.{ts,luau}` regenerate cleanly to identical content under both `cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js` and `cargo run -p postretro --bin gen-script-types`. The drift-detection test in `cargo test -p postretro` passes.
- [ ] Luau scripts continue to see `world`, `flicker`, `pulse`, `colorShift`, `sweep`, `timeline`, `sequence`, `registerReaction`, `registerEntities` as bare globals with unchanged behavior. `wrapLightEntity` is not a bare global — it is an internal function used during `world.luau` evaluation and nil'd out before the sandbox freezes. The `LightEntityHandle` type (Luau handle, distinct from the `LightEntity` generated snapshot type) is defined in `entities/lights.luau` and returned by `world:query` when `component = "light"`.
- [ ] `cargo test -p postretro` passes — including the existing handle-method, animation-builder, and prelude-evaluation tests.
- [ ] Existing scripts under `content/` (modder examples, test maps) load without source edits and behave identically. The hallway-pulse doc-comment example in `sdk/lib/entities/lights.{ts,luau}` is valid TypeScript/Luau against the new layout and compiles (TS via `scripts-build`; Luau via `luau --typecheck`).
- [ ] `context/lib/scripting.md` mentions both `entities/emitters` and `entities/lights` as instances of the per-entity-type vocabulary convention.

## Tasks

### Task 1: Create `sdk/lib/util/keyframes.{ts,luau}`

Move `Keyframe`, `timeline`, and `sequence` out of `sdk/lib/light_animation.{ts,luau}` into new `sdk/lib/util/keyframes.{ts,luau}` files. The keyframe primitives are structurally generic (validation of `[absolute_ms, ...value]` arrays) and have no light-specific content. The TS file exports `Keyframe`, `timeline`, `sequence`; the Luau file returns a table with `timeline` and `sequence` fields (Luau types are not exported as values but the `Keyframe` type alias lives at the top of the file). Mirror doc comments from the source.

### Task 2: Create `sdk/lib/entities/lights.{ts,luau}`

Move into the new files: the light-vocabulary helpers from `sdk/lib/light_animation.{ts,luau}` (`flicker`, `pulse`, `colorShift`, `sweep`); plus the light-only pieces of `sdk/lib/world.{ts,luau}` (`EasingCurve`, the `LightEntity` interface, `wrapLightEntity`, `readLightComponent`, `idDebug`, the `EASE_SAMPLES` constant, `resolveEasing`, `easeAt`, `buildIntensityAnimation`, `buildColorAnimation`). Import `Keyframe` from `../util/keyframes` for any usage (currently none — the type was in `light_animation.ts` but not consumed by `lights.ts` directly). Keep doc comments and the canonical hallway-pulse example. Export `wrapLightEntity` so `world.{ts,luau}` can call it from the `component === "light"` branch. Mirror the file structure and section comments used in `entities/emitters.{ts,luau}`.

Naming note (TypeScript): `world.ts` already resolves the naming collision between the generated `LightEntity` type (from the `postretro` bare specifier) and the local `LightEntity` interface by importing the generated type as `import type { LightEntity as GeneratedLightEntity } from "postretro"`. When moving `LightEntity` into `entities/lights.ts`, adopt this same `as GeneratedLightEntity` alias for the generated type import — do not invent a different alias. The local interface keeps the unqualified name `LightEntity`.

Naming note (Luau): the Luau counterpart in `world.luau` uses `LightEntityHandle` for the handle type (not `LightEntity`). The generated snapshot type is `LightEntity` (ambiently provided by `postretro.d.luau`). There is an intentional asymmetry: TS calls the handle `LightEntity` (extending the generated type) while Luau calls the handle `LightEntityHandle` (a separate table type referencing `LightEntity` for the snapshot parameter). `entities/lights.luau` must preserve this Luau naming: define the handle type as `LightEntityHandle`, take `LightEntity` (snapshot) as the input to `wrapLightEntity`, and return `LightEntityHandle`.

### Task 3: Reduce `sdk/lib/world.{ts,luau}` to the generic query

Strip the light-specific imports, types, and helper functions. The file ends up containing only: `EntityForComponent<T>` (dispatch type), the `World` interface, and the `world` singleton with `query`. The `component === "light"` branch imports `wrapLightEntity` (and the `LightEntity` type) from `./entities/lights`. The non-light branch is unchanged. Remove the canonical hallway example from `world` — it now lives in `entities/lights`.

### Task 4: Update `sdk/lib/index.ts` re-exports

Re-source light symbols from `./entities/lights`; re-source keyframe utilities from `./util/keyframes`. Keep `world` and `World` sourced from `./world`. Delete the `./light_animation` re-export block.

### Task 5: Rewire Luau prelude in `luau.rs`

Replace `LIGHT_ANIMATION_LUAU_SRC` + `LIGHT_ANIMATION_FIELDS` with two new entries:
- `LIGHTS_LUAU_SRC` → `include_str!("../../../../sdk/lib/entities/lights.luau")` with fields `LIGHTS_LUAU_FIELDS = ["flicker", "pulse", "colorShift", "sweep"]`. `wrapLightEntity` is intentionally NOT in this list — it is an internal function consumed by `world.luau`, not a bare global for author scripts.
- `KEYFRAMES_LUAU_SRC` → `include_str!("../../../../sdk/lib/util/keyframes.luau")` with fields `timeline`, `sequence`.

Also add the missing emitter prelude wiring: `EMITTERS_LUAU_SRC` → `include_str!("../../../../sdk/lib/entities/emitters.luau")` with fields `emitter`, `smokeEmitter`, `sparkEmitter`, `dustEmitter`. (`SpinAnimation`, `BillboardEmitter`, `EmitterProps`, and `ComponentDescriptor` are `export type` only and serve luau-lsp completions; they must NOT become Lua globals.)

Evaluate order in `evaluate_prelude`: evaluate `lights.luau` first, extract `wrapLightEntity` from the returned table, and set it as a Lua global (`lua.globals().set("wrapLightEntity", value)`) before evaluating `world.luau`. After `world.luau` is evaluated and the `world` global is installed, nil out `wrapLightEntity` (`lua.globals().set("wrapLightEntity", mlua::Value::Nil)`) so it does not leak into author scripts. This reuses the existing `globals().set` pattern already used for every other prelude field — no new infrastructure required. `world.luau` simply calls the bare-name `wrapLightEntity` global during its `query` body; the nil-out happens after the module has been evaluated and captured the function reference in its closure. Update all `set_name` strings and error messages to name the new source paths.

### Task 6: Delete old files and regenerate artifacts

Remove `sdk/lib/light_animation.ts` and `sdk/lib/light_animation.luau`. Run the prelude regeneration command (`cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js`) and the type-definition generator (`cargo run -p postretro --bin gen-script-types`). Confirm the drift-detection test passes (`cargo test -p postretro`).

### Task 7: Update prose references

Find every occurrence of `world.ts`, `world.luau`, `light_animation.ts`, `light_animation.luau`, `sdk/lib/world`, `sdk/lib/light_animation` in:
- `crates/postretro/src/scripting/typedef.rs` (comments and the embedded `.d.ts` / `.d.luau` source-of-truth header)
- `crates/postretro/src/scripting/primitives.rs`, `primitives_light.rs`, `components/light.rs`
- Doc comments inside the SDK files themselves
- `context/lib/scripting.md` (§7, §11)
- `docs/scripting-reference.md` — references `sdk/light_animation.ts`, `require("sdk/light_animation")`, and the old `LightEntity` path; update import paths and module references to reflect the new layout

Note: `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` are generated artifacts — they are regenerated in Task 6 by `gen-script-types` and do not need a separate manual prose update here. Do not hand-edit them.

Replace with the new paths. Preserve meaning — these comments are correct in spirit; only the filenames change. Also add a note to §7 documenting the `util/keyframes` module and the fact that `entities/emitters.luau` is now wired into the Luau prelude.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — create `util/keyframes` and `entities/lights`; each is a new file with no dependencies on each other.
**Phase 2 (concurrent):** Task 3, Task 4, Task 5 — update consumers (`world.{ts,luau}`, `index.ts`, Luau prelude wiring); each touches different files and depends only on Phase 1 output.
**Phase 3 (sequential):** Task 6 — deletes old files and regenerates committed artifacts; must run after all consumers are updated.
**Phase 4 (concurrent):** Task 7 — prose updates; no code dependencies.

## Rough sketch

Result layout:

```
sdk/lib/
  index.ts                 // re-exports — world + entities/{emitters,lights} + util/keyframes
  world.ts                 // generic world.query + EntityForComponent dispatch
  world.luau               // ditto
  data_script.{ts,luau}    // unchanged
  prelude.js               // regenerated
  entities/
    emitters.{ts,luau}     // unchanged (Luau now wired into prelude)
    lights.{ts,luau}       // NEW — LightEntity handle, easing, animation builders, vocabulary helpers
  util/
    keyframes.{ts,luau}    // NEW — Keyframe type, timeline(), sequence()
```

Mental model: `world.{ts,luau}` is the "router" — it dispatches on the `component` literal and delegates to the right handle-wrapper. Each `entities/<name>.{ts,luau}` file owns the per-type handle, vocabulary, and presets. `util/` is shared infrastructure with no entity-type ownership; `data_script.{ts,luau}` is context-lifecycle vocabulary and lives at the top level.

Notable Luau subtlety: `world.luau` today defines `wrapLightEntity` inline. After the move, `lights.luau` owns it. The mechanism in `evaluate_prelude`: evaluate `lights.luau` first, then extract `wrapLightEntity` from the returned table and install it as a temporary Lua global via `lua.globals().set("wrapLightEntity", value)`. Evaluate `world.luau` next — it references `wrapLightEntity` by bare name; because the function is captured in the `world.query` closure, the nil-out that follows doesn't affect the closure. After `world.luau` returns and the `world` global is installed, nil out `wrapLightEntity` with `lua.globals().set("wrapLightEntity", mlua::Value::Nil)` before `lua.sandbox(true)` runs. This approach adds zero new infrastructure — it is the same `globals().set` calls already used for every other prelude field. Author scripts never see `wrapLightEntity` as a global because it is nil'd before the sandbox freezes `_G`.

## Open questions

None unresolved. Type-only symbols (`BillboardEmitter`, `ComponentDescriptor`, `SpinAnimation`, `EasingCurve`, `Keyframe`) are `export type` in both TS and Luau and serve luau-lsp / IDE completions only — they are not promoted to globals in either runtime. Only functions and values become bare globals.
