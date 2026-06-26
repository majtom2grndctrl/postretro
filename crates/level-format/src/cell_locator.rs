// CellLocator PRL section (ID 39): point-to-cell decision tree.
// See: context/lib/build_pipeline.md §PRL Compilation

use crate::FormatError;

pub const CELL_LOCATOR_VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 16;
pub const NODE_RECORD_SIZE: usize = 32;

pub const CELL_LOCATOR_KIND_CELL: u32 = 0;
pub const CELL_LOCATOR_KIND_NODE: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellLocatorChild {
    Cell(u32),
    Node(u32),
}

impl CellLocatorChild {
    fn kind_index(self) -> (u32, u32) {
        match self {
            Self::Cell(index) => (CELL_LOCATOR_KIND_CELL, index),
            Self::Node(index) => (CELL_LOCATOR_KIND_NODE, index),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CellLocatorNodeRecord {
    pub plane_normal: [f32; 3],
    pub plane_distance: f32,
    pub front: CellLocatorChild,
    pub back: CellLocatorChild,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CellLocatorSection {
    pub root: CellLocatorChild,
    pub nodes: Vec<CellLocatorNodeRecord>,
}

impl CellLocatorSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_SIZE + self.nodes.len() * NODE_RECORD_SIZE);
        let (root_kind, root_index) = self.root.kind_index();
        buf.extend_from_slice(&CELL_LOCATOR_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.nodes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&root_kind.to_le_bytes());
        buf.extend_from_slice(&root_index.to_le_bytes());

        for node in &self.nodes {
            for v in node.plane_normal {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            buf.extend_from_slice(&node.plane_distance.to_le_bytes());
            let (front_kind, front_index) = node.front.kind_index();
            let (back_kind, back_index) = node.back.kind_index();
            buf.extend_from_slice(&front_kind.to_le_bytes());
            buf.extend_from_slice(&front_index.to_le_bytes());
            buf.extend_from_slice(&back_kind.to_le_bytes());
            buf.extend_from_slice(&back_index.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8], cell_count: u32) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(locator_invalid(format!(
                "CellLocator section too short for header: need {HEADER_SIZE} bytes, got {}",
                data.len()
            )));
        }
        if cell_count == 0 {
            return Err(locator_invalid(
                "CellLocator section requires cell_count greater than zero",
            ));
        }

        let version = read_u32(data, 0);
        if version != CELL_LOCATOR_VERSION {
            return Err(locator_invalid(format!(
                "CellLocator section version {version}, expected {CELL_LOCATOR_VERSION}"
            )));
        }

        let node_count = read_u32(data, 4);
        let root_kind = read_u32(data, 8);
        let root_index = read_u32(data, 12);

        let nodes_len = checked_bytes(node_count, NODE_RECORD_SIZE, "node_count")?;
        let expected_len = HEADER_SIZE
            .checked_add(nodes_len)
            .ok_or_else(|| locator_invalid("CellLocator section count multiplication overflow"))?;
        if data.len() != expected_len {
            return Err(locator_invalid(format!(
                "CellLocator section length mismatch: expected {expected_len} bytes for node_count \
                 {node_count}, got {}",
                data.len()
            )));
        }

        let root = parse_child("root", root_kind, root_index, node_count, cell_count)?;
        let mut nodes = Vec::with_capacity(node_count as usize);
        let mut cursor = HEADER_SIZE;
        for node_index in 0..node_count {
            let plane_normal = [
                read_f32(data, cursor),
                read_f32(data, cursor + 4),
                read_f32(data, cursor + 8),
            ];
            let plane_distance = read_f32(data, cursor + 12);
            let front_kind = read_u32(data, cursor + 16);
            let front_index = read_u32(data, cursor + 20);
            let back_kind = read_u32(data, cursor + 24);
            let back_index = read_u32(data, cursor + 28);
            cursor += NODE_RECORD_SIZE;

            validate_plane(node_index, plane_normal, plane_distance)?;
            let front = parse_child("front", front_kind, front_index, node_count, cell_count)?;
            let back = parse_child("back", back_kind, back_index, node_count, cell_count)?;
            nodes.push(CellLocatorNodeRecord {
                plane_normal,
                plane_distance,
                front,
                back,
            });
        }

        validate_reachability(root, &nodes)?;
        Ok(Self { root, nodes })
    }
}

fn parse_child(
    label: &'static str,
    kind: u32,
    index: u32,
    node_count: u32,
    cell_count: u32,
) -> crate::Result<CellLocatorChild> {
    match kind {
        CELL_LOCATOR_KIND_CELL => {
            if index >= cell_count {
                Err(locator_invalid(format!(
                    "CellLocator {label} cell reference {index} out of range for cell_count {cell_count}"
                )))
            } else {
                Ok(CellLocatorChild::Cell(index))
            }
        }
        CELL_LOCATOR_KIND_NODE => {
            if index >= node_count {
                Err(locator_invalid(format!(
                    "CellLocator {label} node reference {index} out of range for node_count {node_count}"
                )))
            } else {
                Ok(CellLocatorChild::Node(index))
            }
        }
        _ => Err(locator_invalid(format!(
            "CellLocator {label} child kind {kind} is invalid; expected 0=cell or 1=node"
        ))),
    }
}

fn validate_plane(
    node_index: u32,
    plane_normal: [f32; 3],
    plane_distance: f32,
) -> crate::Result<()> {
    if !plane_distance.is_finite() {
        return Err(locator_invalid(format!(
            "CellLocator node {node_index} has non-finite plane_distance {plane_distance}"
        )));
    }

    let mut len_sq = 0.0f32;
    for (axis, v) in plane_normal.into_iter().enumerate() {
        if !v.is_finite() {
            return Err(locator_invalid(format!(
                "CellLocator node {node_index} has non-finite plane_normal[{axis}] {v}"
            )));
        }
        len_sq += v * v;
    }
    if len_sq == 0.0 {
        return Err(locator_invalid(format!(
            "CellLocator node {node_index} plane_normal must be nonzero"
        )));
    }

    Ok(())
}

fn validate_reachability(
    root: CellLocatorChild,
    nodes: &[CellLocatorNodeRecord],
) -> crate::Result<()> {
    let mut colors = vec![0u8; nodes.len()];
    if let CellLocatorChild::Node(root_idx) = root {
        visit_node(root_idx as usize, nodes, &mut colors)?;
    }

    if let Some(unreachable) = colors.iter().position(|&color| color == 0) {
        return Err(locator_invalid(format!(
            "CellLocator node {unreachable} is unreachable from root"
        )));
    }

    Ok(())
}

fn visit_node(
    node_index: usize,
    nodes: &[CellLocatorNodeRecord],
    colors: &mut [u8],
) -> crate::Result<()> {
    match colors[node_index] {
        1 => {
            return Err(locator_invalid(format!(
                "CellLocator contains a cycle involving node {node_index}"
            )));
        }
        2 => return Ok(()),
        _ => {}
    }

    colors[node_index] = 1;
    for child in [nodes[node_index].front, nodes[node_index].back] {
        if let CellLocatorChild::Node(child_index) = child {
            visit_node(child_index as usize, nodes, colors)?;
        }
    }
    colors[node_index] = 2;
    Ok(())
}

fn checked_bytes(count: u32, stride: usize, name: &'static str) -> crate::Result<usize> {
    (count as usize).checked_mul(stride).ok_or_else(|| {
        locator_invalid(format!(
            "CellLocator section count multiplication overflow for {name} {count} * stride {stride}"
        ))
    })
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn locator_invalid(msg: impl Into<String>) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        msg.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;

    fn valid_section() -> CellLocatorSection {
        CellLocatorSection {
            root: CellLocatorChild::Node(0),
            nodes: vec![
                CellLocatorNodeRecord {
                    plane_normal: [1.0, 0.0, 0.0],
                    plane_distance: 0.0,
                    front: CellLocatorChild::Node(1),
                    back: CellLocatorChild::Cell(0),
                },
                CellLocatorNodeRecord {
                    plane_normal: [0.0, 1.0, 0.0],
                    plane_distance: 2.0,
                    front: CellLocatorChild::Cell(2),
                    back: CellLocatorChild::Cell(1),
                },
            ],
        }
    }

    fn assert_invalid_data(err: FormatError) {
        match err {
            FormatError::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
            other => panic!("expected FormatError::Io(InvalidData), got {other:?}"),
        }
    }

    #[test]
    fn cell_locator_section_id_registered() {
        assert_eq!(SectionId::CellLocator as u32, 39);
        assert_eq!(SectionId::from_u32(39), Some(SectionId::CellLocator));
    }

    #[test]
    fn cell_locator_round_trip_valid_section() {
        let section = valid_section();
        let bytes = section.to_bytes();
        let restored = CellLocatorSection::from_bytes(&bytes, 3).unwrap();
        assert_eq!(section, restored);
        assert_eq!(restored.to_bytes(), bytes);
    }

    #[test]
    fn cell_locator_allows_single_cell_root_with_no_nodes() {
        let section = CellLocatorSection {
            root: CellLocatorChild::Cell(0),
            nodes: Vec::new(),
        };
        assert_eq!(
            CellLocatorSection::from_bytes(&section.to_bytes(), 1).unwrap(),
            section
        );
    }

    #[test]
    fn cell_locator_rejects_too_short_header() {
        let err = CellLocatorSection::from_bytes(&[0u8; HEADER_SIZE - 1], 1).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cell_locator_rejects_unsupported_version() {
        let mut bytes = valid_section().to_bytes();
        bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        let err = CellLocatorSection::from_bytes(&bytes, 3).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cell_locator_rejects_truncated_records() {
        let mut bytes = valid_section().to_bytes();
        bytes.truncate(bytes.len() - 1);
        let err = CellLocatorSection::from_bytes(&bytes, 3).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cell_locator_rejects_trailing_bytes() {
        let mut bytes = valid_section().to_bytes();
        bytes.push(0);
        let err = CellLocatorSection::from_bytes(&bytes, 3).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cell_locator_rejects_bad_kinds() {
        let mut bytes = valid_section().to_bytes();
        bytes[8..12].copy_from_slice(&99u32.to_le_bytes());
        let err = CellLocatorSection::from_bytes(&bytes, 3).unwrap_err();
        assert_invalid_data(err);

        let mut bytes = valid_section().to_bytes();
        let first_front_kind = HEADER_SIZE + 16;
        bytes[first_front_kind..first_front_kind + 4].copy_from_slice(&99u32.to_le_bytes());
        let err = CellLocatorSection::from_bytes(&bytes, 3).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cell_locator_rejects_bad_node_refs() {
        let mut section = valid_section();
        section.nodes[0].front = CellLocatorChild::Node(9);
        let err = CellLocatorSection::from_bytes(&section.to_bytes(), 3).unwrap_err();
        assert_invalid_data(err);

        let section = CellLocatorSection {
            root: CellLocatorChild::Node(0),
            nodes: Vec::new(),
        };
        let err = CellLocatorSection::from_bytes(&section.to_bytes(), 1).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cell_locator_rejects_bad_cell_refs() {
        let mut section = valid_section();
        section.nodes[1].front = CellLocatorChild::Cell(9);
        let err = CellLocatorSection::from_bytes(&section.to_bytes(), 3).unwrap_err();
        assert_invalid_data(err);

        let section = CellLocatorSection {
            root: CellLocatorChild::Cell(1),
            nodes: Vec::new(),
        };
        let err = CellLocatorSection::from_bytes(&section.to_bytes(), 1).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cell_locator_rejects_bad_planes_and_normals() {
        let mut section = valid_section();
        section.nodes[0].plane_normal = [0.0, 0.0, 0.0];
        let err = CellLocatorSection::from_bytes(&section.to_bytes(), 3).unwrap_err();
        assert_invalid_data(err);

        let mut section = valid_section();
        section.nodes[0].plane_normal[1] = f32::NAN;
        let err = CellLocatorSection::from_bytes(&section.to_bytes(), 3).unwrap_err();
        assert_invalid_data(err);

        let mut section = valid_section();
        section.nodes[0].plane_distance = f32::INFINITY;
        let err = CellLocatorSection::from_bytes(&section.to_bytes(), 3).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cell_locator_rejects_cycles() {
        let section = CellLocatorSection {
            root: CellLocatorChild::Node(0),
            nodes: vec![
                CellLocatorNodeRecord {
                    plane_normal: [1.0, 0.0, 0.0],
                    plane_distance: 0.0,
                    front: CellLocatorChild::Node(1),
                    back: CellLocatorChild::Cell(0),
                },
                CellLocatorNodeRecord {
                    plane_normal: [0.0, 1.0, 0.0],
                    plane_distance: 0.0,
                    front: CellLocatorChild::Node(0),
                    back: CellLocatorChild::Cell(1),
                },
            ],
        };
        let err = CellLocatorSection::from_bytes(&section.to_bytes(), 2).unwrap_err();
        assert_invalid_data(err);
    }

    #[test]
    fn cell_locator_rejects_unreachable_nodes() {
        let section = CellLocatorSection {
            root: CellLocatorChild::Node(0),
            nodes: vec![
                CellLocatorNodeRecord {
                    plane_normal: [1.0, 0.0, 0.0],
                    plane_distance: 0.0,
                    front: CellLocatorChild::Cell(0),
                    back: CellLocatorChild::Cell(1),
                },
                CellLocatorNodeRecord {
                    plane_normal: [0.0, 1.0, 0.0],
                    plane_distance: 0.0,
                    front: CellLocatorChild::Cell(0),
                    back: CellLocatorChild::Cell(1),
                },
            ],
        };
        let err = CellLocatorSection::from_bytes(&section.to_bytes(), 2).unwrap_err();
        assert_invalid_data(err);
    }
}
