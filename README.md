```text
      ██████╗                           ██████╗ 
     ██╔══██╗ ██████╗ ███████╗████████╗██╔══██╗███████╗████████╗██████╗  ██████╗ 
    ██║  ██║██╔═══██╗██╔════╝╚══██╔══╝██║  ██║██╔════╝╚══██╔══╝██╔══██╗██╔═══██╗
   ██║  ██║██║   ██║███████╗   ██║   ██║  ██║█████╗     ██║   ██████╔╝██║   ██║
  ██████╔╝██║   ██║╚════██║   ██║   ██████╔╝██╔══╝     ██║   ██╔══██╗██║   ██║
 ██║     ╚██████╔╝███████║   ██║   ██╔══██╗███████╗   ██║   ██║  ██║╚██████╔╝
╚═╝      ╚═════╝ ╚══════╝   ╚═╝   ╚═╝  ╚═╝╚══════╝   ╚═╝   ╚═╝  ╚═╝ ╚═════╝ 

                            -=[ BOOMER SHOOTER ENGINE ]=-
```

Postretro is an experiment: can you build a Doom/Quake-style FPS engine in Rust that genuinely looks and feels retro, loads quick and snappy like old games used to, runs smooth as butter, but embellishes the past with some rendering techniques that were impossible back then but are trivial now?

The visual target is something like Prodeus — chunky pixels, billboard sprites, baked lighting, cyberpunk atmosphere. But the goal is to earn that look through genuinely old-school rendering techniques. No modern deferred pipeline underneath with a retro filter slapped on top.

It's early days. Right now Postretro loads BSP levels, flies through them in wireframe, and culls non-visible areas using portal-based PVS. Everything else is ahead of us — and that's kind of the fun part.

## Planned Milestones

The engine is being built in phases, each of which produces something you can actually see and test. Here's the rough shape of the road ahead:

- **Phases 1** ✅ — BSP and PRL level loading, portal-based BSP visibility with frustum clipping at runtime, free-fly camera, wireframe rendering, custom PRL level compiler
- **Phase 2** ✅ — Fixed-timestep game loop, action-mapped input (keyboard, mouse, gamepad)
- **Phase 3** — Textured world with solid rendering, depth buffer, material system
- **Phase 4** — Light probes: bake lighting offline, sample at runtime for spatially varying illumination — no lightmap atlas, no per-face UVs
- **Phase 5** — Lighting refinement: dynamic point lights, shadow maps for moving lights (intentionally low-res for chunky pixel shadows), emissives, billboard sprites, fog
- **Phase 6** — Post-processing: bloom on emissive surfaces, optional CRT filter, cubemap reflections
- **Phase 7** — Grounded player movement: gravity, collision, slide, step-up, jump
- **Phase 8** — Entity framework: doors, pickups, triggers, game event system
- **Future** — Audio, enemies, weapons, HUD, and whatever else a boomer shooter needs

The full phased plan with acceptance criteria lives in `context/plans/roadmap.md`.

## Tech Stack

- **Language:** Rust (edition 2024, MSRV 1.85)
- **Renderer:** wgpu
- **Windowing:** winit
- **Math:** glam 0.30 (pinned for qbsp compatibility)
- **BSP loading:** qbsp
- **Audio:** kira
- **Gamepad input:** gilrs
- **Level editor:** TrenchBroom
- **Level compiler:** custom (postretro-level-compiler)

## Building

This is a Cargo workspace with multiple crates.

```bash
cargo run -p postretro                                          # engine (debug)
cargo run -p postretro -- assets/maps/test.bsp                 # load a BSP map
cargo run -p postretro -- assets/maps/test.prl                 # load a PRL map
cargo run -p postretro-level-compiler -- input.map -o out.prl  # compile a level
cargo run --release -p postretro                               # optimized build
RUST_LOG=info cargo run -p postretro                           # with logging
```

## Architecture

Five architectural invariants govern the engine:

| Principle | Rule |
|-----------|------|
| Renderer owns GPU | All wgpu calls live in the renderer module. Other subsystems never touch wgpu types. |
| Baked over computed | Lighting and light probes baked offline by the level compiler. Dynamic lights supplement, not replace. |
| Subsystem boundaries | Renderer, audio, input, game logic are distinct modules with explicit contracts. |
| Frame ordering | Input → Game logic → Audio → Render → Present. |
| No `unsafe` | If `unsafe` appears necessary, stop and consult the project owner. |

## Project Documentation

Postretro separates **durable knowledge** from **ephemeral work artifacts**. The code is the source of truth for implementation details. Documentation outside the codebase that describes specific code decisions — function signatures, struct layouts, algorithm choices — becomes stale the moment the code changes. The more detail a document carries about code, the faster it drifts.

This drives a deliberate split:

- **Durable knowledge** (`context/lib/`) captures what survives refactoring: design principles, subsystem boundaries, contracts, pipeline topology. These change rarely and are worth maintaining, because they provide context to agents at the start of every agent lifecycle.
- **Ephemeral artifacts** (`context/plans/`) carry the implementation detail — task breakdowns, acceptance criteria, specific code decisions. They're consumed during development and cleaned up after, avoiding long-lived documents that drift from the source of truth in the codebase.

AI agents start from `context/lib/index.md`, which routes them to the minimum docs needed for a given task.

### Context Folder Structure

```
context/
  lib/                  # Durable architectural knowledge (the "library")
    index.md            # Entry point for AI agents
    development_guide.md
    testing_guide.md
    context_style_guide.md
    rendering_pipeline.md
    audio.md
    build_pipeline.md
    entity_model.md
    input.md
    resource_management.md

  plans/                # Work tracking (ephemeral by design)
    drafts/             # Specs being written, not yet reviewed
    ready/              # Reviewed specs, queued for implementation
    in-progress/        # Actively being worked on
    done/               # Recently completed (max 15, older plans pruned)

  reference/            # External reference material (often historical)
```

### Decision Lifecycle

```
               idea
                │
                ▼
               spec
      (implementation details)
            (ephemeral)
                │
                │─────────────────┐
                │                 │
                ▼                 │
           context/lib/           │
      (conceptual summaries)      │
            (durable)             │
                                  ▼
                            code + comments
                              (durable)
```

#### 1. Plan

A new feature starts as a spec in `context/plans/drafts/`. The spec defines acceptance criteria, task sequencing, and which context library files need updates.

#### 2. Finalize (drafts/ → ready/)

At the end of drafting, durable architectural decisions get written into `context/lib/` — new subsystem boundaries, contracts, pipeline topology, design principles. This happens before the plan moves to `ready/`, so accurate context is in place before any code is written. Once reviewed and approved, the plan moves to `ready/`, the backlog of work queued for execution.

#### 3. Execute (ready/ → in-progress/)

Implementation proceeds according to the plan's sequencing (serial or parallel phases), referencing `context/lib/` for architectural guidance. Code comments capturing implementation-level "why" decisions are written during development, not as a separate step.

#### 4. Review and Verify

Implementation is checked against acceptance criteria, architectural constraints, and coding conventions. A quality gate runs fmt, clippy, and tests.

#### 5. Distill (in-progress/ → done/)

Revise `context/lib/` to address any context drift that emerged during execution — assumptions that proved wrong, boundaries that shifted, contracts that evolved. Once the context library is up to date, move the completed plan to `done/`. The `done/` folder keeps recently completed plans accessible for reference; old plans are periodically pruned to keep no more than 15.

The guiding principle: context files describe what survives refactoring. If a sentence would break when a module is reorganized or a function is renamed, it belongs in a task spec or code comment — not in `context/lib/`.

### Skills (.claude/skills/)

The lifecycle is supported by Claude Code skills:

| Skill | Role |
|-------|------|
| `plan` | Creates feature specs with task breakdown, sequencing, and acceptance criteria |
| `orchestrate` | Coordinates plan execution — spawns agents, tracks progress, moves plans through stages |
| `code-review` | Reviews implementations against specs, architecture, and conventions |
| `preflight` | Pre-commit quality gate: fmt, clippy, test |
| `create-skill` | Builds new skills for the project |

## Non-Goals

- General-purpose game engine
- ECS architecture
- Deferred rendering
- Extending or forking ericw-tools
- Runtime BSP compilation
- Multiplayer / networking

## License

MIT
