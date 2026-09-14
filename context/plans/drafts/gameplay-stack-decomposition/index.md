# Gameplay-Stack Decomposition (Epic)

> **Status:** draft. The gameplay-side analog of `E19--render-stack-decomposition`.
> Multiple per-spec folders sharing the `gameplay-stack--*` prefix, grouped into
> three milestones. Source-grounded findings in `research.md`.
> **Layout:** this folder is the epic hub (index + `research.md`). Each spec lives
> in a sibling `gameplay-stack--<spec>` folder, independently reviewable and
> promotable.
> **Related:** `context/lib/development_guide.md` §Layering invariants / §Target
> shape · `context/lib/scripting.md §12` (engine-data floor / `script-ffi` pattern)
> · `context/lib/networking.md` · `context/lib/index.md §2` · the
> `E19--render-stack-decomposition` hub (the playbook and conventions this mirrors)
> · shipped `context/plans/done/M15--p0-headless-sim-seam/` (the `simulate` seam).

## Goal

Decompose the `postretro` binary's fixed-tick gameplay — the fused scripting
runtime, simulation, netcode, and combat/AI — into a correct, one-way crate graph,
so routine engine edits stop recompiling gameplay and combat/AI edits recompile as
little as possible. **North star:** an enemy-AI or combat edit recompiles its own
crate and relinks the binary, but rebuilds neither the 43K netcode crate nor the
render/wgpu stack; and an edit to input/audio/render/startup recompiles no gameplay
at all. This is the render-side analog realized on the gameplay side, and the
extraction `development_guide.md` §Target shape names but does not yet build.

## Scoping philosophy — build more right faster

Scope to the correct end-state crate graph, then extract in dependency order with
hard verification gates. Because incremental human checkpoints are removed, replace
them with verification: every spec proves correctness by construction (`cargo tree`
isolation, acyclicity-by-compile, `layering_invariants_hold`, behavior-preservation),
not by reviewer trust. Structural facts — what a crate depends on, what a touched file
rebuilds — are the acceptance criteria; compile timings are reported, never gated on.

**Owner-chosen first step, shipped:** `gameplay-stack--baseline-boundary-prep` (M1)
stood up the lowest leaf and recorded the baseline before any large module moved. The
end-state graph below is committed; the large extractions land as M2 (`postretro-sim`
and `postretro-netcode`, one stride) and M3 (`postretro-ai`), each re-grounded against
the live tree when opened (detail-on-open).

## Scope

### In scope
- A baseline + dev Cargo-config measurement harness (mirrors E19's).
- Four new workspace crates (see **Target crate graph**): `postretro-combat-model`,
  `postretro-sim`, `postretro-netcode`, `postretro-ai`.
- Breaking the `scripting ↔ sim ↔ netcode` cycle by sinking the two shared type
  families (shot-authority, carried-loadout) into `postretro-combat-model`.
- Moving collision into `postretro-sim`, per §Target shape.
- Carving enemy AI into its own fast-rebuilding `postretro-ai` crate (M3), plus a
  `postretro-physics` leaf (collision, movement, kinematic movers) below sim. On the
  merged M2 tree AI is already VM-decoupled, so M3 is a tick-seam inversion behind an
  `AiHost` trait — not the VM decomposition the target graph below sketches (see M3).

### Out of scope
- Runtime/behavior changes, PRL wire-format changes, scripting-semantics or
  SDK-typedef changes. Extractions are behavior-preserving.
- Combat *feature* work, and consolidating the combat logic already spread across the
  binary (roadmap Epic 16 — largely shipped) into `postretro-combat-model`. This epic
  sinks only the two cycle-break type families; it adds no gameplay and relocates no
  existing combat logic. Future consolidation into the leaf is a separate effort.
- Removing mlua/rquickjs from supported builds. The VM host stays; the sim/ai crates
  legitimately carry it (they are the runtime). `cpu-only` is not claimed for them.
- A dedicated-server entry point. The `simulate` seam is already headless
  (`research.md`); a second binary is a later, separate consumer.

## Target crate graph

One-way edges, top depends on bottom. New crates marked `*`. The renderer (E19) is a
**sibling** over `entities` — the binary orchestrates tick-then-draw, per §Target
shape. `postretro-ai` is carved out of `postretro-sim` at M3; through M2 the AI code
lives inside `postretro-sim`.

```
                         postretro (binary)
        main/App · event loop · Session::build wiring · tick-then-draw
        input · startup · session · audio · camera · view_feel · frame-path bridges
                 │ drives sim + net                        │ reads entity state
                 ▼                                         ▼
         postretro-netcode*                       postretro-renderer (E19, GPU)
     client/server, replication,                    (sibling over entities)
     reconcile, seat, interp,
     state_slots, lifecycle          netcode → sim / ai / scripting-runtime:
                 │                   all DOWN-edges once the cluster sits below
                 ▼
             postretro-sim*  ◄───  postretro-ai*   (M3; ai ABOVE sim — sim→ai inverted via AiHost trait)
     fixed-tick core: weapon_stage · movement · collision (moved in) · nav ·
     triggers · kinematic_mover · projectile · impact_policy/effects · spawner ·
     frame_timing · presentation pool · scripting host + systems (hit_zones,
     health, reactions, builtins)
                 │                             │
                 ▼                             ▼
        postretro-combat-model*          scripting-core
     stat/resource/augment/damage        VM host: mlua/rquickjs, primitives::store,
     taxonomy + shot-authority (sunk)    reaction_dispatch + store (already here;
     + carried-loadout (sunk)            M3 needs no further VM sink)
                 │             │                 │
                 ▼             ▼                 ▼
                entities  ◄─────────────  foundation          net
             (POD component columns,                     (transport, wire —
              compile chokepoint)                         glam-free)
```

- **`postretro-combat-model*`** — new leaf over `entities`/`foundation`. Holds the
  shot-authority and carried-loadout types sunk out of `netcode` (the cycle-break),
  and is the natural home for combat-model-domain math (stat/resource/augment/damage
  taxonomy) to consolidate into over time — off both the `entities` chokepoint and the
  sim crate. Relocating the combat logic already spread across the binary is out of
  scope here (see Scope); the crate is justified by the cycle-break alone.
- **`postretro-sim*`** — the fused fixed-tick core plus collision. The
  `scripting ↔ sim` half of the cycle and every fixed-tick ↔ collision/nav edge
  become intra-crate. Carries the scripting host + systems (AI included, through M2).
  The frame-path bridges under `scripting/systems/` — the modules that feed
  `render-cpu`, `postretro-ui` and `visibility` — stay in the binary: sim is the
  renderer's sibling, never its dependent.
- **`postretro-netcode*`** — rises to the top of the gameplay stack. Its heavy edges
  into sim/scripting (`research.md`) become clean down-edges once the shared types
  are sunk. Largest single module (43K), lowest churn (23 commits) — evicting it
  from the AI rebuild unit is the core goal.
- **`postretro-ai*`** (M3) — enemy behavior graphs, targeting, perception, the
  `AiRuntime` and AI tick. Sits **above** `postretro-sim` (ai→sim down-edge for
  nav/collision/weapon/spawn; sim→ai inverted via the injected `AiHost` trait). The
  churn locus, isolated so an AI edit rebuilds neither sim nor netcode.

## Global acceptance criteria (every spec inherits)

- [ ] `cargo build --workspace` and `cargo test --workspace` pass after each spec.
- [ ] Dependency graph is acyclic and one-way (proven by `cargo build --workspace`);
  no new crate depends on the binary. `layering_invariants_hold` passes.
- [ ] `cargo tree -p <new-crate>` shows no `wgpu`/`winit`/`glyphon`. The sim/ai
  crates DO carry `mlua`/`rquickjs` transitively via `scripting-core` — they are the
  runtime; this is expected, not a firewall breach (mirrors E19 Decision 13).
  `postretro-combat-model` carries neither VM nor wgpu.
- [ ] Behavior-preserving: no runtime, wire, scripting-semantics, or SDK-typedef
  change. Any type that crosses the replication wire keeps its codec derive (bitcode
  `Encode`/`Decode`, or serde `Serialize`/`Deserialize` for JSON payloads) and field
  layout (`networking.md`).
- [ ] No net-new `unsafe` (pre-existing `unsafe` travels with moved code).
- [ ] Each extraction PR names the rebuild unit it shrank. Timings are reported when
  cheap, never as a gate: the M1 baseline is `cargo check` only, which times the
  compiler frontend, not the codegen and link where the dev loop's cost sits.

## Milestones

Each milestone is a shippable checkpoint: the build stays green and behavior-preserving
at every one, so the epic can pause after any without a half-migrated tree. Within a
milestone, work is built in dependency order and re-grounded against the live tree
just before it is built.

### M1 — Baseline + boundary-prep (`gameplay-stack--baseline-boundary-prep`) — shipped
Established warm-edit measurement; stood up `postretro-combat-model`; sank the
shot-authority and carried-loadout type families into it, inverting the two
`sim/scripting → netcode` up-edge families to down-edges. Low risk, small LOC out of
the binary. **Outcome:** `postretro-combat-model` is a workspace member with real
consumers; the two sunk type families no longer point up into `netcode`; the baseline
records the pre-move `cargo check` reference.

### M2 — `postretro-sim` + `postretro-netcode` (`gameplay-stack--sim-and-netcode-crates`)
Move the fixed-tick core + collision + scripting host/systems into `postretro-sim`,
and lift `netcode/` above it as `postretro-netcode`, in one stride. Sequencing inside
the stride is unchanged — sim exists before netcode sits on it — but the two are not
separate checkpoints. Split, netcode stays bin-only while sim exists, and every sim
test that reaches a netcode harness type must relocate to the binary through a widened
sim API. Combined, `postretro-sim` dev-depends on `postretro-netcode` while netcode
depends on sim — legal, because a dev-dependency binds only test targets — and the
tests stay where they are, except where a helper's signature names a sim type: sim
compiles twice under test, so those cross as two distinct types and their callers move
to netcode. Largest, highest-risk step; the contained cycle is now `scripting ↔ sim`
only (intra-crate, legal).

Boundary-prep, re-grounded at open. `research.md`'s list has drifted: `session` and
`App` no longer reach in from the fixed-tick cluster, and edges it never named do.

| Up-edge into binary-only code | Reached from |
|---|---|
| `frame_timing::TICK_DURATION` | `scripting/systems/reaction_scheduler.rs` |
| `input::InputMode` | `scripting/systems/input_mode.rs` — App composition; stays behind with its binary-side consumers |
| `fx::{emitter,fog}_reactions` | `scripting/reactions/{mod,registry}.rs` |
| `netcode::MAX_DELAY_MICROS` | `spawner.rs` — the one cluster→netcode up-edge M1 left behind |
| `resolve_weapon_placement` (`main.rs`) | `sim/weapon_stage/commands.rs` and `netcode/mod.rs` |
| `presentation_pool` (names a renderer type) | `impact_policy.rs` and `netcode/presentation.rs` |
| `App` / `session::Session` | `netcode/endpoint.rs` — the client control drains, App composition |

Everything else netcode reaches in the binary root is test-only, and a crate cannot
dev-depend on a bin: those tests move up or shed the reach.

**Testable outcome:** editing a binary-only file recompiles neither crate;
`cargo tree` isolation holds for both; `postretro-netcode` is a crate above
`postretro-sim` and no `netcode/` module remains in the binary. An AI edit still
rebuilds netcode: AI lives in sim until M3, and Cargo rebuilds every dependent of a
changed crate. The AI-loop win lands at M3, not here.

### M3 — `postretro-ai` + `postretro-physics` (`gameplay-stack--ai-and-physics-crates`, promoted)
Re-grounded on the merged M2 tree, the "scripting-VM decomposition" framing this hub
sketches elsewhere is **superseded**: enemy AI holds no script-store or reaction-dispatch
state (`scripting-core` owns both), `AiRuntime` is plain data, and no host→systems
registration exists. M3 carves `postretro-ai` **above** `postretro-sim` by inverting the
injected `simulate_tick` → AI edge behind a synchronous `AiHost` effects trait (so sim never
depends on ai), sinks two graph-query helpers + two state constants to `foundation` (severing
the one `netcode → ai` edge, `locomotion_animation`), and extracts a `postretro-physics` leaf
— collision, movement, kinematic movers — below sim to shrink the sim compilation unit and
set up multi-threaded pathfinding. **Testable outcome:** an AI-logic edit recompiles
`postretro-ai` + relinks; rebuilds neither `postretro-sim` nor `postretro-netcode`. Fully
detailed in the promoted brief.

## Execution model

Build specs sequentially in dependency order — one spec or brief per build, lowest
crate first. Per spec: (1) re-ground against the live tree; (2) update the spec;
(3) build; (4) run the full global-AC gate before the next spec opens. Do not
deep-ground later specs now — they change as lower crates land (detail-on-open).
Parallelism is the exception (genuinely file-disjoint boundary-prep only). M3 is now
designed and promoted (`gameplay-stack--ai-and-physics-crates`): the M2 re-grounding
replaced the sketched VM decomposition with a tick-seam inversion behind an `AiHost`
trait plus a `postretro-physics` leaf — the earlier cuts falsified the guess, as intended.

## Decisions

1. **`postretro-combat-model` is the cycle-break sink.** The shot-authority and
   carried-loadout families sink here (both are damage/loadout authority, both carry
   `Vec3`/gameplay types so `net` is out). Principle: the lowest common leaf both sides
   depend down on, off the `entities` chokepoint and VM-free — and the natural home for
   combat-model-domain math to consolidate into later. Justified by the cycle-break
   alone; the consolidation is a separate, out-of-scope effort. (M1.)
2. **AI stays inside `postretro-sim` through M2; carved out at M3.** AI is coupled to
   `sim::spawn_projectile` and the fixed-tick core (`research.md`), so co-location
   resolves the `sim ↔ ai` cycle for free until M3 inverts it deliberately. (The epic
   first assumed AI was fused to the VM runtime and M3 a VM decomposition; the M2
   re-grounding overturned that — see M3.) Principle: don't force a boundary blocked by
   source coupling; sequence it behind the decomposition that unblocks it.
3. **`postretro-netcode` on top, not left in the binary.** §Target shape names only
   `postretro-sim` + combat-model; source shows netcode is the largest, lowest-churn
   module and the actual lever for combat/AI compile-isolation, and the cycle forces
   it to move if sim does. Adding it as a third crate above sim is the divergence that
   serves the epic's goal. Principle: the documented target predates the warm-cache
   goal; realize its intent, extended by what source requires.
4. **Baseline spike precedes the big cuts.** Owner-chosen. M1 proves the mechanics
   and fixes the measurement before M2's large, risky move. Principle: resolve the
   measurable uncertainty first when the destination shape is already decided.
5. **`postretro-sim` and `postretro-netcode` land in one stride.** Owner-chosen, on
   build-more-right-faster grounds. A separate netcode milestone would leave netcode
   bin-only while sim exists, forcing sim's harness-reaching tests into the binary
   through a widened API; a dev-dependency edge from sim to netcode keeps most of them
   in their files, the exception being helpers whose signatures name a sim type, which
   no dev-dependency can carry across. The deeper reason is that `netcode → sim` is
   already one-way after M1: fusing the two into a single crate instead would legalize
   a second cycle, leaving M3 owing two inversions. Principle: preserve a boundary
   already earned, and skip an intermediate checkpoint whose cost the end state deletes.

## Divergence from `development_guide.md` §Target shape

§Target shape names a `postretro-sim` crate (collision moving in) and a combat-model
crate, and frames `sim` as extractable "when a dedicated server forces it, or when
measured compile pressure justifies the lift." This epic:
- Pulls the trigger on the compile-pressure clause (the warm-cache goal).
- Adds two crates the guide does not name — `postretro-netcode` (Decision 3) and
  `postretro-ai` (Decision 2) — because source shows the cycle forces netcode to
  move with sim, and the AI churn locus only isolates behind the VM decomposition.
- Confirms the guide's finding that collision and the fixed-tick systems are
  binary-bound because they call collision — resolved by moving collision into
  `postretro-sim`.

§Target shape (updated on the M2-merge base) names the netcode and ai crates and
records that AI, though still in sim, is VM-decoupled — M3 is a tick-seam inversion
behind an `AiHost` trait plus a `postretro-physics` leaf, not a VM decomposition.

## Open questions

- **M3 seam shape.** Resolved by the M2 re-grounding: the `AiHost` effects trait is
  defined in `postretro-sim` and injected by the binary (ai sits above sim); and no
  further VM sink is needed — `primitives::store` and reaction dispatch already live in
  `scripting-core`. See the promoted M3 brief.
- **`restore_carried_health` placement.** Resolved from source (M1): it names only
  `CarriedState`/`EntityRegistry`/`EntityId` + entities' `set_health_absolute`, so it
  travels with the structs into combat-model and the netcode seat re-export drops it.
- **Carried-loadout wire status.** Resolved from source (M1): `TuningPayload` and
  `WieldableTuningPayload` derive serde `Serialize`/`Deserialize` and cross as an opaque
  JSON payload — the move preserves both derives; `CarriedState` and every shot-authority
  type derive no codec. As later milestones move more wire types, confirm each per-type
  against the global AC.
- **`fx`-reactions placement** (M2). Resolved from source: the binary's `fx` is entirely
  emitter/fog reaction primitives over `entities` and `scripting-core` — the presentation
  data it once held now lives in `postretro-render-cpu::fx`. It moves into `postretro-sim`
  whole, adding no dependency edge.
