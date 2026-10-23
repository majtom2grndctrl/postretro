// Back-to-front order of a frame's skinned and mover ghosts, and its recording: each
// ghost writes its own nearest surface into the ghost depth target, then draws its colour there.
// See: context/lib/rendering_pipeline.md §7.9

use glam::{Mat4, Vec3};
use postretro_render_cpu::ghost_transparency::GhostTransparency;
use postretro_render_cpu::mesh_instances::{MeshFramePlan, MeshPaletteCacheKey, PlannedInstance};

use super::GpuTexture;
use super::kinematic_brush::KinematicBrushPass;
use super::kinematic_ghost::KinematicGhostPass;
use super::mesh_pass::MeshPass;

/// Which half of one ghost's draw pair to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GhostDrawStep {
    /// Colour writes off; depth written where nearer than everything drawn
    /// so far, so the target holds this ghost's nearest surface.
    Depth,
    /// Colour where the fragment equals the ghost depth; no depth write.
    Colour,
}

/// One skinned plan group's instances that belong to one ghost, as an
/// absolute instance-buffer range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SkinnedGhostSpan {
    pub(super) group: u32,
    pub(super) start: u32,
    pub(super) end: u32,
}

impl SkinnedGhostSpan {
    pub(super) fn instances(&self) -> std::ops::Range<u32> {
        self.start..self.end
    }
}

/// One ghost: the unit that runs its depth draw then its colour draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GhostUnit {
    /// One holder and its attachments seen through one link:
    /// `count` spans from `first`.
    Skinned { first: u32, count: u32 },
    /// One mover ghost, by its slot in the mover ghost list.
    Mover { draw: u32 },
}

#[derive(Debug, Clone, Copy)]
struct OrderedUnit {
    depth: f32,
    /// Build order, the tie-break that keeps equal depths deterministic.
    order: u32,
    unit: GhostUnit,
}

/// One skinned ghost instance, keyed by its ghost (namespace, holder).
#[derive(Debug, Clone, Copy)]
struct SkinnedMember {
    ghost: (u32, u32),
    group: u32,
    index: u32,
}

/// The frame's ghosts, farthest first. Every list is retained scratch, so a
/// warm frame builds the order without allocating.
#[derive(Default)]
pub(super) struct GhostDrawOrder {
    units: Vec<OrderedUnit>,
    spans: Vec<SkinnedGhostSpan>,
    members: Vec<SkinnedMember>,
}

/// Everything the ordered ghost draws read.
pub(super) struct GhostDrawSources<'a> {
    pub(super) mesh_pass: &'a MeshPass,
    pub(super) skinned: Option<&'a MeshFramePlan>,
    pub(super) movers: &'a KinematicGhostPass,
    pub(super) brush: &'a KinematicBrushPass,
    pub(super) materials: &'a [GpuTexture],
}

impl GhostDrawOrder {
    /// Rebuild this frame's order. Skinned instances group into one ghost by
    /// their palette key's holder group (a holder and its attachments through
    /// one link); each mover ghost is its own. A ghost sorts by the depth of
    /// its bounds centre under `view_proj` (the holder's, for a skinned
    /// ghost). A centre at or behind the eye sorts nearest, so it draws last.
    pub(super) fn build(
        &mut self,
        skinned: Option<&MeshFramePlan>,
        mover_centers: impl IntoIterator<Item = Vec3>,
        view_proj: Mat4,
    ) {
        self.units.clear();
        self.spans.clear();
        self.members.clear();

        if let Some(plan) = skinned {
            for (group, draw_group) in plan.groups.iter().enumerate() {
                for (index, instance) in draw_group.instances.iter().enumerate() {
                    self.members.push(SkinnedMember {
                        ghost: instance.palette_cache_key.holder_group(),
                        group: group as u32,
                        index: index as u32,
                    });
                }
            }
            // One ghost's members become contiguous, each group's in buffer
            // order, so consecutive instances merge into one span.
            self.members
                .sort_unstable_by_key(|member| (member.ghost, member.group, member.index));
            let mut run_start = 0;
            while run_start < self.members.len() {
                let ghost = self.members[run_start].ghost;
                let run_end = self.members[run_start..]
                    .iter()
                    .position(|member| member.ghost != ghost)
                    .map_or(self.members.len(), |length| run_start + length);
                let run = &self.members[run_start..run_end];
                let anchor = run
                    .iter()
                    .find(|member| is_holder(planned(plan, member)))
                    .unwrap_or(&run[0]);
                let depth = view_depth(view_proj, instance_center(planned(plan, anchor)));

                let first = self.spans.len();
                for member in run {
                    let slot = plan.groups[member.group as usize].instance_offset + member.index;
                    let extends = self.spans[first..]
                        .last()
                        .is_some_and(|span| span.group == member.group && span.end == slot);
                    if extends {
                        self.spans.last_mut().expect("an extended span exists").end += 1;
                    } else {
                        self.spans.push(SkinnedGhostSpan {
                            group: member.group,
                            start: slot,
                            end: slot + 1,
                        });
                    }
                }
                let order = self.units.len() as u32;
                self.units.push(OrderedUnit {
                    depth,
                    order,
                    unit: GhostUnit::Skinned {
                        first: first as u32,
                        count: (self.spans.len() - first) as u32,
                    },
                });
                run_start = run_end;
            }
        }

        for (draw, center) in mover_centers.into_iter().enumerate() {
            let order = self.units.len() as u32;
            self.units.push(OrderedUnit {
                depth: view_depth(view_proj, center),
                order,
                unit: GhostUnit::Mover { draw: draw as u32 },
            });
        }

        // Unstable sorts never allocate; the build-order tie-break keeps the
        // result deterministic.
        self.units.sort_unstable_by(|a, b| {
            b.depth
                .total_cmp(&a.depth)
                .then_with(|| a.order.cmp(&b.order))
        });
    }

    /// The frame's ghosts, farthest first.
    pub(super) fn units(&self) -> impl Iterator<Item = GhostUnit> + '_ {
        self.units.iter().map(|ordered| ordered.unit)
    }

    /// A skinned ghost's spans; empty for a mover ghost.
    pub(super) fn spans(&self, unit: GhostUnit) -> &[SkinnedGhostSpan] {
        match unit {
            GhostUnit::Skinned { first, count } => {
                &self.spans[first as usize..(first + count) as usize]
            }
            GhostUnit::Mover { .. } => &[],
        }
    }

    /// Record every ghost, farthest first, into one pass holding the scene
    /// colour and the seeded ghost depth (load and store). Each ghost's depth
    /// draw then its colour draw: its inner surfaces never show through its
    /// outer ones, and a nearer ghost blends over a farther one, whose colour
    /// is already down. Where two ghosts interpenetrate, the later one draws
    /// only where it is nearer. Group 0 (camera) must be set by the caller.
    pub(super) fn record(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        sources: &GhostDrawSources<'_>,
        mode: GhostTransparency,
    ) {
        for unit in self.units() {
            for step in [GhostDrawStep::Depth, GhostDrawStep::Colour] {
                match unit {
                    GhostUnit::Skinned { .. } => {
                        if let Some(plan) = sources.skinned {
                            sources.mesh_pass.record_ghost_spans(
                                pass,
                                plan,
                                self.spans(unit),
                                mode,
                                step,
                            );
                        }
                    }
                    GhostUnit::Mover { draw } => sources.movers.record_ghost(
                        pass,
                        sources.brush,
                        sources.materials,
                        draw,
                        mode,
                        step,
                    ),
                }
            }
        }
    }
}

fn planned<'p>(plan: &'p MeshFramePlan, member: &SkinnedMember) -> &'p PlannedInstance {
    &plan.groups[member.group as usize].instances[member.index as usize]
}

fn is_holder(instance: &PlannedInstance) -> bool {
    matches!(
        instance.palette_cache_key,
        MeshPaletteCacheKey::Ghost {
            attachment_index: None,
            ..
        } | MeshPaletteCacheKey::Entity(_)
    )
}

/// World-space centre of an instance's pose envelope; its origin when the
/// envelope is degenerate.
fn instance_center(instance: &PlannedInstance) -> Vec3 {
    let local = (instance.bounds.min + instance.bounds.max) * 0.5;
    if local.is_finite() {
        instance.transform.transform_point3(local)
    } else {
        instance.transform.w_axis.truncate()
    }
}

/// Depth of `point` under `view_proj`, larger farther. At or behind the eye
/// it is nearest of all.
fn view_depth(view_proj: Mat4, point: Vec3) -> f32 {
    let clip = view_proj * point.extend(1.0);
    let depth = clip.z / clip.w;
    if clip.w > f32::EPSILON && depth.is_finite() {
        depth
    } else {
        f32::NEG_INFINITY
    }
}

#[cfg(test)]
#[path = "ghost_order_tests.rs"]
mod tests;
