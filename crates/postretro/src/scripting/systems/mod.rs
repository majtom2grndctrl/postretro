// Per-frame systems that bridge the scripting surface to other engine
// subsystems. Each system is plain data-and-logic — no wgpu types, no input
// handles — the owning subsystem (renderer, audio, …) consumes the system's
// outputs through a narrow API.
//
// See: context/plans/ready/scripting-foundation/plan-2-light-entity.md §Sub-plan 4

pub(crate) mod light_bridge;
