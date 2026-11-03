// Test seam: extracted faces a test removes from the traced geometry right
// after extraction, before the coverage proof and the BVH read it, so the
// proof and the tracer see the same faces.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::cell::RefCell;

use crate::geometry::GeometryResult;

/// Whether to remove a face, by the extracted geometry and its face index.
type RemovePredicate = Box<dyn Fn(&GeometryResult, usize) -> bool>;

thread_local! {
    static REMOVE: RefCell<Option<RemovePredicate>> = const { RefCell::new(None) };
}

/// Run `work` with every face `remove` names taken out of the geometry the
/// pipeline extracts on this thread. The previous predicate is restored when
/// `work` returns or panics.
pub(crate) fn with_removed_faces<T>(
    remove: impl Fn(&GeometryResult, usize) -> bool + 'static,
    work: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<RemovePredicate>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            REMOVE.with(|cell| *cell.borrow_mut() = previous);
        }
    }

    let _restore = Restore(REMOVE.with(|cell| cell.replace(Some(Box::new(remove)))));
    work()
}

/// Collapse every triangle of each face the active predicate names onto its
/// first corner: the face keeps its record and index count, but no triangle
/// of it bounds anything, blocks a ray or plans a chart.
pub(super) fn remove_faces(geometry: &mut GeometryResult) {
    REMOVE.with(|cell| {
        let Some(remove) = cell.borrow().as_ref().map(|remove| {
            (0..geometry.face_index_ranges.len())
                .filter(|&face| remove(geometry, face))
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        for face in remove {
            let range = geometry.face_index_ranges[face];
            let start = range.index_offset as usize;
            let indices = &mut geometry.geometry.indices[start..start + range.index_count as usize];
            if let Some(&first) = indices.first() {
                indices.fill(first);
            }
        }
    });
}
