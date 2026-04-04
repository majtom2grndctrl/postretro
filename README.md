# PostRetro

Retro-style FPS engine inspired by Doom and Quake. Cyberpunk aesthetic, baked lightmaps, billboard sprites, modern embellishments (dynamic colored lights, bloom, particles). Near-instant boot, tiny binary.

The visual target is Prodeus's look achieved through genuinely retro technology — not a modern engine with retro filters.

## Tech Stack

- **Language:** Rust (edition 2024, MSRV 1.85)
- **Renderer:** wgpu
- **Windowing:** winit
- **Math:** glam 0.30 (pinned for qbsp compatibility)
- **BSP loading:** qbsp
- **Audio:** kira
- **Gamepad input:** gilrs
- **Level editor:** TrenchBroom + ericw-tools 2.0.0-alpha

## Building

```bash
cargo run                  # debug build
cargo run --release        # optimized build
RUST_LOG=info cargo run    # with logging
```

## Architecture

Five architectural invariants govern the engine:

| Principle | Rule |
|-----------|------|
| Renderer owns GPU | All wgpu calls live in the renderer module. Other subsystems never touch wgpu types. |
| Baked over computed | Lighting, AO, light probes baked offline by ericw-tools. Dynamic lights supplement, not replace. |
| Subsystem boundaries | Renderer, audio, input, game logic are distinct modules with explicit contracts. |
| Frame ordering | Input -> Game logic -> Audio -> Render -> Present. |
| No `unsafe` | If `unsafe` appears necessary, stop and consult the project owner. |

## Project Documentation

PostRetro separates **durable knowledge** from **ephemeral work artifacts**. The code is the source of truth for implementation details. Documentation outside the codebase that describes specific code decisions — function signatures, struct layouts, algorithm choices — becomes stale the moment the code changes. The more detail a document carries about code, the faster it drifts.

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
    initial-prompt.md
    (planned: rendering_pipeline, audio, entity_model, build_pipeline, input, resource_management)

  plans/                # Work tracking (ephemeral by design)
    drafts/             # Specs being written, not yet reviewed
    ready/              # Reviewed specs, queued for implementation
    in-progress/        # Actively being worked on
    done/               # Recently completed (max 15, older plans pruned)

  reference/            # External reference material (often historical)
```

### Decision Lifecycle

```
                    context/lib/
                   (durable knowledge)
                     ^           ^
                     |           |
                update lib    fix drift
                     |           |
drafts/ --> ready/ --+--> in-progress/ --+--> done/
  (plan)  (backlog)        |       ^   (distill)
                           v       |
                      code + comments
                           |
                           v
                      Review & Verify
```

#### 1. Plan

A new feature starts as a spec in `context/plans/drafts/`. The spec defines acceptance criteria, task sequencing, and which context library files need updates.

#### 2. Review (drafts/ -> ready/)

The plan is reviewed while still in `drafts/`. Once approved, it moves to `ready/` — a backlog of plans queued for execution.

#### 3. Update Context Library

When a plan is picked up for execution, its durable architectural decisions are first written into `context/lib/` — new subsystem boundaries, contracts, pipeline topology, design principles. This ensures accurate, up-to-date guidance is in place before code is written.

#### 4. Execute (ready/ -> in-progress/)

Implementation proceeds according to the plan's sequencing (serial or parallel phases), referencing `context/lib/` for architectural guidance. Code comments capturing implementation-level "why" decisions are written during development, not as a separate step.

#### 5. Review and Verify

Implementation is checked against acceptance criteria, architectural constraints, and coding conventions. A quality gate runs fmt, clippy, and tests.

#### 6. Distill (in-progress/ -> done/)

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
