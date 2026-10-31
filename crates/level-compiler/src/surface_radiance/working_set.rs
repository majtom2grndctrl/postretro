// SH base working-set gate: the patch cache and id-34 volume priced before any SH work.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::fmt;

use thiserror::Error;

use super::bounce::later_bounce_buffer_bytes;
use super::layer::{layer_bytes, layer_transient_bytes};
use super::layer_set::PlannedLayer;
use super::patches::{PatchCountExceeded, PatchSet, Side};
use super::reach::{PatchReachIndex, REACH_INDEX_BYTES_PER_PATCH};
use crate::geometry::GeometryResult;
use crate::map_data::MapLight;
use crate::sh_bake::{probe_grid_dims, sh_volume_bytes};

/// The SH base stage's predicted host working set: the peak of each phase
/// and the terms they sum. Every term is priced from the patch set, the
/// static lights' ranges, the probe grid's dimensions and the animated
/// lights' tables: no ray, texture read or emitter test. The estimate is the
/// largest phase, since a phase frees what the next does not read: layers
/// drop at the sum, the later-bounce buffers before the gather, the patch
/// cache before the volume's cache write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShBaseWorkingSetProjection {
    pub(crate) patch_count: u64,
    pub(crate) static_light_count: u64,
    /// The patch set itself, resident from the gate to the end of the stage.
    pub(crate) patch_set_bytes: u64,
    /// Every light's layer co-resident before the sum: Σ over lights of the
    /// patches in its range × both sides × one entry, counted per light
    /// before identical lights share a layer, so a conservative bound.
    pub(crate) layer_bytes: u64,
    /// The summed radiance, both sides of every patch.
    pub(crate) sum_bytes: u64,
    /// Buffers later bounces hold beside the sum: zero at fill 0, which
    /// allocates none.
    pub(crate) bounce_buffer_bytes: u64,
    /// The gate: the patch set's build, then the reach index that counts
    /// each light's patches.
    pub(crate) gate_phase_bytes: u64,
    /// Every layer resolving at once, each with its shading scratch, cache
    /// stream buffer or raw entry, beside the reach index.
    pub(crate) layer_phase_bytes: u64,
    /// The layers and the sum they reduce into.
    pub(crate) sum_phase_bytes: u64,
    /// The sum and the later-bounce buffers.
    pub(crate) bounce_phase_bytes: u64,
    /// The sum read by the probe gather beside the dense probes and atlas,
    /// and the compact section built from them.
    pub(crate) gather_phase_bytes: u64,
    /// The id-34 section beside its build-cache entry: written on a miss
    /// (the patch cache already freed), read and decoded on a hit.
    pub(crate) volume_entry_phase_bytes: u64,
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
            "{} patches, {} static lights: peak of gate {}, layers {}, sum {}, later bounces {}, \
             probe gather {}, id-34 entry {} bytes",
            self.patch_count,
            self.static_light_count,
            self.gate_phase_bytes,
            self.layer_phase_bytes,
            self.sum_phase_bytes,
            self.bounce_phase_bytes,
            self.gather_phase_bytes,
            self.volume_entry_phase_bytes,
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
    #[error("SH base working-set gate refused before patch shading: {0}")]
    PatchCount(#[from] PatchCountExceeded),
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
/// `budget_bytes`. `animation_table_bytes` is id 34's animation tables
/// ([`crate::sh_bake::animation_table_bytes`]), which the section carries.
/// The projection is returned whatever the budget; [`ShBaseWorkingSetProjection::admit`]
/// decides, so the pipeline can order this gate against the others. The
/// exceptions are an `ArithmeticOverflow` and a `PatchCount` refusal, which
/// return before ordering.
/// A patch set past [`super::patches::MAX_PATCHES`] refuses here by name.
pub(crate) fn plan_sh_base(
    geometry: &GeometryResult,
    static_lights: &[&MapLight],
    animation_table_bytes: u64,
    probe_spacing: f32,
    bounce_fill: f32,
    budget_bytes: u64,
) -> Result<(ShBaseHandoff, ShBaseWorkingSetProjection), ShBaseWorkingSetGateError> {
    debug_assert!(
        super::bounce::is_valid_bounce_fill(bounce_fill),
        "bounce fill {bounce_fill} resolves at parse"
    );
    let patches = PatchSet::try_build(geometry)?;
    let light_reach = PatchReachIndex::new(patches.patches()).counts_in_reach(static_lights);
    let projection = project(
        ProjectionInputs {
            patches: &patches,
            patch_build_bytes: patches.build_peak_bytes(geometry),
            light_reach: &light_reach,
            animation_table_bytes,
            probe_dims: probe_grid_dims(geometry, probe_spacing),
            bounce_fill,
        },
        budget_bytes,
    )?;
    #[cfg(test)]
    crate::live_bytes::phase("gate");
    Ok((
        ShBaseHandoff {
            patches,
            light_reach,
            bounce_fill,
        },
        projection,
    ))
}

struct ProjectionInputs<'a> {
    patches: &'a PatchSet,
    /// [`PatchSet::build_peak_bytes`]; `None` on overflow.
    patch_build_bytes: Option<u64>,
    light_reach: &'a [u64],
    animation_table_bytes: u64,
    probe_dims: [u32; 3],
    bounce_fill: f32,
}

/// Sum `terms`, or name `calculation` on overflow.
fn sum(calculation: &'static str, terms: &[u64]) -> Result<u64, ShBaseWorkingSetGateError> {
    terms
        .iter()
        .try_fold(0u64, |total, &term| total.checked_add(term))
        .ok_or(ShBaseWorkingSetGateError::ArithmeticOverflow { calculation })
}

fn project(
    inputs: ProjectionInputs<'_>,
    budget_bytes: u64,
) -> Result<ShBaseWorkingSetProjection, ShBaseWorkingSetGateError> {
    let overflow = |calculation| ShBaseWorkingSetGateError::ArithmeticOverflow { calculation };
    let patches = inputs.patches;
    let patch_count = patches.len() as u64;
    let patch_set_bytes = patches.heap_bytes();
    let light_count = inputs.light_reach.len() as u64;

    let mut layer_total = 0u64;
    let mut layer_transient_total = 0u64;
    for &reach in inputs.light_reach {
        layer_total = layer_bytes(reach)
            .and_then(|bytes| layer_total.checked_add(bytes))
            .ok_or_else(|| overflow("patch layer projection"))?;
        layer_transient_total = layer_transient_bytes(reach)
            .and_then(|bytes| layer_transient_total.checked_add(bytes))
            .ok_or_else(|| overflow("patch layer scratch projection"))?;
    }
    let sum_bytes = patch_count
        .checked_mul((Side::BOTH.len() * size_of::<[f32; 3]>()) as u64)
        .ok_or_else(|| overflow("summed patch radiance projection"))?;
    let bounce_buffer_bytes = later_bounce_buffer_bytes(patch_count, inputs.bounce_fill)
        .ok_or_else(|| overflow("later-bounce buffer projection"))?;
    let volume =
        sh_volume_bytes(inputs.probe_dims).ok_or_else(|| overflow("SH volume projection"))?;
    let reach_index = patch_count
        .checked_mul(REACH_INDEX_BYTES_PER_PATCH)
        .ok_or_else(|| overflow("reach index projection"))?;
    // Per static light: its reference in the stage's and the gather's light
    // lists, its reach count, and its plan entry with the plan's build
    // scratch.
    let metadata = light_count
        .checked_mul((3 * size_of::<u64>() + 2 * size_of::<PlannedLayer>()) as u64)
        .ok_or_else(|| overflow("per-light metadata projection"))?;
    let tables = inputs.animation_table_bytes;
    // Resident from the plan to the end of the stage.
    let resident = sum("SH base resident projection", &[metadata, volume.layout])?;
    let patch_cache = sum(
        "SH base patch cache projection",
        &[patch_set_bytes, resident],
    )?;

    let gate_phase_bytes = sum(
        "SH base gate projection",
        &[patch_set_bytes, reach_index, metadata],
    )?
    .max(
        inputs
            .patch_build_bytes
            .ok_or_else(|| overflow("patch set build projection"))?,
    );
    let layer_phase_bytes = sum(
        "SH base layer phase projection",
        &[patch_cache, reach_index, layer_total, layer_transient_total],
    )?;
    let sum_phase_bytes = sum(
        "SH base sum phase projection",
        &[patch_cache, layer_total, sum_bytes],
    )?;
    let bounce_phase_bytes = sum(
        "SH base later-bounce phase projection",
        &[patch_cache, sum_bytes, bounce_buffer_bytes],
    )?;
    let gather_phase_bytes = sum(
        "SH base probe gather projection",
        &[
            patch_cache,
            sum_bytes,
            volume.dense,
            volume.section,
            volume.atlas,
            tables,
        ],
    )?;
    // A miss writes the section serialized whole: the serializer reserves
    // the payload, then appends the empty tables' counts past it, so its
    // buffer doubles. A hit reads the raw entry beside its decoded section
    // while the patch set still stands.
    let encoded_buffer = volume
        .encoded
        .checked_mul(2)
        .ok_or_else(|| overflow("SH volume entry projection"))?;
    let volume_entry_phase_bytes = sum(
        "SH volume entry write projection",
        &[resident, volume.section, tables, encoded_buffer],
    )?
    .max(sum(
        "SH volume entry read projection",
        &[patch_cache, volume.section, tables, volume.encoded],
    )?);

    let estimated_peak_bytes = [
        gate_phase_bytes,
        layer_phase_bytes,
        sum_phase_bytes,
        bounce_phase_bytes,
        gather_phase_bytes,
        volume_entry_phase_bytes,
    ]
    .into_iter()
    .max()
    .expect("six phases");
    Ok(ShBaseWorkingSetProjection {
        patch_count,
        static_light_count: light_count,
        patch_set_bytes,
        layer_bytes: layer_total,
        sum_bytes,
        bounce_buffer_bytes,
        gate_phase_bytes,
        layer_phase_bytes,
        sum_phase_bytes,
        bounce_phase_bytes,
        gather_phase_bytes,
        volume_entry_phase_bytes,
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

    fn at(
        patches: &PatchSet,
        light_reach: &[u64],
        probe_dims: [u32; 3],
        bounce_fill: f32,
        budget_bytes: u64,
    ) -> ShBaseWorkingSetProjection {
        project(
            ProjectionInputs {
                patches,
                patch_build_bytes: Some(0),
                light_reach,
                animation_table_bytes: 0,
                probe_dims,
                bounce_fill,
            },
            budget_bytes,
        )
        .expect("priced")
    }

    // A map with no static light prices no layer, and with no patch its
    // peak is the probe gather's: the dense volume beside the compact
    // section, both copies of the compact atlas.
    #[test]
    fn no_static_lights_price_no_layer() {
        let patches = no_patches();
        let projection = at(&patches, &[], [4, 3, 2], 0.0, u64::MAX);
        let volume = sh_volume_bytes([4, 3, 2]).unwrap();
        assert_eq!(projection.layer_bytes, 0);
        assert_eq!(projection.bounce_buffer_bytes, 0);
        assert_eq!(
            projection.gather_phase_bytes,
            patches.heap_bytes() + volume.layout + volume.dense + volume.section + volume.atlas
        );
        assert_eq!(
            projection.estimated_peak_bytes,
            projection.gather_phase_bytes
        );
    }

    #[test]
    fn layers_price_both_sides_of_every_patch_in_each_lights_range() {
        let patches = no_patches();
        let projection = at(&patches, &[10, 0, 7], [0, 0, 0], 0.0, u64::MAX);
        assert_eq!(
            projection.layer_bytes,
            17 * 2 * size_of::<super::super::layer::LayerEntry>() as u64
        );
        // Each layer resolves beside at least its own size of scratch.
        assert!(projection.layer_phase_bytes >= 2 * projection.layer_bytes);
        assert_eq!(
            projection.estimated_peak_bytes,
            projection.layer_phase_bytes
        );
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
        let at = |fill| at(&patches, &[], [0, 0, 0], fill, u64::MAX);
        assert_eq!(at(0.0).bounce_buffer_bytes, 0);
        let expected = 3 * 16 * 12 + u64::from(super::super::bounce::BOUNCE_RAYS_PER_PATCH) * 12;
        assert_eq!(at(0.25).bounce_buffer_bytes, expected);
        assert_eq!(at(1.0).bounce_buffer_bytes, expected);
        assert_eq!(
            at(1.0).bounce_phase_bytes - at(0.0).bounce_phase_bytes,
            expected
        );
    }

    // A patch set past its cap refuses through the gate by name, before any
    // projection: past `u32::MAX` a patch index would truncate. The quad
    // holds 16 patches, so a cap of 15 stands in for the real one.
    #[test]
    fn a_patch_set_past_its_cap_refuses_through_the_gate_by_name() {
        let geometry = super::super::tests::polygon_geometry(&[vec![
            glam::Vec3::new(0.0, 0.0, 0.0),
            glam::Vec3::new(0.0, 0.0, 2.0),
            glam::Vec3::new(2.0, 0.0, 2.0),
            glam::Vec3::new(2.0, 0.0, 0.0),
        ]]);
        let size = super::super::patches::PATCH_SIZE_M;
        assert_eq!(
            PatchSet::build_capped(&geometry, size, 16)
                .expect("16 patches fit a cap of 16")
                .len(),
            16
        );
        let refused: ShBaseWorkingSetGateError = PatchSet::build_capped(&geometry, size, 15)
            .err()
            .expect("16 patches exceed a cap of 15")
            .into();
        assert_eq!(
            refused.to_string(),
            "SH base working-set gate refused before patch shading: the surface radiance cache \
             needs more than 15 patches; a patch index is a u32"
        );
    }

    #[test]
    fn a_budget_one_byte_under_the_prediction_refuses_by_name() {
        let patches = no_patches();
        let peak = at(&patches, &[3], [2, 2, 2], 1.0, 0).estimated_peak_bytes;
        assert!(at(&patches, &[3], [2, 2, 2], 1.0, peak).admit().is_ok());
        let refused = at(&patches, &[3], [2, 2, 2], 1.0, peak - 1);
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
