//! Per-command host intents fed to `build_sim_command`.

/// Host-side intents for one sim command, beyond what the action snapshot
/// carries. `Default` is everything off.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SimIntents {
    /// Level: crouch intent held this tick.
    pub(crate) crouch: bool,
    /// Level: sprint intent held this tick (becomes `running`).
    pub(crate) sprint: bool,
    /// Edge: dash pressed this tick.
    pub(crate) dash: bool,
    /// Edge: shoot pressed this tick (the `active` level comes from the snapshot).
    pub(crate) shoot: bool,
    /// Edge: a wieldable-slot select was pressed this tick.
    pub(crate) select: bool,
    /// Edge: use pressed this tick.
    pub(crate) use_: bool,
    /// Edge: drop pressed this tick.
    pub(crate) drop: bool,
    /// Level: assist held, carried across ticks.
    pub(crate) assist: bool,
}
