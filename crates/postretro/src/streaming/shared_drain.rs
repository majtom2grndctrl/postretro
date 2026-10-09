//! One level-scope drain step: every resource's ready items, admitted once.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

use super::drain_budget::{
    DrainBytesOverflow, DrainItem, MAX_INSTALL_DECODED_BYTES_PER_DRAIN, admit_drain,
};
use super::request::StreamResource;

/// The merged ready list of one drain. Each streamed resource offers its
/// ready items, [`Self::admit`] orders and admits them once against the one
/// per-drain budget, and each resource then takes its own admitted prefix.
///
/// Owned at level scope and reused across drains, so a steady drain with
/// nothing ready allocates nothing.
#[derive(Debug, Default)]
pub(crate) struct SharedDrain {
    items: Vec<DrainItem>,
    admitted: usize,
}

/// One resource's share of an admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ResourceAdmission {
    pub(crate) admitted: usize,
    pub(crate) bytes: u64,
    /// The resource offered ready work this drain left behind.
    pub(crate) left_behind: bool,
}

impl SharedDrain {
    /// Starts a drain: forgets the previous drain's items, keeps capacity.
    pub(crate) fn begin(&mut self) {
        self.items.clear();
        self.admitted = 0;
    }

    pub(crate) fn offer(&mut self, item: DrainItem) {
        self.items.push(item);
    }

    /// Orders every offered item on the shared scale and admits the leading
    /// items that fit the per-drain budget (the first always).
    #[cfg_attr(
        not(feature = "capture"),
        allow(dead_code, reason = "capture and tests preload synchronously")
    )]
    pub(crate) fn admit(&mut self) -> Result<(), DrainBytesOverflow> {
        self.admit_within(MAX_INSTALL_DECODED_BYTES_PER_DRAIN)
    }

    /// [`Self::admit`] against `budget` instead of the in-play budget.
    pub(crate) fn admit_within(&mut self, budget: u64) -> Result<(), DrainBytesOverflow> {
        self.admitted = admit_drain(&mut self.items, budget)?.admitted;
        Ok(())
    }

    /// Admitted keys of `resource`, in admission order.
    pub(crate) fn admitted_keys(&self, resource: StreamResource) -> impl Iterator<Item = u32> + '_ {
        self.items[..self.admitted]
            .iter()
            .filter(move |item| item.rank.resource() == resource)
            .map(|item| item.rank.key())
    }

    pub(crate) fn admission(&self, resource: StreamResource) -> ResourceAdmission {
        let mut share = ResourceAdmission::default();
        for item in self.items[..self.admitted]
            .iter()
            .filter(|item| item.rank.resource() == resource)
        {
            share.admitted += 1;
            share.bytes += item.bytes;
        }
        share.left_behind = self.items[self.admitted..]
            .iter()
            .any(|item| item.rank.resource() == resource);
        share
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.items.capacity()
    }
}
