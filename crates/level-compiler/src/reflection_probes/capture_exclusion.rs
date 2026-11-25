// Reflection-probe capture exclusion: a probe's face mask, excluded-hull exit classification and the leak guard.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::{DVec3, Vec3};

use super::capture_bounds::{HeldDepth, ProjectionBox};
use super::capture_fill::{Fill, KeptCandidate};
use super::hit::{FaceMask, SceneTracer, SurfaceHit};
use super::placement::PlacedProbe;
use crate::partition::Aabb;
use crate::partition::brush_bsp::REGION_CLASSIFICATION_EPSILON;

use super::capture_brushes::CaptureBrushes;

// Exit classification and the trace read this module's private state, so
// they are children.
#[path = "capture_exclusion_crossing.rs"]
mod crossing;
#[path = "capture_exclusion_trace.rs"]
mod trace;

/// A point this far outside an excluded hull, off its planes, touches it.
/// Brush CSG cuts both facing faces across a gap up to its 1 mm epsilon,
/// measured off a brush's planes, so this contact holds at every ray angle,
/// and the half again above the epsilon keeps such a gap from ever tying.
/// A gap of 1 to 1.5 mm that CSG left whole reads the same: the fill reads
/// the surviving kept face before the exit is classified, and an excluded
/// neighbour is crossed whether entered at once or by a re-trace. Bounds
/// this close are contact candidates.
pub(super) const CONTACT_TOLERANCE_M: f64 = 1.5 * REGION_CLASSIFICATION_EPSILON;

/// What a capture ray reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RayOutcome {
    /// Nothing within reach: no distance (the tile stores 0).
    Miss,
    /// A kept front face, as traced, behind any excluded brush the ray crossed.
    Hit(SurfaceHit),
    /// The first excluded surface the ray skipped, read as if it were kept:
    /// the ray found nothing behind the excluded brushes it may show.
    LeakGuard(SurfaceHit),
    /// A kept brush's side, crossed inside an excluded hull or in contact
    /// with it, where CSG removed it against that brush.
    Fill(Fill),
}

impl RayOutcome {
    /// The traced surface the ray reads, with its distance from the ray
    /// origin. A fill's surface is synthesized instead
    /// ([`super::capture_fill::fill_surface`]).
    #[cfg(test)]
    pub(crate) fn surface(&self) -> Option<&SurfaceHit> {
        match self {
            Self::Miss | Self::Fill(_) => None,
            Self::Hit(hit) | Self::LeakGuard(hit) => Some(hit),
        }
    }

    /// How far along the ray its surface lies; `None` on a miss.
    #[cfg(test)]
    pub(crate) fn distance(&self) -> Option<f32> {
        match self {
            Self::Miss => None,
            Self::Hit(hit) | Self::LeakGuard(hit) => Some(hit.distance),
            Self::Fill(fill) => Some(fill.distance),
        }
    }
}

/// How a test's trace treats a carried [`HeldDepth`].
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
enum HeldDepthCheck {
    Bounds,
    Ignored,
    RefusesAny,
}

/// One probe's excluded set as its capture rays read it.
#[derive(Debug, Clone)]
pub(crate) struct ProbeExclusion<'a> {
    brushes: CaptureBrushes<'a>,
    /// Excluded brush indices, sorted and deduplicated.
    excluded: Vec<u32>,
    /// Their surviving faces.
    mask: FaceMask,
    /// Faces absent from every primary ray, sorted: a glossy probe's
    /// reflector plane ([`super::self_transparency`]); empty for water.
    transparent_faces: Vec<u32>,
    transparent: FaceMask,
    /// The probe's facing normal: what "in front of" the reflector means.
    facing: DVec3,
    /// The frontmost plane of `transparent_faces` along `facing`; `None`
    /// without them. A ray heading behind it reads past it with the
    /// excluded brushes absent, no fill or leak guard (`trace`'s behind
    /// read).
    reflector_plane: Option<ReflectorPlane>,
    /// Per excluded brush, parallel to `excluded`: the brushes whose bounds
    /// touch its own within [`CONTACT_TOLERANCE_M`], among those touching the
    /// projection box. An exit point or a covering side's crossing lies within
    /// that tolerance of the hull's planes, which the padded bounds hold at
    /// every corner of 84° or more; past an acute edge (a thin wedge's tip) a
    /// brush a few millimetres off can be missed, and the ray re-traces. One
    /// off the box
    /// could only place the ray outside it, where the leak guard reads the
    /// same surface whatever that brush is.
    contacts: Vec<Contacts>,
    /// Every kept brush some excluded brush touches, sorted by brush.
    kept: Vec<KeptCandidate>,
    projection_box: ProjectionBox,
    /// Whether reads past a [`HeldDepth`] take the leak guard; off only so
    /// the no-leak oracle can show it catches the trace without it.
    #[cfg(test)]
    held_depth_check: HeldDepthCheck,
}

/// A probe's reflector plane: its transparent faces' frontmost height along
/// its facing normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ReflectorPlane {
    pub normal: DVec3,
    pub height: f64,
}

impl ReflectorPlane {
    /// The frontmost of `faces` along `normal`, each at its vertices' mean
    /// height; `None` for no face.
    fn frontmost(tracer: &SceneTracer<'_>, normal: DVec3, faces: &[u32]) -> Option<Self> {
        let geometry = tracer.geometry();
        let section = &geometry.geometry;
        faces
            .iter()
            .filter_map(|&face| {
                let range = geometry.face_index_ranges.get(face as usize)?;
                let start = range.index_offset as usize;
                let indices = section
                    .indices
                    .get(start..start + range.index_count as usize)?;
                let sum: f64 = indices
                    .iter()
                    .map(|&i| {
                        Vec3::from(section.vertices[i as usize].position)
                            .as_dvec3()
                            .dot(normal)
                    })
                    .sum();
                (!indices.is_empty()).then(|| sum / indices.len() as f64)
            })
            .max_by(f64::total_cmp)
            .map(|height| Self { normal, height })
    }
}

#[derive(Debug, Clone, Default)]
struct Contacts {
    /// Brushes this probe keeps, its kept solid, as slots of `kept`.
    kept: Vec<usize>,
    /// Other brushes this probe excludes, as slots of `excluded`.
    excluded: Vec<usize>,
}

impl<'a> ProbeExclusion<'a> {
    /// `excluded`: brush indices into `brushes.volumes`. Contacts are drawn
    /// per probe, so a brush another probe excludes is kept solid here when
    /// this one keeps it.
    pub(crate) fn new(
        tracer: &SceneTracer<'_>,
        brushes: CaptureBrushes<'a>,
        excluded: &[u32],
        probe: &PlacedProbe,
    ) -> Self {
        debug_assert!(
            excluded
                .iter()
                .all(|&brush| (brush as usize) < brushes.volumes.len()),
            "excluded brushes index the brush volumes"
        );
        let projection_box = ProjectionBox::of(probe);
        // Leaving out brushes wholly off the box captures the same only
        // while the capture point is inside it.
        debug_assert!(
            projection_box.holds(probe.capture_point_vec3().as_dvec3()),
            "the projection box holds the capture point: {probe:?}"
        );
        let mut excluded: Vec<u32> = excluded
            .iter()
            .copied()
            .filter(|&brush| (brush as usize) < brushes.volumes.len())
            .collect();
        // Slots and contact lookups binary-search it.
        excluded.sort_unstable();
        excluded.dedup();
        let mask = if excluded.is_empty() {
            FaceMask::default()
        } else {
            tracer.face_mask(
                excluded
                    .iter()
                    .flat_map(|&brush| brushes.faces.of(brush).iter().copied()),
            )
        };
        // One pass over the hulls per probe; every contact scan after it is
        // bounded by the brushes near the box.
        let near: Vec<u32> = if excluded.is_empty() {
            Vec::new()
        } else {
            (0..brushes.volumes.len() as u32)
                .filter(|&brush| projection_box.touches(&brushes.volumes[brush as usize].aabb))
                .collect()
        };
        let mut touching: Vec<(Vec<u32>, Vec<usize>)> = Vec::with_capacity(excluded.len());
        for &brush in &excluded {
            let bounds = &brushes.volumes[brush as usize].aabb;
            let (mut kept, mut others) = (Vec::new(), Vec::new());
            for &other in &near {
                if other == brush || !bounds_touch(bounds, &brushes.volumes[other as usize].aabb) {
                    continue;
                }
                match excluded.binary_search(&other) {
                    Ok(slot) => others.push(slot),
                    Err(_) => kept.push(other),
                }
            }
            touching.push((kept, others));
        }
        let mut kept_brushes: Vec<u32> = touching
            .iter()
            .flat_map(|(kept, _)| kept)
            .copied()
            .collect();
        kept_brushes.sort_unstable();
        kept_brushes.dedup();
        let kept = KeptCandidate::build(
            brushes.volumes,
            brushes.faces,
            tracer.geometry(),
            &kept_brushes,
        );
        let contacts = touching
            .into_iter()
            .map(|(kept, excluded)| Contacts {
                kept: kept
                    .iter()
                    .map(|brush| kept_brushes.binary_search(brush).expect("collected above"))
                    .collect(),
                excluded,
            })
            .collect();
        Self {
            brushes,
            excluded,
            mask,
            transparent_faces: Vec::new(),
            transparent: FaceMask::default(),
            facing: probe.facing.normal(),
            reflector_plane: None,
            contacts,
            kept,
            projection_box,
            #[cfg(test)]
            held_depth_check: HeldDepthCheck::Bounds,
        }
    }

    /// This exclusion with `faces` (sorted id-17 faces) absent from every
    /// primary ray, as if the static world lacked them. Shadow rays and the
    /// box fit trace the whole world, so they still see them.
    pub(crate) fn with_transparent_faces(
        mut self,
        tracer: &SceneTracer<'_>,
        faces: Vec<u32>,
    ) -> Self {
        self.transparent = tracer.face_mask(faces.iter().copied());
        self.reflector_plane = ReflectorPlane::frontmost(tracer, self.facing, &faces);
        self.transparent_faces = faces;
        self
    }

    /// The faces absent from every primary ray, ascending.
    pub(crate) fn transparent_faces(&self) -> &[u32] {
        &self.transparent_faces
    }

    /// This exclusion with no reflector plane: a ray heading behind it
    /// crosses excluded hulls as water's do, filling or taking the leak
    /// guard at the plane. The behind read's control.
    #[cfg(test)]
    pub(super) fn without_behind_reads(mut self) -> Self {
        self.reflector_plane = None;
        self
    }

    /// This exclusion with reads past a [`HeldDepth`] admitted, as before
    /// the depth was carried: the no-leak oracle's control.
    #[cfg(test)]
    pub(super) fn ignoring_held_depth(mut self) -> Self {
        self.held_depth_check = HeldDepthCheck::Ignored;
        self
    }

    /// This exclusion with any carried [`HeldDepth`] refusing every read, at
    /// any distance: a read that differs from the real trace's carried one.
    #[cfg(test)]
    pub(super) fn refusing_any_held_depth(mut self) -> Self {
        self.held_depth_check = HeldDepthCheck::RefusesAny;
        self
    }

    /// Whether `held` admits a read `distance` from the ray origin.
    pub(super) fn admits(&self, held: HeldDepth, distance: f64) -> bool {
        #[cfg(test)]
        match self.held_depth_check {
            HeldDepthCheck::Bounds => {}
            HeldDepthCheck::Ignored => return true,
            HeldDepthCheck::RefusesAny => return held == HeldDepth::NONE,
        }
        held.admits(distance)
    }

    /// The excluded brushes, ascending.
    pub(crate) fn excluded(&self) -> &[u32] {
        &self.excluded
    }

    /// Every kept brush some excluded brush touches, ascending: the
    /// candidates whose sides a fill reads and whose hulls are kept solid.
    pub(crate) fn kept_contacts(&self) -> impl Iterator<Item = u32> + '_ {
        self.kept.iter().map(|candidate| candidate.brush)
    }

    /// The post-cut faces of the excluded brush in `slot` of
    /// [`Self::excluded`]: its share of the mask, ascending.
    pub(crate) fn faces_of(&self, slot: usize) -> &[u32] {
        self.brush_faces(self.excluded[slot])
    }

    /// The post-cut faces of `brush`, ascending: for a kept contact
    /// candidate, the faces its fill pools.
    pub(crate) fn brush_faces(&self, brush: u32) -> &[u32] {
        self.brushes.faces.of(brush)
    }

    /// The contacts of the excluded brush in `slot` of [`Self::excluded`]:
    /// its kept brushes, then its other excluded brushes, each ascending.
    pub(crate) fn contacts_of(&self, slot: usize) -> (Vec<u32>, Vec<u32>) {
        let contacts = &self.contacts[slot];
        (
            contacts.kept.iter().map(|&k| self.kept[k].brush).collect(),
            contacts
                .excluded
                .iter()
                .map(|&e| self.excluded[e])
                .collect(),
        )
    }

    /// Whether the probe excludes nothing: its capture is the unfiltered one.
    pub(crate) fn is_empty(&self) -> bool {
        self.mask.is_empty()
    }
}

/// Bounds that overlap or touch within [`CONTACT_TOLERANCE_M`], so every
/// hull a contact test can reach is a candidate.
fn bounds_touch(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|axis| {
        a.min[axis] <= b.max[axis] + CONTACT_TOLERANCE_M
            && b.min[axis] <= a.max[axis] + CONTACT_TOLERANCE_M
    })
}
