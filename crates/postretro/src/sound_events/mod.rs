// Gameplay sound events on the app drain: turns host-local emissions into
// anchored sound requests after the tick loop, and answers where each anchor
// is each frame. The sim supplies emitter identity only.
// See: context/lib/audio.md §4

mod anchors;
mod client_reload;
mod descriptors;
mod movers;

pub(crate) use anchors::{AnchorScene, listener_attached_key};
pub(crate) use client_reload::ClientReloadEdges;
pub(crate) use descriptors::{
    DescriptorSoundTable, ai_sounds, movement_sound, warn_unknown_sound_keys,
    weapon_emission_sound, weapon_sound,
};
pub(crate) use movers::{MoverEdge, resolve_mover_edges};
