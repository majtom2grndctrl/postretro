// Which lights the direct half gates only at a mid pose, per mover: the
// count the mover light gate's report adds beside its per-terminus rows.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover light gate, Report)

use std::collections::{BTreeMap, HashMap};

use postretro_level_format::mover_probe_channel::MoverLightNamespace;

use super::direct::{ChannelLight, MoverDirectBake};
use crate::mover_light_gate::MidPoseOnlyLights;

/// Per mover, the lights in `bake` whose entries hold q below 255 at some
/// mid pose for some probe and 255 at both termini for every probe. Each
/// light is named by its source index, read from the `lights` the bake
/// gated. A mid pose that encloses a probe stores its traced q, so a light
/// whose only block is an enclosing mid pose counts here, unless edge copy
/// replaced that q: the first and last read pose of a run of enclosing
/// poses carry their clear neighbours' q, so a light blocked only by a run
/// of one or two read poses is not counted. An enclosing terminus keeps
/// 255, so it never makes a light terminus-gated.
pub(crate) fn mid_pose_only_lights(
    bake: &MoverDirectBake,
    lights: &[ChannelLight<'_>],
) -> MidPoseOnlyLights {
    let source_of: HashMap<_, usize> = lights
        .iter()
        .map(|light| ((light.namespace, light.light_index), light.source_index))
        .collect();
    MidPoseOnlyLights::from_pairs(mid_pose_only_pairs(bake, |namespace, light_index| {
        *source_of
            .get(&(namespace, light_index))
            .expect("every baked light is one the bake gated")
    }))
}

/// `(mover id, source index)` per light and mover gated only at a mid pose,
/// naming each light by `source_of(namespace, light index)`.
fn mid_pose_only_pairs(
    bake: &MoverDirectBake,
    source_of: impl Fn(MoverLightNamespace, u32) -> usize,
) -> Vec<(u32, usize)> {
    let mut pairs = Vec::new();
    for light in &bake.lights {
        // Per mover: (gated at a terminus, gated at a mid pose).
        let mut gated: BTreeMap<u32, (bool, bool)> = BTreeMap::new();
        for entry in &light.entries {
            let poses = entry.pose_count;
            let flags = gated.entry(entry.mover_id).or_default();
            for q in entry.visibility.chunks_exact(poses) {
                flags.0 |= q[0] < 255 || q[poses - 1] < 255;
                flags.1 |= q[1..poses - 1].iter().any(|&q| q < 255);
            }
        }
        let source = source_of(light.namespace, light.light_index);
        pairs.extend(
            gated
                .into_iter()
                .filter(|&(_, (terminus, mid))| mid && !terminus)
                .map(|(mover_id, _)| (mover_id, source)),
        );
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::super::direct::{DirectEntry, LightEntries};
    use super::*;

    /// An entry of `mover` with `poses` poses, its probes' q in pose order.
    fn entry(mover_id: u32, poses: usize, probes: &[&[u8]]) -> DirectEntry {
        DirectEntry {
            cell: 0,
            mover_id,
            pose_count: poses,
            visibility: probes.concat(),
            tiles: Vec::new(),
        }
    }

    fn light(light_index: u32, entries: Vec<DirectEntry>) -> LightEntries {
        LightEntries {
            namespace: MoverLightNamespace::StaticLight,
            light_index,
            entries,
        }
    }

    // A light counts for a mover only where some probe's q drops at a mid
    // pose and no probe's q drops at either terminus, over every cell's
    // entry of that mover. A two-pose mover has no mid pose to count.
    #[test]
    fn a_light_counts_only_where_a_mid_pose_alone_gates_it() {
        let bake = MoverDirectBake {
            lights: vec![
                // Mover 1: a mid pose alone, in a second cell's entry.
                // Mover 2: its first terminus also gates the light.
                light(
                    0,
                    vec![
                        entry(1, 4, &[&[255, 255, 255, 255]]),
                        entry(1, 4, &[&[255, 255, 255, 255], &[255, 255, 40, 255]]),
                        entry(2, 3, &[&[255, 10, 255], &[200, 255, 255]]),
                    ],
                ),
                // Mover 1 again: the last terminus gates it. Mover 3: two
                // poses, gated at both.
                light(
                    1,
                    vec![entry(1, 4, &[&[255, 0, 0, 254]]), entry(3, 2, &[&[0, 0]])],
                ),
                // Mover 2: its only mid pose.
                light(2, vec![entry(2, 3, &[&[255, 128, 255]])]),
            ],
        };
        let pairs = mid_pose_only_pairs(&bake, |_, light_index| 10 + light_index as usize);
        assert_eq!(pairs, [(1, 10), (2, 12)]);
    }
}
