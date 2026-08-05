/// Host-minted per-player session address.
///
/// This type lives in foundation so both the engine and future floor-level
/// per-seat storage can name it. The network wire carries only a bare `u16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Seat(pub u16);
