// Data-context descriptors: player events (`players().on`).
// See: context/lib/scripting.md §12 (Player events)

use postretro_foundation::ir::IrNode;

/// The edge of a player's condition a player event fires on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerEventEdge {
    /// `becomes(cond)`: false → true.
    Becomes,
    /// `ceases(cond)`: true → false.
    Ceases,
}

impl PlayerEventEdge {
    pub fn from_wire(word: &str) -> Option<Self> {
        match word {
            "becomes" => Some(Self::Becomes),
            "ceases" => Some(Self::Ceases),
            _ => None,
        }
    }

    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Becomes => "becomes",
            Self::Ceases => "ceases",
        }
    }
}

/// `players().on(edge(condition), fire, { levels? })`, returned under a
/// `playerEvents` manifest key. Wire: `{ edge, condition, fire, levels? }`.
/// `levels` scopes a `ModManifest` entry; a level script's entry carrying it
/// is skipped at parse with a warning, and its siblings install.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerEventDescriptor {
    pub edge: PlayerEventEdge,
    /// A Bool fluent expression. Plain per-player reads bind to the player
    /// being evaluated.
    pub condition: IrNode,
    pub fire: Vec<String>,
    pub levels: Vec<String>,
    /// The entry's position in its authored `playerEvents` array, counted
    /// from 0 in both runtimes. Diagnostics only — not part of the entry's
    /// content: a skipped sibling leaves later survivors at their authored
    /// positions, and edge memory and dedupe key on `condition` and `edge`.
    pub authored_index: usize,
}

/// Where a composed player event was declared, for diagnostics that name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEventSource {
    /// `ModManifest.playerEvents[index]`.
    ModGlobal { index: usize },
    /// `setupLevel().playerEvents[index]`.
    Level { index: usize },
}

impl std::fmt::Display for PlayerEventSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModGlobal { index } => write!(formatter, "ModManifest.playerEvents[{index}]"),
            Self::Level { index } => write!(formatter, "setupLevel().playerEvents[{index}]"),
        }
    }
}

/// One active player event after composition, in composed order.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposedPlayerEvent {
    pub descriptor: PlayerEventDescriptor,
    pub source: PlayerEventSource,
}
