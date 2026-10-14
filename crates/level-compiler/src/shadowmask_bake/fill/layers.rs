//! Layer close completes uncut faces and the cuts whose last sub-face arrives.
use super::*;
use crate::lightmap_bake::copy_unit_rect;
use crate::shadowmask_bake::encode::encode_blocks_bc5;

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
            let local = CellBlock {
                x: 0,
                y: 0,
                layer: 0,
                ..block
            };
            self.encoded[block_id] =
                encode_blocks_bc5(&raw, block.width, block.height, 1, &[local])
                    .pop()
                    .unwrap();
            drop(raw);
        }
        #[cfg(test)]
        {
            let start = layer as usize * self.plane * SHADOWMASK_OWNER_CHANNELS;
            let raw = self.data.as_ref().unwrap();
            self.raw_snapshot[start..start + raw.len()].copy_from_slice(raw);
        }
        let mut ready = Vec::new();
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
                ready.push(CellBlock { layer: 0, ..block });
                ready_ids.push(block_id);
            }
        }
        if !ready.is_empty() {
            let groups = encode_blocks_bc5(
                self.data.as_ref().unwrap(),
                self.width,
                self.height,
                1,
                &ready,
            );
            for (block_id, groups) in ready_ids.into_iter().zip(groups) {
                self.encoded[block_id] = groups;
            }
        }
        // This release precedes the next layer's allocation, including when
        // another worker has already baked the first partition of that layer.
        self.data = None;
        self.layer = None;
        self.next_layer = layer + 1;
        #[cfg(test)]
        self.probe.close_layer();
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
