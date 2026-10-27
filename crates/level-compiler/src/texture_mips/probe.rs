// Zero-cost seams where the material bake reports decodes and held texel buffers.
// See: context/lib/build_pipeline.md §Baked texture mips

use std::path::Path;

use postretro_level_format::prm::PrmSlot;

#[cfg(test)]
use super::probe_test_metrics as metrics;

/// One bake-loop iteration's view of the texel buffers it holds. Outside
/// `cfg(test)` it is a unit type with empty inline methods, and every byte
/// count is a closure, so production builds never compute one.
#[cfg(not(test))]
pub(super) struct TextureProbe;

#[cfg(not(test))]
#[must_use]
pub(super) struct StatsBorrow;

#[cfg(test)]
pub(super) use metrics::StatsBorrow;

#[cfg(not(test))]
impl TextureProbe {
    #[inline(always)]
    pub fn begin() -> Self {
        Self
    }

    #[inline(always)]
    pub fn sources(&mut self, _bytes: impl FnOnce() -> usize) {}

    #[inline(always)]
    pub fn holding(&self, _bytes: impl FnOnce() -> usize) {}

    #[inline(always)]
    pub fn stats_borrow(&self, _bytes: impl FnOnce() -> usize) -> StatsBorrow {
        StatsBorrow
    }

    #[inline(always)]
    pub fn decode_attempt(_path: &Path) {}

    #[inline(always)]
    pub fn decode_success(_path: &Path, _rgba_bytes: impl FnOnce() -> usize) {}
}

#[cfg(test)]
pub(super) struct TextureProbe {
    lifetime: metrics::TextureLifetime,
    source_bytes: usize,
}

#[cfg(test)]
impl TextureProbe {
    pub fn begin() -> Self {
        Self {
            lifetime: metrics::TextureLifetime::begin(),
            source_bytes: 0,
        }
    }

    /// Source PNG bytes stay held for the rest of the iteration.
    pub fn sources(&mut self, bytes: impl FnOnce() -> usize) {
        self.source_bytes = bytes();
        self.lifetime.observe_owned(self.source_bytes);
    }

    /// Buffers held beside the sources at an ownership seam; zero resets to
    /// the sources alone.
    pub fn holding(&self, bytes: impl FnOnce() -> usize) {
        self.lifetime.observe_owned(self.source_bytes + bytes());
    }

    /// Texels the stats pass borrows, held until the guard drops.
    pub fn stats_borrow(&self, bytes: impl FnOnce() -> usize) -> StatsBorrow {
        self.lifetime.borrow_stats(bytes())
    }

    pub fn decode_attempt(path: &Path) {
        metrics::decode_attempt(path);
    }

    pub fn decode_success(path: &Path, rgba_bytes: impl FnOnce() -> usize) {
        metrics::decode_success(path, rgba_bytes());
    }
}

/// Allocated slot payloads can coexist with source bytes and encoded sidecars.
pub(super) fn payload_bytes<'a>(slots: impl IntoIterator<Item = &'a PrmSlot>) -> usize {
    slots.into_iter().map(|slot| slot.payload.capacity()).sum()
}

/// Mip-zero texels of the colour slots the stats pass reads.
pub(super) fn stats_bytes(diffuse: Option<&PrmSlot>, emissive: Option<&PrmSlot>) -> usize {
    let mip_zero = |slot: &PrmSlot| usize::from(slot.width) * usize::from(slot.height) * 4;
    diffuse.map_or(0, |diffuse| {
        mip_zero(diffuse) + emissive.map_or(0, mip_zero)
    })
}
