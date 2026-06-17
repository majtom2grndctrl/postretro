# Luau SDK Virtual Modules

> **Status:** in-progress
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
- Virtual SDK modules are available only in Luau contexts that install the
  require resolver. Long-lived definition state still has no `require`.
- Virtual SDK modules are VM-local read-only singletons. Repeated requires return
  the same table; nested namespace tables are read-only too.
- Module tables snapshot evaluated SDK return-table values from an internal
  export inventory. Freezing module tables does not freeze or mutate bare globals.
- Preserve current bare-global SDK behavior during this plan.
- Resolve virtual modules before mod-root files. Mods cannot shadow them.
- Exclude virtual modules from staged hot-reload dependency paths.
- Luau type declarations for the new module tables and literal `require` overloads.
- Data-script Luau VMs keep the same full mod-root `require` resolver as
  mod-init VMs. Virtual modules resolve first.
- `context/lib/scripting.md` is updated with the durable virtual-module
  contract.

### Out of scope

- Removing bare globals.
- Removing UI exports from the root `postretro` module.
- TypeScript import restructuring.
- General package semantics: upward search, `init.luau`, package manifests, or
  third-party module roots.
- Caching or changing behavior for mod-root file `require`. Existing file modules
  keep current no-cache semantics.

## Acceptance criteria

- [ ] `require("postretro")` returns the initial root export inventory listed in
      Task 3.
- [ ] `require("postretro/ui")` returns the initial UI export inventory listed in
      Task 3.
- [ ] Repeated `require("postretro")` and `require("postretro/ui")` calls in one
      VM return the same read-only table. Writes to the module table or nested
      namespace tables fail under `pcall`.
- [ ] `require("postretro")` and `require("postretro/ui")` work in mod-init and
      data-script Luau contexts.
- [ ] `require("postretro")` and `require("postretro/ui")` do not attempt to
      read `<mod_root>/postretro.luau` or `<mod_root>/postretro/ui.luau`.
- [ ] Mod files named `postretro.luau` or `postretro/ui.luau` cannot shadow the
      engine modules.
- [ ] Staged Luau manifest dependency tracking records authored files only, not
      `postretro` or `postretro/ui`.
- [ ] Existing Luau scripts that use bare globals still run.
- [ ] `sdk/types/postretro.d.luau` exposes typed module tables and `require`
      overloads for `postretro` and `postretro/ui`.

## Tasks

### Task 1: Confirm split Luau prelude and require code

Current-state gate before remaining work. SDK source constants, field lists, and
`evaluate_prelude` live in the Luau prelude module. `LuauRequireTracker`,
`install_require_resolver`, `resolve_require_path`, and
`canonical_require_dependency` live in the require-focused sibling module.
`build_lua_state` and `build_lua_state_with_require_tracking` remain the
construction entry points. Virtual modules stay scoped to construction paths
that install the require resolver.

### Task 2: Add a virtual-module registry

Introduce one per-VM registry owned by the Luau state construction path. The
require resolver captures it. Prelude evaluation populates it before
`sandbox(true)`. A virtual require performs an exact string lookup first and
returns the registered read-only module singleton. The registry owns each module
table and any nested namespace tables; callers cannot mutate module exports, the
registry, or a later require result. Freezing module-owned tables must not freeze
or mutate the current bare globals.

### Task 3: Populate `postretro` and `postretro/ui`

Build the initial module tables from evaluated SDK return-table values recorded
in an internal export inventory, not from author-visible globals. The export
inventory is authoritative: it stores export names by SDK source group and reads
the corresponding evaluated values when populating module tables. In this plan,
`postretro` mirrors the current flat SDK surface for opt-in module use.
`postretro/ui` contains UI factories, layout helpers, UI tree helpers, UI state
helpers, UI reactions, `getGameState`, and theme helpers. Nested namespace
tables copied into module tables are separately read-only. Later plans may add
`getDesignTokens` and remove UI from the root table.

Initial runtime export inventory:

| Module | Exports |
|---|---|
| `postretro` | `world`, `runtime`, `getGameState`, `timeline`, `sequence`, `defineReaction`, `defineEntity`, `defineStore`, `emitter`, `smokeEmitter`, `sparkEmitter`, `dustEmitter`, plus every `postretro/ui` export below |
| `postretro/ui` | `Text`, `Panel`, `Image`, `Spacer`, `Button`, `Slider`, `Bar`, `Announce`, `VStack`, `HStack`, `Grid`, `Tree`, `defineUiTree`, `getGameState`, `bindState`, `stateEquals`, `createLocalState`, `ui`, `Switch`, `defineTheme`, `onStateCrossing`, `playSound`, `rumble`, `flashScreen`, `vignette`, `screenShake`, `showDialog`, `openMenu`, `closeDialog`, `openTextEntry`, `KEYBOARD_TREE`, `CLOSE_DIALOG_ACTION`, `EXIT_TO_DESKTOP_ACTION`, `updateState`, `appendText`, `backspaceText`, `clearText` |

Type-only declarations may name more helper types. Runtime module tables contain
values only.

### Task 4: Generate Luau module types

Extend the Luau type generator to emit module table types and literal `require`
overloads from the same export inventory used by Task 3. Keep the existing
global declarations for now. The overloads must make
`local UI = require("postretro/ui")` type as the UI module table while
unrecognized require strings remain `any` for mod-root files.

### Task 5: Test mod-init, data-script, and staging paths

Cover both short-lived Luau VM paths and staged manifest dependency tracking.
Tests should prove virtual modules bypass filesystem resolution, cannot be
shadowed by mod files, and do not enter hot-reload dependency sets. Tests must
also validate runtime/type inventory agreement, root module behavior, read-only
write failure under `pcall`, nested table immutability, bare-global
preservation, and typedef overload output.

### Task 6: Update durable scripting context

Update `context/lib/scripting.md` with the durable virtual SDK module contract:
module IDs and export vocabulary are symmetric with TypeScript, syntax is
language-native, virtual modules resolve before mod files, virtual modules are
VM-local read-only singletons in short-lived require-enabled Luau VMs, and
file-backed require remains no-cache.

## Sequencing

**Phase 1 (sequential):** Task 1 — confirm the split is present before adding
behavior.

**Phase 2 (sequential):** Task 2 — virtual-module registry; Task 3 consumes it.

**Phase 3 (sequential):** Task 3, then Task 4. Runtime modules establish the
shared export inventory and drift guard before typedef generation consumes it.

**Phase 4 (sequential):** Task 5 — validates runtime and typedef behavior.

**Phase 5 (sequential):** Task 6 — durable context update.

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
| Root SDK module | read-only virtual require registry singleton | n/a | n/a | `require("postretro")` | n/a |
| UI SDK module | read-only virtual require registry singleton | n/a | n/a | `require("postretro/ui")` | n/a |
| Mod-root require | `resolve_require_path` file path | n/a | n/a | `require("./path")` / `require("path")` | n/a |
| Dependency tracking | `LuauRequireTracker` paths | n/a | n/a | file-backed require only | n/a |

## Open questions

None. File-backed `require` caching remains out of scope by design.
