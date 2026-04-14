# Development Guide

> **Read this when:** starting work on the project, adding a feature, or onboarding a new agent. Covers conventions, constraints, and coding standards.
> **Key invariant:** context files describe durable decisions — follow them, update them when they're wrong, never silently deviate.
> **Related:** [Architecture Index](./index.md) · [Context Style Guide](./context_style_guide.md) · [Testing Guide](./testing_guide.md)

---

## Workspace

Three crates in a Cargo workspace:

| Crate | Type | Purpose |
|-------|------|---------|
| `postretro` | binary | Engine / game runtime |
| `postretro-level-format` | library | Shared PRL binary format types. Depended on by both engine and compiler. |
| `postretro-level-compiler` | binary | Offline level compiler. TrenchBroom `.map` → `.prl` binary. |

## Stack

### Engine (`postretro`)

| Concern | Crate |
|---------|-------|
| Windowing | winit 0.30 |
| GPU | wgpu 29 (Vulkan, Metal, DX12) |
| Math | glam |
| PRL loading | postretro-level-format |
| Audio | kira 0.12 |
| Gamepad | gilrs 0.11 |
| Errors | thiserror 2 (subsystems), anyhow 1 (top-level) |
| Async blocking | pollster 0.4 (wgpu adapter/device init only) |
| Logging | log 0.4 + env_logger 0.11 |

### Level compiler (`postretro-level-compiler`)

| Concern | Crate |
|---------|-------|
| .map parsing | shambler (re-exports shalrath). Uses nalgebra internally — convert to glam at boundary. |
| PRL format | postretro-level-format |
| Math | glam |
| Parallelism | rayon (SH baker and other CPU-heavy compile stages) |

---

## Agent TL;DR

- Optimize for **readability over cleverness**; prefer small, explicit changes.
- Respect **subsystem boundaries**: renderer, audio, input, game logic are distinct modules with explicit contracts.
- **Deliver the impact defined in specs and tasks.** Specs define what and why; use judgment on how. When the plan doesn't survive contact with the code, adapt — but surface deviations and update the context files. See §1.
- Do not flatten module structure. See §2.
- **No `unsafe` blocks.** See §3.5.

---

## 1) Implementation Quality

Specs and task descriptions define the **impact** to deliver — the *what* and *why*. The recommended approach is the starting plan, not a mandate. Deliver the intended impact cleanly; use judgment when the plan doesn't survive contact with the code.

### 1.1 Deliver the impact

Read the spec and task before writing code. They tell you what outcome matters and what constraints apply. Start with the recommended approach, but treat it as a plan, not a contract.

**Do:**
- Handle error states and edge cases within the defined scope.
- Write tests now, while you have full context on what the code should do.
- When the approach works as specced, deliver it without embellishment.

**Don't:**
- Add capabilities the task didn't ask for ("while I'm here, I'll also add...").
- Skip work that's clearly within scope and justify it with `// TODO` or version labels.
- Invent abstractions, helpers, or config options for hypothetical future needs.

### 1.2 Plan deviations

Sometimes the spec's approach hits a wall — type conflict, borrow checker issue, API constraint. The response depends on scale:

**Small adjustment** (same impact, minor approach change):
1. Explain what you found and propose the alternative.
2. On confirmation, implement the alternative.
3. Update the spec/context files to reflect the actual approach taken.

**Significant change** (different contracts, shifted scope, new trade-offs):
1. Stop and surface the issue with enough detail to decide.
2. Propose options with trade-offs.
3. On resolution, update context files *before* resuming implementation.

The key principle: **specs are working documents — update them during implementation, never silently deviate.** After the feature ships, durable knowledge lives in context files and code comments; the spec is consumed and removed. See §1.5.

### 1.3 Clean, not clever

Over-engineering is as costly as under-delivering. Both create surface area that has to be understood, tested, and maintained.

| | Under-delivering | Over-engineering |
|---|---|---|
| **What** | Shipping scope with missing validation, error handling, or tests | Adding scope, abstractions, or infrastructure the task didn't request |
| **Cost** | Broken states, follow-up work, lost context | Unnecessary complexity, harder reviews, maintenance burden |
| **Example** | "Map loading works but panics on missing lightmap section" | "Added a generic asset loading framework with plugin hooks" |

### 1.4 Follow-up criteria

Adjacent work discovered during implementation gets a follow-up task, not a scope expansion.

- **Robustness gaps** outside the task's scope → file a follow-up with enough context for the next agent.
- **Optimizations** with no current performance problem → file a follow-up, don't cache preemptively.
- **Abstractions** without multiple concrete consumers → three similar lines beat a premature helper.

Never leave a bare `// TODO: fix later`. Either file a follow-up with context or fix it now.

### 1.5 Documentation lifecycle

See [Context Style Guide](./context_style_guide.md) §Documentation Lifecycle. Specs are consumed during implementation, then deleted. Durable knowledge lives in context files; implementation-level "why" lives in code comments.

### 1.6 Breaking API changes

This project does not maintain backward compatibility. There are no external consumers, and compatibility shims add maintenance weight with no current benefit.

If a task requires a breaking change to a public API (function signatures, format types, wire format):

1. **Confirm with the project owner before proceeding.** Breaking changes ripple across crates; get explicit sign-off.
2. **Scope the full break as a plan.** Identify all call sites and include the fixes. Don't land the API change and leave callers broken.

When users are brought in and a stable platform is required, this policy will be revisited.

---

## 2) Module Organization

**Rule: split by responsibility, not by line count.** A file earns a split when it serves distinct jobs. Line count alone is not a trigger.

### 2.1 File size guidance

- **~400–500 lines** (source, non-test): yellow flag. Consider splitting on next significant addition.
- **~600+ lines**: split before adding more code.
- **Test files**: exempt. Test suites are flat and linear; large is fine.

Rust is more verbose than some languages (explicit error handling, pattern matching, lifetime annotations). Use judgment — 500 lines of straightforward match arms is different from 500 lines of tangled logic.

Existing files above these thresholds are not immediate refactoring targets. Apply when adding significant new code.

### 2.2 Valid seams for splitting

Split along natural boundaries:

1. **Responsibility** — file serves two distinct jobs. Extract each into its own module.
2. **Consumer** — different modules use different subsets of exports. Each subset becomes its own module.
3. **Change frequency** — stable plumbing vs. actively-evolving logic. Separate to reduce churn.

### 2.3 Splits to avoid

- Arbitrary line-count splits with no conceptual boundary.
- No grab-bag `utils.rs` / `helpers.rs` files. Co-locate single-caller helpers with their caller.
- Separating types from implementation. Co-locate structs, enums, and their `impl` blocks with the code that uses them (exception: shared types at subsystem boundaries).

### 2.4 Directory structure

- **Subsystem directories** (`src/render/`, `src/audio/`, `src/input/`): use `mod.rs` or a barrel file for the public API. Internal modules are `pub(crate)` or private.
- **Shaders directory** `src/shaders`: Keep all shaders in `.wgsl` files under `src/shaders`. Load them with `include_str!()`. Never embed shader source inline in Rust files.
- **Flat is fine for uniform directories** (all the same kind of thing — e.g., all entity types, all texture loaders). 20+ files OK.
- **Mixed-concern directories**: introduce subdirectories when you can't tell at a glance which files relate to each other.

### 2.5 Split timing

- **Proactively**: when adding significant new functionality to an already-large file. You have full context; the split is cheapest now.
- **Not retroactively** just to meet a number. Only when the file actively causes pain (hard to navigate, too many responsibilities).
- **Never during a bugfix.** Don't mix structural refactoring with behavior changes in one changeset.

---

## 3) Rust Conventions

### 3.1 Readability first

- Prefer **plain, explicit Rust** over clever type-level gymnastics.
- Prefer **descriptive names** and early returns over deeply nested logic.
- Keep functions small; extract helpers only when it reduces duplication.

### 3.2 Ownership at module boundaries

In Rust, ownership semantics *are* the data contract. When defining how subsystems exchange data, be explicit about:

- **Owned vs. borrowed** — does the receiver take ownership or borrow? A map loader handing vertex data to the renderer is an ownership transfer. A renderer reading from a shared resource table is a borrow.
- **Lifetime constraints** — if a borrowed reference crosses a subsystem boundary, the lifetime relationship must be clear and intentional, not accidental.
- **Clone decisions** — cloning to avoid a borrow fight is sometimes the right call. Document why when the clone isn't obvious.

### 3.3 Error strategy

| Mechanism | Use for | Notes |
|-----------|---------|-------|
| `Result<T, E>` | Operations that can fail | Default error path |
| `Option<T>` | Absence is normal, not an error | e.g., optional lightmap section |
| `thiserror` | Error types at subsystem boundaries | Typed, matchable errors |
| Ad-hoc errors | Internal helpers | Convert at the boundary |
| `anyhow` | Top-level application code only | Main loop, initialization — not subsystem code |
| `?` | Error propagation | Explicit `match` only when handling specific variants or adding context |

### 3.4 Trait contracts

When a subsystem is abstracted behind a trait, the trait's semantic contract matters more than its signature. Document what an implementor must guarantee — not just the types, but the behavioral invariants.

Traits earn their existence when there are (or will concretely be) multiple implementations. Don't extract a trait from a single concrete type speculatively.

### 3.5 No `unsafe`

Do not write `unsafe` blocks. The crate stack (wgpu, winit, kira, gilrs, glam) provides safe APIs — there is no routine need for `unsafe` in engine code.

If a situation appears to require `unsafe`, stop and consult the project owner. Do not proceed until the need is confirmed and a risk mitigation approach is agreed on. If `unsafe` is approved, document the rationale and invariants with a `// SAFETY:` comment.

---

## 4) Engine Constraints

### 4.1 Renderer owns GPU

All `wgpu` calls live in the renderer module. Other subsystems do not depend on `wgpu` types or interact with GPU resources directly. This keeps the rendering logic cohesive and prevents GPU concerns from leaking across the codebase.

Within the renderer, separate data logic (visibility determination, atlas packing, vertex generation) from GPU interaction (buffer uploads, render passes, pipeline creation). Data logic operates on engine types and is testable without a GPU context. GPU interaction is a thin layer that consumes the output.

### 4.2 Event loop ownership

winit owns the event loop. Once `event_loop.run()` is called, winit controls the top-level control flow. Engine subsystems respond to events dispatched by the loop — they never block it.

Blocking the event loop freezes the window, breaks input handling, and on some platforms triggers an OS "not responding" state. Long-running work (asset parsing, level decompression) must be done before entering the loop or on a background thread for CPU-side processing.

### 4.3 Frame ordering

Each frame follows a fixed sequence. Subsystems run in this order because later stages depend on results from earlier ones:

1. **Input** — poll events, update input state
2. **Game logic** — fixed timestep update (entity movement, collision, game rules)
3. **Audio** — update listener position, trigger sounds based on game events
4. **Render** — determine visible set, draw visible geometry, dynamic lights, sprites, post-processing
5. **Present** — swap buffers

Game logic runs at a fixed timestep decoupled from the render rate. Render interpolates between the last two game states for smooth visuals at variable framerates.

### 4.4 Pipeline state discipline

wgpu uses explicit render pipelines and bind groups — there is no hidden global state. Each rendering stage creates or references the pipelines and bind groups it needs. Shared resources (textures, uniform buffers) are passed explicitly.

---

## 5) Code Comments

### 5.1 Comments that earn their keep

- **Why, not what.** Explain rationale. The code shows behavior.
- **Non-obvious context.** Why a capability lives in this module, ordering dependencies, wgpu quirks, performance-sensitive paths. Things a reader can't derive from the code alone.
- **Spec pointers.** Brief reference to the governing context file or contract when code implements a specific architectural decision.

### 5.2 File headers

File headers orient a reader, not educate them. Two lines: what this file owns, then which context file governs it.

**Good:**
```
// Lightmap atlas packing and upload to GPU.
// See: context/rendering_pipeline.md §3
```

**Too much:**
```
// This module handles the full lightmap lifecycle: parsing baked
// lightmap data from BSP faces, packing them into a texture atlas
// using a shelf-packing algorithm, uploading to wgpu via
// queue.write_texture, and providing UV offset/scale lookups for
// the renderer to use during face drawing...
```

The header's job is to tell you *which context file to load*, not to summarize that file.

### 5.3 Comments to avoid

- **Restating code.** If the code is unclear, improve the code — don't narrate it.
- **Changelog annotations.** `// Added in commit abc123`. Git handles provenance.
- **Orphan TODOs.** `// TODO: fix later` without a follow-up task or actionable context. File a task and reference it, or fix it now.
- **Duplicating context files.** Don't restate architectural decisions in comments. Two sources of truth, both eventually wrong.

### 5.4 Revising on encounter

When you find a misleading, stale, or code-restating comment in a file you're already changing:

- Fix or remove it in the same changeset. A missing comment beats a lying one.
- Scope to code you're touching. Don't sweep unrelated files for comment cleanup.

---

## 6) Logging & Debuggability

### 6.1 Logging rules

- Prefix logs with a subsystem tag: `[Renderer]`, `[Loader]`, `[Audio]`, `[Input]`, etc.
- Log actionable failures once at the boundary; avoid spamming logs in hot paths (per-frame rendering, per-face traversal).
- Prefer structured errors for failures that could surface to the user or to diagnostic tooling.
- Make the happy path easy to follow in code; keep failure branches explicit.

### 6.2 Panic policy

- **Never panic in subsystem code** for recoverable conditions. Return `Result`.
- **`unwrap()` and `expect()` are allowed** only when the invariant is structurally guaranteed — e.g., a mutex lock in single-threaded code, an index into a vec you just checked the length of.
- **`expect()` over `unwrap()`** when the invariant isn't obvious from immediate context. The message documents what went wrong.
- **Panics in initialization are acceptable.** If the GPU adapter can't be acquired or a required asset is missing at startup, crashing with a clear message is better than limping along.
