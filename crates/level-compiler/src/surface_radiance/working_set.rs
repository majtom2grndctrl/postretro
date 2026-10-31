// SH base working-set gate: the patch cache and id-34 volume priced before any SH work.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::fmt;

use thiserror::Error;

use super::bounce::later_bounce_buffer_bytes;
use super::layer::LayerEntry;
use super::patches::{PatchSet, Side};
use super::reach::PatchReachIndex;
use crate::geometry::GeometryResult;
use crate::map_data::MapLight;
use crate::sh_bake::{probe_grid_dims, sh_volume_bytes};

/// The SH base stage's predicted host working set, by term. Every term is
/// priced from the patch set, the static lights' ranges and the probe grid's
/// dimensions: no ray, texture read or emitter test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShBaseWorkingSetProjection {
    pub(crate) patch_count: u64,
    pub(crate) static_light_count: u64,
    /// The patch set itself, resident from the gate to the end of the stage.
    pub(crate) patch_set_bytes: u64,
    /// Every light's layer co-resident before the sum: Σ over lights of the
    /// patches in its range × both sides × one entry.
    pub(crate) layer_bytes: u64,
    /// The summed radiance, both sides of every patch.
    pub(crate) sum_bytes: u64,
    /// Buffers later bounces hold beside the sum: zero at fill 0, which
    /// allocates none.
    pub(crate) bounce_buffer_bytes: u64,
    /// The id-34 bake's dense probes and atlas.
    pub(crate) volume_bytes: u64,
    /// The id-34 section's encode and decode copies, one section each: on
    /// a warm miss the gathered-volume cache entry's encoded bytes beside
    /// the section, on a hit the entry's raw payload beside the decoded
    /// section (`super::volume`).
    pub(crate) volume_copy_bytes: u64,
    pub(crate) estimated_peak_bytes: u64,
    pub(crate) budget_bytes: u64,
}

impl ShBaseWorkingSetProjection {
    /// Admit the projection, or refuse it by name when it exceeds the budget.
    pub(crate) fn admit(&self) -> Result<(), ShBaseWorkingSetGateError> {
        if self.estimated_peak_bytes > self.budget_bytes {
            Err(ShBaseWorkingSetGateError::BudgetExceeded(Box::new(
                self.clone(),
            )))
        } else {
            Ok(())
        }
    }
}

impl fmt::Display for ShBaseWorkingSetProjection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} patches, {} static lights: patch set {} + layers {} + sum {} + bounce buffers {} \
             + id-34 volume {} + encode and decode copies {} bytes",
            self.patch_count,
            self.static_light_count,
            self.patch_set_bytes,
            self.layer_bytes,
            self.sum_bytes,
            self.bounce_buffer_bytes,
            self.volume_bytes,
            self.volume_copy_bytes,
        )
    }
}

#[derive(Debug, Error)]
pub(crate) enum ShBaseWorkingSetGateError {
    #[error("SH base working-set gate refused before patch shading: {calculation} overflowed")]
    ArithmeticOverflow { calculation: &'static str },
    #[error(
        "SH base working-set gate refused before patch shading: estimated peak {peak} bytes \
         exceeds budget {budget} bytes ({0})",
        peak = .0.estimated_peak_bytes,
        budget = .0.budget_bytes
    )]
    BudgetExceeded(Box<ShBaseWorkingSetProjection>),
}

/// The gate's patch set, the reach and the fill it priced, moved whole into
/// the bake so the bake shades exactly the set the gate admitted.
pub(crate) struct ShBaseHandoff {
    pub(crate) patches: PatchSet,
    /// Patches in each static light's range, in static-light order.
    pub(crate) light_reach: Vec<u64>,
    /// The map's resolved `_bounce_fill`, in [0, 1].
    pub(crate) bounce_fill: f32,
}

/// Build the patch set over `geometry` (pre-cut static world faces after
/// exterior culling) and price the SH base stage at `bounce_fill` against
/// `budget_bytes`.
/// The projection is returned whatever the budget; [`ShBaseWorkingSetProjection::admit`]
/// decides, so the pipeline can order this gate against the others.
pub(crate) fn plan_sh_base(
    geometry: &GeometryResult,
    static_lights: &[&MapLight],
    probe_spacing: f32,
    bounce_fill: f32,
    budget_bytes: u64,
) -> Result<(ShBaseHandoff, ShBaseWorkingSetProjection), ShBaseWorkingSetGateError> {
    debug_assert!(
        super::bounce::is_valid_bounce_fill(bounce_fill),
        "bounce fill {bounce_fill} resolves at parse"
    );
    let patches = PatchSet::build(geometry);
    let light_reach = PatchReachIndex::new(patches.patches()).counts_in_reach(static_lights);
    let projection = project(
        &patches,
        &light_reach,
        probe_grid_dims(geometry, probe_spacing),
        bounce_fill,
        budget_bytes,
    )?;
    Ok((
        ShBaseHandoff {
            patches,
            light_reach,
            bounce_fill,
        },
        projection,
    ))
}

fn project(
    patches: &PatchSet,
    light_reach: &[u64],
    probe_dims: [u32; 3],
    bounce_fill: f32,
    budget_bytes: u64,
) -> Result<ShBaseWorkingSetProjection, ShBaseWorkingSetGateError> {
    let overflow = |calculation| ShBaseWorkingSetGateError::ArithmeticOverflow { calculation };
    let patch_count = patches.len() as u64;
    let entry_bytes = (Side::BOTH.len() * size_of::<LayerEntry>()) as u64;
    let mut layer_bytes = 0u64;
    for &reach in light_reach {
        layer_bytes = reach
            .checked_mul(entry_bytes)
            .and_then(|bytes| layer_bytes.checked_add(bytes))
            .ok_or_else(|| overflow("patch layer projection"))?;
    }
    let sum_bytes = patch_count
        .checked_mul((Side::BOTH.len() * size_of::<[f32; 3]>()) as u64)
        .ok_or_else(|| overflow("summed patch radiance projection"))?;
    let bounce_buffer_bytes = later_bounce_buffer_bytes(patch_count, bounce_fill)
        .ok_or_else(|| overflow("later-bounce buffer projection"))?;
    let volume = sh_volume_bytes(probe_dims).ok_or_else(|| overflow("SH volume projection"))?;
    let volume_copy_bytes = volume
        .section
        .checked_mul(2)
        .ok_or_else(|| overflow("SH volume copy projection"))?;
    let estimated_peak_bytes = [
        layer_bytes,
        sum_bytes,
        bounce_buffer_bytes,
        volume.dense,
        volume_copy_bytes,
    ]
    .into_iter()
    .try_fold(patches.heap_bytes(), u64::checked_add)
    .ok_or_else(|| overflow("SH base working-set projection"))?;
    Ok(ShBaseWorkingSetProjection {
        patch_count,
        static_light_count: light_reach.len() as u64,
        patch_set_bytes: patches.heap_bytes(),
        layer_bytes,
        sum_bytes,
        bounce_buffer_bytes,
        volume_bytes: volume.dense,
        volume_copy_bytes,
        estimated_peak_bytes,
        budget_bytes,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::geometry::extract_geometry;
    use crate::partition::BspTree;

    fn no_patches() -> PatchSet {
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
        };
        PatchSet::build(&extract_geometry(&[], &tree, &HashSet::new()))
    }

    // A map with no static light prices the probe volume terms alone (and
    // its patches, which the gather still reads as zero).
    #[test]
    fn no_static_lights_price_no_layer() {
        let patches = no_patches();
        let projection = project(&patches, &[], [4, 3, 2], 0.0, u64::MAX).unwrap();
        let volume = sh_volume_bytes([4, 3, 2]).unwrap();
        assert_eq!(projection.layer_bytes, 0);
        assert_eq!(projection.bounce_buffer_bytes, 0);
        assert_eq!(projection.volume_bytes, volume.dense);
        assert_eq!(projection.volume_copy_bytes, 2 * volume.section);
        assert_eq!(
            projection.estimated_peak_bytes,
            projection.patch_set_bytes + volume.dense + 2 * volume.section
        );
    }

    #[test]
    fn layers_price_both_sides_of_every_patch_in_each_lights_range() {
        let patches = no_patches();
        let projection = project(&patches, &[10, 0, 7], [0, 0, 0], 0.0, u64::MAX).unwrap();
        assert_eq!(
            projection.layer_bytes,
            17 * 2 * size_of::<LayerEntry>() as u64
        );
        assert_eq!(projection.volume_bytes, 0);
    }

    // Fill 0 allocates no later-bounce buffer, so it prices none; any other
    // fill prices the previous, next and running front sides of every patch
    // and the shared direction set, whatever the fill.
    #[test]
    fn later_bounce_buffers_price_three_front_sides_per_patch_above_fill_zero() {
        let geometry = super::super::tests::polygon_geometry(&[vec![
            glam::Vec3::new(0.0, 0.0, 0.0),
            glam::Vec3::new(0.0, 0.0, 2.0),
            glam::Vec3::new(2.0, 0.0, 2.0),
            glam::Vec3::new(2.0, 0.0, 0.0),
        ]]);
        let patches = PatchSet::build(&geometry);
        assert_eq!(patches.len(), 16);
        let at = |fill| project(&patches, &[], [0, 0, 0], fill, u64::MAX).unwrap();
        assert_eq!(at(0.0).bounce_buffer_bytes, 0);
        let expected = 3 * 16 * 12 + u64::from(super::super::bounce::BOUNCE_RAYS_PER_PATCH) * 12;
        assert_eq!(at(0.25).bounce_buffer_bytes, expected);
        assert_eq!(at(1.0).bounce_buffer_bytes, expected);
        assert_eq!(
            at(1.0).estimated_peak_bytes - at(0.0).estimated_peak_bytes,
            expected
        );
    }

    #[test]
    fn a_budget_one_byte_under_the_prediction_refuses_by_name() {
        let patches = no_patches();
        let at = project(&patches, &[3], [2, 2, 2], 1.0, 0).unwrap();
        let peak = at.estimated_peak_bytes;
        let admitted = project(&patches, &[3], [2, 2, 2], 1.0, peak).unwrap();
        assert!(admitted.admit().is_ok());
        let refused = project(&patches, &[3], [2, 2, 2], 1.0, peak - 1).unwrap();
        let message = refused.admit().unwrap_err().to_string();
        assert!(
            message.starts_with(&format!(
                "SH base working-set gate refused before patch shading: estimated peak {peak} \
                 bytes exceeds budget {} bytes",
                peak - 1
            )),
            "{message}"
        );
    }
}
