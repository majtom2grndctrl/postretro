// Reflection-probe capture brushes: the world brushes a capture may exclude, and each brush's post-cut faces.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use crate::map_data::BrushVolume;

/// The world brushes a capture may exclude, and the brush each face came from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptureBrushes<'a> {
    /// `MapData::brush_volumes`: convex hulls before CSG.
    pub volumes: &'a [BrushVolume],
    /// Post-cut face → brush index into `volumes`, index-aligned with the
    /// geometry's faces; `None` for a face no brush owns.
    pub face_brushes: &'a [Option<u32>],
    /// `face_brushes` inverted.
    pub faces: &'a BrushFaces,
}

/// Each brush's post-cut faces, ascending: the face → brush table inverted
/// once per stage, so a probe's mask and contact faces cost what its brushes
/// own rather than a pass over every face in the world.
#[derive(Debug, Clone, Default)]
pub(crate) struct BrushFaces {
    /// Brush `b`'s faces are `faces[offsets[b]..offsets[b + 1]]`.
    offsets: Vec<u32>,
    faces: Vec<u32>,
}

impl BrushFaces {
    /// `face_brushes` inverted over `brush_count` brushes; a face naming a
    /// brush past them, or none, is in no brush's list.
    pub(crate) fn new(face_brushes: &[Option<u32>], brush_count: usize) -> Self {
        let owner = |brush: &Option<u32>| brush.filter(|&b| (b as usize) < brush_count);
        let mut offsets = vec![0u32; brush_count + 1];
        for brush in face_brushes.iter().filter_map(owner) {
            offsets[brush as usize + 1] += 1;
        }
        for b in 0..brush_count {
            offsets[b + 1] += offsets[b];
        }
        let mut next = offsets.clone();
        let mut faces = vec![0u32; offsets[brush_count] as usize];
        // Faces visited in ascending order land ascending in each list.
        for (face, brush) in face_brushes.iter().enumerate() {
            if let Some(brush) = owner(brush) {
                faces[next[brush as usize] as usize] = face as u32;
                next[brush as usize] += 1;
            }
        }
        Self { offsets, faces }
    }

    /// Brush `brush`'s faces, ascending; empty for a brush past the table.
    pub(crate) fn of(&self, brush: u32) -> &[u32] {
        let b = brush as usize;
        match (self.offsets.get(b), self.offsets.get(b + 1)) {
            (Some(&start), Some(&end)) => &self.faces[start as usize..end as usize],
            _ => &[],
        }
    }
}
