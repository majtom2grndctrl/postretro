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
use super::encode::encode_blocks_bc5;
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
}

pub(super) struct OwnershipFill<'a> {
    width: u32,
    height: u32,
    layer_count: u32,
    plane: usize,
    /// Cell blocks the encoder slices out of the bake layers, in block order.
    blocks: Vec<CellBlock>,
    /// Layer-major, four channels per texel, 0 = occluded. An owned channel
    /// therefore reads occluded wherever its owner wrote nothing.
    data: RawFillBuffer,
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
                }
            })
            .collect();
        let plane = texel_plane_len(shared.atlas_width, shared.atlas_height);
        let data_len = plane
            .checked_mul(layer_count as usize)
            .and_then(|texels| texels.checked_mul(SHADOWMASK_OWNER_CHANNELS))
            .expect("shadowmask atlas byte count exceeds addressable memory");
        Self {
            width: shared.atlas_width,
            height: shared.atlas_height,
            layer_count,
            plane,
            blocks: shared.layout.blocks.clone(),
            data: super::allocate_shadowmask_raw_fill(data_len),
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
        let layer_offset = (partition.target_layer as usize)
            .checked_mul(self.plane)
            .expect("shadowmask layer offset exceeds addressable memory");
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
                let offset =
                    (layer_offset + texel.idx as usize) * SHADOWMASK_OWNER_CHANNELS + channel;
                self.data[offset] = visibility;
            }
        }
    }

    /// Order every face's owners ascending, moving their mask values with them,
    /// then encode. No face owned means no section.
    pub(super) fn finish(
        mut self,
        shared: &SharedAtlas<'_>,
    ) -> (Option<ShadowmaskAtlasSection>, OwnerReport) {
        let mut owners: Vec<FaceOwners> = vec![NO_FACE_OWNERS; self.faces.len()];
        let mut any_owner = false;
        for face in 0..self.faces.len() {
            if self.cut_of[face].is_some() {
                continue;
            }
            let (entry, source) = self.faces[face].ascending_order();
            if entry != NO_FACE_OWNERS {
                any_owner = true;
                self.reorder_channels(face, source, shared);
            }
            owners[face] = entry;
        }
        let mut peak_cut_demand = 0;
        for c in 0..self.cuts.len() {
            self.release_owners_that_baked_nothing(c, shared);
            let cut = &self.cuts[c];
            let (entry, source) = cut.channels.ascending_order();
            peak_cut_demand = peak_cut_demand.max(cut.exact.len() as u32);
            let site = DropSite::CutFace {
                parent: cut.parent,
                first_face: cut.faces.start as u32,
                last_face: cut.faces.end as u32 - 1,
            };
            for (&spec_index, &lit_texels) in &cut.exact {
                if lit_texels >= LIT_TEXEL_FLOOR && cut.channels.channel_of(spec_index).is_none() {
                    self.drops.push(OwnerDrop {
                        site,
                        spec_index,
                        lit_texels,
                    });
                }
            }
            if entry != NO_FACE_OWNERS {
                any_owner = true;
                for face in self.cuts[c].faces.clone() {
                    self.reorder_channels(face, source, shared);
                    owners[face] = entry;
                }
            }
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
                .max(peak_cut_demand),
            drops,
        };
        let Self {
            width,
            height,
            layer_count,
            blocks,
            data: raw,
            face_map,
            ..
        } = self;
        drop(face_map);
        if !any_owner {
            return (None, report);
        }
        // Test-only full copy of the raw fill for exact-value assertions. It
        // is not a `RawFillBuffer`, so the residency counters never see it.
        #[cfg(test)]
        super::record_raw_fill(&raw);
        let blocks = encode_blocks_bc5(&raw, width, height, layer_count, &blocks);
        drop(raw);
        (Some(ShadowmaskAtlasSection { owners, blocks }), report)
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

    /// Every texel a bilinear tap on `face` can reach: its whole padded chart.
    fn padded_rect_offsets(
        &self,
        face: usize,
        shared: &SharedAtlas<'_>,
    ) -> impl Iterator<Item = usize> + use<> {
        let chart = &shared.charts[face];
        let placement = shared.placements[face];
        let (width, plane) = (self.width as usize, self.plane);
        let (chart_w, chart_h) = (chart.width_texels as usize, chart.height_texels as usize);
        (0..chart_h).flat_map(move |row| {
            let start = placement.layer as usize * plane
                + (placement.y as usize + row) * width
                + placement.x as usize;
            (start..start + chart_w).map(|texel| texel * SHADOWMASK_OWNER_CHANNELS)
        })
    }

    /// An evicted owner's values leave with it.
    fn zero_channel(&mut self, face: usize, channel: usize, shared: &SharedAtlas<'_>) {
        for base in self.padded_rect_offsets(face, shared).collect::<Vec<_>>() {
            self.data[base + channel] = 0;
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
        for base in self.padded_rect_offsets(face, shared).collect::<Vec<_>>() {
            let held: [u8; SHADOWMASK_OWNER_CHANNELS] =
                std::array::from_fn(|channel| self.data[base + channel]);
            for (position, channel) in source.iter().enumerate() {
                self.data[base + position] = channel.map_or(0, |channel| held[channel]);
            }
        }
    }
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

/// The raw fill buffer. Test builds count its live bytes when it is dropped,
/// so the compile-time bound observes the release itself.
pub(super) struct RawFillBuffer {
    bytes: Vec<u8>,
}

impl RawFillBuffer {
    pub(super) fn new(bytes: Vec<u8>) -> Self {
        #[cfg(test)]
        super::raw_fill_live_bytes_add(bytes.len());
        Self { bytes }
    }
}

impl std::ops::Deref for RawFillBuffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

impl std::ops::DerefMut for RawFillBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

#[cfg(test)]
impl Drop for RawFillBuffer {
    fn drop(&mut self) {
        super::raw_fill_live_bytes_sub(self.bytes.len());
    }
}

pub(super) fn texel_plane_len(width: u32, height: u32) -> usize {
    (width as usize)
        .checked_mul(height as usize)
        .expect("shadowmask atlas plane exceeds addressable memory")
}
