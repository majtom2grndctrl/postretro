# AI Pathfinding Multithreading Readiness

> **Read this when:** parallelizing enemy pathfinding or changing how AI reads navigation and registry state.
> **Key invariant:** navigation and collision values are thread-shareable; registry access must be removed from the parallel query window before pathfinding is forked.
> **Related:** [Architecture Index](./index.md) · [Build Pipeline](./build_pipeline.md) §Navigation bake · [Development Guide](./development_guide.md) §Workspace

`NavGraph` and `CollisionWorld` are immutable shared-reference query surfaces and are guarded by compile-time `Send + Sync` assertions. Their data types are ready to cross worker-thread boundaries.

The blocker is scheduling, not spatial-data ownership. AI currently interleaves enemy-eye reads from the single-threaded entity registry with navigation queries. The first task in a parallel-pathfinding project must split that loop into two phases: snapshot all registry-derived request data on the simulation thread, then execute an ordered batch of navigation-only requests in a fork-join window.

The scheduler owns request identity, deterministic result ordering, worker-local A* scratch, and the fallback when a worker cannot produce a route. Keep registry handles and mutable gameplay effects outside the batch. Do not add a batch API before its scheduler consumer defines those contracts.

## Non-goals

- Moving navigation into the physics substrate; navigation is expected to churn during this rewrite and stays in sim.
- Parallel entity-registry access.
- Routing hot navigation or collision reads through the AI effects trait.
