/// Host-minted per-player session address.
///
/// This type lives in foundation so both the engine and future floor-level
/// per-seat storage can name it. The network wire carries only a bare `u16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seat(pub u16);

/// Remote clients a listen host accepts at once. The host's own seat is extra.
pub const MAX_CLIENTS: usize = 8;

/// Seats one session holds at most while every place is filled: the host's
/// seat plus `MAX_CLIENTS` remote seats. Capacity of the per-seat player list.
pub const SESSION_PLAYER_CAPACITY: usize = MAX_CLIENTS + 1;
