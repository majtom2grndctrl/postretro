//! The Build Summary's mover bounce fidelity block: per analysed mover, its
//! pose count, the fidelity its states show, its projected `--release`
//! bounce-bake time and its skipped mid-pose states.
//! See: context/lib/build_pipeline.md §Build Cache (Bake fidelity)

use std::time::Duration;

use crate::bake_fidelity::BakeMode;
use crate::mover_probe_channel::{MoverPoseBounces, SkipCounts};

/// One terminus costs about one id-34 bake (research §Cost), so a mover's
/// two termini project as two id-34 bakes.
const TERMINUS_ID34_BAKES: f64 = 2.0;

/// The block's lines, or none when no mover re-bakes. `movers` holds each
/// mover's poses at the counts they resolved at; `skips` each mover with
/// mid poses by id; `sh_base_bake_time` id 34's recorded bake time.
///
/// A mover reads `high` unless some pose holds fewer posed bounces than its
/// own `light_bounces` (clamped to id 34's): only then does what it shows
/// differ from a `high` build, and it reads `low, approximate`. A `low`
/// build's mover served entirely from `high` entries, or at
/// `light_bounces` 1, reads `high`.
pub(super) fn fidelity_lines(
    mode: BakeMode,
    movers: &[MoverPoseBounces],
    skips: &[(u32, SkipCounts)],
    sh_base_bake_time: Option<Duration>,
) -> Vec<String> {
    if movers.is_empty() {
        return Vec::new();
    }
    let approximate = movers.iter().filter(|mover| is_approximate(mover)).count();
    let mut lines = vec![format!(
        "Mover bounce fidelity: {} build; {approximate} of {} mover(s) approximate",
        mode.label(),
        movers.len(),
    )];
    for mover in movers {
        let mut line = format!(
            "  {} (#{}): {} pose(s), {}; --release bounce bake {}",
            mover.label,
            mover.mover_id,
            mover.poses.len(),
            shown_fidelity(mover),
            projected_release_time(mover, sh_base_bake_time),
        );
        if let Some((_, counts)) = skips.iter().find(|(id, _)| *id == mover.mover_id) {
            line += &format!(
                "; mid-pose states skipped: bounce {} of {}, depth {} of {}",
                counts.bounce_skipped,
                counts.bounce_skipped + counts.bounce_stored,
                counts.depth_skipped,
                counts.depth_skipped + counts.depth_stored,
            );
        }
        lines.push(line);
    }
    lines
}

fn is_approximate(mover: &MoverPoseBounces) -> bool {
    mover
        .poses
        .iter()
        .any(|&bounces| bounces < mover.high_bounces)
}

/// `high`, or `low, approximate` with the poses short of the mover's own
/// count.
fn shown_fidelity(mover: &MoverPoseBounces) -> String {
    let low: Vec<u32> = mover
        .poses
        .iter()
        .copied()
        .filter(|&bounces| bounces < mover.high_bounces)
        .collect();
    match low.iter().max() {
        None => "high".to_owned(),
        Some(baked) => format!(
            "low, approximate: {} of {} pose(s) at {baked} of {} posed bounce(s)",
            low.len(),
            mover.poses.len(),
            mover.high_bounces,
        ),
    }
}

/// Id 34's recorded bake time T × 2 for the mover's termini, plus T × the
/// share of static patches its near-field domain holds for each mid pose,
/// which traces its later bounces over that domain alone (owner,
/// 2026-11-11). Labelled an estimate; "—" with none recorded, and none for
/// a mover baking no bounce.
fn projected_release_time(mover: &MoverPoseBounces, sh_base_bake_time: Option<Duration>) -> String {
    if mover.high_bounces == 0 {
        return "none (light_bounces 0)".to_owned();
    }
    let Some(time) = sh_base_bake_time else {
        return "est. — (no id 34 bake time recorded)".to_owned();
    };
    let mid_poses = mover.poses.len().saturating_sub(2);
    let share = if mover.static_patches == 0 {
        0.0
    } else {
        mover.domain_patches as f64 / mover.static_patches as f64
    };
    let id34_bakes = TERMINUS_ID34_BAKES + mid_poses as f64 * share;
    let projected = time.as_secs_f64() * id34_bakes;
    if mid_poses == 0 {
        return format!(
            "est. {projected:.1} s (id 34's recorded {:.1} s × 2 termini)",
            time.as_secs_f64(),
        );
    }
    format!(
        "est. {projected:.1} s (id 34's recorded {:.1} s × (2 termini + {mid_poses} mid pose(s) × \
         {:.1}% of static patches))",
        time.as_secs_f64(),
        100.0 * share,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake_fidelity::BakeFidelity;

    /// A mover whose domain holds `domain` of 1,000 static patches.
    fn mover(id: u32, high: u32, poses: &[u32], domain: usize) -> MoverPoseBounces {
        MoverPoseBounces {
            mover_id: id,
            label: format!("kinematic_mover 'm{id}'"),
            high_bounces: high,
            poses: poses.to_vec(),
            domain_patches: domain,
            static_patches: 1_000,
        }
    }

    // AC 29: per mover its pose count, its fidelity and its projected
    // `--release` bounce-bake time, labelled an estimate: id 34's recorded
    // time × 2 for the termini, plus, per mid pose, id 34's time × the share
    // of static patches the mover's domain holds (restated by the owner,
    // 2026-11-11). Approximate only where what it shows differs from high:
    // served entirely from `high` entries, or at `light_bounces` 1, it
    // prints `high`; some pose baked low past 1, `low, approximate`.
    #[test]
    fn each_mover_prints_its_poses_its_shown_fidelity_and_its_projected_release_time() {
        let lines = fidelity_lines(
            BakeMode::Interactive(BakeFidelity::Low),
            &[
                mover(1, 5, &[5, 5, 5, 5], 100),
                mover(2, 5, &[5, 1, 1, 5], 240),
                mover(3, 1, &[1, 1, 1], 80),
                mover(4, 0, &[0, 0], 0),
                mover(5, 5, &[5, 5], 0),
            ],
            &[(
                2,
                SkipCounts {
                    bounce_stored: 30,
                    bounce_skipped: 10,
                    depth_stored: 7,
                    depth_skipped: 3,
                    squeezed: 0,
                },
            )],
            Some(Duration::from_millis(12_500)),
        );
        assert_eq!(
            lines,
            [
                "Mover bounce fidelity: low build; 1 of 5 mover(s) approximate",
                "  kinematic_mover 'm1' (#1): 4 pose(s), high; --release bounce bake est. 27.5 s (id 34's \
                 recorded 12.5 s × (2 termini + 2 mid pose(s) × 10.0% of static patches))",
                "  kinematic_mover 'm2' (#2): 4 pose(s), low, approximate: 2 of 4 pose(s) at 1 of 5 posed \
                 bounce(s); --release bounce bake est. 31.0 s (id 34's recorded 12.5 s × (2 termini + 2 \
                 mid pose(s) × 24.0% of static patches)); mid-pose states skipped: bounce 10 of 40, depth \
                 3 of 10",
                "  kinematic_mover 'm3' (#3): 3 pose(s), high; --release bounce bake est. 26.0 s (id 34's \
                 recorded 12.5 s × (2 termini + 1 mid pose(s) × 8.0% of static patches))",
                "  kinematic_mover 'm4' (#4): 2 pose(s), high; --release bounce bake none (light_bounces 0)",
                "  kinematic_mover 'm5' (#5): 2 pose(s), high; --release bounce bake est. 25.0 s (id 34's \
                 recorded 12.5 s × 2 termini)",
            ]
        );
    }

    #[test]
    fn with_no_recorded_id_34_time_the_projection_reads_a_dash() {
        let lines = fidelity_lines(
            BakeMode::Release,
            &[mover(7, 5, &[5, 5, 5], 300)],
            &[],
            None,
        );
        assert_eq!(
            lines,
            [
                "Mover bounce fidelity: high (--release) build; 0 of 1 mover(s) approximate",
                "  kinematic_mover 'm7' (#7): 3 pose(s), high; --release bounce bake est. — (no id 34 bake \
                 time recorded)",
            ]
        );
        assert!(fidelity_lines(BakeMode::Release, &[], &[], None).is_empty());
    }
}
