// Raw shadowmask fill under per-face ownership: admission, eviction and the
// mask write as each partition arrives in the fused walk.
// See: context/lib/build_pipeline.md §PRL (ShadowmaskAtlas ownership)

use std::collections::BTreeMap;
use std::ops::Range;

use postretro_level_format::shadowmask_atlas::{
    FaceOwners, NO_FACE_OWNERS, SHADOWMASK_OWNER_CHANNELS, ShadowmaskAtlasSection,
};

use crate::bake_control::BakeControl;
use crate::chart_raster::{CHART_PADDING_TEXELS, chart_texel_world_position};
use crate::lightmap_bake::{CellBlock, CutParent, light_contribution_and_direction};
use crate::lightmap_layer::{LightmapLayer, SharedAtlas, for_each_light_layer_chart_texel};

use super::SHADOWMASK_FILL_CHECKPOINT_TEXELS;
use super::eligibility::EligibleLight;
use super::raw::{RawFillBuffer, RawFillProbe};

mod layers;
use super::ownership::{
    Admission, Candidate, DropSite, FaceChannels, LIT_TEXEL_FLOOR, OwnerDrop, OwnerReport,
};

/// Raw visibility quantized as the mask stores it, or `None` for a non-finite
/// value, which neither admits a light nor adds to its contribution.
pub(super) fn quantize_visibility(raw_visibility: f32) -> Option<u8> {
    raw_visibility
        .is_finite()
        .then(|| (raw_visibility.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

/// Which face owns each chart-interior texel of one bake layer. Partitions
/// arrive layer-major, so one layer's map is live at a time.
struct LayerFaceMap {
    layer: Option<u32>,
    faces: Vec<u32>,
}

impl LayerFaceMap {
    const NONE: u32 = u32::MAX;

    fn ensure(&mut self, layer: u32, shared: &SharedAtlas<'_>) {
        if self.layer == Some(layer) {
            return;
        }
        self.layer = Some(layer);
        let plane = shared.atlas_width as usize * shared.atlas_height as usize;
        self.faces.clear();
        self.faces.resize(plane, Self::NONE);
        for face in 0..shared.placements.len() {
            if shared.placements[face].layer != layer {
                continue;
            }
            for_each_light_layer_chart_texel(shared, face, |sample| {
                self.faces[sample.idx as usize] = face as u32;
            });
        }
    }

    fn face(&self, idx: u32) -> Option<usize> {
        let face = *self.faces.get(idx as usize)?;
        (face != Self::NONE).then_some(face as usize)
    }
}

/// One cut face: owners fixed by its estimate before the walk, and the exact
/// lit-texel count of every light the walk saw on it, each parent-grid texel
/// counted once.
struct CutOwners {
    parent: u32,
    faces: Range<usize>,
    own: Vec<[Range<u32>; 2]>,
    channels: FaceChannels,
    exact: BTreeMap<u16, u32>,
    last_layer: u32,
}

pub(super) struct OwnershipFill<'a> {
    width: u32,
    height: u32,
    layer_count: u32,
    plane: usize,
    /// Cell blocks the encoder slices out of the bake layers, in block order.
    blocks: Vec<CellBlock>,
    /// Exactly one working layer, allocated only when its first item arrives.
    data: Option<RawFillBuffer>,
    layer: Option<u32>,
    next_layer: u32,
    carry: Vec<Option<RawFillBuffer>>,
    hold_until: Vec<u32>,
    encoded: Vec<[Vec<u8>; 2]>,
    owners: Vec<FaceOwners>,
    any_owner: bool,
    peak_cut_demand: u32,
    pub(super) probe: RawFillProbe,
    #[cfg(test)]
    raw_snapshot: Vec<u8>,
    /// One per chart: the id-17 face order. A cut face's sub-faces use
    /// `cuts` instead.
    faces: Vec<FaceChannels>,
    cuts: Vec<CutOwners>,
    /// Per face, the cut it belongs to.
    cut_of: Vec<Option<usize>>,
    drops: Vec<OwnerDrop>,
    face_map: LayerFaceMap,
    control: Option<&'a BakeControl>,
}

impl<'a> OwnershipFill<'a> {
    /// `cut_owners` holds each of `cut_parents`' estimated owners, in order.
    pub(super) fn new(
        shared: &SharedAtlas<'_>,
        layer_count: u32,
        cut_parents: &[CutParent],
        cut_owners: Vec<FaceChannels>,
        control: Option<&'a BakeControl>,
    ) -> Self {
        debug_assert_eq!(cut_parents.len(), cut_owners.len());
        let mut cut_of = vec![None; shared.charts.len()];
        let cuts = cut_parents
            .iter()
            .zip(cut_owners)
            .enumerate()
            .map(|(c, (cut, channels))| {
                for face in cut.faces.clone() {
                    cut_of[face] = Some(c);
                }
                CutOwners {
                    parent: cut.parent,
                    faces: cut.faces.clone(),
                    own: cut.own.clone(),
                    channels,
                    exact: BTreeMap::new(),
                    last_layer: cut
                        .faces
                        .clone()
                        .map(|face| shared.placements[face].layer)
                        .max()
                        .unwrap_or(0),
                }
            })
            .collect();
        let plane = texel_plane_len(shared.atlas_width, shared.atlas_height);
        let probe = RawFillProbe::default();
        #[cfg(test)]
        super::register_raw_fill_probe(&probe);
        Self {
            width: shared.atlas_width,
            height: shared.atlas_height,
            layer_count,
            plane,
            blocks: shared.layout.blocks.clone(),
            data: None,
            layer: None,
            next_layer: 0,
            carry: (0..shared.layout.blocks.len()).map(|_| None).collect(),
            hold_until: super::carry::block_hold_until(
                shared.placements,
                shared.layout,
                cut_parents,
            ),
            encoded: vec![[Vec::new(), Vec::new()]; shared.layout.blocks.len()],
            owners: vec![NO_FACE_OWNERS; shared.charts.len()],
            any_owner: false,
            peak_cut_demand: 0,
            probe,
            #[cfg(test)]
            raw_snapshot: vec![0; plane * layer_count as usize * SHADOWMASK_OWNER_CHANNELS],
            faces: vec![FaceChannels::default(); shared.charts.len()],
            cuts,
            cut_of,
            drops: Vec::new(),
            face_map: LayerFaceMap {
                layer: None,
                faces: Vec::new(),
            },
            control,
        }
    }

    /// Rank `light` on every face its partition lights, then write its mask
    /// values into the channels it holds. The whole partition is summed before
    /// any face decides, so no admission rests on a partial sum.
    pub(super) fn consume(
        &mut self,
        light: &EligibleLight<'_>,
        partition: &LightmapLayer,
        shared: &SharedAtlas<'_>,
    ) {
        self.advance_layer(partition.target_layer, shared);
        self.face_map.ensure(partition.target_layer, shared);
        let mut sums: BTreeMap<usize, (u32, f64)> = BTreeMap::new();
        for chunk in partition.texels.chunks(SHADOWMASK_FILL_CHECKPOINT_TEXELS) {
            self.checkpoint();
            for texel in chunk {
                let Some(face) = self.face_map.face(texel.idx) else {
                    continue;
                };
                let Some(visibility) = quantize_visibility(texel.raw_visibility) else {
                    continue;
                };
                if visibility == 0 {
                    continue;
                }
                if let Some(c) = self.cut_of[face] {
                    if self.in_own_segment(c, face, texel.idx, shared) {
                        *self.cuts[c].exact.entry(light.spec_index).or_default() += 1;
                    }
                    continue;
                }
                let chart = &shared.charts[face];
                let world_p = texel_world_position(shared, face, texel.idx);
                let (contribution, _) =
                    light_contribution_and_direction(light.light, world_p, chart.normal);
                let sum = sums.entry(face).or_default();
                sum.0 += 1;
                sum.1 += f64::from(contribution.x + contribution.y + contribution.z)
                    * f64::from(visibility)
                    / 255.0;
            }
        }

        let mut held: BTreeMap<usize, usize> = BTreeMap::new();
        for (face, (lit_texels, contribution)) in sums {
            let candidate = Candidate {
                spec_index: light.spec_index,
                lit_texels,
                contribution,
                promoted: light.promoted,
            };
            match self.faces[face].offer(candidate) {
                Admission::Admitted { channel, evicted } => {
                    if let Some(evicted) = evicted {
                        self.zero_channel(face, channel, shared);
                        self.note_drop(face, evicted);
                    }
                    held.insert(face, channel);
                }
                Admission::Rejected => self.note_drop(face, candidate),
            }
        }
        let owns_a_cut = self
            .cuts
            .iter()
            .any(|cut| cut.channels.channel_of(light.spec_index).is_some());
        if held.is_empty() && !owns_a_cut {
            return;
        }
        for chunk in partition.texels.chunks(SHADOWMASK_FILL_CHECKPOINT_TEXELS) {
            self.checkpoint();
            for texel in chunk {
                let Some(face) = self.face_map.face(texel.idx) else {
                    continue;
                };
                let channel = match self.cut_of[face] {
                    Some(c) => self.cuts[c].channels.channel_of(light.spec_index),
                    None => held.get(&face).copied(),
                };
                let Some(channel) = channel else {
                    continue;
                };
                let Some(visibility) = quantize_visibility(texel.raw_visibility) else {
                    continue;
                };
                let offset = texel.idx as usize * SHADOWMASK_OWNER_CHANNELS + channel;
                self.data.as_mut().expect("the partition's layer is open")[offset] = visibility;
            }
        }
    }

    /// Close the last working layer after id 22 has been written. A section
    /// is present only if any face admitted an owner over the completed walk.
    pub(super) fn finish(
        mut self,
        shared: &SharedAtlas<'_>,
    ) -> (Option<ShadowmaskAtlasSection>, OwnerReport) {
        while self.next_layer < self.layer_count {
            let layer = self.next_layer;
            self.advance_layer(layer, shared);
            self.close_layer(shared);
        }
        let mut drops = std::mem::take(&mut self.drops);
        drops.sort_unstable();
        let report = OwnerReport {
            peak_face_demand: self
                .faces
                .iter()
                .map(FaceChannels::candidates)
                .max()
                .unwrap_or(0)
                .max(self.peak_cut_demand),
            drops,
        };
        if !self.any_owner {
            return (None, report);
        }
        #[cfg(test)]
        super::record_raw_fill(&self.raw_snapshot);
        (
            Some(ShadowmaskAtlasSection {
                owners: self.owners,
                blocks: self.encoded,
            }),
            report,
        )
    }

    /// Whether bake-layer texel `idx` of sub-face `face` lies in that sub-face's
    /// own segment of its parent's grid. Overlap texels belong to the
    /// neighbour, so the parent counts each texel once.
    fn in_own_segment(&self, c: usize, face: usize, idx: u32, shared: &SharedAtlas<'_>) -> bool {
        let cut = &self.cuts[c];
        let [u, v] = &cut.own[face - cut.faces.start];
        let placement = &shared.placements[face];
        let window = shared.charts[face]
            .window
            .expect("a cut sub-face's chart windows its parent");
        let tx = idx % shared.atlas_width - placement.x - CHART_PADDING_TEXELS;
        let ty = idx / shared.atlas_width - placement.y - CHART_PADDING_TEXELS;
        u.contains(&(window.origin[0] + tx)) && v.contains(&(window.origin[1] + ty))
    }

    /// An estimated owner whose partitions lit no texel of its cut face holds
    /// no channel in the final table.
    fn release_owners_that_baked_nothing(&mut self, c: usize, shared: &SharedAtlas<'_>) {
        for channel in 0..SHADOWMASK_OWNER_CHANNELS {
            let Some(owner) = self.cuts[c].channels.owner(channel) else {
                continue;
            };
            if self.cuts[c]
                .exact
                .get(&owner.spec_index)
                .copied()
                .unwrap_or(0)
                == 0
            {
                self.cuts[c].channels.clear(channel);
                for face in self.cuts[c].faces.clone() {
                    self.zero_channel(face, channel, shared);
                }
            }
        }
    }

    fn note_drop(&mut self, face: usize, candidate: Candidate) {
        if candidate.above_floor() {
            self.drops.push(OwnerDrop {
                site: DropSite::Face(face as u32),
                spec_index: candidate.spec_index,
                lit_texels: candidate.lit_texels,
            });
        }
    }

    fn checkpoint(&self) {
        if let Some(control) = self.control {
            control.governor().checkpoint();
        }
    }

    /// Address a chart in the working layer, or in its retained cell block.
    fn face_rect(&self, face: usize, shared: &SharedAtlas<'_>) -> (usize, usize, usize) {
        let placement = shared.placements[face];
        if self.layer == Some(placement.layer) {
            (
                self.width as usize,
                placement.x as usize,
                placement.y as usize,
            )
        } else {
            let block = &self.blocks[shared.layout.chart_blocks[face] as usize];
            (
                block.width as usize,
                (placement.x - block.x) as usize,
                (placement.y - block.y) as usize,
            )
        }
    }

    fn face_data_mut(&mut self, face: usize, shared: &SharedAtlas<'_>) -> &mut [u8] {
        if self.layer == Some(shared.placements[face].layer) {
            self.data.as_mut().expect("the working layer is open")
        } else {
            self.carry[shared.layout.chart_blocks[face] as usize]
                .as_mut()
                .expect("an unfinished cut retains its block")
        }
    }

    fn padded_rect_offsets(
        &self,
        face: usize,
        shared: &SharedAtlas<'_>,
    ) -> impl Iterator<Item = usize> + use<> {
        let chart = &shared.charts[face];
        let (width, x, y) = self.face_rect(face, shared);
        let (chart_w, chart_h) = (chart.width_texels as usize, chart.height_texels as usize);
        (0..chart_h).flat_map(move |row| {
            let start = (y + row) * width + x;
            (start..start + chart_w).map(|texel| texel * SHADOWMASK_OWNER_CHANNELS)
        })
    }

    /// An evicted owner's values leave with it.
    fn zero_channel(&mut self, face: usize, channel: usize, shared: &SharedAtlas<'_>) {
        let offsets = self.padded_rect_offsets(face, shared);
        let data = self.face_data_mut(face, shared);
        for base in offsets {
            data[base + channel] = 0;
        }
    }

    fn reorder_channels(
        &mut self,
        face: usize,
        source: [Option<usize>; SHADOWMASK_OWNER_CHANNELS],
        shared: &SharedAtlas<'_>,
    ) {
        if source
            .iter()
            .enumerate()
            .all(|(position, channel)| channel.is_none_or(|channel| channel == position))
        {
            return;
        }
        let offsets = self.padded_rect_offsets(face, shared);
        let data = self.face_data_mut(face, shared);
        for base in offsets {
            let held: [u8; SHADOWMASK_OWNER_CHANNELS] =
                std::array::from_fn(|channel| data[base + channel]);
            for (position, channel) in source.iter().enumerate() {
                data[base + position] = channel.map_or(0, |channel| held[channel]);
            }
        }
    }

    /// Fill `face`'s chart padding in every `owned` position from that face's
    /// own chart-interior values, so a bilinear tap straddling the interior
    /// edge reads the owner's edge visibility rather than half of it. Mirrors
    /// the lightmap's edge dilation: `CHART_PADDING_TEXELS` passes, the
    /// interior covered from the start, and each pass giving every uncovered
    /// texel the mean of the 8-neighbours covered when the pass began. That
    /// makes pass `n` exactly padding ring `n` reading ring `n - 1`, so the
    /// rings fill in place with no snapshot.
    ///
    /// Unlike the lightmap's whole-layer pass, this one never reads outside
    /// `face`'s padded rect: channel `s` on a neighbouring chart holds a
    /// different light. The mean is the integer mean rounded half up, taken
    /// on the stored u8 values. Unowned positions stay zero, and an owner
    /// occluded along an edge leaves zero padding there.
    fn dilate_owned_padding(
        &mut self,
        face: usize,
        owned: [bool; SHADOWMASK_OWNER_CHANNELS],
        shared: &SharedAtlas<'_>,
    ) {
        let chart = &shared.charts[face];
        // The walk writes nothing on a zero-extent chart, so it has no edge.
        if !owned.contains(&true) || chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
            return;
        }
        let (atlas_width, placement_x, placement_y) = self.face_rect(face, shared);
        let data = self.face_data_mut(face, shared);
        let padding = i64::from(CHART_PADDING_TEXELS);
        // Inclusive interior bounds in padded-rect coordinates.
        let (x_lo, x_hi) = (padding, i64::from(chart.width_texels) - padding - 1);
        let (y_lo, y_hi) = (padding, i64::from(chart.height_texels) - padding - 1);
        debug_assert!(x_lo <= x_hi && y_lo <= y_hi, "a lit chart has an interior");
        // Chebyshev distance from the interior: 0 inside, ring `n` in padding.
        let ring_of =
            move |x: i64, y: i64| (x_lo - x).max(x - x_hi).max(y_lo - y).max(y - y_hi).max(0);
        // Only called for texels of ring `CHART_PADDING_TEXELS` or less, which
        // all lie inside the padded rect, so both coordinates are non-negative.
        let offset = move |x: i64, y: i64| {
            ((placement_y + y as usize) * atlas_width + placement_x + x as usize)
                * SHADOWMASK_OWNER_CHANNELS
        };
        for ring in 1..=padding {
            for (x, y) in rect_perimeter(x_lo - ring, x_hi + ring, y_lo - ring, y_hi + ring) {
                let mut sums = [0u32; SHADOWMASK_OWNER_CHANNELS];
                let mut covered = 0u32;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        // Inner ring only: the outer side and this ring were
                        // uncovered when the pass began.
                        if ring_of(nx, ny) != ring - 1 {
                            continue;
                        }
                        let base = offset(nx, ny);
                        for (position, sum) in sums.iter_mut().enumerate() {
                            *sum += u32::from(data[base + position]);
                        }
                        covered += 1;
                    }
                }
                // Every ring texel touches at least one inner-ring texel.
                debug_assert!(covered > 0);
                let base = offset(x, y);
                for (position, &sum) in sums.iter().enumerate() {
                    if owned[position] {
                        data[base + position] = ((sum + covered / 2) / covered) as u8;
                    }
                }
            }
        }
    }
}

/// Every texel on the border of the inclusive rect `[x0, x1] × [y0, y1]`,
/// each once. The rect is at least two texels across on each axis.
fn rect_perimeter(x0: i64, x1: i64, y0: i64, y1: i64) -> impl Iterator<Item = (i64, i64)> {
    let rows = [y0, y1]
        .into_iter()
        .flat_map(move |y| (x0..=x1).map(move |x| (x, y)));
    let columns = (y0 + 1..y1).flat_map(move |y| [(x0, y), (x1, y)]);
    rows.chain(columns)
}

/// World position of the chart-interior texel at bake-layer index `idx`.
fn texel_world_position(shared: &SharedAtlas<'_>, face: usize, idx: u32) -> glam::Vec3 {
    let placement = &shared.placements[face];
    let atlas_x = idx % shared.atlas_width;
    let atlas_y = idx / shared.atlas_width;
    let tx = (atlas_x - placement.x - CHART_PADDING_TEXELS) as i32;
    let ty = (atlas_y - placement.y - CHART_PADDING_TEXELS) as i32;
    chart_texel_world_position(&shared.charts[face], tx, ty)
}

pub(super) fn texel_plane_len(width: u32, height: u32) -> usize {
    (width as usize)
        .checked_mul(height as usize)
        .expect("shadowmask atlas plane exceeds addressable memory")
}
