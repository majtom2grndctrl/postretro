# Luau SDK Virtual Modules

> **Status:** draft
> **Related:** `context/lib/scripting.md` · `crates/postretro/src/scripting/luau.rs` · `sdk/types/postretro.d.luau`

## Goal

Add engine-owned Luau SDK modules so authors can require stable built-ins such as
`postretro` and `postretro/ui` without files under the mod root. This gives Luau
the same namespace shape TypeScript can express with bare imports, and unblocks a
later UI import cleanup.

## Scope

### In scope

- Exact-match virtual Luau modules resolved by `require(...)`.
- Initial modules: `postretro` and `postretro/ui`.
- Preserve current bare-global SDK behavior during this plan.
- Resolve virtual modules before mod-root files. Mods cannot shadow them.
- Exclude virtual modules from staged hot-reload dependency paths.
- Luau type declarations for the new module tables and literal `require` overloads.

### Out of scope

- Removing bare globals.
- Removing UI exports from the root `postretro` module.
- TypeScript import restructuring.
- General package semantics: upward search, `init.luau`, package manifests, or
  third-party module roots.
- Caching or changing behavior for mod-root file `require`. Existing file modules
  keep current no-cache semantics.

## Acceptance criteria

- [ ] `require("postretro")` returns a table with the current root SDK surface.
- [ ] `require("postretro/ui")` returns a table with the current UI SDK surface.
- [ ] `require("postretro/ui")` works in mod-init and data-script Luau contexts.
- [ ] `require("postretro/ui")` does not attempt to read
      `<mod_root>/postretro/ui.luau`.
- [ ] A mod file named `postretro/ui.luau` cannot shadow the engine module.
- [ ] Staged Luau manifest dependency tracking records authored files only, not
      `postretro` or `postretro/ui`.
- [ ] Existing Luau scripts that use bare globals still run.
- [ ] `sdk/types/postretro.d.luau` exposes typed module tables and `require`
      overloads for `postretro` and `postretro/ui`.

## Tasks

### Task 1: Split Luau prelude and require code

Behavior-preserving split before extension. Move SDK source constants, field
lists, and `evaluate_prelude` out of the oversized Luau subsystem file. Move
`LuauRequireTracker`, `install_require_resolver`, `resolve_require_path`, and
dependency canonicalization into a require-focused sibling module. Keep
`build_lua_state` and `build_lua_state_with_require_tracking` as the construction
entry points. Update tests only for module paths, not behavior.

### Task 2: Add a virtual-module registry

Introduce one per-VM registry owned by the Luau state construction path. The
require resolver captures it. Prelude evaluation populates it before
`sandbox(true)`. A virtual require performs an exact string lookup first and
returns a fresh shallow table of the registered exports. Mutating that returned
table must not mutate globals, the registry, or a later virtual require.

### Task 3: Populate `postretro` and `postretro/ui`

Build the initial module tables from the same evaluated SDK values installed as
globals today. `postretro` mirrors the current flat SDK surface for opt-in
module use. `postretro/ui` contains UI factories, layout helpers, UI tree
helpers, UI state helpers, UI reactions, `getGameState`, and theme helpers.
Later plans may add `getDesignTokens` and remove UI from the root table.

### Task 4: Generate Luau module types

Extend the Luau type generator to emit module table types and literal `require`
overloads. Keep the existing global declarations for now. The overloads must
make `local UI = require("postretro/ui")` type as the UI module table while
unrecognized require strings remain `any` for mod-root files.

### Task 5: Test mod-init, data-script, and staging paths

Cover both short-lived Luau VM paths and staged manifest dependency tracking.
Tests should prove virtual modules bypass filesystem resolution, cannot be
shadowed by mod files, and do not enter hot-reload dependency sets.

## Sequencing

**Phase 1 (sequential):** Task 1 — split oversized files before adding behavior.

**Phase 2 (sequential):** Task 2 — virtual-module registry; Task 3 consumes it.

**Phase 3 (concurrent):** Task 3, Task 4 — runtime modules and typedefs share the
export inventory but do not block each other after Task 2.

**Phase 4 (sequential):** Task 5 — validates runtime and typedef behavior.

## Rough sketch

`build_lua_state_with_require_tracking` should allocate the virtual-module
registry, pass a handle to the require resolver, and pass the same handle to
prelude evaluation. `install_require_resolver` should check the registry before
calling `resolve_require_path`; only file-backed requires pass through
`LuauRequireTracker`.

The export inventory should be declared once per module. Reuse the existing
field slices for UI and non-UI SDK groups where possible, but avoid making the
root cleanup plan edit the resolver itself.

`crates/postretro/src/scripting/luau.rs` and
`crates/postretro/src/scripting/typedef.rs` are already above the split smell
threshold. The first implementation step must reduce the Luau file before adding
module behavior. If the typedef changes would add a large second static block,
split SDK-lib typedef text into a focused helper module in the same change.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Root SDK module | virtual require registry entry | n/a | n/a | `require("postretro")` | n/a |
| UI SDK module | virtual require registry entry | n/a | n/a | `require("postretro/ui")` | n/a |
| Mod-root require | `resolve_require_path` file path | n/a | n/a | `require("./path")` / `require("path")` | n/a |
| Dependency tracking | `LuauRequireTracker` paths | n/a | n/a | file-backed require only | n/a |

## Open questions

None. File-backed `require` caching remains out of scope by design.
