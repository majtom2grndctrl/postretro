// SH base working-set gate: the patch cache and id-34 volume priced before any SH work.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::fmt;

use thiserror::Error;

use super::bounce::later_bounce_buffer_bytes;
use super::layer::{layer_bytes, layer_transient_bytes};
use super::layer_set::{LAYERS_IN_FLIGHT, PlannedLayer};
use super::patches::{PatchCountExceeded, PatchSet, Side};
use super::reach::{PatchReachIndex, REACH_INDEX_BYTES_PER_PATCH};
use crate::geometry::GeometryResult;
use crate::map_data::MapLight;
use crate::mover_probe_channel::MoverChannelCost;
use crate::sh_bake::{depth_tile_bytes, probe_grid_dims, sh_volume_bytes};

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
    /// Every finished layer, and the shading scratch, cache stream buffer or
    /// raw entry of the largest layers in flight together
    /// ([`LAYERS_IN_FLIGHT`]), beside the reach index.
    pub(crate) layer_phase_bytes: u64,
    /// The layers and the sum they reduce into.
    pub(crate) sum_phase_bytes: u64,
    /// The sum and the later-bounce buffers.
    pub(crate) bounce_phase_bytes: u64,
    /// The sum read by the probe gather beside the dense probes and atlas,
    /// and the compact section built from them, with the probe depth tiles'
    /// ranked candidates and the section they become.
    pub(crate) gather_phase_bytes: u64,
    /// The id-34 section and depth tiles beside their build-cache entry:
    /// written on a miss (the patch cache already freed), read and decoded
    /// on a hit.
    pub(crate) volume_entry_phase_bytes: u64,
    /// The mover probe channel's bounce bake ([`Self::price_movers`]): the
    /// patch cache and id 34 beside the blocks at the cap, one mover's
    /// states and one pose's shading, or the finish of one mover, or the
    /// layers, bounce one and id 34's later bounces a slot builds when its
    /// read-back fails; zero with no mover.
    pub(crate) mover_bounce_phase_bytes: u64,
    /// The direct half, after the SH delta and direct-SH stages: every SH
    /// section alive there, the posed solids, the blocks and the id-57
    /// section at the cap with the direct entries at their bound.
    pub(crate) mover_direct_phase_bytes: u64,
    pub(crate) estimated_peak_bytes: u64,
    pub(crate) budget_bytes: u64,
    /// Terms [`Self::price_movers`] reads.
    resident_bytes: u64,
    reach_index_bytes: u64,
    /// Id 34's section, animation tables and depth tiles.
    pub(crate) section_bytes: u64,
    light_reach_total: u64,
    /// `--mover-payload-max-size`, when a mover was priced: the refusal
    /// names it, since the blocks are priced at it.
    pub(crate) mover_payload_cap: u64,
    /// Test only: what [`Self::price_movers`] priced the channel and its
    /// direct window from, for the windows' per-section checks.
    #[cfg(test)]
    pub(crate) mover_cost: MoverChannelCost,
    #[cfg(test)]
    pub(crate) direct_window: DirectWindow,
}

/// The SH sections alive beside the mover channel's direct half, priced
/// before any ray: the delta bakes' dense bound, id 35 from the probe grid
/// and the entity-shadow selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DirectWindow {
    /// Ids 27, 41 and 45 at the SH-delta gate's dense bound, never their
    /// post-drop payload cap: they are still dense there.
    pub(crate) dense_delta_bytes: u64,
    /// Id 35 at its bound from the probe grid, and its `--sh-analyze`
    /// clone, held for every bake with a static light.
    pub(crate) direct_volume_bytes: u64,
    /// The entity-shadow light selection.
    pub(crate) entity_shadow_bytes: u64,
    /// The delta sections' per-cell vectors (CSR offsets, levels, valid
    /// masks) and light lists, in every copy the dense bound counts: they
    /// scale with the grid, not the payload.
    pub(crate) delta_metadata_bytes: u64,
}

impl ShBaseWorkingSetProjection {
    /// Price the mover probe channel into every phase it outlives: the posed
    /// scenes from the gate on; the summed bounce one kept for the posed
    /// bakes and id 34's later bounces kept for them from the sum on; the
    /// blocks at the cap in every phase from the mover bake to the id-34
    /// entry's write; its bake; and its direct window, beside `window`'s
    /// sections. Movers finish one at a time, so one mover's states are
    /// priced, never every mover's. The peak is re-taken. Every term is
    /// checked: `--mover-payload-max-size` feeds it directly, so a cap too
    /// large to price refuses by name.
    pub(crate) fn price_movers(
        &mut self,
        cost: &MoverChannelCost,
        window: &DirectWindow,
    ) -> Result<(), ShBaseWorkingSetGateError> {
        if *cost == MoverChannelCost::default() {
            return Ok(());
        }
        #[cfg(test)]
        {
            self.mover_cost = cost.clone();
            self.direct_window = window.clone();
        }
        self.mover_payload_cap = cost.payload_cap;
        let overflow = |calculation| ShBaseWorkingSetGateError::ArithmeticOverflow { calculation };
        let times = |factor: u64, bytes: u64, calculation| {
            factor
                .checked_mul(bytes)
                .ok_or_else(|| overflow(calculation))
        };
        let patch_count = self.patch_count;
        let scenes = cost.scene_bytes;
        let blocks = cost.payload_cap;
        let kept = cost.kept_later_bytes(patch_count);
        let bounce_one = if cost.radiance_movers > 0 {
            self.sum_bytes
        } else {
            0
        };
        let held = scenes + bounce_one + kept;
        self.gate_phase_bytes += scenes;
        self.layer_phase_bytes += scenes;
        self.sum_phase_bytes += scenes + bounce_one;
        self.bounce_phase_bytes += held;
        self.gather_phase_bytes += held;
        // Cold, the volume's entry is written after the movers finish.
        self.volume_entry_phase_bytes = sum(
            "mover id-34 entry projection",
            &[self.volume_entry_phase_bytes, scenes, blocks],
        )?;
        let patch_cache = self.patch_set_bytes + self.resident_bytes;
        // Held from the first slot to the last, beside every pose's bake and
        // every mover's finish: the blocks joined so far and the loop's own
        // buffers.
        let resident = sum(
            "mover bake resident projection",
            &[
                patch_cache,
                self.reach_index_bytes,
                self.section_bytes,
                scenes,
                blocks,
                cost.loop_bytes(),
            ],
        )?;
        // One pose baking: cold beside the patch cache's radiance.
        let pose = sum(
            "mover pose projection",
            &[
                resident,
                bounce_one,
                kept,
                cost.pose_bytes(patch_count, self.light_reach_total),
            ],
        )?;
        // One mover finishing: its states and their skipped copy, encoding
        // into the blocks.
        let finish = sum(
            "mover finish projection",
            &[
                resident,
                bounce_one,
                kept,
                cost.states_bytes(),
                cost.finish_bytes(),
            ],
        )?;
        let cold = pose.max(finish) + self.sum_bytes;
        // Warm, a slot whose read-back fails builds what its pose reads, its
        // other poses read back and everything the loop holds beside it:
        // the layers resolving or summing, with the slot's own reach index
        // beside the layers', then id 34's later bounces gathered or read
        // from their memo beside bounce one.
        let raw = cost.raw_mover_bytes();
        let resolve = sum(
            "mover read-back resolve projection",
            &[
                self.layer_phase_bytes.max(self.sum_phase_bytes),
                self.reach_index_bytes,
                self.section_bytes,
                blocks,
                cost.loop_bytes(),
                raw,
            ],
        )?;
        let warm_tail = sum(
            "mover read-back later bounce projection",
            &[
                resident,
                bounce_one,
                kept,
                self.bounce_buffer_bytes.max(kept),
                raw,
            ],
        )?;
        self.mover_bounce_phase_bytes = cold.max(resolve).max(warm_tail);
        // After the SH delta and direct-SH stages: id 34 and its analysis
        // clone, id 35 and its clone, the dense deltas, the entity-shadow
        // selection and the posed solids; the blocks, the id-57 section the
        // blocks assemble into and the copy they assemble from, each at the
        // cap; and the direct entries at their bound in the bake, in the
        // record parts the section assembles from and in the section.
        self.mover_direct_phase_bytes = sum(
            "mover direct window projection",
            &[
                self.resident_bytes,
                times(2, self.section_bytes, "mover direct id-34 projection")?,
                window.direct_volume_bytes,
                window.dense_delta_bytes,
                window.delta_metadata_bytes,
                window.entity_shadow_bytes,
                cost.solid_bytes,
                times(3, blocks, "mover direct blocks projection")?,
                times(
                    3,
                    cost.direct_entry_bytes,
                    "mover direct entries projection",
                )?,
            ],
        )?;
        self.estimated_peak_bytes = [
            self.gate_phase_bytes,
            self.layer_phase_bytes,
            self.sum_phase_bytes,
            self.bounce_phase_bytes,
            self.gather_phase_bytes,
            self.volume_entry_phase_bytes,
            self.mover_bounce_phase_bytes,
            self.mover_direct_phase_bytes,
        ]
        .into_iter()
        .max()
        .expect("eight phases");
        Ok(())
    }

    /// What the mover channel carries through the SH delta and direct-SH
    /// stages, for the SH-delta gate's projection: the blocks at the cap,
    /// the posed solids, and id 34 (with id 59) and its analysis clone.
    /// Zero with no mover; a cap too large to price refuses by name.
    pub(crate) fn carried_bytes(
        &self,
        cost: &MoverChannelCost,
    ) -> Result<u64, ShBaseWorkingSetGateError> {
        if *cost == MoverChannelCost::default() {
            return Ok(0);
        }
        sum(
            "mover carried projection",
            &[
                cost.payload_cap,
                cost.solid_bytes,
                self.section_bytes,
                self.section_bytes,
            ],
        )
    }

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
             probe gather {}, id-34 entry {}, mover bounce {}, mover direct {} bytes",
            self.patch_count,
            self.static_light_count,
            self.gate_phase_bytes,
            self.layer_phase_bytes,
            self.sum_phase_bytes,
            self.bounce_phase_bytes,
            self.gather_phase_bytes,
            self.volume_entry_phase_bytes,
            self.mover_bounce_phase_bytes,
            self.mover_direct_phase_bytes,
        )?;
        if self.mover_bounce_phase_bytes > 0 {
            write!(
                f,
                "; the mover probe channel's blocks are priced at --mover-payload-max-size {} \
                 bytes in the mover bake and three times in mover direct",
                self.mover_payload_cap
            )?;
        }
        Ok(())
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

/// The scratch of the `in_flight` largest of `transients`: what layer
/// resolution holds beside the finished layers when it admits at most that
/// many at once, whichever they are. Every light's, priced before
/// identical lights share a layer, so a conservative bound. `None` on
/// overflow.
pub(super) fn in_flight_scratch(mut transients: Vec<u64>, in_flight: usize) -> Option<u64> {
    transients.sort_unstable_by(|a, b| b.cmp(a));
    transients
        .iter()
        .take(in_flight)
        .try_fold(0u64, |total, &bytes| total.checked_add(bytes))
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
    let mut transients = Vec::with_capacity(inputs.light_reach.len());
    for &reach in inputs.light_reach {
        layer_total = layer_bytes(reach)
            .and_then(|bytes| layer_total.checked_add(bytes))
            .ok_or_else(|| overflow("patch layer projection"))?;
        transients.push(
            layer_transient_bytes(reach)
                .ok_or_else(|| overflow("patch layer scratch projection"))?,
        );
    }
    let layer_transient_total = in_flight_scratch(transients, LAYERS_IN_FLIGHT)
        .ok_or_else(|| overflow("patch layer scratch projection"))?;
    let sum_bytes = patch_count
        .checked_mul((Side::BOTH.len() * size_of::<[f32; 3]>()) as u64)
        .ok_or_else(|| overflow("summed patch radiance projection"))?;
    let bounce_buffer_bytes = later_bounce_buffer_bytes(patch_count, inputs.bounce_fill)
        .ok_or_else(|| overflow("later-bounce buffer projection"))?;
    let volume =
        sh_volume_bytes(inputs.probe_dims).ok_or_else(|| overflow("SH volume projection"))?;
    // Depth tiles at the cap, in two representations at once: the ranked
    // candidates and the section they become, the section and its entry
    // bytes, or the raw entry and its decode.
    let tiles = inputs
        .probe_dims
        .iter()
        .try_fold(1u64, |acc, &d| acc.checked_mul(u64::from(d)))
        .map(depth_tile_bytes)
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or_else(|| overflow("probe depth tile projection"))?;
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
            tiles,
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
        &[resident, volume.section, tables, encoded_buffer, tiles],
    )?
    .max(sum(
        "SH volume entry read projection",
        &[patch_cache, volume.section, tables, volume.encoded, tiles],
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
        mover_bounce_phase_bytes: 0,
        mover_direct_phase_bytes: 0,
        estimated_peak_bytes,
        budget_bytes,
        resident_bytes: resident,
        reach_index_bytes: reach_index,
        section_bytes: volume.section + tables + tiles,
        light_reach_total: inputs.light_reach.iter().sum(),
        mover_payload_cap: 0,
        #[cfg(test)]
        mover_cost: MoverChannelCost::default(),
        #[cfg(test)]
        direct_window: DirectWindow::default(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::geometry::extract_geometry;
    use crate::mover_probe_channel::MoverChannelCost;
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
    // section, both copies of the compact atlas, and two representations of
    // a depth tile per probe (the cap is far above 24 probes).
    #[test]
    fn no_static_lights_price_no_layer() {
        let patches = no_patches();
        let projection = at(&patches, &[], [4, 3, 2], 0.0, u64::MAX);
        let volume = sh_volume_bytes([4, 3, 2]).unwrap();
        assert_eq!(projection.layer_bytes, 0);
        assert_eq!(projection.bounce_buffer_bytes, 0);
        assert_eq!(
            projection.gather_phase_bytes,
            patches.heap_bytes()
                + volume.layout
                + volume.dense
                + volume.section
                + volume.atlas
                + 2 * depth_tile_bytes(24)
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

    /// A one-mover channel cost: a two-pose mover over `valid` probes,
    /// re-baking radiance or not.
    fn one_mover(radiance: bool, payload_cap: u64) -> MoverChannelCost {
        MoverChannelCost {
            movers: 1,
            radiance_movers: u64::from(radiance),
            scene_bytes: 1,
            valid_probes: 10,
            total_probes: 10,
            depth_cap: 32_768,
            payload_cap,
            ..MoverChannelCost::default()
        }
    }

    // Movers at `light_bounces` 0 re-bake no radiance, so the price holds no
    // copy of bounce one: the sum phase gains only the posed scenes.
    #[test]
    fn movers_at_light_bounces_0_price_no_bounce_one_copy() {
        let patches = no_patches();
        let base = at(&patches, &[3], [2, 2, 2], 1.0, u64::MAX);
        let mut depth_only = base.clone();
        depth_only
            .price_movers(&one_mover(false, 0), &DirectWindow::default())
            .expect("priced");
        let mut radiance = base.clone();
        radiance
            .price_movers(&one_mover(true, 0), &DirectWindow::default())
            .expect("priced");
        assert_eq!(depth_only.sum_phase_bytes, base.sum_phase_bytes + 1);
        assert_eq!(
            radiance.sum_phase_bytes,
            base.sum_phase_bytes + 1 + base.sum_bytes
        );
    }

    // A payload cap too large to price refuses by the gate's overflow error,
    // never wrapping into a small price or panicking.
    #[test]
    fn a_payload_cap_too_large_to_price_refuses_by_name() {
        let patches = no_patches();
        let mut projection = at(&patches, &[3], [2, 2, 2], 1.0, u64::MAX);
        let error = projection
            .price_movers(&one_mover(true, u64::MAX), &DirectWindow::default())
            .expect_err("u64::MAX overflows the price");
        assert!(
            matches!(error, ShBaseWorkingSetGateError::ArithmeticOverflow { .. }),
            "{error}"
        );
        assert!(
            projection
                .carried_bytes(&one_mover(true, u64::MAX))
                .is_err()
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
