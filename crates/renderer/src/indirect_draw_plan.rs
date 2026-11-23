// GPU-free ordered binds and draws over a frame's visible bucket ranges, and
// their one execution over the global indirect buffer.
// See: context/lib/rendering_pipeline.md §5 (Visible-span draws), §7.3

use postretro_render_data::geometry::BucketRange;

use crate::compute_cull::{DRAW_INDIRECT_SIZE, SetTextureFn};

/// GPU-free ordered binds and draws over the frame's range list. Expansion
/// is lazy, so adapters without multi-draw need no per-slot scratch allocation.
pub(crate) struct IndirectDrawPlan<'a> {
    ranges: &'a [BucketRange],
    multi_draw: bool,
    bind_material: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndirectDrawCommand {
    BindMaterial(u32),
    MultiDraw { byte_offset: u64, count: u32 },
    Draw { byte_offset: u64 },
}

impl<'a> IndirectDrawPlan<'a> {
    pub(crate) fn new(ranges: &'a [BucketRange], multi_draw: bool, bind_material: bool) -> Self {
        Self {
            ranges,
            multi_draw,
            bind_material,
        }
    }

    pub(crate) fn visit(&self, mut issue: impl FnMut(IndirectDrawCommand)) {
        let mut bound_bucket = None;
        for range in self.ranges.iter().filter(|range| range.leaf_count > 0) {
            if self.bind_material && bound_bucket != Some(range.material_bucket_id) {
                issue(IndirectDrawCommand::BindMaterial(range.material_bucket_id));
                bound_bucket = Some(range.material_bucket_id);
            }
            let byte_offset = u64::from(range.first_leaf) * DRAW_INDIRECT_SIZE;
            if self.multi_draw {
                issue(IndirectDrawCommand::MultiDraw {
                    byte_offset,
                    count: range.leaf_count,
                });
            } else {
                for slot in 0..range.leaf_count {
                    issue(IndirectDrawCommand::Draw {
                        byte_offset: byte_offset + u64::from(slot) * DRAW_INDIRECT_SIZE,
                    });
                }
            }
        }
    }
}

/// Execute the pure plan one to one over the camera's visible runs.
/// Depth-only pipelines have no group-1 material slot, so omit that bind.
pub(crate) fn draw_indirect_buckets<'a>(
    render_pass: &mut wgpu::RenderPass<'a>,
    indirect_buffer: &'a wgpu::Buffer,
    bucket_ranges: &[BucketRange],
    has_multi_draw_indirect: bool,
    set_texture_fn: Option<&SetTextureFn<'a>>,
) {
    let plan = IndirectDrawPlan::new(
        bucket_ranges,
        has_multi_draw_indirect,
        set_texture_fn.is_some(),
    );
    plan.visit(|command| match command {
        IndirectDrawCommand::BindMaterial(bucket) => {
            if let Some(bind) = set_texture_fn {
                bind(render_pass, bucket);
            }
        }
        IndirectDrawCommand::MultiDraw { byte_offset, count } => {
            render_pass.multi_draw_indexed_indirect(indirect_buffer, byte_offset, count);
        }
        IndirectDrawCommand::Draw { byte_offset } => {
            render_pass.draw_indexed_indirect(indirect_buffer, byte_offset);
        }
    });
}

#[cfg(test)]
mod draw_plan_tests {
    use super::*;

    fn commands(ranges: &[BucketRange], multi: bool, material: bool) -> Vec<IndirectDrawCommand> {
        let mut out = Vec::new();
        IndirectDrawPlan::new(ranges, multi, material).visit(|c| out.push(c));
        out
    }

    #[test]
    fn visible_runs_bind_once_per_bucket_and_expand_identically_without_multi_draw() {
        let ranges = [
            BucketRange {
                material_bucket_id: 2,
                first_leaf: 3,
                leaf_count: 2,
            },
            BucketRange {
                material_bucket_id: 2,
                first_leaf: 6,
                leaf_count: 1,
            },
            BucketRange {
                material_bucket_id: 7,
                first_leaf: 8,
                leaf_count: 2,
            },
        ];
        use IndirectDrawCommand::*;
        assert_eq!(
            commands(&ranges, true, true),
            vec![
                BindMaterial(2),
                MultiDraw {
                    byte_offset: 60,
                    count: 2
                },
                MultiDraw {
                    byte_offset: 120,
                    count: 1
                },
                BindMaterial(7),
                MultiDraw {
                    byte_offset: 160,
                    count: 2
                },
            ]
        );
        assert_eq!(
            commands(&ranges, false, true),
            vec![
                BindMaterial(2),
                Draw { byte_offset: 60 },
                Draw { byte_offset: 80 },
                Draw { byte_offset: 120 },
                BindMaterial(7),
                Draw { byte_offset: 160 },
                Draw { byte_offset: 180 },
            ]
        );
        for multi in [false, true] {
            let forward = commands(&ranges, multi, true);
            let draws: Vec<_> = forward
                .into_iter()
                .filter(|c| !matches!(c, BindMaterial(_)))
                .collect();
            assert_eq!(commands(&ranges, multi, false), draws);
            assert!(commands(&[], multi, true).is_empty());
        }
    }
}
