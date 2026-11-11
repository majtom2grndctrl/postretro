// The mover-absent later bounces' memo: id 34's later bounces per static
// patch, keyed on id 34's whole volume key, so a build whose id 34 comes
// from the cache re-bakes a mover without re-tracing them.
// See: context/lib/build_pipeline.md §Build Cache

use std::io::Write;

use super::absent_bounces::AbsentLaterBounces;
use crate::cache::{CacheKey, StageCache};
use crate::surface_radiance::LATER_BOUNCE_COUNT;

/// The memo's cache stage. Bump the version when the payload changes. What
/// the layers are computed from (the patch set, bounce one's layers, the
/// bounce materials, the later bounces' count, rays and patch lookup, the
/// fill and the tracer epoch) all fold through id 34's volume key.
const STAGE_ID: &str = "mover_absent_bounces";
/// 1: id 34's later bounces, each a front side per static patch.
const STAGE_VERSION: u32 = 1;

/// The memo under one id-34 volume key.
pub(crate) struct AbsentBounceMemo<'c> {
    cache: &'c StageCache,
    key: CacheKey,
}

impl<'c> AbsentBounceMemo<'c> {
    /// `volume_key` is id 34's whole volume key: the layers are a function
    /// of exactly what it folds.
    pub(crate) fn new(cache: &'c StageCache, volume_key: &CacheKey) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"mover absent later bounces\0");
        hasher.update(volume_key.as_filename().as_bytes());
        Self {
            cache,
            key: CacheKey::new(STAGE_ID, STAGE_VERSION, hasher.finalize().as_bytes()),
        }
    }

    /// The layers over `patch_count` static patches, or `None` on a miss. A
    /// payload over another patch count, with more layers than id 34
    /// gathers, or torn, is corrupt: a miss that re-traces. Logs one
    /// `[cache]` line. Runs outside any governor permit.
    pub(crate) fn get(&self, patch_count: usize) -> Option<AbsentLaterBounces> {
        let found = self.cache.get(&self.key).and_then(|bytes| {
            #[cfg(test)]
            let _payload = crate::live_bytes::hold_vec("mover absent bounce memo", &bytes);
            let decoded = decode(&bytes, patch_count);
            if decoded.is_none() {
                log::warn!("[cache] corrupt {STAGE_ID} entry, re-tracing");
            }
            decoded
        });
        log::info!(
            "[cache] {STAGE_ID} {}",
            if found.is_some() { "hit" } else { "miss" }
        );
        found
    }

    /// Keep the entry in this build's record without reading it, so a warm
    /// build that needs no layers still holds them for the next mover edit.
    pub(crate) fn mark_used(&self) {
        self.cache.mark_used(&self.key);
    }

    /// Store `bounces` over `patch_count` static patches, streamed. Runs
    /// outside any governor permit.
    pub(crate) fn put(&self, bounces: &AbsentLaterBounces, patch_count: usize) {
        let len = 4 + 8 + (bounces.layers.len() * patch_count * size_of::<[f32; 3]>()) as u64;
        self.cache.put_streamed(&self.key, len, |writer| {
            encode(writer, bounces, patch_count)
        });
    }
}

/// `u32` layer count, `u64` patch count, then each layer's front sides in
/// patch order as little-endian `f32` RGB.
fn encode(
    writer: &mut dyn Write,
    bounces: &AbsentLaterBounces,
    patch_count: usize,
) -> std::io::Result<()> {
    writer.write_all(&(bounces.layers.len() as u32).to_le_bytes())?;
    writer.write_all(&(patch_count as u64).to_le_bytes())?;
    for layer in &bounces.layers {
        debug_assert_eq!(layer.len(), patch_count, "one front side per patch");
        for value in layer {
            for component in value {
                writer.write_all(&component.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

fn decode(bytes: &[u8], patch_count: usize) -> Option<AbsentLaterBounces> {
    let layers = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
    let patches = u64::from_le_bytes(bytes.get(4..12)?.try_into().ok()?);
    if layers > LATER_BOUNCE_COUNT || patches != patch_count as u64 {
        return None;
    }
    let body = &bytes[12..];
    let layer_bytes = patch_count.checked_mul(size_of::<[f32; 3]>())?;
    if body.len() != layer_bytes.checked_mul(layers as usize)? {
        return None;
    }
    let layers = (0..layers as usize)
        .map(|index| {
            body[index * layer_bytes..(index + 1) * layer_bytes]
                .as_chunks::<12>()
                .0
                .iter()
                .map(|value| {
                    [0, 1, 2].map(|c| {
                        f32::from_le_bytes(value[4 * c..4 * c + 4].try_into().expect("four bytes"))
                    })
                })
                .collect()
        })
        .collect();
    Some(AbsentLaterBounces { layers })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounces() -> AbsentLaterBounces {
        AbsentLaterBounces {
            layers: (0..3)
                .map(|layer| {
                    (0..5)
                        .map(|patch| [layer as f32, patch as f32 * 0.5, -1.25e-7])
                        .collect()
                })
                .collect(),
        }
    }

    fn encoded(bounces: &AbsentLaterBounces, patch_count: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        encode(&mut bytes, bounces, patch_count).expect("encodes");
        bytes
    }

    // The memo hands back the layers bit for bit, and refuses another patch
    // count, more layers than id 34 gathers and a torn payload.
    #[test]
    fn the_absent_bounce_payload_round_trips_and_refuses_malformed_ones() {
        let bounces = bounces();
        let bytes = encoded(&bounces, 5);
        assert_eq!(decode(&bytes, 5), Some(bounces.clone()));
        assert_eq!(decode(&bytes, 4), None, "another patch count");
        let mut many = bytes.clone();
        many[0..4].copy_from_slice(&(LATER_BOUNCE_COUNT + 1).to_le_bytes());
        assert_eq!(decode(&many, 5), None, "more layers than id 34 gathers");
        let mut torn = bytes;
        torn.pop();
        assert_eq!(decode(&torn, 5), None, "a torn layer");
        let none = AbsentLaterBounces::default();
        assert_eq!(decode(&encoded(&none, 5), 5), Some(none), "no layer");
    }
}
