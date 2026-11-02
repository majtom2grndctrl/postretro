// Entity-resident data-context descriptor types.
// See: context/lib/scripting.md §12.

pub mod types {
    pub mod entity;
    pub mod life_states;
    pub mod player_events;
    pub mod reactions;
}

pub mod validate {
    pub mod entities;
}

pub use postretro_foundation::data_descriptors::*;

pub use types::entity::*;
pub use types::life_states::*;
pub use types::player_events::*;
pub use types::reactions::*;
pub use validate::entities::*;
