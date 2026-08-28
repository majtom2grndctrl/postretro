# Task 1 addendum — id-41-only production decision

## Decision

Production/default coarsening is limited to direct delta section id 41. Ids 27
and 45 remain uniform L0 for every production bake. `_sh_coarsen "0"` also
leaves id 41 uniform L0.

## Rationale

The runtime-safe id-41 envelope repaired all measured failures on the 1.0 m
Stress-Warren safety bake: **38 → 0**. Id-41 traffic retained **18.96%** of
uniform traffic, an approximately **81% reduction**.

Id-27 script-mutable and id-45 animated/script-mutable contributions have no
finite bake-known amplitude bound. Forcing their mutable cells to L0 retains
nearly their entire payloads in the measured safety bake: id 27 **2.1 → 80.1
MiB** and id 45 **2.5 → 76.3 MiB**. An all-unit bake proxy is not a safe
runtime substitute for that bound.

## Follow-up boundary

Safe adaptive coarsening for ids 27 and 45 is deferred to separate research
and planning. It requires a bounded authoring/runtime contract for script and
animation amplitudes, then an envelope controller evaluated against that
contract. This plan does not change those runtime semantics.
