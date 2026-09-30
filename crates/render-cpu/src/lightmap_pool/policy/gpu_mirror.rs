// Test-only CPU mirror of the pool texture and block table, executing plans as the GPU layer does.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::super::{BLOCK_TABLE_ENTRY_BYTES, BlockTableEntry};
use super::{DrainPlan, LightmapPoolModel};

/// Each texel records which block's data it holds.
pub(super) struct GpuMirror {
    edge: u32,
    layers: u32,
    texels: Vec<Option<u32>>,
    table: Vec<BlockTableEntry>,
    texture_allocations: u32,
}

impl GpuMirror {
    /// The pool as level install creates it: `layers + 1` array layers and
    /// the model's whole table.
    pub(super) fn new(model: &LightmapPoolModel) -> Self {
        let bytes = model.table_bytes();
        let table = bytes
            .chunks_exact(BLOCK_TABLE_ENTRY_BYTES)
            .map(|entry| {
                BlockTableEntry::from_words(std::array::from_fn(|w| {
                    u32::from_ne_bytes(entry[w * 4..w * 4 + 4].try_into().unwrap())
                }))
            })
            .collect();
        let edge = model.edge;
        Self {
            edge,
            layers: model.layers(),
            texels: vec![None; Self::texel_count(edge, model.layers())],
            table,
            texture_allocations: 1,
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

    pub(super) fn execute(&mut self, plan: &DrainPlan) {
        if let Some(growth) = plan.growth {
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
        for copy in &plan.copies {
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
        for upload in &plan.uploads {
            assert!(
                upload.placement.layer < self.layers,
                "upload into the spare"
            );
            for y in 0..upload.height {
                for x in 0..upload.width {
                    let p = upload.placement;
                    let at = self.at(p.layer, p.x + x, p.y + y);
                    self.texels[at] = Some(upload.block);
                }
            }
        }
        for write in &plan.table_writes {
            self.table[write.index as usize] = write.entry;
        }
    }

    /// The mirrored table equals the model's, and every resident entry
    /// samples its own block's texels in a usable layer of the active pool.
    pub(super) fn assert_matches(&self, model: &LightmapPoolModel) {
        assert_eq!(self.layers, model.layers(), "pool layers");
        assert_eq!(
            self.texture_allocations,
            model.texture_allocations(),
            "texture allocations"
        );
        assert_eq!(self.table[0], BlockTableEntry::None);
        for block in 0..model.block_count() {
            let entry = self.table[block as usize + 1];
            assert_eq!(entry, model.table_entry(block), "entry of block {block}");
            if let BlockTableEntry::Resident {
                placement,
                width,
                height,
            } = entry
            {
                assert!(placement.layer < self.layers, "block {block} in the spare");
                for y in 0..height {
                    for x in 0..width {
                        let at = self.at(placement.layer, placement.x + x, placement.y + y);
                        assert_eq!(
                            self.texels[at],
                            Some(block),
                            "block {block} texel ({x}, {y}) at {placement:?}"
                        );
                    }
                }
            }
        }
    }
}
