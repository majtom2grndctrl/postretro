// Shared authored-light distance falloff (binding-agnostic).
//
// Linear keeps its established smooth fade to zero. Inverse-distance models
// match the compiler bake: pure reciprocal attenuation through the effective
// range, then a hard cutoff. Keep this helper shared by shading and SDF
// selection so limited visibility slices rank the same terms forward shades.

fn light_eval_falloff(distance: f32, range: f32, model: u32) -> f32 {
    switch model {
        case 0u: {
            let r = max(range, 0.001);
            return max(1.0 - distance / r, 0.0);
        }
        case 1u: {
            let r = max(range, 0.0001);
            if distance > r {
                return 0.0;
            }
            return 1.0 / max(distance, 0.0001);
        }
        case 2u: {
            let r = max(range, 0.0001);
            if distance > r {
                return 0.0;
            }
            let d2 = max(distance * distance, 0.0001);
            return 1.0 / d2;
        }
        default: {
            return 0.0;
        }
    }
}
