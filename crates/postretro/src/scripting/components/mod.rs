// Script-facing component structs. Each submodule defines one component kind.
// These are independent of engine-side runtime types (`MapLight` etc.) — the
// scripting module owns parallel definitions so the FFI boundary stays clean.
//
// See: context/plans/ready/scripting-foundation/plan-2-light-entity.md

pub(crate) mod billboard_emitter;
pub(crate) mod light;
pub(crate) mod particle;
pub(crate) mod sprite_visual;
