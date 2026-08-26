use glam::DVec3;

#[derive(Clone, Copy)]
pub(super) struct PortalMetrics {
    pub(super) centroid: Option<DVec3>,
    pub(super) minimum_width: f64,
}

pub(super) fn portal_metrics(vertices: &[DVec3]) -> PortalMetrics {
    if vertices.len() < 3 || vertices.iter().any(|vertex| !vertex.is_finite()) {
        return PortalMetrics {
            centroid: None,
            minimum_width: 0.0,
        };
    }
    let first = vertices[0];
    let mut normal = DVec3::ZERO;
    let mut weighted_centroid = DVec3::ZERO;
    let mut total_area = 0.0;
    for index in 1..vertices.len() - 1 {
        let second = vertices[index];
        let third = vertices[index + 1];
        let cross = (second - first).cross(third - first);
        let area = cross.length() * 0.5;
        normal += cross;
        weighted_centroid += (first + second + third) * (area / 3.0);
        total_area += area;
    }
    if !total_area.is_finite() || total_area <= 0.0 || normal.length_squared() <= 0.0 {
        return PortalMetrics {
            centroid: None,
            minimum_width: 0.0,
        };
    }
    let centroid = weighted_centroid / total_area;
    let normal = normal.normalize();
    let mut minimum_width = f64::INFINITY;
    for index in 0..vertices.len() {
        let edge = vertices[(index + 1) % vertices.len()] - vertices[index];
        if edge.length_squared() <= 0.0 {
            continue;
        }
        let in_plane_normal = normal.cross(edge).normalize();
        let (minimum, maximum) = vertices.iter().fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(minimum, maximum), vertex| {
                let projection = vertex.dot(in_plane_normal);
                (minimum.min(projection), maximum.max(projection))
            },
        );
        minimum_width = minimum_width.min(maximum - minimum);
    }
    PortalMetrics {
        centroid: centroid.is_finite().then_some(centroid),
        minimum_width: if minimum_width.is_finite() {
            minimum_width
        } else {
            0.0
        },
    }
}

/// Returns the wire value and whether the upper fixed-point range clamped.
pub(super) fn fixed_point_value(value: f64, scale: u32) -> (u32, bool) {
    let scaled = value * f64::from(scale);
    if !scaled.is_finite() || scaled > f64::from(u32::MAX) {
        return (u32::MAX, true);
    }
    if scaled <= 0.0 {
        return (0, false);
    }
    (scaled.round() as u32, false)
}
