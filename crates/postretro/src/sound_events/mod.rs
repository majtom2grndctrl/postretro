// Gameplay sound events on the app drain: turns host-local emissions into
// anchored sound requests after the tick loop, and answers where each anchor
// is each frame. The sim supplies emitter identity only.
// See: context/lib/audio.md §4

mod anchors;
mod client_overheat;
mod client_reload;
mod descriptors;
mod movers;

pub(crate) use anchors::{AnchorScene, listener_attached_key, mover_entity};
pub(crate) use client_overheat::{ClientOverheatEdge, OverheatReading, ProjectedHeatWeapon};
pub(crate) use client_reload::{ClientReloadEdges, ProjectedWeapon, ReloadReading};
pub(crate) use descriptors::{
    DescriptorSoundTable, LocalFluids, ai_sounds, frozen_sound, movement_sound,
    warn_unknown_sound_keys, weapon_emission_addresses, weapon_emission_sound, weapon_sound,
};
pub(crate) use movers::{MoverEdge, resolve_mover_edges};

mod teleporters;
pub(crate) use teleporters::{arrival_sound, observed_arrival_sounds};
