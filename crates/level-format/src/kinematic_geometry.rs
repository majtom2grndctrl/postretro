// KinematicGeometry PRL section (ID 43): origin-relative brush mover geometry
// plus waypoint path records.
// See: context/lib/build_pipeline.md §PRL KinematicGeometrySection.

use crate::FormatError;
use crate::geometry::{FaceMeta, Vertex};

pub const KINEMATIC_GEOMETRY_VERSION: u16 = 1;
const MOVE_MODE_ONCE: u8 = 0;
const MOVE_MODE_PING_PONG: u8 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct KinematicGeometrySection {
    pub version: u16,
    pub movers: Vec<KinematicMoverRecord>,
    pub waypoints: Vec<KinematicWaypointRecord>,
}

impl Default for KinematicGeometrySection {
    fn default() -> Self {
        Self {
            version: KINEMATIC_GEOMETRY_VERSION,
            movers: Vec::new(),
            waypoints: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KinematicMoverRecord {
    pub mover_id: u32,
    pub name: String,
    pub tags: Vec<String>,
    pub origin: [f32; 3],
    pub path: String,
    pub speed: f32,
    pub wait_ms: f32,
    pub move_mode: u8,
    pub start_on_spawn: bool,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<FaceMeta>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KinematicWaypointRecord {
    pub name: String,
    pub next: String,
    pub origin: [f32; 3],
}

impl KinematicGeometrySection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.version.to_le_bytes());
        write_count(&mut buf, self.movers.len());
        for mover in &self.movers {
            write_mover(&mut buf, mover);
        }
        write_count(&mut buf, self.waypoints.len());
        for waypoint in &self.waypoints {
            write_string(&mut buf, &waypoint.name);
            write_string(&mut buf, &waypoint.next);
            write_vec3(&mut buf, waypoint.origin);
        }
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut offset = 0usize;
        let version = read_u16(data, &mut offset, "version")?;
        if version != KINEMATIC_GEOMETRY_VERSION {
            return invalid_data(format!(
                "kinematic geometry: unsupported version {version} (expected {KINEMATIC_GEOMETRY_VERSION})"
            ));
        }

        let mover_count = read_count(data, &mut offset, "mover count")?;
        let mut movers = Vec::with_capacity(mover_count);
        for mover_idx in 0..mover_count {
            movers.push(read_mover(data, &mut offset, mover_idx)?);
        }

        let waypoint_count = read_count(data, &mut offset, "waypoint count")?;
        let mut waypoints = Vec::with_capacity(waypoint_count);
        for waypoint_idx in 0..waypoint_count {
            let name = read_string(data, &mut offset, &format!("waypoint {waypoint_idx} name"))?;
            let next = read_string(data, &mut offset, &format!("waypoint {waypoint_idx} next"))?;
            let origin = read_vec3(
                data,
                &mut offset,
                &format!("waypoint {waypoint_idx} origin"),
            )?;
            if !origin.iter().all(|component| component.is_finite()) {
                return invalid_data(format!(
                    "kinematic geometry: waypoint {waypoint_idx} origin is non-finite: {origin:?}"
                ));
            }
            waypoints.push(KinematicWaypointRecord { name, next, origin });
        }

        if offset != data.len() {
            return invalid_data(format!(
                "kinematic geometry: trailing bytes: expected {offset}, got {}",
                data.len()
            ));
        }

        Ok(Self {
            version,
            movers,
            waypoints,
        })
    }
}

fn write_mover(buf: &mut Vec<u8>, mover: &KinematicMoverRecord) {
    buf.extend_from_slice(&mover.mover_id.to_le_bytes());
    write_string(buf, &mover.name);
    write_count(buf, mover.tags.len());
    for tag in &mover.tags {
        write_string(buf, tag);
    }
    write_vec3(buf, mover.origin);
    write_string(buf, &mover.path);
    buf.extend_from_slice(&mover.speed.to_le_bytes());
    buf.extend_from_slice(&mover.wait_ms.to_le_bytes());
    buf.push(mover.move_mode);
    buf.push(if mover.start_on_spawn { 1 } else { 0 });

    write_count(buf, mover.vertices.len());
    for vertex in &mover.vertices {
        write_vertex(buf, vertex);
    }
    write_count(buf, mover.indices.len());
    for &index in &mover.indices {
        buf.extend_from_slice(&index.to_le_bytes());
    }
    write_count(buf, mover.face_meta.len());
    for face in &mover.face_meta {
        buf.extend_from_slice(&face.leaf_index.to_le_bytes());
        buf.extend_from_slice(&face.texture_index.to_le_bytes());
    }
}

fn read_mover(
    data: &[u8],
    offset: &mut usize,
    mover_idx: usize,
) -> crate::Result<KinematicMoverRecord> {
    let mover_id = read_u32(data, offset, &format!("mover {mover_idx} id"))?;
    let name = read_string(data, offset, &format!("mover {mover_idx} name"))?;
    let tag_count = read_count(data, offset, &format!("mover {mover_idx} tag count"))?;
    let mut tags = Vec::with_capacity(tag_count);
    for tag_idx in 0..tag_count {
        tags.push(read_string(
            data,
            offset,
            &format!("mover {mover_idx} tag {tag_idx}"),
        )?);
    }
    let origin = read_vec3(data, offset, &format!("mover {mover_idx} origin"))?;
    let path = read_string(data, offset, &format!("mover {mover_idx} path"))?;
    let speed = read_f32(data, offset, &format!("mover {mover_idx} speed"))?;
    let wait_ms = read_f32(data, offset, &format!("mover {mover_idx} wait_ms"))?;
    let move_mode = read_u8(data, offset, &format!("mover {mover_idx} move_mode"))?;
    if move_mode != MOVE_MODE_ONCE && move_mode != MOVE_MODE_PING_PONG {
        return invalid_data(format!(
            "kinematic geometry: mover {mover_idx} has invalid move_mode {move_mode}"
        ));
    }
    let start_on_spawn_raw = read_u8(data, offset, &format!("mover {mover_idx} start_on_spawn"))?;
    let start_on_spawn = match start_on_spawn_raw {
        0 => false,
        1 => true,
        value => {
            return invalid_data(format!(
                "kinematic geometry: mover {mover_idx} has invalid start_on_spawn byte {value}"
            ));
        }
    };

    let vertex_count = read_count(data, offset, &format!("mover {mover_idx} vertex count"))?;
    let mut vertices = Vec::with_capacity(vertex_count);
    for vertex_idx in 0..vertex_count {
        vertices.push(read_vertex(data, offset, mover_idx, vertex_idx)?);
    }

    let index_count = read_count(data, offset, &format!("mover {mover_idx} index count"))?;
    let mut indices = Vec::with_capacity(index_count);
    for index_idx in 0..index_count {
        indices.push(read_u32(
            data,
            offset,
            &format!("mover {mover_idx} index {index_idx}"),
        )?);
    }

    let face_count = read_count(data, offset, &format!("mover {mover_idx} face_meta count"))?;
    let mut face_meta = Vec::with_capacity(face_count);
    for face_idx in 0..face_count {
        let leaf_index = read_u32(
            data,
            offset,
            &format!("mover {mover_idx} face_meta {face_idx} leaf_index"),
        )?;
        let texture_index = read_u32(
            data,
            offset,
            &format!("mover {mover_idx} face_meta {face_idx} texture_index"),
        )?;
        face_meta.push(FaceMeta {
            leaf_index,
            texture_index,
        });
    }

    validate_mover_geometry(mover_idx, origin, speed, wait_ms, &vertices, &indices)?;

    Ok(KinematicMoverRecord {
        mover_id,
        name,
        tags,
        origin,
        path,
        speed,
        wait_ms,
        move_mode,
        start_on_spawn,
        vertices,
        indices,
        face_meta,
    })
}

fn write_vertex(buf: &mut Vec<u8>, vertex: &Vertex) {
    for component in vertex.position {
        buf.extend_from_slice(&component.to_le_bytes());
    }
    for component in vertex.uv {
        buf.extend_from_slice(&component.to_le_bytes());
    }
    for component in vertex.normal_oct {
        buf.extend_from_slice(&component.to_le_bytes());
    }
    for component in vertex.tangent_packed {
        buf.extend_from_slice(&component.to_le_bytes());
    }
    for component in vertex.lightmap_uv {
        buf.extend_from_slice(&component.to_le_bytes());
    }
    buf.extend_from_slice(&vertex.lightmap_layer.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
}

fn read_vertex(
    data: &[u8],
    offset: &mut usize,
    mover_idx: usize,
    vertex_idx: usize,
) -> crate::Result<Vertex> {
    let position = read_vec3(
        data,
        offset,
        &format!("mover {mover_idx} vertex {vertex_idx} position"),
    )?;
    let uv = [
        read_f32(
            data,
            offset,
            &format!("mover {mover_idx} vertex {vertex_idx} uv.u"),
        )?,
        read_f32(
            data,
            offset,
            &format!("mover {mover_idx} vertex {vertex_idx} uv.v"),
        )?,
    ];
    let normal_oct = [
        read_u16(
            data,
            offset,
            &format!("mover {mover_idx} vertex {vertex_idx} normal.u"),
        )?,
        read_u16(
            data,
            offset,
            &format!("mover {mover_idx} vertex {vertex_idx} normal.v"),
        )?,
    ];
    let tangent_packed = [
        read_u16(
            data,
            offset,
            &format!("mover {mover_idx} vertex {vertex_idx} tangent.u"),
        )?,
        read_u16(
            data,
            offset,
            &format!("mover {mover_idx} vertex {vertex_idx} tangent.v"),
        )?,
    ];
    let lightmap_uv = [
        read_u16(
            data,
            offset,
            &format!("mover {mover_idx} vertex {vertex_idx} lightmap.u"),
        )?,
        read_u16(
            data,
            offset,
            &format!("mover {mover_idx} vertex {vertex_idx} lightmap.v"),
        )?,
    ];
    let lightmap_layer = read_u16(
        data,
        offset,
        &format!("mover {mover_idx} vertex {vertex_idx} lightmap_layer"),
    )?;
    let _padding = read_u16(
        data,
        offset,
        &format!("mover {mover_idx} vertex {vertex_idx} padding"),
    )?;
    Ok(Vertex {
        position,
        uv,
        normal_oct,
        tangent_packed,
        lightmap_uv,
        lightmap_layer,
        _padding: 0,
    })
}

fn validate_mover_geometry(
    mover_idx: usize,
    origin: [f32; 3],
    speed: f32,
    wait_ms: f32,
    vertices: &[Vertex],
    indices: &[u32],
) -> crate::Result<()> {
    if !origin.iter().all(|component| component.is_finite()) {
        return invalid_data(format!(
            "kinematic geometry: mover {mover_idx} origin is non-finite: {origin:?}"
        ));
    }
    if !speed.is_finite() || speed <= 0.0 {
        return invalid_data(format!(
            "kinematic geometry: mover {mover_idx} speed must be finite and positive, got {speed}"
        ));
    }
    if !wait_ms.is_finite() || wait_ms < 0.0 {
        return invalid_data(format!(
            "kinematic geometry: mover {mover_idx} wait_ms must be finite and non-negative, got {wait_ms}"
        ));
    }
    if indices.len() % 3 != 0 {
        return invalid_data(format!(
            "kinematic geometry: mover {mover_idx} index count {} is not divisible by 3",
            indices.len()
        ));
    }
    for (vertex_idx, vertex) in vertices.iter().enumerate() {
        if !vertex
            .position
            .iter()
            .all(|component| component.is_finite())
        {
            return invalid_data(format!(
                "kinematic geometry: mover {mover_idx} vertex {vertex_idx} has non-finite position {:?}",
                vertex.position
            ));
        }
        if !vertex.uv.iter().all(|component| component.is_finite()) {
            return invalid_data(format!(
                "kinematic geometry: mover {mover_idx} vertex {vertex_idx} has non-finite UV {:?}",
                vertex.uv
            ));
        }
        if vertex.lightmap_uv != [0, 0] || vertex.lightmap_layer != 0 {
            return invalid_data(format!(
                "kinematic geometry: mover {mover_idx} vertex {vertex_idx} carries lightmap data"
            ));
        }
    }
    for (index_idx, &vertex_index) in indices.iter().enumerate() {
        if vertex_index as usize >= vertices.len() {
            return invalid_data(format!(
                "kinematic geometry: mover {mover_idx} index {index_idx} references vertex {vertex_index} out of range for {} vertices",
                vertices.len()
            ));
        }
    }
    Ok(())
}

fn write_count(buf: &mut Vec<u8>, count: usize) {
    buf.extend_from_slice(&(count as u32).to_le_bytes());
}

fn write_vec3(buf: &mut Vec<u8>, values: [f32; 3]) {
    for component in values {
        buf.extend_from_slice(&component.to_le_bytes());
    }
}

fn write_string(buf: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
}

fn read_count(data: &[u8], offset: &mut usize, ctx: &str) -> crate::Result<usize> {
    Ok(read_u32(data, offset, ctx)? as usize)
}

fn read_u8(data: &[u8], offset: &mut usize, ctx: &str) -> crate::Result<u8> {
    if *offset + 1 > data.len() {
        return unexpected_eof(format!("kinematic geometry: truncated {ctx}"));
    }
    let value = data[*offset];
    *offset += 1;
    Ok(value)
}

fn read_u16(data: &[u8], offset: &mut usize, ctx: &str) -> crate::Result<u16> {
    if *offset + 2 > data.len() {
        return unexpected_eof(format!("kinematic geometry: truncated {ctx}"));
    }
    let value = u16::from_le_bytes([data[*offset], data[*offset + 1]]);
    *offset += 2;
    Ok(value)
}

fn read_u32(data: &[u8], offset: &mut usize, ctx: &str) -> crate::Result<u32> {
    if *offset + 4 > data.len() {
        return unexpected_eof(format!("kinematic geometry: truncated {ctx}"));
    }
    let value = u32::from_le_bytes([
        data[*offset],
        data[*offset + 1],
        data[*offset + 2],
        data[*offset + 3],
    ]);
    *offset += 4;
    Ok(value)
}

fn read_f32(data: &[u8], offset: &mut usize, ctx: &str) -> crate::Result<f32> {
    if *offset + 4 > data.len() {
        return unexpected_eof(format!("kinematic geometry: truncated {ctx}"));
    }
    let value = f32::from_le_bytes([
        data[*offset],
        data[*offset + 1],
        data[*offset + 2],
        data[*offset + 3],
    ]);
    *offset += 4;
    Ok(value)
}

fn read_vec3(data: &[u8], offset: &mut usize, ctx: &str) -> crate::Result<[f32; 3]> {
    Ok([
        read_f32(data, offset, ctx)?,
        read_f32(data, offset, ctx)?,
        read_f32(data, offset, ctx)?,
    ])
}

fn read_string(data: &[u8], offset: &mut usize, ctx: &str) -> crate::Result<String> {
    let byte_len = read_count(data, offset, &format!("{ctx} length"))?;
    if *offset + byte_len > data.len() {
        return unexpected_eof(format!("kinematic geometry: truncated {ctx} payload"));
    }
    let string = std::str::from_utf8(&data[*offset..*offset + byte_len]).map_err(|_| {
        FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("kinematic geometry: invalid UTF-8 in {ctx}"),
        ))
    })?;
    *offset += byte_len;
    Ok(string.to_string())
}

fn unexpected_eof<T>(message: String) -> crate::Result<T> {
    Err(FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        message,
    )))
}

fn invalid_data<T>(message: String) -> crate::Result<T> {
    Err(FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;

    fn sample_vertex(position: [f32; 3]) -> Vertex {
        Vertex::new(
            position,
            [0.25, 0.5],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    }

    fn sample_section() -> KinematicGeometrySection {
        KinematicGeometrySection {
            version: KINEMATIC_GEOMETRY_VERSION,
            movers: vec![KinematicMoverRecord {
                mover_id: 7,
                name: "lift_a".to_string(),
                tags: vec!["platform".to_string(), "arena".to_string()],
                origin: [1.0, 2.0, 3.0],
                path: "wp_a".to_string(),
                speed: 2.5,
                wait_ms: 125.0,
                move_mode: MOVE_MODE_PING_PONG,
                start_on_spawn: true,
                vertices: vec![
                    sample_vertex([0.0, 0.0, 0.0]),
                    sample_vertex([1.0, 0.0, 0.0]),
                    sample_vertex([0.0, 1.0, 0.0]),
                ],
                indices: vec![0, 1, 2],
                face_meta: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 3,
                }],
            }],
            waypoints: vec![
                KinematicWaypointRecord {
                    name: "wp_a".to_string(),
                    next: "wp_b".to_string(),
                    origin: [1.0, 2.0, 3.0],
                },
                KinematicWaypointRecord {
                    name: "wp_b".to_string(),
                    next: String::new(),
                    origin: [1.0, 3.0, 3.0],
                },
            ],
        }
    }

    #[test]
    fn round_trip_preserves_records() {
        let section = sample_section();
        let restored = KinematicGeometrySection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn empty_section_round_trips_with_version_and_zero_counts() {
        let section = KinematicGeometrySection::default();
        let bytes = section.to_bytes();
        assert_eq!(bytes, vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            KinematicGeometrySection::from_bytes(&bytes).unwrap(),
            section
        );
    }

    #[test]
    fn rejects_invalid_utf8() {
        let mut bytes = KinematicGeometrySection {
            version: KINEMATIC_GEOMETRY_VERSION,
            movers: Vec::new(),
            waypoints: vec![KinematicWaypointRecord {
                name: "a".to_string(),
                next: String::new(),
                origin: [0.0, 0.0, 0.0],
            }],
        }
        .to_bytes();
        let name_payload = 2 + 4 + 4 + 4;
        bytes[name_payload] = 0xff;
        assert!(KinematicGeometrySection::from_bytes(&bytes).is_err());
    }

    #[test]
    fn rejects_invalid_bool() {
        let mut bytes = sample_section().to_bytes();
        let index = find_first_move_mode_and_bool_offset(&bytes).1;
        bytes[index] = 2;
        assert!(KinematicGeometrySection::from_bytes(&bytes).is_err());
    }

    #[test]
    fn rejects_invalid_move_mode() {
        let mut bytes = sample_section().to_bytes();
        let index = find_first_move_mode_and_bool_offset(&bytes).0;
        bytes[index] = 9;
        assert!(KinematicGeometrySection::from_bytes(&bytes).is_err());
    }

    #[test]
    fn rejects_nonzero_lightmap_data() {
        let mut section = sample_section();
        section.movers[0].vertices[0].lightmap_uv = [1, 0];
        assert!(KinematicGeometrySection::from_bytes(&section.to_bytes()).is_err());
    }

    #[test]
    fn section_id_is_43_and_from_u32_matches() {
        assert_eq!(SectionId::KinematicGeometry as u32, 43);
        assert_eq!(SectionId::from_u32(43), Some(SectionId::KinematicGeometry));
    }

    fn find_first_move_mode_and_bool_offset(bytes: &[u8]) -> (usize, usize) {
        let mut offset = 0usize;
        offset += 2; // version
        offset += 4; // mover count
        offset += 4; // mover_id
        skip_string(bytes, &mut offset); // name
        let tag_count = read_u32_for_test(bytes, &mut offset);
        for _ in 0..tag_count {
            skip_string(bytes, &mut offset);
        }
        offset += 12; // origin
        skip_string(bytes, &mut offset); // path
        offset += 4; // speed
        offset += 4; // wait
        (offset, offset + 1)
    }

    fn skip_string(bytes: &[u8], offset: &mut usize) {
        let len = read_u32_for_test(bytes, offset) as usize;
        *offset += len;
    }

    fn read_u32_for_test(bytes: &[u8], offset: &mut usize) -> u32 {
        let value = u32::from_le_bytes([
            bytes[*offset],
            bytes[*offset + 1],
            bytes[*offset + 2],
            bytes[*offset + 3],
        ]);
        *offset += 4;
        value
    }
}
