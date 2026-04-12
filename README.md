```text
      ██████╗                           ██████╗ 
     ██╔══██╗ ██████╗ ███████╗████████╗██╔══██╗███████╗████████╗██████╗  ██████╗ 
    ██║  ██║██╔═══██╗██╔════╝╚══██╔══╝██║  ██║██╔════╝╚══██╔══╝██╔══██╗██╔═══██╗
   ██████╔╝██║   ██║███████╗   ██║   ██████╔╝█████╗     ██║   ██████╔╝██║   ██║
  ██║     ██║   ██║╚════██║   ██║   ██╔══██╗██╔══╝     ██║   ██╔══██╗██║   ██║
 ██║     ╚██████╔╝███████║   ██║   ██║  ██║███████╗   ██║   ██║  ██║╚██████╔╝
╚═╝      ╚═════╝ ╚══════╝   ╚═╝   ╚═╝  ╚═╝╚══════╝   ╚═╝   ╚═╝  ╚═╝ ╚═════╝ 

                          -=[ BOOMER SHOOTER ENGINE ]=-
```

Postretro is a Quake-style FPS engine in Rust that genuinely looks and feels retro, loads quick and snappy like old games used to, runs smooth as butter, but embellishes the past with some rendering techniques that were impossible back then but are trivial now.

The visual target is something like Prodeus — chunky pixels, billboard sprites, baked lighting, cyberpunk atmosphere. But the goal is to earn that look through low-cost rendering techniques, resulting in a game that looks like you remember but feels better.

It's early days. Right now Postretro loads BSP levels, flies through them with textures, and culls non-visible areas using portal-based PVS. Everything else is ahead of us — and that's kind of the fun part.

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

## Project Documentation

Most projects end up with documentation that quietly lies — a design doc describing a function renamed two months ago, an architecture guide pointing at a module that got split in half. The more specific a document is about code, the faster it rots. That's not a discipline problem you can fix by trying harder; it's structural. The code is where truth lives, and copies of the truth go stale.

So Postretro splits its docs into two layers with very different expectations:

- **Durable knowledge** (`context/lib/`) — the things that stay true even when code shifts underneath them: design principles, subsystem boundaries, data contracts, frame ordering, the reasoning behind the architecture. A renderer rewrite shouldn't invalidate "all wgpu calls live in the renderer module." These docs rarely need maintenance, which is exactly why they're worth maintaining.
- **Ephemeral artifacts** (`context/plans/`) — the working notes of active development: specs, task breakdowns, function names, algorithm choices. Essential while a feature is being built, disposable once it ships. Plans move `drafts/` → `ready/` → `in-progress/` → `done/`, and old ones get pruned.

The litmus test: *if we rewrote this module differently, would the sentence still be true?* Yes → durable. No → it's a plan, or better, a code comment next to the thing it describes.

This matters more for an AI-assisted project than a traditional one. Every agent session starts cold and loads context from scratch. Feed it stale detail and it gives confident answers about a codebase that no longer exists. Feed it a compact, durable picture of how the system is *meant* to fit together, and it can read the live code for the specifics. Agents start from `context/lib/index.md`, which routes them to the minimum docs they need.

### Context Folder Structure

```
context/
  lib/                  # Durable architectural knowledge (the "context library")
    index.md            # "Agent router" — directs agents to files relevant to their current task
    [topic].md          # Durable architectural knowledge about a specific topic

  plans/                # Work tracking (ephemeral by design)
    drafts/             # Specs being written, not yet reviewed
    ready/              # Reviewed specs, queued for implementation
    in-progress/        # Actively being worked on
    done/               # Recently completed (max 15, older plans pruned)
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

## Architecture

Five architectural invariants govern the engine:

| Principle | Rule |
|-----------|------|
| Renderer owns GPU | All wgpu calls live in the renderer module. Other subsystems never touch wgpu types. |
| Baked over computed | Lighting and light probes baked offline by the level compiler. Dynamic lights supplement, not replace. |
| Subsystem boundaries | Renderer, audio, input, game logic are distinct modules with explicit contracts. |
| Frame ordering | Input → Game logic → Audio → Render → Present. |
| No `unsafe` | If `unsafe` appears necessary, stop and consult the project owner. |

## Non-Goals

- General-purpose game engine
- ECS architecture
- Deferred rendering
- Extending or forking ericw-tools
- Runtime BSP compilation
- Multiplayer / networking

## License

MIT
