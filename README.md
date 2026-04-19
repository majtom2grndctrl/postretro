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

Postretro is a Quake-style FPS engine in Rust that looks and feels “like they used to make,” but embellishes the past with updated, lean technologies under the hood that enable game builders to **bring more boom to the boomer shooter.**

The visual target is something like Prodeus — chunky pixels, baked lighting, specular maps, cyberpunk atmosphere. But the goal is to earn that look through low-cost rendering techniques, resulting in _a game that **feels like** you remember_ but _**looks better** than the real thing._

It's early days, but the foundation is coming together fast. Right now Postretro compiles TrenchBroom maps into a custom binary format, renders fully textured levels, and drives the whole thing through a GPU-culled rendering pipeline: per-frame portal traversal narrows the visible set, a global BVH handles frustum culling on the GPU, and geometry is dispatched via indirect draw calls with zero per-object CPU overhead. Everything after that — lighting, movement, enemies, the whole game — is still ahead of us. That's kind of the fun part.

## Context Architecture (Like Project Docs for Agents)

**The real reason I’ve open sourced this project**

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

**The guiding principle:** context files describe what survives refactoring. If a sentence would break when a module is reorganized or a function is renamed, it belongs in a task spec or code comment — not in `context/lib/`.

### Skills (.claude/skills/)

The lifecycle is supported by Claude Code skills:

| Skill | Role |
|-------|------|
| `plan` | Creates feature specs with task breakdown, sequencing, and acceptance criteria |
| `orchestrate` | Coordinates plan execution — spawns agents, tracks progress, moves plans through stages |
| `code-review` | Reviews implementations against specs, architecture, and conventions |
| `review-panel` | Spawns 3 reviewer agents that approach review from different angles |
| `preflight` | Pre-commit quality gate: fmt, clippy, test |
| `create-skill` | Builds new skills for the project |

## Planned Milestones

The engine is being built in phases, each of which produces something you can actually see and test. Here's the rough shape of the road ahead:

- **Milestone 1** ✅ — BSP loading and wireframe rendering, PVS culling, free-fly camera
- **Milestone 1.5** ✅ — Custom PRL level compiler: .map → .prl with voxel-based visibility, portal geometry, and exterior void sealing
- **Milestone 2** ✅ — Fixed-timestep game loop, action-mapped input (keyboard, mouse, gamepad)
- **Milestone 3** ✅ — Textured world with solid rendering, depth buffer, material system
- **Milestone 3.5** ✅ — Rendering foundation: vertex format upgrade (packed normals and tangents), GPU-driven indirect draw dispatch. Same visuals as Milestone 3, new architecture underneath.
- **Milestone 4** ✅ — BVH foundation: global SAH BVH over all static geometry, GPU compute frustum culling via skip-index DFS traversal, fixed-slot indirect buffer. Per-cell chunking from Milestone 3.5 retired in favor of the global BVH.
- **Milestone 5** — Lighting foundation: SH irradiance volume (baked indirect), clustered forward+ dynamic lights, normal maps, shadow maps
- **Milestone 6** — Embedded scripting and entity foundation: scripted entity model, FGD entity parsing, hot reload, modder-facing API
- **Milestone 7** — Player movement: collision, gravity, step-up, jump — engine floor exposed as a script API so modders can craft their own feel
- **Milestone 8** — Weapons as scripted entities: hitscan, projectiles, pickups, viewmodel hooks
- **Milestone 9** — NPC entities: scripted AI with engine-provided navigation and line-of-sight primitives
- **Milestone 10** — World entities: doors, triggers, brush movers, scripted set pieces
- **Future** — Visual polish (sprites, emissives, fog), post-processing (bloom, CRT filter, cubemap reflections), audio, HUD, and whatever else a boomer shooter needs

The full phased plan with acceptance criteria lives in `context/plans/roadmap.md`.

## Tech Stack

- **Language:** Rust (edition 2024, MSRV 1.85)
- **Renderer:** wgpu
- **Windowing:** winit
- **Math:** glam
- **Audio:** kira
- **Gamepad input:** gilrs
- **Level editor:** TrenchBroom
- **Level compiler:** custom (postretro-level-compiler)

## Building

This is a Cargo workspace with multiple crates.

```bash
cargo run -p postretro                                          # engine (debug)
cargo run -p postretro -- assets/maps/test.prl                 # load a PRL map
cargo run -p postretro-level-compiler -- input.map -o out.prl  # compile a level
cargo run --release -p postretro                               # optimized build
RUST_LOG=info cargo run -p postretro                           # with logging
```

## Compiling Levels

Levels are authored in TrenchBroom and compiled into Postretro's binary `.prl` format by `prl-build` (the binary in the `postretro-level-compiler` crate). A typical invocation:

```bash
cargo run -p postretro-level-compiler -- input.map -o output.prl
```

The compiler accepts the following flags:

| Flag | Default | Description |
|------|---------|-------------|
| `-o <PATH>` | input path with `.prl` extension | Output `.prl` path. |
| `--pvs` | off | Emit a precomputed PVS (LeafPvs section) instead of the default portal graph. |
| `-v`, `--verbose` | off | Detailed per-stage logging. |
| `--format <FORMAT>` | `idtech2` | Map dialect to parse (e.g. `idtech2`, `idtech3`). |
| `--probe-spacing <METERS>` | `1.0` | SH irradiance probe grid spacing, in meters. |
| `--lightmap-density <METERS>` | `0.04` | Starting lightmap texel size, in meters. The baker automatically retries at coarser densities if the atlas overflows, so this is the *starting* value. |

`--lightmap-density` is the knob to play with if you're chasing the look. Crank the number up for chunkier, more pixelated lighting that leans hard into the retro aesthetic, or dial it down for finer, smoother shading on closer surfaces.

## Architecture

Five architectural invariants govern the engine:

| Principle | Rule |
|-----------|------|
| Renderer owns GPU | All wgpu calls live in the renderer module. Other subsystems never touch wgpu types. |
| Baked over computed | Indirect lighting baked offline (SH irradiance volume). Direct illumination is fully dynamic (clustered forward+ with shadow maps). |
| Subsystem boundaries | Renderer, audio, input, game logic are distinct modules with explicit contracts. |
| Frame ordering | Input → Game logic → Audio → Render → Present. |
| No `unsafe` | If `unsafe` appears necessary, stop and consult the project owner. |

## Non-Goals

- General-purpose game engine
- ECS architecture
- Deferred rendering
- Runtime level compilation
- Multiplayer / networking

## License

MIT
