// Ghost instance look: what one ghost draw presents with, its packing into the skinned,
// kinematic-mover and billboard ghost records, and CPU twins of the ghost shade and screen-door.
// See: context/lib/rendering_pipeline.md §7.9

/// One ghost instance's presented look, after the per-ghost flash clamp.
/// Every field lies in `[0, 1]`; packing clamps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GhostInstanceLook {
    /// Presented opacity: blend alpha, or the screen-door keep share.
    pub opacity: f32,
    /// 1 keeps the base color, 0 is its grey (Rec. 709 luma).
    pub saturation: f32,
    /// Multiplier on the (desaturated) base color.
    pub tint: [f32; 3],
}

/// Rec. 709 luma weights: the ghost shader's grey, and the limiter's luminance.
pub const GHOST_LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// WGSL `pack*unorm` quantization: clamp to `[0, 1]`, scale, round half away
/// from zero. A NaN packs as 0.
fn unorm(value: f32, max: f32) -> u32 {
    let clamped = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    (clamped * max).round() as u32
}

impl GhostInstanceLook {
    /// Pack into the skinned `Instance.base_and_pad.zw` lanes, which the opaque
    /// draw leaves zero. The instance record keeps its 80-byte stride.
    ///
    /// - z: `pack2x16unorm(opacity, saturation)` — opacity in bits 0..16,
    ///   saturation in bits 16..32; the shader reads `unpack2x16unorm`.
    /// - w: `pack4x8unorm(tint.r, tint.g, tint.b, 0)` — red in the low byte;
    ///   the shader reads `unpack4x8unorm`. 8 bits a channel is finer than any
    ///   tint step a player can see on an unlit ghost.
    pub fn pack(self) -> [u32; 2] {
        let z = unorm(self.opacity, 65535.0) | (unorm(self.saturation, 65535.0) << 16);
        let w = unorm(self.tint[0], 255.0)
            | (unorm(self.tint[1], 255.0) << 8)
            | (unorm(self.tint[2], 255.0) << 16);
        [z, w]
    }

    /// The look a packed `[z, w]` pair decodes to, as the shader decodes it.
    pub fn unpack([z, w]: [u32; 2]) -> Self {
        let lane16 = |shift: u32| ((z >> shift) & 0xFFFF) as f32 / 65535.0;
        let lane8 = |shift: u32| ((w >> shift) & 0xFF) as f32 / 255.0;
        Self {
            opacity: lane16(0),
            saturation: lane16(16),
            tint: [lane8(0), lane8(8), lane8(16)],
        }
    }
}

/// One kinematic-mover ghost to draw: a mover's geometry at a link's mapped
/// model with its presented look. A mover in two links is two of these, so the
/// list never deduplicates by mover id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KinematicGhostInstance {
    pub mover_id: u32,
    pub transform: glam::Mat4,
    pub look: GhostInstanceLook,
}

/// Byte size of one kinematic-mover ghost instance: the mover's model matrix
/// (64 bytes) then a `vec4<u32>` whose `xy` is [`GhostInstanceLook::pack`]
/// and `zw` zero. Matches `Instance` in `kinematic_ghost.wgsl`.
pub const KINEMATIC_GHOST_INSTANCE_SIZE: usize = 80;

/// One kinematic-mover ghost instance's bytes: `model` column-major, then the
/// packed look.
pub fn pack_kinematic_ghost_instance(
    model: glam::Mat4,
    look: GhostInstanceLook,
) -> [u8; KINEMATIC_GHOST_INSTANCE_SIZE] {
    let mut bytes = [0u8; KINEMATIC_GHOST_INSTANCE_SIZE];
    for (i, value) in model.to_cols_array().iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    let [z, w] = look.pack();
    bytes[64..68].copy_from_slice(&z.to_ne_bytes());
    bytes[68..72].copy_from_slice(&w.to_ne_bytes());
    bytes
}

/// Byte size of one billboard ghost instance, the sprite instance's stride
/// (`smoke::SPRITE_INSTANCE_SIZE`), so ghost regions lay out like sprite
/// regions: position and age, size, rotation, then the packed look in place
/// of the sprite's opacity and pad lanes. Matches `GhostSpriteInstance` in
/// `billboard_ghost.wgsl`.
pub const GHOST_SPRITE_INSTANCE_SIZE: usize = 32;

/// Append one billboard ghost instance. The look carries the presented
/// opacity, which already folds in the sprite's own opacity.
pub fn pack_ghost_sprite_instance(
    position: glam::Vec3,
    age: f32,
    size: f32,
    rotation: f32,
    look: GhostInstanceLook,
    out: &mut Vec<u8>,
) {
    for value in [position.x, position.y, position.z, age, size, rotation] {
        out.extend_from_slice(&value.to_ne_bytes());
    }
    for lane in look.pack() {
        out.extend_from_slice(&lane.to_ne_bytes());
    }
}

/// CPU twin of the ghost fragment shade (`ghost_look.wgsl` `ghost_shade`):
/// mix the base color toward its luma by `1 - saturation`, then multiply by
/// `tint`. Unlit, so a base color and tint in `[0, 1]` stay in `[0, 1]`.
pub fn ghost_shade(base: [f32; 3], look: &GhostInstanceLook) -> [f32; 3] {
    let luma = base[0] * GHOST_LUMA[0] + base[1] * GHOST_LUMA[1] + base[2] * GHOST_LUMA[2];
    let s = look.saturation;
    std::array::from_fn(|i| (luma + (base[i] - luma) * s) * look.tint[i])
}

/// CPU twin of the screen-door threshold (`ghost_look.wgsl`
/// `screen_door_threshold`): the 4×4 ordered (Bayer) index of the pixel at
/// `(x, y)`, centered in its sixteenth. A pixel draws when the presented
/// opacity exceeds it, so a 4×4 tile keeps `round_down(16 · opacity)` pixels.
pub fn screen_door_threshold(x: u32, y: u32) -> f32 {
    let a = (x ^ y) & 3;
    let b = y & 3;
    let index = ((a & 1) << 3) | ((b & 1) << 2) | ((a >> 1) << 1) | (b >> 1);
    (index as f32 + 0.5) / 16.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(opacity: f32, saturation: f32, tint: [f32; 3]) -> GhostInstanceLook {
        GhostInstanceLook {
            opacity,
            saturation,
            tint,
        }
    }

    #[test]
    fn ghost_look_packing_round_trips_within_lane_precision() {
        let cases = [
            look(0.0, 0.0, [0.0; 3]),
            look(1.0, 1.0, [1.0; 3]),
            look(0.45, 0.2, [0.6, 0.8, 1.0]),
            look(0.5, 0.0, [1.0, 0.4, 0.3]),
            look(0.123_456, 0.987_654, [0.01, 0.5, 0.99]),
        ];
        for case in cases {
            let decoded = GhostInstanceLook::unpack(case.pack());
            assert!(
                (decoded.opacity - case.opacity).abs() <= 0.5 / 65535.0 + 1e-6,
                "{case:?}"
            );
            assert!(
                (decoded.saturation - case.saturation).abs() <= 0.5 / 65535.0 + 1e-6,
                "{case:?}"
            );
            for channel in 0..3 {
                assert!(
                    (decoded.tint[channel] - case.tint[channel]).abs() <= 0.5 / 255.0 + 1e-6,
                    "{case:?}"
                );
            }
        }
    }

    #[test]
    fn ghost_look_packing_places_lanes_where_wgsl_unpack_reads_them() {
        let [z, w] = look(1.0, 0.0, [1.0, 0.0, 0.0]).pack();
        assert_eq!(z, 0x0000_FFFF, "opacity is unpack2x16unorm's x (low half)");
        assert_eq!(w, 0x0000_00FF, "red is unpack4x8unorm's x (low byte)");
        let [z, w] = look(0.0, 1.0, [0.0, 0.0, 1.0]).pack();
        assert_eq!(z, 0xFFFF_0000);
        assert_eq!(w, 0x00FF_0000, "the fourth byte stays zero");
    }

    #[test]
    fn ghost_look_packing_clamps_out_of_range_and_nan() {
        let decoded = GhostInstanceLook::unpack(look(2.0, f32::NAN, [-1.0, 1.5, 0.5]).pack());
        assert_eq!(decoded.opacity, 1.0);
        assert_eq!(decoded.saturation, 0.0);
        assert_eq!(decoded.tint[0], 0.0);
        assert_eq!(decoded.tint[1], 1.0);
    }

    #[test]
    fn kinematic_and_sprite_ghost_instances_carry_the_packed_look_after_their_pose() {
        let model = glam::Mat4::from_translation(glam::Vec3::new(1.0, 2.0, 3.0));
        let ghost = look(0.5, 0.25, [1.0, 0.5, 0.0]);
        let bytes = pack_kinematic_ghost_instance(model, ghost);
        let word = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(
            f32::from_bits(word(48)),
            1.0,
            "translation lives in column 3"
        );
        assert_eq!([word(64), word(68)], ghost.pack());
        assert_eq!([word(72), word(76)], [0, 0]);

        let mut sprite = Vec::new();
        pack_ghost_sprite_instance(glam::Vec3::X, 0.25, 2.0, 0.5, ghost, &mut sprite);
        assert_eq!(sprite.len(), GHOST_SPRITE_INSTANCE_SIZE);
        assert_eq!(
            GHOST_SPRITE_INSTANCE_SIZE,
            crate::smoke::SPRITE_INSTANCE_SIZE
        );
        let word = |at: usize| u32::from_ne_bytes(sprite[at..at + 4].try_into().unwrap());
        assert_eq!(f32::from_bits(word(12)), 0.25, "age rides position's w");
        assert_eq!(f32::from_bits(word(16)), 2.0);
        assert_eq!(f32::from_bits(word(20)), 0.5);
        assert_eq!([word(24), word(28)], ghost.pack());
    }

    #[test]
    fn ghost_shade_spans_full_color_to_grey_and_applies_tint() {
        let base = [0.8, 0.2, 0.1];
        let full = ghost_shade(base, &look(1.0, 1.0, [1.0; 3]));
        for channel in 0..3 {
            assert!((full[channel] - base[channel]).abs() < 1e-6);
        }
        let grey = ghost_shade(base, &look(1.0, 0.0, [1.0; 3]));
        let luma = 0.8 * GHOST_LUMA[0] + 0.2 * GHOST_LUMA[1] + 0.1 * GHOST_LUMA[2];
        for channel in grey {
            assert!((channel - luma).abs() < 1e-6);
        }
        let tinted = ghost_shade(base, &look(1.0, 1.0, [0.5, 1.0, 0.0]));
        assert!((tinted[0] - 0.4).abs() < 1e-6);
        assert!((tinted[1] - 0.2).abs() < 1e-6);
        assert_eq!(tinted[2], 0.0);
    }

    #[test]
    fn screen_door_tile_keeps_the_opacity_share_of_pixels() {
        let mut thresholds: Vec<f32> = (0..4)
            .flat_map(|y| (0..4).map(move |x| screen_door_threshold(x, y)))
            .collect();
        thresholds.sort_by(f32::total_cmp);
        thresholds.dedup();
        assert_eq!(
            thresholds.len(),
            16,
            "every Bayer index appears once a tile"
        );
        for kept in 0..=16u32 {
            let opacity = kept as f32 / 16.0;
            let coverage = (0..4)
                .flat_map(|y| (0..4).map(move |x| (x, y)))
                .filter(|&(x, y)| opacity > screen_door_threshold(x, y))
                .count() as u32;
            assert_eq!(coverage, kept, "opacity {opacity}");
        }
        // The pattern tiles: the pixel four over or four down shares its threshold.
        assert_eq!(screen_door_threshold(1, 2), screen_door_threshold(5, 6));
    }
}
