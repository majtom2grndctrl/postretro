# Boot Sequence

> **Read this when:** wiring mod loading, level loading, the mod browser, or any startup-phase work; or when reasoning about where a new piece of init code belongs.
> **Key invariant:** mods own scripts and assets; the engine owns the schedule. Mod code runs only inside phases the engine grants it.
> **Related:** [Architecture Index](./index.md) · [Scripting](./scripting.md) · [Entity Model](./entity_model.md) · [Build Pipeline](./build_pipeline.md)

---

## 1. Folder Structure (planned — paths below are aspirational; most do not exist yet)

```
content/
  base/                          # base game (always present)
    start-script.{ts,luau}       # mod entry point — fixed path at mod root
    actors/                      # enemies, NPCs, any autonomous mobile entity
      <actor-name>/
        _sounds/                 # actor-specific sounds
        <actor-name>.png         # actor-specific textures
        <actor-name>.ts          # schema + reactions
    weapons/
      <weapon-name>/
        _sounds/                 # weapon-specific sounds
        <weapon-name>.png
        <weapon-name>.ts
    levels/
      _textures/                 # shared level-surface textures (walls, floors, sky)
      _sounds/                   # shared level ambient and music
      <level-name>/
        <level-name>.prl         # compiled level
        <level-name>.ts          # level-specific entity definitions; auto-discovered by name
  mods/
    <mod-name>/                  # one folder per mod; same shape as base/
      ...
```

| Concept | Definition |
|---------|-----------|
| Base game | The shipped game in `content/base/`. Loaded as a mod with implicit highest precedence. |
| Mod | A folder under `content/mods/`. Augments or replaces base content. |
| Total conversion | A mod that defines its own UI, menus, and full content set. |
| Level pack | A loosely-defined mod — primarily maps, light scripting. |
| Actor | Any autonomous mobile entity regardless of faction. Faction is a schema field, not a folder. |

The domain folder structure, the fixed `start-script` entry, and the `mods/` directory are **planned**. The folder layout above is aspirational — these paths do not yet exist.

---

## 2. Script Roles

| Role | Context | Lifetime | Discovery |
|------|---------|----------|-----------|
| Start script | Definition (one-shot) | Runs once at mod init | Fixed path: `<mod>/start-script.{ts,luau}` |
| Domain script | Definition (one-shot) | Runs once at mod init via start-script imports | Explicit `import`/`require` from start-script |
| Level script | Definition (one-shot) | Runs once at level load | Auto-discovered: `levels/<name>/<name>.{ts,luau}` |
| UI definition script | Declarative (no VM at runtime) | Parsed once; rendered from data | **Open** — format and load point unspecified |

Start scripts and domain scripts declare entity types as `entities` on the `setupMod()` return value; the engine boot caller drains them into the engine-global `DataRegistry` after `run_mod_init` returns. Level scripts export `setupLevel(ctx)`, which returns per-level reactions; those land in the per-level reaction registry. See `scripting.md` §2.

---

## 3. Boot Sequence (planned full product)

| Phase | Stage | Owner | Status |
|-------|-------|-------|--------|
| 0 | Engine init: wgpu adapter, input, scripting runtimes constructed; SDK preludes installed | Engine | today |
| 1 | Discover mods: scan `content/base/` and `content/mods/*/` for valid manifests | Engine | planned |
| 2 | Mod browser UI: present discovered mods, user selects active set (or skip via CLI / saved selection) | Engine + UI system | planned |
| 3 | Resolve load order: base first, then selected mods in user-specified order | Engine | planned |
| 4 | Per-mod init: for each active mod in order — run `start-script` in a definition VM (module imports resolve domain scripts); fire `modLoad` event | Engine + scripts | planned |
| 5 | Main menu: rendered from UI definitions contributed by active mods. User picks a level (mod-defined level selector / class chooser / etc.) | UI system | planned |
| 6 | Level load (see §4) | Engine | partial today |
| 7 | First game tick: Input → Game logic → Audio → Render → Present | Engine | today |

**Open (D2):** how scripts declare tick order across mods.
**Open (D3):** whether `data/` is a single entry file or a lexicographic multi-file scan.
**Open:** mod manifest format (name, version, dependencies, UI contributions). Required by phase 1.
**Open:** UI system — declarative format, where definitions live (per-mod `ui/` folder?), and how the renderer consumes them.

### 3a. Boot State Machine (today)

Engine startup today runs a three-state progression: pre-window → splash → running.

The engine transitions from pre-window to splash when the OS window and GPU device are ready. From that point, a per-frame splash protocol runs:

- **Frame 0.** Renders a black screen so the OS window appears immediately. After present, decodes the splash image synchronously and uploads it to the GPU. No mod or level work runs yet.
- **Frame 1.** Renders the splash so the user sees it before any user-authored work executes. After present: runs mod init (compiles stale scripts, executes the start-script, drains entity-type descriptors into the global registry); optionally swaps in a mod-supplied splash image; spawns the level-load worker thread. The worker handles PRL parse, texture decode, and UV normalization off the main thread.
- **Frames 2+.** Polls the worker channel each frame. Splash keeps painting while the worker runs. When the worker delivers, the main thread performs GPU upload and level install, then transitions to running.

The purpose of the two-frame delay is causal: pixels reach the user before any mod-supplied or level-load work consumes CPU. All GPU work (texture upload, geometry upload) stays on the main thread; only file I/O, parsing, and decoding run on the worker.

---

## 4. Level Load Sequence

Today:

| Order | Stage |
|-------|-------|
| 1 | PRL parse, texture decode, UV normalize |
| 2 | Geometry and texture upload to GPU |
| 3 | Spatial subsystems initialized from level data: fog volumes registered per leaf; collision world populated from static geometry (separate from BSP) |
| 4 | Built-in classname dispatch: `player_spawn` placements routed to player-spawn logic; `billboard_emitter` placements materialized as engine emitter entities. Lights come from PRL data via the light bridge, not classname dispatch. |
| 5 | Level script runs in a short-lived VM; `setupLevel(ctx)` returns `{reactions}` → per-level reaction registry. Entity types are engine-global and arrive via `setupMod`, not here. |
| 6 | Entity spawn sweep: match map entity list against `DataRegistry`, spawn |
| 7 | `levelLoad` event fired |

Planned change: stage 5 level script sourced from `levels/<name>/<name>.{ts,luau}` (auto-discovered by name convention) instead of bundled in PRL.

**Open (D3):** if data scripts move out of PRL, level launch parameters (chosen by the mod's menu in phase 5) need a delivery channel into the data context.

---

## 5. Lifetimes

| Scope | Cleared on |
|-------|-----------|
| Engine init (preludes, primitive registry) | Process exit |
| Mod init state (start-script effects) | Mod unload / engine restart (planned) |
| `DataRegistry` (entity-type descriptors from `setupMod` return) | Engine-global; survives level unload. Cleared on full reload of mod set. |
| Per-level reaction registry | Level unload |

Hot reload (debug only) triggers recompilation of changed script files; definition-context changes require an engine restart.

---

## 6. Mod Browser (planned)

Phase 2 must run before any mod scripts execute. Constraints:

- Cannot depend on mod-supplied UI definitions (they aren't loaded yet).
- Renders in engine-native UI only (declarative, not VM-driven).
- Output: ordered list of active mod paths handed to phase 3.
- Skippable: CLI flag (`--mods base,foo,bar`) or persisted selection from previous session.

Reachable from main menu (phase 5) for re-selection; triggers a full mod unload + reload cycle.

---

## 7. Non-Goals

- Per-entity script lifecycle callbacks (see `entity_model.md` §9)
- Networked mod sync
- Runtime mod hot-swap mid-level
- Sandboxing mods from each other (mods share the same VM contexts and `DataRegistry` by design)

---

## 8. Boot-Phase Concurrency Model

- **Main thread owns** the winit event loop, wgpu (device, queue, all GPU work), the audio mixer, and all script-VM execution. `ScriptRuntime` and `Renderer` are not `Send`; enforced by the types, not by convention.
- **Worker threads own** file I/O, parsing, and decoding. Outputs must be plain `Send` data — no engine handles, no GPU resources.
- **Handoff** is `mpsc` channels carrying POD. One worker per kicked-off task; no thread pool until measurement demands one.
- **Phases are sequential; intra-phase work is parallel.** Phase N does not advance until its worker outputs are consumed and main-thread follow-up (GPU upload, script run, registry populate) completes.
- **No async runtime.** `std::thread` + `mpsc`.
