# Descriptor Hot Reload

## Goal

Support dev-mode hot reload for authored entity descriptors without pausing gameplay while scripts reprocess.

Hot reload reruns the same mod-init authoring path used at startup: scripts produce JSON-like descriptor objects, the engine converts them into Rust-owned manifest data, then the VM is dropped. Scripts remain declarative. Rust remains the only runtime executor.

Gameplay keeps running on the last committed descriptor snapshot until a newer staged snapshot validates and commits.

## Scope

### In scope

- Build mod-init descriptor manifests on a serialized debug worker lane.
- Track which source files affect the active mod-init manifest.
- Re-run mod init only when a changed file is part of the active dependency set.
- Rebuild the TypeScript start-script bundle when any bundled dependency changes.
- Track Luau files loaded through mod-init `require`.
- Stage a full descriptor manifest before touching `DataRegistry` or live components.
- Commit descriptor registry, dependency manifest, and live refresh together.
- Refresh descriptor-authored runtime components through component policies.
- Keep release hot reload as a no-op. Existing absent-start-script semantics stay unchanged: debug no-op, release error.

### Out of scope

- Runtime mod hot-swap.
- Live script callbacks or persistent VMs.
- Script-driven gameplay ticks.
- Dependency-aware reload for level data scripts.
- Auto-discovery of domain scripts not imported or required by the start script.
- Re-spawning map placements from changed descriptors.
- Replacing the TypeScript sidecar architecture.
- Changes to the existing level-load worker or level data-script execution.
- Enemy AI runtime execution.
- Runtime stat modifier storage or APIs, including future augments. Hot reload still treats descriptor fields as authored baselines so future modifiers can recompute from refreshed base values.

## Terms

| Term | Meaning |
|---|---|
| Staged manifest build | Worker job that compiles scripts, runs `setupMod()`, collects dependencies, and returns Rust-owned manifest data. |
| Manifest snapshot | Complete descriptor manifest produced by one successful mod-init run. Not a patch stream. |
| Manifest commit | Main-thread step that validates and installs a staged snapshot plus its dependency set and live refresh plan. |
| Authored baseline | Value declared by the descriptor manifest. Hot reload may replace it. |
| Map override | Spawn-time override from map KVPs. Hot reload preserves it. |
| Base value | Authored baseline after any map override is applied for a live entity. |
| Runtime state | Transient state such as cooldown, trigger edge, velocity, animation phase, burst progress, and particle lifetime. Hot reload preserves it when compatible. |
| Effective value | Value gameplay reads after the base value is formed. Runtime state is preserved beside the value, not folded into the descriptor baseline. Future stat modifiers compose over the refreshed base value. |

## Decisions

### Dev-Mode Authoring Contract

Hot reload is a dev-mode authoring pipeline. It does not make scripts live.

The staged build uses the same semantic path as startup:

1. Resolve the active start script.
2. Compile TypeScript when needed.
3. Create a short-lived QuickJS or Luau authoring VM.
4. Evaluate the start script and call `setupMod()`.
5. Extract JSON-like manifest data from script values.
6. Convert the manifest into Rust-owned descriptor values through the existing descriptor validation path.
7. Drop the VM.

No script VM persists after the build. No VM drives gameplay. No script object, `ScriptCtx`, `Rc<RefCell<_>>`, renderer state, audio state, input state, or entity registry crosses the worker boundary.

The worker returns only `Send` data:

- Manifest name.
- Entity descriptors.
- Dependency paths.
- Diagnostics.
- Job generation.

Descriptor helpers exposed in the worker VM are pure authoring helpers. They build or validate manifest data. They do not mutate engine registries or capture live engine state. `setupMod()` remains the only mod-init output channel.

The existing gameplay primitive installers capture `ScriptCtx` and are not worker-safe. The staged authoring runtime must not install those live primitive closures. Add an authoring-only install path, or bypass primitive installation entirely when `setupMod()` only needs descriptor helpers.

### Serialized Manifest Build Lane

Debug hot reload uses one manifest-build lane. VM execution is serialized. If file changes arrive while a job is running, keep only the latest pending generation; intermediate pending generations may be coalesced before they start.

The lane may run off the main thread because each job owns its short-lived VM and returns only Rust-owned data. Main-thread startup may use the same staging path and block before gameplay starts.

The main thread never blocks waiting for a reload job during gameplay. It polls completed jobs once per frame. If a job fails, the previous committed snapshot and live state remain active.

Poll and commit at a frame boundary after Present and before the next frame's Input stage. A committed snapshot becomes visible to game logic on the next frame. This preserves frame order: Input -> Game logic -> Audio -> Render -> Present.

### Job Generations

Each reload request receives a generation number. Latest requested source state wins.

Once generation `N + 1` is requested, any completed result from generation `N` or older is stale and cannot commit, even if the newer generation later fails. A failed latest generation preserves the last committed snapshot and logs diagnostics.

Only the latest requested generation may commit, and only if it succeeds.

### Dependency Identity

The active dependency set stores normalized canonical real paths. Watcher paths are normalized the same way before comparison. Mod-relative paths exist only for diagnostics and logs.

If a watcher event names a path that no longer exists, normalize it as:

1. Canonicalize the nearest existing parent directory.
2. Append the missing file name segments.
3. Normalize separators before comparison.

Rename events classify both old and new paths. A rename triggers a staged build when either path belongs to the active dependency set.

The entry start-script path is always part of the dependency set:

- TypeScript: `start-script.ts`.
- JavaScript-only mods: `start-script.js`.
- Luau: `start-script.luau`.

Active entry resolution:

| Files present | Active entry |
|---|---|
| `start-script.ts` only | TypeScript entry. Compile to `start-script.js`; track `start-script.ts` and bundled TypeScript dependencies. |
| `start-script.ts` and generated `start-script.js` | TypeScript entry. Treat `start-script.js` as compiler output, not a source dependency. |
| `start-script.js` only | JavaScript-only entry. Track `start-script.js` only. |
| `start-script.luau` only | Luau entry. Track `start-script.luau` and resolved Luau requires. |
| TypeScript/JavaScript and Luau entries together | Error, preserving existing mixed-runtime start-script behavior. |
| No entry in debug | No-op manifest snapshot with watched candidate entry paths. Creating `start-script.ts`, `start-script.js`, or `start-script.luau` later starts a staged build. |
| No entry in release | Error, preserving existing release behavior. |

Edits to inactive entries do not trigger a manifest build, except creating an entry from the debug absent-entry state.

Only the mod-init entry point contributes to this set: `start-script.{ts,js,luau}` and its TypeScript imports or Luau requires. Level data-script `require` calls are excluded.

JavaScript-only mods are entry-only for this plan. Editing `start-script.js` can trigger a staged build. Imported JavaScript dependency discovery is out of scope until the project adds a JavaScript dependency scanner or module system.

Committed dependencies must stay under the active mod root. A TypeScript import or Luau require that resolves outside the mod root is a staged-build failure. Watcher registration expands to every committed dependency path under the mod root, while keeping the existing mod-root and script-tree watches for entry creation and broad edit detection.

### TypeScript Sidecar Contract

`scripts-build` remains the only TypeScript compiler. Add an engine-facing dependency mode:

```text
scripts-build --in <entry.ts> --out <output.js> --dep-json
```

The command writes the bundled JavaScript to `--out` as before. Stdout contains exactly one JSON object and no human text:

```json
{
  "entry": "/canonical/path/start-script.ts",
  "output": "/canonical/path/start-script.js",
  "dependencies": ["/canonical/path/start-script.ts"]
}
```

Fields:

| Field | Meaning |
|---|---|
| `entry` | Canonical real path of the entry source. |
| `output` | Canonical real path of the generated JavaScript bundle. |
| `dependencies` | Canonical real paths of every source file loaded by the bundler, including the entry. |

All paths are absolute canonical real paths after symlink resolution. `dependencies` entries are unique and sorted lexicographically for stable diagnostics. The engine still treats dependency membership as order-insensitive.

Human diagnostics go to stderr. Compiler failure returns a non-zero exit code. Success with malformed JSON, extra stdout text, or a missing field is a staged-build failure.

### Snapshot Commit

Reload commits are transactional.

The worker returns a complete manifest snapshot. The main thread treats it as the next authored descriptor set for the active mod-init manifest, not as a list of descriptor upserts.

Commit order:

1. Receive latest successful staged result.
2. Validate the Rust-owned manifest snapshot.
3. Build the next descriptor registry snapshot.
4. Build the next dependency set.
5. Compute live refresh and removal plans from the old snapshot, new snapshot, and entity provenance.
6. Commit `DataRegistry`, dependency set, and live component plan together.

Failure before step 6 leaves the previous committed registry, dependency set, and live component state active.

Descriptor removal is real. If a descriptor existed in the previous committed snapshot and is absent from the new snapshot, the new `DataRegistry` no longer contains it. Live entities spawned from that descriptor are reconciled by provenance rules; they are not respawned.

If a live entity's source descriptor is removed entirely, remove every descriptor-owned component recorded in provenance. Preserve non-descriptor-owned components and the entity shell. If no descriptor-owned component can be proven, preserve the entity and log a diagnostic.

Use registry-snapshot plus refresh-plan construction as the transaction boundary. Build the next registry and refresh plan first, then install them together.

### Authored Baselines And Runtime State

Descriptor-authored fields become authored baselines, not the only live value.

Default precedence for descriptor fields that support map overrides:

1. Authored baseline.
2. Map override.

Map overrides are absolute replacements for authored baselines. Runtime state is preserved separately by each component policy.

No runtime stat modifier layer exists today. Future modifier systems, such as augments, must compose over the refreshed base value instead of overwriting it. For fields with map overrides, the override is the live entity's base value. This plan does not implement modifier storage, modifier APIs, or a generic `LayeredValue<T>` abstraction.

### Descriptor Provenance

Live descriptor-authored entities carry explicit provenance in the entity registry. Provenance records:

- Source descriptor canonical name.
- Which components were descriptor-owned at spawn.
- Which fields had map overrides.
- Which spawn path created the entity.

Use a live entity component in the central component registry for provenance. It belongs to live entities, not `DataRegistry`. `DataRegistry` remains descriptor storage. Avoid an `App` side table unless a migration step needs it temporarily.

Provenance is required before safe live refresh, descriptor removal, or component removal.

### Live Refresh Policy

Hot reload reconciles existing descriptor-authored entities. It does not respawn map placements.

Compatible means:

- Provenance still matches the descriptor.
- The live component still exists where the new descriptor declares it.
- Map overrides can be reapplied.
- The refreshed component validates.
- The component policy accepts preserving the current runtime state.

If compatibility fails, keep the previous live component and log a diagnostic. If the refreshed descriptor no longer declares a descriptor-owned component, remove it when provenance proves ownership. Do not remove components without provenance; log a diagnostic instead.

Refresh planners own component-specific compatibility predicates. Initial predicates:

| Component | Incompatible when |
|---|---|
| `weapon` | Refreshed descriptor validation fails. |
| `movement` | Refreshed descriptor validation fails, capsule config is invalid for the current pawn transform, or preserved jump/air state exceeds the refreshed limits. |
| `light` | Refreshed descriptor validation fails. Descriptor-spawned lights remain dynamic even when authored or overridden `is_dynamic` is false. |
| `emitter` | Refreshed descriptor validation fails, sprite resource identity changes in a way live particles cannot reference, or preserved burst/animation state no longer maps onto the refreshed emitter mode. |

### Component Policies

| Descriptor field/component | Refresh policy |
|---|---|
| `weapon` | Update authored stats from the refreshed descriptor. Preserve cooldown and trigger-edge state. No runtime stat modifier layer exists today. Future modifiers must compose over the refreshed base value. Remove stale descriptor-owned weapon components. |
| `movement` | Update authored movement tuning while Rust remains the sole executor. Preserve velocity, grounded state, air-jump state, and air-tick state when the refreshed config validates. Remove stale descriptor-owned movement components. |
| `light` | Update authored intensity, color, and range. Preserve compatible animation state. Descriptor-spawned lights remain runtime dynamic; hot reload must not set the live `is_dynamic` field to `false`. Keep PRL lights out of descriptor refresh. Remove stale descriptor-owned light components. |
| `emitter` | Update authored emitter fields. Preserve current burst progress, animation state, and live particles where compatible. Preserve map overrides. Remove stale descriptor-owned emitter components. |
| `defaultWeapon` | A changed default affects future spawns only. Never switch the current weapon. Changes to the currently wielded weapon descriptor may refresh that weapon's authored stats. |

Movement descriptors are authoring data. They may expose player walk speed, acceleration, jump behavior, capsule config, and related physics tuning. Scripts do not observe or drive the movement tick.

Level/world gravity reload is out of scope for this plan. Dependency-aware reload for level data scripts remains out of scope.

Current map override keys are limited to `initial_intensity`, `initial_range`, `initial_is_dynamic`, and `initial_color` for light descriptors; and `initial_rate`, `initial_spread`, `initial_lifetime`, `initial_buoyancy`, `initial_drag`, `initial_spin_rate`, `initial_burst`, `initial_sprite`, `initial_color`, and `initial_velocity` for emitter descriptors. Other descriptor tuning remains descriptor-owned.

`initial_is_dynamic` remains parsed for compatibility with existing map override behavior and diagnostics. Descriptor-spawned live lights are still forced dynamic because baked indirect lighting is not supported for descriptor-spawned lights.

## Boundary Inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Entity descriptor | `EntityTypeDescriptor` | `canonicalName`, `defaultWeapon`, `components` | `EntityTypeDescriptor` | table with same keys | `classname` matches `canonicalName` |
| Weapon descriptor | `WeaponDescriptor` | `weapon` under `components` | `WeaponDescriptor` | table with same keys | n/a |
| Movement descriptor | `PlayerMovementDescriptor` | `movement` under `components` | `PlayerMovementDescriptor` | table with same keys | n/a |
| Light descriptor | `LightDescriptor` | `light` under `components` | `LightDescriptor` | table with same keys | `initial_*` overrides on placements |
| Emitter descriptor/runtime component | `BillboardEmitterComponent` | `emitter` under `components` | `BillboardEmitterComponent` | table with same keys | `initial_*` overrides on placements |
| Mod manifest | `ModManifestResult` | `name`, `entities` | `setupMod()` return | `setupMod()` return table | n/a |
| Dependency manifest | New runtime-owned type | internal only | n/a | n/a | n/a |
| Scripts-build dependency report | n/a | `entry`, `output`, `dependencies` | stdout JSON from `--dep-json` | n/a | n/a |
| Descriptor provenance | New live entity component | internal only | n/a | n/a | n/a |

## Current Entry Points

- `ScriptWatcher::spawn` watches `<mod>/scripts/` recursively and the mod root non-recursively.
- `ReloadKind` currently distinguishes `Scripts` from `ModInit`, but the frame loop treats both as mod-init reloads.
- `ScriptRuntime::run_mod_init` runs `start-script.js` or `start-script.luau`.
- `compile_start_script` always rebuilds `start-script.js` from `start-script.ts` in debug builds.
- `scripts-build` bundles TypeScript through `bundle_entry`.
- Luau `require` is installed by `install_require_resolver`.
- `DataRegistry::upsert_entity_type` updates descriptor storage keyed by `canonical_name`.
- `App::refresh_active_wieldable_from_descriptors` is the only live descriptor refresh hook today.

## Tasks

### Task 1: Staged Manifest Build Job

Add a staged manifest build path used by debug reload.

The path creates a short-lived authoring VM, runs mod init, extracts JSON-like manifest data, converts it into Rust-owned descriptor values, collects diagnostics, then drops the VM. The returned staged result contains only `Send` data.

The staged authoring VM must not install live gameplay primitive closures that capture `ScriptCtx`. It installs only the authoring surface needed to evaluate `setupMod()` and descriptor helpers.

Gameplay reload jobs run through the serialized manifest-build lane. Startup may use the same staging path and block before gameplay starts.

### Task 2: TypeScript Dependency Reporting

Extend `scripts-build` with `--dep-json`.

The mode writes the bundle to `--out` and emits exactly one dependency JSON object on stdout. Human diagnostics stay on stderr. The bundler reports canonical real paths for the entry, output, and every loaded source file. The engine treats compiler failure, malformed JSON, extra stdout text, or missing fields as a failed staged build.

### Task 3: Luau Require Tracking

Teach the mod-init Luau resolver to record every resolved `require` path during staged manifest builds. Include `start-script.luau` in the dependency set.

Keep data-script `require` calls out of the mod-init dependency set.

### Task 4: Dependency-Aware Watcher Classification

Replace broad `Scripts` reload behavior with changed-path membership checks against the last committed dependency set.

The watcher enqueues changed source paths. The frame loop normalizes each path, handles missing paths and renames as specified above, checks dependency membership, and starts a new manifest build only when the changed path affects the active mod-init manifest.

JavaScript-only start scripts are entry-only. Editing `start-script.js` triggers a staged build; editing other JavaScript files does not unless later work adds JS dependency discovery.

### Task 5: Provenance And Component Refresh Planning

Add descriptor provenance to live descriptor-authored entities. Use it to identify descriptor-owned components, preserved map overrides, spawn paths, and safe component removal.

Add component refresh planners for `weapon`, `movement`, `light`, and `emitter`. Each planner validates the refreshed authored config, decides whether runtime state is compatible, and emits a refresh/remove/keep-old action.

### Task 6: Snapshot Commit

Add a main-thread commit path for staged manifest results.

It validates the staged manifest, builds the next `DataRegistry` snapshot, builds the next dependency set, computes live refresh/removal plans, and commits them together. Stale generations are discarded. Failed builds and failed commits preserve the previous committed state.

Add a replacement-style registry commit path for entity descriptors. Existing `upsert` behavior may remain for startup or tests, but hot reload commits a complete snapshot.

The live refresh plan contains validated component replacements and removals. Apply it while the main thread owns the entity registry. If a target entity no longer exists at commit time, drop that action and log a diagnostic instead of aborting the whole commit.

Live-plan application is otherwise infallible by construction: validation, resource checks, provenance checks, and compatibility decisions happen before commit. Any unexpected apply-time error aborts the commit and preserves the previous descriptor registry and dependency set.

Descriptor removal from the new manifest removes the descriptor from the committed registry and triggers provenance-based live reconciliation.

### Task 7: Tests And Diagnostics

Add regression tests for:

- Worker staged build success.
- Worker staged build failure preserving committed state.
- Stale generation discard under latest-request-wins semantics.
- Pending generation coalescing while a build is running.
- Whole-manifest replacement, including descriptor removal.
- TypeScript dependency-aware reload classification.
- Luau dependency-aware reload classification.
- JavaScript-only entry reload and non-entry no-op behavior.
- Editing `start-script.ts`, `start-script.js`, and `start-script.luau`.
- Unrelated script no-op reloads.
- Atomic rename for TypeScript and Luau dependency files, JavaScript-only `start-script.js`, unrelated files, and start-script files.
- Descriptor refresh preserving weapon runtime state.
- Movement refresh preserving compatible runtime state.
- Light refresh preserving compatible runtime state while keeping descriptor-spawned lights dynamic.
- Emitter refresh preserving compatible runtime state and live particles.
- Descriptor component removal through provenance.
- `cargo test -p postretro`.

Log enough debug detail to explain why a changed path did or did not trigger a manifest build, why a staged build failed, why a generation was discarded, and why a live component did or did not refresh.

## Sequencing

**Phase 1 (sequential):** Task 1 - establishes the staged authoring boundary.
**Phase 2 (concurrent):** Task 2, Task 3 - independent dependency producers.
**Phase 3 (sequential):** Task 4 - consumes the committed dependency manifest.
**Phase 4 (sequential):** Task 5 - creates provenance and live refresh plans.
**Phase 5 (sequential):** Task 6 - commits staged snapshots safely.
**Phase 6 (sequential):** Task 7 - verifies the reload path.

## Acceptance Criteria

- [ ] Gameplay continues with the last committed manifest while a debug reload job compiles and runs mod init.
- [ ] Hot reload uses the script-to-manifest-to-Rust-owned-data path and drops the VM after each staged build.
- [ ] Editing a TypeScript file imported by `start-script.ts` starts a staged manifest build, recompiles `start-script.js`, commits updated descriptors, and refreshes supported live descriptor-authored components.
- [ ] Editing `start-script.ts` starts the same staged reload path.
- [ ] Editing `start-script.js` in a JavaScript-only mod starts the staged reload path.
- [ ] Editing an unrelated `.ts`, `.js`, or `.luau` file under `<mod>/scripts/` does not start a manifest build unless it is in the active dependency set.
- [ ] Editing a Luau file required by `start-script.luau` starts a staged manifest build and refreshes supported live descriptor-authored components.
- [ ] Editing `start-script.luau` starts the same staged reload path.
- [ ] A failed compile, failed mod-init run, invalid dependency report, or failed commit leaves the previous descriptor registry, dependency set, and live component state active.
- [ ] If two reload jobs overlap, only the latest requested generation can commit. Failed latest builds leave the last committed snapshot active.
- [ ] Committing a manifest replaces the descriptor snapshot instead of only upserting descriptors.
- [ ] Descriptor refresh updates authored baselines while preserving map overrides and compatible runtime state.
- [ ] Movement descriptors refresh authored tuning without handing movement execution to scripts.
- [ ] Movement, light, and emitter descriptors refresh without dropping compatible runtime state.
- [ ] Descriptor-spawned lights remain runtime dynamic after refresh.
- [ ] Level/world gravity changes do not reload through this path.
- [ ] Removing a descriptor-owned component removes the live component only when provenance proves descriptor ownership.
- [ ] Watcher edit and atomic-rename tests cover TypeScript and Luau dependency files, unrelated files, and start-script files with expected reload or no-op behavior.
- [ ] `cargo test -p postretro` passes.
