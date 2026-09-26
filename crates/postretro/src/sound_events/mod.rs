// Gameplay sound events on the app drain: turns host-local emissions into
// anchored sound requests after the tick loop, and answers where each anchor
// is each frame. The sim supplies emitter identity only.
// See: context/lib/audio.md §4

mod anchors;
mod movers;

pub(crate) use anchors::{AnchorScene, listener_attached_key};
pub(crate) use movers::{MoverEdge, resolve_mover_edges};
