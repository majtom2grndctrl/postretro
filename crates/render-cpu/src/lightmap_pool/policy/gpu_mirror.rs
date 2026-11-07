// Test-only CPU mirror of both pool textures and the block table, executing plans as the GPU layer does.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap_pool::{PerPool, PoolKind, mask_item};

use super::super::{
    BLOCK_TABLE_ENTRY_BYTES, BlockAddresses, BlockPlacement, BlockTableEntry, MaskAddresses,
};
use super::{DrainPlan, LightmapPoolModel, PoolCopy, PoolGrowth};

/// One pool texture. Each texel records which item's data it holds: a block
/// id in the lightmap pool, a plane in the mask pool, so a block's two planes
/// stay told apart.
pub(super) struct TextureMirror {
    edge: u32,
    layers: u32,
    texels: Vec<Option<u32>>,
    texture_allocations: u32,
}

impl TextureMirror {
    fn new(edge: u32, layers: u32, texture_allocations: u32) -> Self {
        Self {
            edge,
            layers,
            texels: vec![None; Self::texel_count(edge, layers)],
            texture_allocations,
        }
    }

    fn texel_count(edge: u32, layers: u32) -> usize {
        (layers as usize + 1) * (edge * edge) as usize
    }

    fn at(&self, layer: u32, x: u32, y: u32) -> usize {
        assert!(layer <= self.layers, "layer {layer} past the spare");
        assert!(x < self.edge && y < self.edge);
        (layer as usize * self.edge as usize + y as usize) * self.edge as usize + x as usize
    }

    fn fill(&mut self, item: u32, place: BlockPlacement, width: u32, height: u32) {
        for y in 0..height {
            for x in 0..width {
                let at = self.at(place.layer, place.x + x, place.y + y);
                self.texels[at] = Some(item);
            }
        }
    }

    fn assert_holds(&self, item: u32, place: BlockPlacement, width: u32, height: u32) {
        for y in 0..height {
            for x in 0..width {
                let at = self.at(place.layer, place.x + x, place.y + y);
                assert_eq!(
                    self.texels[at],
                    Some(item),
                    "item {item} texel ({x}, {y}) at {place:?}"
                );
            }
        }
    }

    fn grow(&mut self, growth: PoolGrowth) {
        assert_eq!(
            growth.from_layers, self.layers,
            "growth from the active pool"
        );
        assert!(growth.to_layers > growth.from_layers);
        let live = (growth.from_layers * self.edge * self.edge) as usize;
        let mut grown = vec![None; Self::texel_count(self.edge, growth.to_layers)];
        grown[..live].copy_from_slice(&self.texels[..live]);
        self.texels = grown;
        self.layers = growth.to_layers;
        self.texture_allocations += 1;
    }

    fn copy(&mut self, copy: &PoolCopy) {
        assert_ne!(
            copy.src.layer, copy.dst.layer,
            "WebGPU forbids a copy within one subresource: {copy:?}"
        );
        let mut rect = Vec::with_capacity((copy.width * copy.height) as usize);
        for y in 0..copy.height {
            for x in 0..copy.width {
                rect.push(self.texels[self.at(copy.src.layer, copy.src.x + x, copy.src.y + y)]);
            }
        }
        for y in 0..copy.height {
            for x in 0..copy.width {
                let at = self.at(copy.dst.layer, copy.dst.x + x, copy.dst.y + y);
                self.texels[at] = rect[(y * copy.width + x) as usize];
            }
        }
    }

    fn upload(&mut self, item: u32, place: BlockPlacement, width: u32, height: u32) {
        assert!(place.layer < self.layers, "upload into the spare");
        self.fill(item, place, width, height);
    }
}

pub(super) struct GpuMirror {
    pools: PerPool<TextureMirror>,
    table: Vec<BlockTableEntry>,
}

impl GpuMirror {
    /// The pools as level install creates them: `layers + 1` array layers
    /// each and the model's whole table.
    pub(super) fn new(model: &LightmapPoolModel) -> Self {
        let bytes = model.table_bytes();
        let table = bytes
            .as_chunks::<BLOCK_TABLE_ENTRY_BYTES>()
            .0
            .iter()
            .map(|entry| {
                BlockTableEntry::from_words(std::array::from_fn(|w| {
                    u32::from_ne_bytes(entry[w * 4..w * 4 + 4].try_into().unwrap())
                }))
            })
            .collect();
        let mirror = |kind| {
            let pool = model.pool(kind);
            TextureMirror::new(pool.edge(), pool.layers(), pool.texture_allocations())
        };
        Self {
            pools: PerPool {
                lightmap: mirror(PoolKind::Lightmap),
                mask: mirror(PoolKind::Mask),
            },
            table,
        }
    }

    /// `kind`'s pool with no model behind it, for a hand-built copy list.
    pub(super) fn blank(kind: PoolKind, edge: u32, layers: u32) -> Self {
        let mut pools = PerPool {
            lightmap: TextureMirror::new(edge, 0, 0),
            mask: TextureMirror::new(edge, 0, 0),
        };
        pools[kind] = TextureMirror::new(edge, layers, 1);
        Self {
            pools,
            table: Vec::new(),
        }
    }

    /// Write `item`'s texels at `place` in `kind`'s pool, as an upload would.
    pub(super) fn fill(
        &mut self,
        kind: PoolKind,
        item: u32,
        place: BlockPlacement,
        width: u32,
        height: u32,
    ) {
        self.pools[kind].fill(item, place, width, height);
    }

    /// Every texel of `width × height` at `place` in `kind`'s pool is `item`'s.
    pub(super) fn assert_holds(
        &self,
        kind: PoolKind,
        item: u32,
        place: BlockPlacement,
        width: u32,
        height: u32,
    ) {
        self.pools[kind].assert_holds(item, place, width, height);
    }

    pub(super) fn execute(&mut self, plan: &DrainPlan) {
        for kind in PoolKind::ALL {
            if let Some(growth) = plan.pools[kind].growth {
                self.pools[kind].grow(growth);
            }
            for copy in &plan.pools[kind].copies {
                self.pools[kind].copy(copy);
            }
        }
        for upload in &plan.uploads {
            let (block, width, height) = (upload.block, upload.width, upload.height);
            self.pools
                .lightmap
                .upload(block, upload.placement, width, height);
            for (plane, place) in upload.planes.iter().enumerate() {
                if let Some(place) = place {
                    let item = mask_item(block, plane);
                    self.pools.mask.upload(item, *place, width, height);
                }
            }
        }
        for write in &plan.table_writes {
            self.table[write.index as usize] = write.entry;
        }
    }

    /// The mirrored table equals the model's, and every resident entry
    /// samples its own block's texels and its own planes in usable layers of
    /// the active pools.
    pub(super) fn assert_matches(&self, model: &LightmapPoolModel) {
        for kind in PoolKind::ALL {
            let (mirror, pool) = (&self.pools[kind], model.pool(kind));
            assert_eq!(mirror.layers, pool.layers(), "{} pool layers", kind.name());
            assert_eq!(
                mirror.texture_allocations,
                pool.texture_allocations(),
                "{} pool texture allocations",
                kind.name()
            );
        }
        assert_eq!(self.table[0], BlockTableEntry::None);
        for block in 0..model.block_count() {
            let entry = self.table[block as usize + 1];
            assert_eq!(entry, model.table_entry(block), "entry of block {block}");
            let BlockTableEntry::Resident {
                addresses: BlockAddresses { lightmap, masks },
                width,
                height,
            } = entry
            else {
                continue;
            };
            let (pools, layers) = (&self.pools, |kind: PoolKind| self.pools[kind].layers);
            assert!(
                lightmap.layer < layers(PoolKind::Lightmap),
                "block {block} in the spare"
            );
            pools.lightmap.assert_holds(block, lightmap, width, height);
            if let MaskAddresses::Planes(planes) = masks {
                for (plane, place) in planes.iter().enumerate() {
                    let Some(place) = place else { continue };
                    assert!(
                        place.layer < layers(PoolKind::Mask),
                        "block {block} plane {plane} in the spare"
                    );
                    let item = mask_item(block, plane);
                    pools.mask.assert_holds(item, *place, width, height);
                }
            }
        }
    }
}
