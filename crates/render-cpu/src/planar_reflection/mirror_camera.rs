// Planar mirror camera: the reflected view, its oblique raster projection, its cull frustum and crop.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{Mat4, Vec3, Vec4};
use postretro_visibility::{MirrorCropScratch, crop_mirror_view_proj};

use super::{
    MIRROR_CROP_PAD_RADIANS, PLANAR_CLIP_BIAS, PlanarCandidate, aabb_in_planes, view_proj_planes,
};
use crate::fluid_surface::fog::FLUID_FOG_BAND;

/// Index of the near plane in [`MirrorCamera::cull_planes`].
const NEAR_PLANE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MirrorCamera {
    pub eye: Vec3,
    pub cull_vp: Mat4,
    pub raster_vp: Mat4,
    /// World-space inward clip half-space: dot(plane, vec4(p, 1)) >= 0.
    pub clip_plane: Vec4,
    pub h: f32,
}

impl MirrorCamera {
    /// `projection` is the main camera's finite RH DirectX perspective.
    /// Cull depth uses D; raster depth keeps the main far distance.
    pub fn new(eye: Vec3, view: Mat4, projection: Mat4, h: f32, d: Option<f32>) -> Option<Self> {
        if !eye.is_finite()
            || !view.is_finite()
            || !projection.is_finite()
            || !h.is_finite()
            || eye.y - h <= FLUID_FOG_BAND
        {
            return None;
        }
        let reflection = Mat4::from_cols(
            Vec4::X,
            -Vec4::Y,
            Vec4::Z,
            Vec4::new(0.0, 2.0 * h, 0.0, 1.0),
        );
        let mirror_view = view * reflection;
        let clip_plane = Vec4::new(0.0, 1.0, 0.0, -(h - PLANAR_CLIP_BIAS));
        let camera_plane = mirror_view.inverse().transpose() * clip_plane;
        // Lengyel's oblique near plane: for [0,1] depth it replaces row 2.
        let corner = Vec4::new(
            if camera_plane.x >= 0.0 { 1.0 } else { -1.0 },
            if camera_plane.y >= 0.0 { 1.0 } else { -1.0 },
            1.0,
            1.0,
        );
        let denominator = camera_plane.dot(projection.inverse() * corner);
        if !denominator.is_finite() || denominator <= 1.0e-6 {
            return None;
        }
        let near_row = camera_plane / denominator;
        let mut oblique = projection;
        oblique.x_axis.z = near_row.x;
        oblique.y_axis.z = near_row.y;
        oblique.z_axis.z = near_row.z;
        oblique.w_axis.z = near_row.w;
        let mut cull_projection = projection;
        if let Some(d) = d.filter(|d| d.is_finite() && *d > 0.0) {
            // An RH [0, 1] perspective stores depth as z·f/(n−f) + n·f/(n−f),
            // so the ratio of those two terms is the near distance; with it,
            // the depth row is rebuilt with D as the far distance.
            let near = projection.w_axis.z / projection.z_axis.z;
            let far = d.max(near + 0.001);
            cull_projection.z_axis.z = far / (near - far);
            cull_projection.w_axis.z = near * far / (near - far);
        }
        // The mirror flips handedness; flipping x restores the main winding.
        let flip_x = Mat4::from_diagonal(Vec4::new(-1.0, 1.0, 1.0, 1.0));
        Some(Self {
            eye: reflection.transform_point3(eye),
            cull_vp: cull_projection * mirror_view,
            raster_vp: flip_x * oblique * mirror_view,
            clip_plane,
            h,
        })
    }

    /// The cull frustum's six inward planes (left, right, bottom, top, near,
    /// far) as `dot(plane, vec4(p, 1)) >= 0`. Unnormalized and
    /// allocation-free, for per-leaf tests on the frame path. Near is
    /// `clip_plane`, the biased water plane the mirror walk and its seeds
    /// take: the cull projection's own near sits the main camera's near
    /// distance from a mirror eye that can be barely more than the fog band
    /// below the water, so it would drop content just above the water that
    /// the walk reaches. The side
    /// planes still bound everything to in front of the eye, and the
    /// oblique raster clips below the water regardless.
    pub fn cull_planes(&self) -> [Vec4; 6] {
        let mut planes = view_proj_planes(self.cull_vp);
        planes[NEAR_PLANE] = self.clip_plane;
        planes
    }

    /// This mirror with its cull view-projection cropped to `plane`
    /// (`rendering_pipeline.md` §7.3a Visibility): the on-screen part of each
    /// face that passes the main frustum (`main_vp`), padded by
    /// [`MIRROR_CROP_PAD_RADIANS`]. Only faces in view count, so a coplanar
    /// pool in another room never widens it. The plane is fixed under the
    /// reflection, so its points project through `cull_vp` where the main
    /// view shows them. `raster_vp` and `clip_plane` stay as they are: the
    /// crop culls, it never moves a pixel. `None` when no face keeps any part
    /// on screen; then no mirror sample lands anywhere.
    pub fn cropped_to_plane(
        &self,
        plane: &PlanarCandidate,
        main_vp: Mat4,
        scratch: &mut MirrorCropScratch,
    ) -> Option<Self> {
        let main = view_proj_planes(main_vp);
        let faces = plane
            .faces
            .iter()
            .filter(|face| aabb_in_planes(face.bounds_min, face.bounds_max, &main))
            .map(|face| face.vertices.as_slice());
        let cull_vp = crop_mirror_view_proj(
            self.cull_vp,
            self.eye,
            faces,
            MIRROR_CROP_PAD_RADIANS,
            scratch,
        )?;
        Some(Self { cull_vp, ..*self })
    }
}
