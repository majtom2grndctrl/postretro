// Per-light influence GPU packing.
// See: context/lib/rendering_pipeline.md §4

use postretro_render_data::influence::LightInfluence;

/// Pack influence records into a contiguous `[f32; 4]` array suitable for
/// GPU upload as `array<vec4<f32>>`.
pub fn pack_influence(records: &[LightInfluence]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(records.len() * 16);
    pack_influence_into(&mut bytes, records);
    bytes
}

/// Append influence records to an existing byte buffer.
pub fn pack_influence_into(bytes: &mut Vec<u8>, records: &[LightInfluence]) {
    bytes.reserve(records.len() * 16);
    for r in records {
        bytes.extend_from_slice(&r.center.x.to_ne_bytes());
        bytes.extend_from_slice(&r.center.y.to_ne_bytes());
        bytes.extend_from_slice(&r.center.z.to_ne_bytes());
        bytes.extend_from_slice(&r.radius.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn pack_influence_produces_correct_bytes() {
        let records = vec![LightInfluence {
            center: Vec3::new(1.0, 2.0, 3.0),
            radius: 10.0,
        }];
        let bytes = pack_influence(&records);
        assert_eq!(bytes.len(), 16);
        let x = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let y = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let z = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let r = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(x, 1.0);
        assert_eq!(y, 2.0);
        assert_eq!(z, 3.0);
        assert_eq!(r, 10.0);
    }

    #[test]
    fn pack_influence_into_reuses_existing_buffer() {
        let records = vec![LightInfluence {
            center: Vec3::new(4.0, 5.0, 6.0),
            radius: 7.0,
        }];
        let mut bytes = Vec::with_capacity(64);
        let original_capacity = bytes.capacity();

        pack_influence_into(&mut bytes, &records);

        assert_eq!(bytes.len(), 16);
        assert_eq!(bytes.capacity(), original_capacity);
        assert_eq!(f32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 4.0);
    }
}
