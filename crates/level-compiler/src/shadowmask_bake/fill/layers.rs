//! Layer close completes uncut faces and the cuts whose last sub-face arrives.
use postretro_level_format::coded_plane::store_plane;

use super::*;
use crate::lightmap_bake::copy_unit_rect;
use crate::shadowmask_bake::encode::encode_mask_block;
use crate::shadowmask_bake::mask_layout::{
    assert_layout_sound, copy_owned_charts, face_offset, layout_mask_block,
};

impl OwnershipFill<'_> {
    /// The window may have future partitions resident; only ordered consumption
    /// can open their raw fill, after the preceding layer has released its own.
    pub(in crate::shadowmask_bake) fn advance_layer(
        &mut self,
        target: u32,
        shared: &SharedAtlas<'_>,
    ) {
        if self.layer == Some(target) {
            return;
        }
        assert!(
            target < self.layer_count,
            "shadowmask partition layer out of range"
        );
        if let Some(layer) = self.layer {
            assert!(
                target > layer,
                "shadowmask partitions must arrive layer-major"
            );
            self.close_layer(shared);
        }
        while self.next_layer < target {
            self.open_layer(self.next_layer);
            self.close_layer(shared);
        }
        self.open_layer(target);
    }

    fn open_layer(&mut self, layer: u32) {
        assert!(
            self.data.is_none(),
            "release the old raw layer before opening another"
        );
        self.layer = Some(layer);
        self.data = Some(super::super::allocate_shadowmask_raw_fill(
            self.plane * SHADOWMASK_OWNER_CHANNELS,
            &self.probe,
        ));
    }

    pub(super) fn close_layer(&mut self, shared: &SharedAtlas<'_>) {
        let layer = self.layer.expect("a shadowmask layer is open");
        for face in 0..shared.placements.len() {
            if shared.placements[face].layer != layer || self.cut_of[face].is_some() {
                continue;
            }
            let (entry, source) = self.faces[face].ascending_order();
            self.finish_face(face, entry, source, shared);
        }
        for c in 0..self.cuts.len() {
            if self.cuts[c].last_layer != layer {
                continue;
            }
            self.release_owners_that_baked_nothing(c, shared);
            let cut = &self.cuts[c];
            let (entry, source) = cut.channels.ascending_order();
            self.peak_cut_demand = self.peak_cut_demand.max(cut.exact.len() as u32);
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
            for face in self.cuts[c].faces.clone() {
                self.finish_face(face, entry, source, shared);
            }
        }
        // Release completed carries before allocating this layer's new carry.
        for block_id in 0..self.blocks.len() {
            if self.hold_until[block_id] > layer {
                continue;
            }
            let Some(raw) = self.carry[block_id].take() else {
                continue;
            };
            let block = self.blocks[block_id];
            #[cfg(test)]
            self.record_block_snapshot(block, &raw);
            // Its last cut face finished above, so its owners and its mask
            // layout are fixed from here on.
            self.finish_mask_block(block_id, &raw, block.width, [block.x, block.y], shared);
            drop(raw);
        }
        #[cfg(test)]
        {
            let start = layer as usize * self.plane * SHADOWMASK_OWNER_CHANNELS;
            let raw = self.data.as_ref().unwrap();
            self.raw_snapshot[start..start + raw.len()].copy_from_slice(raw);
        }
        let mut ready_ids = Vec::new();
        for (block_id, &block) in self.blocks.iter().enumerate() {
            if block.layer != layer {
                continue;
            }
            if self.hold_until[block_id] > layer {
                let raw = copy_unit_rect(
                    self.data.as_ref().unwrap(),
                    self.width,
                    4,
                    [block.x, block.y, block.width, block.height],
                );
                self.carry[block_id] = Some(RawFillBuffer::new(raw, &self.probe));
            } else {
                ready_ids.push(block_id);
            }
        }
        let data = self.data.take().expect("the closing layer is open");
        for block_id in ready_ids {
            self.finish_mask_block(block_id, &data, self.width, [0, 0], shared);
        }
        // This release precedes the next layer's allocation, including when
        // another worker has already baked the first partition of that layer.
        drop(data);
        self.layer = None;
        self.next_layer = layer + 1;
        #[cfg(test)]
        self.probe.close_layer();
    }

    /// Lay out `block_id`'s mask block from its faces' final owners, copy each
    /// owned chart into it, record each owned face's offset and the block's
    /// extents, and encode it. `source` holds the block's raw mask, 4 bytes
    /// per texel and `source_width` texels per row, its texel `(0, 0)` at
    /// bake-layer `source_origin`: the working layer, or the block's carry.
    fn finish_mask_block(
        &mut self,
        block_id: usize,
        source: &[u8],
        source_width: u32,
        source_origin: [u32; 2],
        shared: &SharedAtlas<'_>,
    ) {
        let block = self.blocks[block_id];
        let faces = &self.block_faces[block_id];
        let layout = layout_mask_block(&block, faces, &self.owners, shared);
        assert_layout_sound(&layout, &self.owners, shared);
        for &(face, mask_origin) in &layout.charts {
            self.mask_offsets[face as usize] = face_offset(&block, face, mask_origin, shared);
        }
        if layout.identity {
            self.identity_mask_blocks += 1;
        }
        let image = copy_owned_charts(source, source_width, source_origin, &layout, shared);
        if let Some(capture) = &self.chart_dump {
            capture.capture_mask_block(
                faces,
                &image,
                layout.extent.0,
                [block.x, block.y],
                &self.mask_offsets,
                &self.owners,
                shared,
            );
        }
        let (width, height) = layout.extent;
        self.encoded[block_id] = encode_mask_block(&image, width, height, layout.plane_b)
            .map(|plane| store_plane(self.codec, plane));
        self.mask_extents[block_id] = layout.extents();
        #[cfg(test)]
        self.mask_snapshots
            .push(crate::shadowmask_bake::MaskBlockSnapshot {
                block: block_id as u32,
                layout,
                image,
            });
    }

    fn finish_face(
        &mut self,
        face: usize,
        entry: FaceOwners,
        source: [Option<usize>; SHADOWMASK_OWNER_CHANNELS],
        shared: &SharedAtlas<'_>,
    ) {
        self.owners[face] = entry;
        if entry != NO_FACE_OWNERS {
            self.any_owner = true;
            self.reorder_channels(face, source, shared);
            self.dilate_owned_padding(face, source.map(|channel| channel.is_some()), shared);
        }
    }

    #[cfg(test)]
    pub(in crate::shadowmask_bake) fn set_raw_fill_probe(&mut self, probe: RawFillProbe) {
        assert!(
            self.data.is_none(),
            "attach accounting before the first partition"
        );
        self.probe = probe;
        super::super::register_raw_fill_probe(&self.probe);
    }

    #[cfg(test)]
    fn record_block_snapshot(&mut self, block: CellBlock, raw: &[u8]) {
        let layer_start = block.layer as usize * self.plane * 4;
        for y in 0..block.height as usize {
            let source = y * block.width as usize * 4;
            let dest =
                layer_start + ((block.y as usize + y) * self.width as usize + block.x as usize) * 4;
            self.raw_snapshot[dest..dest + block.width as usize * 4]
                .copy_from_slice(&raw[source..source + block.width as usize * 4]);
        }
    }
}
