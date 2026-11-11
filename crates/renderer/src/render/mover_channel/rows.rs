// Per-mover row lists the streamed planner recomposes when a mover travels,
// per pass, and the rows whose membership the channel widens.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

/// Rows per mover slot, CSR: which affinity rows carry a mover's entries in
/// one pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::render) struct MoverRows {
    offsets: Vec<u32>,
    rows: Vec<u32>,
}

impl MoverRows {
    pub(super) fn from_sets(sets: Vec<Vec<u32>>) -> Self {
        let mut offsets = Vec::with_capacity(sets.len() + 1);
        let mut rows = Vec::new();
        offsets.push(0);
        for mut set in sets {
            set.sort_unstable();
            set.dedup();
            rows.extend(set);
            offsets.push(rows.len() as u32);
        }
        Self { offsets, rows }
    }

    /// Rows carrying mover slot `slot`'s entries, ascending.
    pub(in crate::render) fn rows(&self, slot: u32) -> &[u32] {
        let slot = slot as usize;
        match self.offsets.get(slot..slot + 2) {
            Some(range) => &self.rows[range[0] as usize..range[1] as usize],
            None => &[],
        }
    }

    /// Whether slot `slot` carries any entry in this pass.
    pub(super) fn has_rows(&self, slot: u32) -> bool {
        !self.rows(slot).is_empty()
    }
}

/// What the streamed planner reads of the channel: per-mover row lists for
/// each pass, and the rows whose membership the channel widens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::render) struct MoverChannelRows {
    /// Pass A: rows carrying a mover's gated static lights. Pass B follows
    /// Pass A's planned rows on its own.
    pub(in crate::render) static_by_mover: MoverRows,
    /// Pass B: rows carrying a mover's gated animated-baked lights.
    pub(in crate::render) animated_by_mover: MoverRows,
    /// The indirect pass: rows carrying a mover's bounce entries.
    pub(in crate::render) bounce_by_mover: MoverRows,
    /// Rows with a gated static light that can promote: a promotion weight
    /// change moves their handoff even with no id-41 entry there.
    pub(in crate::render) promotable: Vec<u32>,
    /// Rows with a gated animated-baked light: its curve moves them even
    /// with no id-45 entry there.
    pub(in crate::render) animated: Vec<u32>,
}

/// A level with no channel, borrowable for any lifetime: planner tests that
/// hand the frame no mover rows.
#[cfg(test)]
pub(in crate::render) static NO_MOVER_ROWS: MoverChannelRows = MoverChannelRows {
    static_by_mover: MoverRows {
        offsets: Vec::new(),
        rows: Vec::new(),
    },
    animated_by_mover: MoverRows {
        offsets: Vec::new(),
        rows: Vec::new(),
    },
    bounce_by_mover: MoverRows {
        offsets: Vec::new(),
        rows: Vec::new(),
    },
    promotable: Vec::new(),
    animated: Vec::new(),
};

impl MoverChannelRows {
    pub(in crate::render) fn has_promotable(&self, row: u32) -> bool {
        self.promotable.binary_search(&row).is_ok()
    }

    pub(in crate::render) fn has_animated(&self, row: u32) -> bool {
        self.animated.binary_search(&row).is_ok()
    }
}
