# Gameplay-Stack Decomposition (Epic)

> **Status:** draft. The gameplay-side analog of `E19--render-stack-decomposition`.
> Multiple per-spec folders sharing the `gameplay-stack--*` prefix, grouped into
> four milestones. Source-grounded findings in `research.md`.
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
not by reviewer trust. A split that does not measurably improve its targeted edit
loop pauses the structural phases for re-evaluation.

**Owner-chosen first step: prove it before the big cuts.** The first spec
(`gameplay-stack--baseline-boundary-prep`) is a baseline + boundary-prep spike — it
establishes the warm-edit measurement and stands up the lowest leaf, before any
large module moves. The end-state graph below is committed; the large extractions
(M2–M4) are built one at a time, each re-grounded against the live tree
(detail-on-open), and the owner reviews the spike's numbers before M2 opens.

## Scope

### In scope
- A baseline + dev Cargo-config measurement harness (mirrors E19's).
- Four new workspace crates (see **Target crate graph**): `postretro-combat-model`,
  `postretro-sim`, `postretro-netcode`, `postretro-ai`.
- Breaking the `scripting ↔ sim ↔ netcode` cycle by sinking the two shared type
  families (shot-authority, carried-loadout) into `postretro-combat-model`.
- Moving collision into `postretro-sim`, per §Target shape.
- The deeper scripting-VM decomposition (M4) that makes enemy AI its own
  fast-rebuilding crate: sinking the VM store + reaction-dispatch primitives to
  `scripting-core`, inverting the `sim ↔ AiRuntime` seam behind a trait, inverting
  the host→systems registration.

### Out of scope
- Runtime/behavior changes, PRL wire-format changes, scripting-semantics or
  SDK-typedef changes. Extractions are behavior-preserving.
- Combat *feature* work (roadmap Epic 16). This epic builds the crate the combat
  features will land in; it does not add stat/augment/damage-type systems. M4's
  `postretro-combat-model` growth is a home, not new gameplay.
- Removing mlua/rquickjs from supported builds. The VM host stays; the sim/ai crates
  legitimately carry it (they are the runtime). `cpu-only` is not claimed for them.
- A dedicated-server entry point. The `simulate` seam is already headless
  (`research.md`); a second binary is a later, separate consumer.

## Target crate graph

One-way edges, top depends on bottom. New crates marked `*`. The renderer (E19) is a
**sibling** over `entities` — the binary orchestrates tick-then-draw, per §Target
shape. `postretro-ai` is carved out of `postretro-sim` at M4; through M3 the AI code
lives inside `postretro-sim`.

```
                         postretro (binary)
        main/App · event loop · Session::build wiring · tick-then-draw
        input · frame_timing · startup · session · audio · camera · view_feel
                 │ drives sim + net            │ reads entity state
                 ▼                             ▼
         postretro-netcode*  ──────────►  postretro-renderer (E19, GPU)
     client/server, replication,   │        (sibling over entities)
     reconcile, seat, interp,      │  netcode → sim / ai / scripting-runtime:
     state_slots, lifecycle        │  all DOWN-edges once the cluster sits below
                 │                 │
                 ▼                 ▼
             postretro-sim*  ◄────►  postretro-ai*   (M4; sim↔ai via trait seam)
     fixed-tick core: weapon_stage · movement · collision (moved in) · nav ·
     triggers · kinematic_mover · projectile · impact_policy/effects · spawner ·
     scripting host + systems (hit_zones, health, reactions, builtins)
                 │                             │
                 ▼                             ▼
        postretro-combat-model*          scripting-core
     stat/resource/augment/damage        VM host: mlua/rquickjs, primitives::store,
     taxonomy + shot-authority (sunk)    reaction_dispatch (M4 sinks store+dispatch
     + carried-loadout (sunk)            further down here)
                 │             │                 │
                 ▼             ▼                 ▼
                entities  ◄─────────────  foundation          net
             (POD component columns,                     (transport, wire —
              compile chokepoint)                         glam-free)
```

- **`postretro-combat-model*`** — new leaf over `entities`/`foundation`. Holds the
  shot-authority and carried-loadout types sunk out of `netcode` (the cycle-break),
  and is the home Epic 16's stat/resource/augment/damage math grows into — keeping
  combat-balance churn out of both the `entities` chokepoint and the sim crate.
- **`postretro-sim*`** — the fused fixed-tick core plus collision. The
  `scripting ↔ sim` half of the cycle and every fixed-tick ↔ collision/nav edge
  become intra-crate. Carries the scripting host + systems (AI included, through M3).
- **`postretro-netcode*`** — rises to the top of the gameplay stack. Its heavy edges
  into sim/scripting (`research.md`) become clean down-edges once the shared types
  are sunk. Largest single module (43K), lowest churn (23 commits) — evicting it
  from the AI rebuild unit is the core goal.
- **`postretro-ai*`** (M4) — enemy behavior graphs, targeting, perception, the
  `AiRuntime`, reaction dispatch invocation. Sibling to `postretro-sim` over
  `combat-model`/`scripting-core`. The churn locus, isolated so an AI edit rebuilds
  neither sim nor netcode.

## Global acceptance criteria (every spec inherits)

- [ ] `cargo build --workspace` and `cargo test --workspace` pass after each spec.
- [ ] Dependency graph is acyclic and one-way (proven by `cargo build --workspace`);
  no new crate depends on the binary. `layering_invariants_hold` passes.
- [ ] `cargo tree -p <new-crate>` shows no `wgpu`/`winit`/`glyphon`. The sim/ai
  crates DO carry `mlua`/`rquickjs` transitively via `scripting-core` — they are the
  runtime; this is expected, not a firewall breach (mirrors E19 Decision 13).
  `postretro-combat-model` carries neither VM nor wgpu.
- [ ] Behavior-preserving: no runtime, wire, scripting-semantics, or SDK-typedef
  change. Any type that crosses the replication wire keeps its `Encode`/`Decode`
  derive and field layout (`networking.md`).
- [ ] No net-new `unsafe` (pre-existing `unsafe` travels with moved code).
- [ ] Each extraction PR quotes before/after warm-edit timings vs. the
  `gameplay-stack--baseline-boundary-prep` baseline for its targeted loop. A split
  that fails to improve its loop meaningfully pauses later structural phases.

## Milestones

Each milestone is a shippable checkpoint: the build stays green and behavior-preserving
at every one, so the epic can pause after any without a half-migrated tree. Within a
milestone, specs are built one at a time in dependency order, each re-grounded against
the live tree just before it is built.

### M1 — Baseline + boundary-prep (`gameplay-stack--baseline-boundary-prep`)
Establish warm-edit measurement; stand up `postretro-combat-model`; sink the
shot-authority and carried-loadout type families into it, inverting the two
`sim/scripting → netcode` up-edge families to down-edges. Low risk, small LOC out of
the binary. **Testable outcome:** `postretro-combat-model` is a workspace member with
real consumers; the cycle's netcode up-edges are gone; the baseline quotes the
warm-edit numbers M2–M4 must beat. **Owner reviews these numbers before M2.**

### M2 — `postretro-sim`
Move the fixed-tick core + collision + scripting host/systems into one crate. The
contained cycle is now `scripting ↔ sim` only (intra-crate, legal). Sever the small
binary up-edges (`research.md`: `TICK_DURATION`, `InputMode`, the `render` mover
structs, the `session`/`App` reach-ins, `fx`-reactions) as boundary-prep, re-grounded.
Largest, highest-risk step — built solo. **Testable outcome:** editing a
non-gameplay binary file recompiles no `postretro-sim`; `cargo tree` isolation holds.

### M3 — `postretro-netcode`
Lift netcode above `postretro-sim`. With the shared types already sunk (M1), its
edges into sim/scripting are down-edges; this is now a clean lift. **Testable
outcome:** touching `scripting/systems/ai/` (once still in sim) does not rebuild
`postretro-netcode`; compare to the M1 baseline.

### M4 — `postretro-ai` (the scripting-VM decomposition)
Carve enemy AI out of `postretro-sim` into its own crate: sink `primitives::store`
and reaction dispatch to `scripting-core`; invert the `sim ↔ AiRuntime` seam behind a
trait; invert the host→systems registration. **Testable outcome:** an AI-logic edit
recompiles `postretro-ai` + relinks; rebuilds neither `postretro-sim` nor
`postretro-netcode`. **Detail-on-open:** M4's exact seam is designed when reached —
its shape depends on what M2/M3 reveal. This milestone is committed scope, not its
current design.

## Execution model

Build specs sequentially in dependency order — one spec per `/orchestrate` run,
lowest crate first. Per spec: (1) re-ground against the live tree; (2) update the
spec; (3) orchestrate; (4) run the full global-AC gate before the next spec opens.
Do not deep-ground later specs now — they change as lower crates land (detail-on-open).
Parallelism is the exception (genuinely file-disjoint boundary-prep only). The M4
scripting-VM decomposition is deliberately left as a target with sketched seams, not
a detailed design — designing it before the sim/ai boundary exists would bake in
guesses the earlier cuts will falsify.

## Decisions

1. **`postretro-combat-model` is the cycle-break sink and the Epic 16 home.** The
   shot-authority and carried-loadout families sink here (both are damage/loadout
   authority, both carry `Vec3`/gameplay types so `net` is out). Principle: the
   lowest common leaf both sides depend down on, sited where the combat-feature math
   will grow — off the `entities` chokepoint. (M1.)
2. **AI stays inside `postretro-sim` through M3; carved out at M4.** AI is fused to
   the VM runtime and to `sim::spawn_projectile` (`research.md`); a leaf AI crate
   needs the VM decomposition. Co-location resolves the `sim ↔ ai` cycle for free
   until M4 inverts it deliberately. Principle: don't force a boundary blocked by
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

At promotion, update §Target shape to name the netcode and ai crates and record that
AI is inseparable from the scripting runtime until the M4 decomposition.

## Open questions

- **M4 seam shape.** Where the inverted `sim ↔ AiRuntime` trait is defined
  (`combat-model`, `scripting-core`, or a new seam crate), and how much of
  `primitives::store` / reaction dispatch sinks to `scripting-core`. Deferred to M4
  re-grounding by design (Execution model).
- **`restore_carried_health` placement.** The struct sinks to combat-model (M1); the
  function is behavior over the registry — decide at M1 build whether it travels with
  the struct or stays netcode-side reading down.
- **Carried-loadout wire status.** Confirm at M1 build whether any of `CarriedState`
  / `TuningPayload` / `WieldableTuningPayload` derives bitcode/serde for replication;
  if so the move preserves the derive and layout (global AC).
- **`fx`-reactions placement** (M2). Move the emitter/fog reaction registrars into
  `postretro-sim`, or sink the `fx` presentation data below it. Affects whether
  `postretro-sim` grows an `fx` dependency.
