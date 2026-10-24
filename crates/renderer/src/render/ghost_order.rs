// Back-to-front order of a frame's skinned and mover ghosts, and its recording:
// each ghost writes its nearest surface to the ghost depth, then its colour.
// See: context/lib/rendering_pipeline.md §7.9

use glam::{Mat4, Vec3};
use postretro_render_cpu::ghost_transparency::GhostTransparency;
use postretro_render_cpu::mesh_instances::{MeshFramePlan, MeshPaletteCacheKey, PlannedInstance};

use super::GpuTexture;
use super::ghost_pipeline::{GhostDrawStep, SkinnedGhostSpan};
use super::kinematic_brush::KinematicBrushPass;
use super::kinematic_ghost::{KinematicGhostPass, MoverGhostBounds};
use super::mesh_pass::MeshPass;

/// How far outside a mover ghost's bounds, in metres along each of the
/// mover's axes, a skinned ghost's feet may sit and still stand on or in it.
/// A standing pawn's collision capsule rests a skin (2 cm) above the floor,
/// and its pose envelope's bottom need not meet the capsule's exactly; a
/// tenth of a metre absorbs both and float error at the box face, and stays
/// well short of an actor merely near a mover, such as one waiting half a
/// metre behind a door.
pub(super) const MOVER_CONTACT_MARGIN: f32 = 0.1;

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

/// Where one skinned ghost stands, and its sort key.
#[derive(Debug, Clone, Copy)]
struct Footing {
    feet: Vec3,
    depth: f32,
}

/// The frame's ghosts, farthest first. Every list is retained scratch, so a
/// warm frame builds the order without allocating.
///
/// Ghosts sort as whole objects, each by one key, not per pixel. Where that
/// order is wrong for a pair at some pixel, the ghost nearer there hides the
/// one drawn after it, as if it were opaque, instead of blending over it.
/// Two ghosts that interpenetrate can do this where they overlap, and so can
/// a ghost close to a large mover ghost without standing on or in it, such
/// as one just behind the near end of a long ghost wall seen along its
/// length.
#[derive(Default)]
pub(super) struct GhostDrawOrder {
    units: Vec<OrderedUnit>,
    spans: Vec<SkinnedGhostSpan>,
    members: Vec<SkinnedMember>,
    footings: Vec<Footing>,
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
    /// one link); each mover ghost is its own, given as its placed bounds in
    /// draw-slot order. Each ghost keys on the depth of its bounds centre
    /// under `view_proj`: a skinned ghost its holder's, a mover ghost its
    /// geometry's.
    ///
    /// Contact then lifts a mover: when a skinned ghost's feet (the bottom
    /// centre of its holder's bounds along world up) lie in the mover's
    /// oriented bounds, grown by [`MOVER_CONTACT_MARGIN`], and that ghost
    /// keys at or past the mover, the mover keys just past it. So a platform,
    /// lift floor or cab draws before the farthest ghost it carries, and its
    /// riders blend over it. Keys stay scalar, so the sort is unchanged.
    /// Cost: one contact test per mover and skinned ghost pair.
    ///
    /// A key point at or behind the eye sorts nearest, so that ghost draws
    /// last: a ghost wrapping the camera, a cab the camera rides in included,
    /// stays on top. A ghost whose bounds or transform are not finite, or a
    /// mover whose transform has no inverse, touches nothing.
    pub(super) fn build(
        &mut self,
        skinned: Option<&MeshFramePlan>,
        movers: impl IntoIterator<Item = MoverGhostBounds>,
        view_proj: Mat4,
    ) {
        self.units.clear();
        self.spans.clear();
        self.members.clear();
        self.footings.clear();

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
                let anchor = planned(plan, anchor);
                let depth = view_depth(view_proj, instance_center(anchor));
                if let Some(feet) = feet_point(anchor) {
                    self.footings.push(Footing { feet, depth });
                }

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

        for (draw, bounds) in movers.into_iter().enumerate() {
            let mut depth = view_depth(view_proj, bounds.center());
            for footing in &self.footings {
                // Strictly past: an equal key would fall to build order,
                // which puts the skinned ghost first.
                if footing.depth >= depth && bounds.contains(footing.feet, MOVER_CONTACT_MARGIN) {
                    depth = footing.depth.next_up();
                }
            }
            let order = self.units.len() as u32;
            self.units.push(OrderedUnit {
                depth,
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
    /// Cost, in both modes: two pipeline switches per ghost, and one draw per
    /// submesh per ghost span (per material span for a mover ghost).
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

/// Where a ghost instance stands: the bottom centre, along world up (+Y), of
/// the world-axis box around its posed envelope. `None` when the envelope is
/// not finite and ordered, or the transform is not finite.
fn feet_point(instance: &PlannedInstance) -> Option<Vec3> {
    let bounds = instance.bounds;
    if !(bounds.min.is_finite() && bounds.max.is_finite() && bounds.min.cmple(bounds.max).all()) {
        return None;
    }
    let center = instance
        .transform
        .transform_point3((bounds.min + bounds.max) * 0.5);
    let half = (bounds.max - bounds.min) * 0.5;
    // How far the transformed envelope reaches below its centre along +Y.
    let below = instance.transform.row(1).truncate().abs().dot(half);
    let feet = center - Vec3::Y * below;
    feet.is_finite().then_some(feet)
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
