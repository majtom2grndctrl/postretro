// The gate's `-v` and debug lines. Never warn: light through a closed mover
// is today's normal.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use super::pose::{MoverPoses, Terminus};
use super::side_count::SideCount;
use super::tally::{GateTally, RowKey, RowTally};

/// Lights an `info` line names before "and N more".
const NAMED_LIGHTS: usize = 4;

/// Log each not-analysed mover at debug, then per (mover, terminus) with
/// rows one `info` line naming its gated lights, and per row one debug line
/// with its counts. `light_label` names a light by source index.
pub(crate) fn log_static_rows(
    poses: &MoverPoses,
    tally: &GateTally,
    light_label: impl Fn(usize) -> String,
) {
    for mover in &poses.not_analysed {
        log::debug!(
            "[MoverLight] {} not analysed: {}",
            mover.label,
            mover.reason
        );
    }
    let rows: Vec<(&RowKey, &RowTally)> = tally.rows().collect();
    for group in
        rows.chunk_by(|(a, _), (b, _)| (a.mover_id, a.terminus) == (b.mover_id, b.terminus))
    {
        let (key, _) = group[0];
        let Some(mover) = poses.mover(key.mover_id) else {
            continue;
        };
        let at = terminus_phrase(&mover.pose(key.terminus).waypoint, key.terminus);
        log::info!(
            "[MoverLight] {} {at} gates {} baked light(s): {}",
            mover.label,
            group.len(),
            name_lights(group.iter().map(|(key, _)| key.source_index), &light_label),
        );
        for (key, row) in group {
            log::debug!(
                "[MoverLight] {} {at}, {}: {} gated texel(s) est. ({} shared, {} differing) \
                 in {} cell block(s) ({} differing), mask est. {} B every gated, \
                 {} B differing only, light inside: {}, gated centres {}",
                mover.label,
                light_label(key.source_index),
                row.gated_texels,
                row.shared_texels,
                row.differing_texels(),
                row.touched_blocks,
                row.differing_blocks,
                row.every_gated_estimate(),
                row.differing_only_estimate(),
                if row.light_inside { "yes" } else { "no" },
                row.texel_centres.as_ref().map_or_else(
                    || "none".to_string(),
                    |bounds| format!(
                        "({:.3}, {:.3}, {:.3})..({:.3}, {:.3}, {:.3}) m",
                        bounds.min.x,
                        bounds.min.y,
                        bounds.min.z,
                        bounds.max.x,
                        bounds.max.y,
                        bounds.max.z
                    ),
                ),
            );
        }
    }
}

/// Each analysed mover's side count at debug, naming its lights.
pub(crate) fn log_side_counts(
    poses: &MoverPoses,
    counts: &[SideCount],
    light_label: impl Fn(usize) -> String,
) {
    for count in counts {
        let Some(mover) = poses.mover(count.mover_id) else {
            continue;
        };
        let group = |lights: &[usize]| {
            if lights.is_empty() {
                String::new()
            } else {
                format!(": {}", name_lights(lights.iter().copied(), &light_label))
            }
        };
        log::debug!(
            "[MoverLight] {} side count: {} carried dynamic light(s){}; {} other dynamic light(s){}",
            mover.label,
            count.carried.len(),
            group(&count.carried),
            count.others.len(),
            group(&count.others),
        );
    }
}

/// Up to [`NAMED_LIGHTS`] light labels, then "and N more".
fn name_lights(
    lights: impl ExactSizeIterator<Item = usize>,
    light_label: &impl Fn(usize) -> String,
) -> String {
    let total = lights.len();
    let mut named: Vec<String> = lights.take(NAMED_LIGHTS).map(light_label).collect();
    if total > NAMED_LIGHTS {
        named.push(format!("and {} more", total - NAMED_LIGHTS));
    }
    named.join(", ")
}

fn terminus_phrase(waypoint: &str, terminus: Terminus) -> String {
    format!("at {} waypoint '{waypoint}'", terminus.label())
}
