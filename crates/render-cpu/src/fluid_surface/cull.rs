// Per-frame fluid surface culling against the visible cells' bounds.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::Vec3;
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_visibility::VisibleCells;

use super::{FluidSurfaceDraw, FluidSurfaceLook, surface_bounds};

fn aabb_intersects(a_min: Vec3, a_max: Vec3, b_min: Vec3, b_max: Vec3) -> bool {
    a_min.cmple(b_max).all() && a_max.cmpge(b_min).all()
}

/// Identity of the volume slice the cached bounds came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BoundsSource {
    ptr: usize,
    len: usize,
}

/// Per-frame fluid surface culling: which volumes draw, with what packed look,
/// in back-to-front order. Reuses its buffers; allocates only while growing.
#[derive(Debug, Default)]
pub struct FluidSurfaceRenderCollector {
    /// `(volume_index, min, max)` for each volume with drawable faces.
    bounds: Vec<(u32, Vec3, Vec3)>,
    bounds_source: Option<BoundsSource>,
    visible_cell_bounds: Vec<(Vec3, Vec3)>,
    /// Draws keyed by squared camera distance, sorted far to near.
    sorted: Vec<(f32, FluidSurfaceDraw)>,
    draws: Vec<FluidSurfaceDraw>,
}

impl FluidSurfaceRenderCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the installed level. Call at level unload, so a later level
    /// whose volume slice lands at the same address rebuilds its bounds.
    pub fn clear(&mut self) {
        self.bounds.clear();
        self.bounds_source = None;
        self.visible_cell_bounds.clear();
        self.sorted.clear();
        self.draws.clear();
    }

    /// Collect this frame's fluid draws. A volume draws when its surface
    /// bounds touch a visible cell's bounds (`DrawAll` draws every volume) and
    /// `look(volume_index)` names a look that shows something: a visible
    /// surface opacity, or depth fog, which draws on its own at opacity 0.
    /// `None` means the client lacks the fluid or the volume is dry, and draws
    /// nothing. Draws come back far to near, the order alpha blending needs.
    pub fn collect<C, L>(
        &mut self,
        volumes: &[EnvironmentVolumeRecord],
        visible: &VisibleCells,
        cell_bounds: C,
        camera_position: Vec3,
        look: L,
    ) -> &[FluidSurfaceDraw]
    where
        C: Fn(u32) -> Option<(Vec3, Vec3)>,
        L: Fn(usize) -> Option<FluidSurfaceLook>,
    {
        self.refresh_bounds(volumes);
        self.sorted.clear();
        self.draws.clear();
        if self.bounds.is_empty() {
            return &self.draws;
        }

        self.visible_cell_bounds.clear();
        if let VisibleCells::Culled(cells) = visible {
            self.visible_cell_bounds
                .extend(cells.iter().filter_map(|&cell| cell_bounds(cell)));
        }

        for &(volume_index, min, max) in &self.bounds {
            let visible_now = match visible {
                VisibleCells::DrawAll => true,
                VisibleCells::Culled(_) => self
                    .visible_cell_bounds
                    .iter()
                    .any(|&(cell_min, cell_max)| aabb_intersects(min, max, cell_min, cell_max)),
            };
            if !visible_now {
                continue;
            }
            let Some(look) = look(volume_index as usize) else {
                continue;
            };
            let params = look.pack();
            if !params.draws() {
                continue;
            }
            let distance_sq = ((min + max) * 0.5).distance_squared(camera_position);
            self.sorted.push((
                distance_sq,
                FluidSurfaceDraw {
                    volume_index,
                    params,
                },
            ));
        }
        self.sorted
            .sort_unstable_by(|(a, _), (b, _)| b.total_cmp(a));
        self.draws.extend(self.sorted.iter().map(|(_, draw)| *draw));
        &self.draws
    }

    pub fn draws(&self) -> &[FluidSurfaceDraw] {
        &self.draws
    }

    fn refresh_bounds(&mut self, volumes: &[EnvironmentVolumeRecord]) {
        let source = BoundsSource {
            ptr: volumes.as_ptr() as usize,
            len: volumes.len(),
        };
        if self.bounds_source == Some(source) {
            return;
        }
        self.bounds.clear();
        self.bounds
            .extend(volumes.iter().enumerate().filter_map(|(index, volume)| {
                surface_bounds(volume).map(|(min, max)| (index as u32, min, max))
            }));
        self.bounds_source = Some(source);
    }
}
