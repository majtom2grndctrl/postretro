//! Dense, pull-based per-row staleness for one streamed SH compose pass.
//! See: context/lib/rendering_pipeline.md §4 "Sampled-row compose"
//!
//! A trigger advances a per-source epoch in O(1) instead of stamping every
//! contributing row. A row is stale when a source it belongs to fired after
//! the row was last made current (commit, residency entry, or a membership
//! change), or when staleness was carried forward across a membership change.
//! Planning therefore touches only gated, pending, and membership-changed rows.

use super::compose_plan::{ComposePass, ComposePassPlan, PassMembership};

const RESIDENT: u8 = 1 << 0;
const MEMBER_0: u8 = 1 << 1;
const MEMBER_1: u8 = 1 << 2;
const MEMBER_MASK: u8 = MEMBER_0 | MEMBER_1;
/// Staleness materialized when membership changed under a lagging row. It
/// holds until the row commits or leaves residency.
const CARRIED: u8 = 1 << 3;
const PENDING: u8 = 1 << 4;
/// The row has an entry in `pending_rows`; keeps that list duplicate-free.
const LISTED: u8 = 1 << 5;

/// Epoch sources that can fire for a pass in one planned frame.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PassTriggers {
    /// Every resident row (light-term/override change, forced full compose).
    pub(super) resident: bool,
    /// Rows with membership bit 0 (the pass's own contributing section).
    pub(super) contributing: bool,
    /// Rows with membership bit 1 (Pass B: rows whose Pass-A input changed).
    pub(super) upstream: bool,
}

impl PassTriggers {
    fn any(self) -> bool {
        self.resident || self.contributing || self.upstream
    }
}

/// Per-row state is one flag byte plus one generation, indexed by affinity row.
pub(super) struct PassStaleness {
    flags: Vec<u8>,
    /// Generation at which the row was last made current.
    composed: Vec<u64>,
    generation: u64,
    resident_epoch: u64,
    member_epochs: [u64; 2],
    /// Whether the most recent planned frame advanced `generation`. A row
    /// joining residency inherits that frame's marks, matching a row that
    /// contributed then but was only resident by this frame's plan.
    last_frame_fired: bool,
    pending_rows: Vec<u32>,
    resident_count: usize,
    /// Current resident rows, bucketed by membership bits. Lag is maintained
    /// from these so diagnostics never walk the resident set.
    current_by_members: [usize; 4],
    /// Set by a control change. Cleared when a full-repair plan commits, or
    /// when the full scan finds no stale row.
    pub(super) full_repair_pending: bool,
    #[cfg(test)]
    pub(super) visited: usize,
}

impl PassStaleness {
    /// Row capacity is fixed at level install to the level's affinity-row
    /// space: the brick grid widened to every sparse CSR row count. Every row
    /// that membership, pending work, or a gate can name comes from that
    /// installed data, so an out-of-range row is a caller bug. Debug builds
    /// assert on one; release builds ignore it rather than grow.
    pub(super) fn with_row_capacity(rows: usize) -> Self {
        Self {
            flags: vec![0; rows],
            composed: vec![0; rows],
            generation: 0,
            resident_epoch: 0,
            member_epochs: [0; 2],
            last_frame_fired: false,
            pending_rows: Vec::new(),
            resident_count: 0,
            current_by_members: [0; 4],
            full_repair_pending: false,
            #[cfg(test)]
            visited: 0,
        }
    }

    /// Forget all staleness and pending work while keeping observed
    /// membership, so every resident row starts current.
    pub(super) fn reset_generation(&mut self) {
        self.generation = 0;
        self.resident_epoch = 0;
        self.member_epochs = [0; 2];
        self.last_frame_fired = false;
        self.full_repair_pending = false;
        self.pending_rows.clear();
        self.composed.fill(0);
        self.current_by_members = [0; 4];
        for flags in &mut self.flags {
            *flags &= RESIDENT | MEMBER_MASK;
            if *flags & RESIDENT != 0 {
                self.current_by_members[members_index(*flags)] += 1;
            }
        }
    }

    /// Drop all observed membership (the residency mirrors were cleared).
    pub(super) fn clear_membership(&mut self) {
        self.flags.fill(0);
        self.pending_rows.clear();
        self.resident_count = 0;
        self.current_by_members = [0; 4];
    }

    /// Apply one row's membership as of this frame's plan. Idempotent.
    pub(super) fn observe_membership(&mut self, row: u32, membership: PassMembership) {
        let index = row as usize;
        let Some(&old) = self.flags.get(index) else {
            debug_assert!(false, "compose row {row} exceeds the planner row space");
            return;
        };
        #[cfg(test)]
        {
            self.visited += 1;
        }
        let members = member_bits(membership);
        match (old & RESIDENT != 0, membership.resident) {
            (false, false) => self.flags[index] = (old & !MEMBER_MASK) | members,
            (false, true) => {
                let carried = self.last_frame_fired
                    && ((old & MEMBER_0 != 0 && self.member_epochs[0] == self.generation)
                        || (old & MEMBER_1 != 0 && self.member_epochs[1] == self.generation));
                self.composed[index] = self.generation;
                self.flags[index] = RESIDENT
                    | members
                    | if carried { CARRIED } else { 0 }
                    | (old & (PENDING | LISTED));
                self.resident_count += 1;
                if !carried {
                    self.current_by_members[members_index(members)] += 1;
                }
            }
            (true, false) => {
                if !self.is_stale(row) {
                    self.current_by_members[members_index(old)] -= 1;
                }
                self.resident_count -= 1;
                self.flags[index] = members | (old & (PENDING | LISTED));
            }
            (true, true) => {
                if old & MEMBER_MASK == members {
                    return;
                }
                let stale = self.is_stale(row);
                if !stale {
                    self.current_by_members[members_index(old)] -= 1;
                    self.current_by_members[members_index(members)] += 1;
                }
                // Carry existing lag across the change, then exclude epochs
                // that fired before this membership began.
                self.composed[index] = self.generation;
                self.flags[index] =
                    (old & !(MEMBER_MASK | CARRIED)) | members | if stale { CARRIED } else { 0 };
            }
        }
    }

    pub(super) fn mark_pending(&mut self, row: u32) {
        let Some(flags) = self.flags.get_mut(row as usize) else {
            debug_assert!(false, "compose row {row} exceeds the planner row space");
            return;
        };
        *flags |= PENDING;
        if *flags & LISTED == 0 {
            *flags |= LISTED;
            self.pending_rows.push(row);
        }
    }

    /// Drop pending work for rows that are no longer resident. O(pending).
    pub(super) fn retain_resident_pending(&mut self) {
        let flags = &mut self.flags;
        #[cfg(test)]
        {
            self.visited += self.pending_rows.len();
        }
        self.pending_rows.retain(|&row| {
            let flags = &mut flags[row as usize];
            let keep = *flags & (PENDING | RESIDENT) == PENDING | RESIDENT;
            if !keep {
                *flags &= !(PENDING | LISTED);
            }
            keep
        });
    }

    /// Advance the firing sources' epochs. Called exactly once per planned
    /// frame, whether or not the frame records compose.
    pub(super) fn fire(&mut self, triggers: PassTriggers) {
        self.last_frame_fired = triggers.any();
        if !self.last_frame_fired {
            return;
        }
        self.generation = match self.generation.checked_add(1) {
            Some(generation) => generation,
            None => {
                self.rebase();
                1
            }
        };
        if triggers.resident {
            self.resident_epoch = self.generation;
            self.current_by_members = [0; 4];
        }
        if triggers.contributing {
            self.member_epochs[0] = self.generation;
            self.current_by_members[0b01] = 0;
            self.current_by_members[0b11] = 0;
        }
        if triggers.upstream {
            self.member_epochs[1] = self.generation;
            self.current_by_members[0b10] = 0;
            self.current_by_members[0b11] = 0;
        }
    }

    /// Generation overflow is practically unreachable. Rebasing materializes
    /// every resident row's lag so ordering survives the reset.
    fn rebase(&mut self) {
        for row in 0..self.flags.len() {
            if self.is_stale(row as u32) {
                self.flags[row] |= CARRIED;
            }
            self.composed[row] = 0;
        }
        #[cfg(test)]
        {
            self.visited += self.flags.len();
        }
        self.generation = 0;
        self.resident_epoch = 0;
        self.member_epochs = [0; 2];
    }

    pub(super) fn is_stale(&self, row: u32) -> bool {
        let index = row as usize;
        let Some(&flags) = self.flags.get(index) else {
            return false;
        };
        if flags & RESIDENT == 0 {
            return false;
        }
        if flags & CARRIED != 0 {
            return true;
        }
        let composed = self.composed[index];
        self.resident_epoch > composed
            || (flags & MEMBER_0 != 0 && self.member_epochs[0] > composed)
            || (flags & MEMBER_1 != 0 && self.member_epochs[1] > composed)
    }

    pub(super) fn plan_into(
        &mut self,
        pass: ComposePass,
        gated: &[u32],
        force_full_resident: bool,
        plan: &mut ComposePassPlan,
    ) {
        plan.reset(pass, self.generation, self.full_repair_pending);
        #[cfg(test)]
        {
            self.visited += self.pending_rows.len();
        }
        plan.rows.extend(
            self.pending_rows.iter().copied().filter(|&row| {
                self.flags[row as usize] & (PENDING | RESIDENT) == PENDING | RESIDENT
            }),
        );
        if force_full_resident || self.full_repair_pending {
            // Intentionally full: exactness bypass and control-change repair.
            #[cfg(test)]
            {
                self.visited += self.flags.len();
            }
            let planned = plan.rows.len();
            plan.rows
                .extend((0..self.flags.len() as u32).filter(|&row| self.is_stale(row)));
            // No stale row means the repair is already complete. The renderer
            // never commits an empty plan, so waiting for commit would rescan
            // the whole row space every frame. Rows that become resident later
            // start current against the control change.
            if plan.rows.len() == planned {
                self.full_repair_pending = false;
            }
        } else {
            #[cfg(test)]
            {
                self.visited += gated.len();
            }
            // SPIKE (sh-compose-half-rate): with POSTRETRO_SPIKE_SH_HALF_RATE=1,
            // compose only gated rows whose parity matches this frame's. A
            // skipped row is not committed, so it stays stale and composes on
            // the next frame of its parity; its stored slot keeps the last
            // composed value meanwhile (the composed atlases persist).
            // Pending (residency) rows above are never skipped.
            let parity = spike_half_rate_parity();
            plan.rows.extend(gated.iter().copied().filter(|&row| {
                parity.is_none_or(|p| row % 2 == p) && self.is_stale(row)
            }));
        }
        plan.rows.sort_unstable();
        plan.rows.dedup();
        plan.lagged_rows = plan.rows.iter().filter(|&&row| self.is_stale(row)).count();
    }

    pub(super) fn commit(&mut self, plan: &ComposePassPlan) {
        #[cfg(test)]
        {
            self.visited += plan.rows.len();
        }
        for &row in &plan.rows {
            let index = row as usize;
            if index >= self.flags.len() {
                continue;
            }
            if self.is_stale(row) {
                self.current_by_members[members_index(self.flags[index])] += 1;
            }
            self.composed[index] = plan.generation;
            self.flags[index] &= !(CARRIED | PENDING);
        }
        if plan.full_repair {
            self.full_repair_pending = false;
        }
    }

    /// Resident rows still lagging. O(1): maintained incrementally.
    pub(super) fn lagging_count(&self) -> usize {
        self.resident_count - self.current_by_members.iter().sum::<usize>()
    }

    /// Move the generation counter while preserving every comparison, so a
    /// test can drive the overflow rebase.
    #[cfg(test)]
    pub(super) fn set_generation_for_test(&mut self, generation: u64) {
        let offset = generation
            .checked_sub(self.generation)
            .expect("test generations only move forward");
        self.generation = generation;
        self.resident_epoch += offset;
        for epoch in &mut self.member_epochs {
            *epoch += offset;
        }
        for composed in &mut self.composed {
            *composed += offset;
        }
    }

    pub(super) fn resident(&self, row: u32) -> bool {
        self.flags
            .get(row as usize)
            .is_some_and(|flags| flags & RESIDENT != 0)
    }

    /// Membership as last observed for `row`.
    #[cfg(test)]
    pub(super) fn observed_membership(&self, row: u32) -> PassMembership {
        let flags = self.flags.get(row as usize).copied().unwrap_or(0);
        PassMembership {
            resident: flags & RESIDENT != 0,
            contributing: flags & MEMBER_0 != 0,
            upstream: flags & MEMBER_1 != 0,
        }
    }
}

fn member_bits(membership: PassMembership) -> u8 {
    (if membership.contributing { MEMBER_0 } else { 0 })
        | (if membership.upstream { MEMBER_1 } else { 0 })
}

fn members_index(flags: u8) -> usize {
    usize::from((flags & MEMBER_MASK) >> 1)
}

static SPIKE_HALF_RATE_FRAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// SPIKE: advance once per planned frame (called from `plan_frame_into`).
pub(super) fn spike_half_rate_advance_frame() {
    SPIKE_HALF_RATE_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// SPIKE: `Some(parity)` of rows to compose this frame when half-rate is on.
fn spike_half_rate_parity() -> Option<u32> {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let enabled = *ENABLED.get_or_init(|| {
        let on = std::env::var("POSTRETRO_SPIKE_SH_HALF_RATE").is_ok_and(|v| v == "1");
        if on {
            log::warn!("[SPIKE] SH compose half-rate: gated rows compose every 2nd frame by parity");
        }
        on
    });
    enabled.then(|| SPIKE_HALF_RATE_FRAME.load(std::sync::atomic::Ordering::Relaxed) % 2)
}
