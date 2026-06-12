// Script-facing component structs. Each submodule defines one component kind.
// These are independent of engine-side runtime types (`MapLight` etc.) — the
// scripting module owns parallel definitions so the FFI boundary stays clean.
//
// See: context/lib/scripting.md

pub(crate) mod billboard_emitter;
pub(crate) mod fog_volume;
pub(crate) mod health;
pub(crate) mod light;
pub(crate) mod mesh;
pub(crate) mod particle;
pub(crate) mod player_movement;
pub(crate) mod sprite_visual;
pub(crate) mod weapon;
