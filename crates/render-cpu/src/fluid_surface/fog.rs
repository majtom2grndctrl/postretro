// Fluid depth fog: the pure, GPU-free CPU reference for `fluid_surface.wgsl`'s fog.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{Mat4, Vec3, Vec4};

// The shader mirrors each constant and function below;
// `render/fluid_surface_tests.rs` pins them against it.

/// Extinction per unit of `1 / fogDistance`: transmittance at a path of one
/// `fogDistance` is `exp(-3)` ≈ 0.05, so `fogDistance` is the path that hides
/// 95%. Packed CPU-side as `3 / fogDistance`; the shader never reads it.
pub const FLUID_FOG_EXTINCTION_PER_DISTANCE: f32 = 3.0;
/// Height, metres, above a face's plane along its outward normal over which
/// its fog ramps from none to full. Equal to the eye tint's hysteresis band
/// (`EYE_FLUID_MARGIN` in postretro's `fluid_presentation`), so an eye bobbing
/// at a plane never flips between murk and clear before the tint takes over.
pub const FLUID_FOG_BAND: f32 = 0.08;
/// Longest fog path, in units of the face's camera distance: the clamp on
/// the ray parameter and the answer when the ray never meets the depth.
pub const FLUID_FOG_PATH_MAX: f32 = 1.0e4;
/// Below this the view ray runs parallel to the depth's iso-surface in clip
/// space and the solve has no answer.
pub const FLUID_FOG_DENOM_EPSILON: f32 = 1.0e-8;
/// Floor on the composite's output alpha before it divides by it.
pub const FLUID_FOG_ALPHA_EPSILON: f32 = 1.0e-4;

/// Metres the view ray travels past the surface fragment `frag` to the opaque
/// scene behind it, given that pixel's scene depth (NDC, 0 near, 1 cleared
/// far). The ray `camera + s · (frag − camera)` is linear in clip space, so
/// solving its NDC depth for `depth` gives `s` with no inverse matrix; `s = 1`
/// is the face itself. Not clamped at the volume's far side, which the pass
/// cannot see.
pub fn fog_path(view_proj: Mat4, camera: Vec3, frag: Vec3, depth: f32) -> f32 {
    let c0 = view_proj * camera.extend(1.0);
    let c1 = view_proj * frag.extend(1.0);
    let denom = (c1.z - c0.z) - depth * (c1.w - c0.w);
    let s = if denom.abs() > FLUID_FOG_DENOM_EPSILON {
        (depth * c0.w - c0.z) / denom
    } else {
        FLUID_FOG_PATH_MAX
    };
    (s - 1.0).clamp(0.0, FLUID_FOG_PATH_MAX) * (frag - camera).length()
}

/// Fraction of the scene behind `path` metres of fluid that shows through.
pub fn fog_transmittance(extinction: f32, path: f32) -> f32 {
    (-extinction * path).exp()
}

/// The face's own fog ramp for an eye at `camera`: 0 at and below the face's
/// plane through `frag`, 1 at one [`FLUID_FOG_BAND`] above it along the
/// outward normal, linear between. Per face, so a near pool's ramp never
/// dims a far one's.
pub fn fog_proximity(camera: Vec3, frag: Vec3, outward: Vec3) -> f32 {
    ((camera - frag).dot(outward) / FLUID_FOG_BAND).clamp(0.0, 1.0)
}

/// Whether the camera sees the face from outside its volume. Reads the
/// interpolated outward normal, never the viewer-flipped shading normal,
/// which always faces the camera.
pub fn fog_seen_from_outside(outward: Vec3, view: Vec3) -> bool {
    outward.dot(view) > 0.0
}

/// The fog layer's opacity: the frame's eye weight × the face's proximity,
/// times the fraction of the scene the fog hides.
pub fn fog_layer_opacity(weight: f32, proximity: f32, transmittance: f32) -> f32 {
    weight * proximity * (1.0 - transmittance)
}

/// Surface over fog over scene as one straight-alpha output: blended by
/// `src · a + dst · (1 − a)`, it equals the surface `(rgb, alpha)` composited
/// over a fog layer `(fog_rgb, fog_opacity)` composited over the scene.
pub fn fog_composite(rgb: Vec3, alpha: f32, fog_rgb: Vec3, fog_opacity: f32) -> Vec4 {
    let total = 1.0 - (1.0 - alpha) * (1.0 - fog_opacity);
    let out = (rgb * alpha + fog_rgb * ((1.0 - alpha) * fog_opacity))
        / total.max(FLUID_FOG_ALPHA_EPSILON);
    out.extend(total)
}

/// Everything one fluid fragment's fog reads besides its surface colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidFogFragment {
    pub view_proj: Mat4,
    pub camera: Vec3,
    pub world_position: Vec3,
    /// The interpolated outward face normal.
    pub outward: Vec3,
    /// The scene depth at the fragment's pixel.
    pub scene_depth: f32,
    /// The row's `3 / fogDistance`; 0 is no fog.
    pub extinction: f32,
    /// The row's packed fog albedo.
    pub fog_albedo: Vec3,
    /// The surface's own lighting, which lights the murk too.
    pub lighting: Vec3,
    /// The frame's eye fog weight.
    pub weight: f32,
}

/// The fragment's output, as `fs_main`'s tail computes it: the surface
/// `(rgb, alpha)` unchanged when the fluid has no fog or the face is seen from
/// inside, else composited over its fog.
pub fn fog_fragment(rgb: Vec3, alpha: f32, fragment: &FluidFogFragment) -> Vec4 {
    let view = (fragment.camera - fragment.world_position).normalize_or_zero();
    if !(fragment.extinction > 0.0 && fog_seen_from_outside(fragment.outward, view)) {
        return rgb.extend(alpha);
    }
    let path = fog_path(
        fragment.view_proj,
        fragment.camera,
        fragment.world_position,
        fragment.scene_depth,
    );
    let transmittance = fog_transmittance(fragment.extinction, path);
    let proximity = fog_proximity(fragment.camera, fragment.world_position, fragment.outward);
    let fog_opacity = fog_layer_opacity(fragment.weight, proximity, transmittance);
    fog_composite(
        rgb,
        alpha,
        fragment.fog_albedo * fragment.lighting,
        fog_opacity,
    )
}

#[cfg(test)]
#[path = "fog_tests.rs"]
mod tests;
