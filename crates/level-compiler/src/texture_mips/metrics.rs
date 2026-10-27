// Test-only observations of decode calls and one-texture material-bake lifetimes.
// Measures explicitly observed buffers, not hidden allocations inside the image crate.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    pub decode_attempts: BTreeMap<PathBuf, usize>,
    pub decode_successes: BTreeMap<PathBuf, usize>,
    pub largest_decoded_rgba: usize,
    pub texture_count: usize,
    pub active_textures: usize,
    pub texture_high_water: usize,
    pub owned_high_water: usize,
    pub active_stats_borrows: usize,
    pub stats_borrow_high_water: usize,
    pub borrowed_high_water: usize,
    pub stats_calls: usize,
}

#[derive(Default)]
struct State {
    snapshot: Snapshot,
    owned_bytes: usize,
}

thread_local! {
    static OBSERVATION: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// A per-thread observation avoids cross-test totals or serialization locks.
pub(crate) struct Observation;

impl Observation {
    pub fn start() -> Self {
        OBSERVATION.with(|state| {
            let mut state = state.borrow_mut();
            assert!(state.is_none(), "material observations must not overlap");
            *state = Some(State::default());
        });
        Self
    }

    pub fn snapshot(&self) -> Snapshot {
        OBSERVATION.with(|state| state.borrow().as_ref().unwrap().snapshot.clone())
    }
}

pub(crate) fn observe<T>(run: impl FnOnce() -> T) -> (T, Snapshot) {
    let observation = Observation::start();
    let result = run();
    (result, observation.snapshot())
}

impl Drop for Observation {
    fn drop(&mut self) {
        OBSERVATION.with(|state| {
            let state = state.borrow_mut().take().unwrap();
            assert_eq!(state.snapshot.active_textures, 0, "texture lifetime leaked");
            assert_eq!(
                state.snapshot.active_stats_borrows, 0,
                "stats borrow leaked"
            );
            assert_eq!(state.owned_bytes, 0, "observed buffers outlived texture");
        });
    }
}

pub(super) fn decode_attempt(path: &Path) {
    OBSERVATION.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            *state
                .snapshot
                .decode_attempts
                .entry(path.to_owned())
                .or_default() += 1;
        }
    });
}

pub(super) fn decode_success(path: &Path, rgba_bytes: usize) {
    OBSERVATION.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            *state
                .snapshot
                .decode_successes
                .entry(path.to_owned())
                .or_default() += 1;
            state.snapshot.largest_decoded_rgba =
                state.snapshot.largest_decoded_rgba.max(rgba_bytes);
        }
    });
}

/// Covers the actual loop iteration, including sidecar serialization/write.
pub(super) struct TextureLifetime;

impl TextureLifetime {
    pub fn begin() -> Self {
        OBSERVATION.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                let snapshot = &mut state.snapshot;
                snapshot.texture_count += 1;
                snapshot.active_textures += 1;
                snapshot.texture_high_water =
                    snapshot.texture_high_water.max(snapshot.active_textures);
                assert_eq!(snapshot.active_textures, 1, "texels span multiple textures");
                assert_eq!(
                    state.owned_bytes, 0,
                    "previous texture retained observed buffers"
                );
            }
        });
        Self
    }

    /// Count actual held source/slot/serialized buffers at ownership seams.
    pub fn observe_owned(&self, bytes: usize) {
        OBSERVATION.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                state.owned_bytes = bytes;
                state.snapshot.owned_high_water = state.snapshot.owned_high_water.max(bytes);
            }
        });
    }

    pub fn borrow_stats(&self, bytes: usize) -> StatsBorrow {
        OBSERVATION.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                assert!(
                    bytes <= state.owned_bytes,
                    "stats borrow exceeds observed owned payloads"
                );
                let snapshot = &mut state.snapshot;
                snapshot.active_stats_borrows += 1;
                snapshot.stats_calls += 1;
                snapshot.stats_borrow_high_water = snapshot
                    .stats_borrow_high_water
                    .max(snapshot.active_stats_borrows);
                snapshot.borrowed_high_water = snapshot.borrowed_high_water.max(bytes);
                assert_eq!(snapshot.active_textures, 1);
            }
        });
        StatsBorrow
    }
}

impl Drop for TextureLifetime {
    fn drop(&mut self) {
        OBSERVATION.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                assert_eq!(state.snapshot.active_stats_borrows, 0);
                state.snapshot.active_textures -= 1;
                state.owned_bytes = 0;
            }
        });
    }
}

pub(super) struct StatsBorrow;

impl Drop for StatsBorrow {
    fn drop(&mut self) {
        OBSERVATION.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                state.snapshot.active_stats_borrows -= 1;
            }
        });
    }
}

/// Allocated slot payloads can coexist with source bytes and encoded sidecars.
pub(super) fn payload_bytes<'a>(
    slots: impl IntoIterator<Item = &'a postretro_level_format::prm::PrmSlot>,
) -> usize {
    slots.into_iter().map(|slot| slot.payload.capacity()).sum()
}

pub(super) fn stats_bytes(
    diffuse: Option<&postretro_level_format::prm::PrmSlot>,
    emissive: Option<&postretro_level_format::prm::PrmSlot>,
) -> usize {
    diffuse.map_or(0, |diffuse| {
        usize::from(diffuse.width) * usize::from(diffuse.height) * 4
            + emissive.map_or(0, |slot| {
                usize::from(slot.width) * usize::from(slot.height) * 4
            })
    })
}
