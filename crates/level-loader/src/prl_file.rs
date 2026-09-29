// The one retained PRL file handle per level, and its always-on per-section read counters.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use postretro_level_format::ContainerMeta;

/// A level's opened PRL file plus the counters every positional read through
/// it increments.
///
/// Owner: one per loaded level, shared by `Arc`. The loader, the SH manifest
/// and the lightmap manifest hold clones of the same handle, so a level opens
/// its file once. The file closes when the last clone drops: the level's
/// streaming storage and any issuer route that cloned a manifest.
#[derive(Debug)]
pub(crate) struct PrlFile {
    file: File,
    reads: Arc<PrlReadCounters>,
}

impl PrlFile {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Ok(Self::new(File::open(path)?))
    }

    pub(crate) fn new(file: File) -> Self {
        Self {
            file,
            reads: Arc::new(PrlReadCounters::default()),
        }
    }

    pub(crate) fn file(&self) -> &File {
        &self.file
    }

    pub(crate) fn len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    pub(crate) fn read_counters(&self) -> &Arc<PrlReadCounters> {
        &self.reads
    }
}

/// Bytes read from one PRL file, attributed to the sections they overlap.
///
/// Always on, in every build: relaxed atomic adds at the positional reader,
/// one per section a read overlaps. A read that coalesces several sections'
/// ranges credits each section only its own bytes; bytes outside every section
/// (header, table, reads before the table is known) count separately. The
/// counts are totals over the file's life: the loader's reads, then every
/// streaming read issued through a manifest.
#[derive(Debug, Default)]
pub struct PrlReadCounters {
    sections: OnceLock<Vec<SectionReadCounter>>,
    outside_sections: AtomicU64,
}

#[derive(Debug)]
struct SectionReadCounter {
    section_id: u32,
    start: u64,
    end: u64,
    bytes: AtomicU64,
}

impl PrlReadCounters {
    /// Bind attribution to the validated section table. Reads before this
    /// count outside every section. A second table is ignored: one file has
    /// one table.
    pub(crate) fn install_table(&self, container: &ContainerMeta) {
        let _ = self.sections.set(
            container
                .sections
                .iter()
                .map(|entry| SectionReadCounter {
                    section_id: entry.section_id,
                    start: entry.offset,
                    end: entry.offset.saturating_add(entry.size),
                    bytes: AtomicU64::new(0),
                })
                .collect(),
        );
    }

    /// Credit a completed read of `len` bytes at `offset`.
    pub(crate) fn record(&self, offset: u64, len: u64) {
        let end = offset.saturating_add(len);
        let mut attributed = 0u64;
        for section in self.sections.get().map_or(&[][..], Vec::as_slice) {
            let overlap = end
                .min(section.end)
                .saturating_sub(offset.max(section.start));
            if overlap > 0 {
                section.bytes.fetch_add(overlap, Ordering::Relaxed);
                attributed += overlap;
            }
        }
        self.outside_sections
            .fetch_add(len - attributed.min(len), Ordering::Relaxed);
    }

    /// Bytes read from section `section_id` (summed over duplicate entries).
    pub fn section_bytes(&self, section_id: u32) -> u64 {
        self.sections.get().map_or(0, |sections| {
            sections
                .iter()
                .filter(|section| section.section_id == section_id)
                .map(|section| section.bytes.load(Ordering::Relaxed))
                .sum()
        })
    }

    /// Bytes read outside every section: the container header and table.
    pub fn bytes_outside_sections(&self) -> u64 {
        self.outside_sections.load(Ordering::Relaxed)
    }

    pub fn total_bytes(&self) -> u64 {
        self.bytes_outside_sections()
            + self.sections.get().map_or(0, |sections| {
                sections
                    .iter()
                    .map(|section| section.bytes.load(Ordering::Relaxed))
                    .sum()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::{CURRENT_VERSION, Header, SectionEntry};

    fn counters() -> PrlReadCounters {
        let counters = PrlReadCounters::default();
        let entry = |section_id, offset, size| SectionEntry {
            section_id,
            offset,
            size,
            version: 1,
        };
        counters.install_table(&ContainerMeta {
            header: Header {
                version: CURRENT_VERSION,
                section_count: 2,
            },
            sections: vec![entry(22, 100, 50), entry(42, 170, 30)],
        });
        counters
    }

    #[test]
    fn a_read_spanning_two_sections_credits_each_only_its_own_bytes() {
        let counters = counters();
        counters.record(140, 40); // 10 of id 22, 10 of the gap, 10 of id 42.
        assert_eq!(counters.section_bytes(22), 10);
        assert_eq!(counters.section_bytes(42), 10);
        assert_eq!(counters.bytes_outside_sections(), 20);
        assert_eq!(counters.total_bytes(), 40);
    }

    #[test]
    fn reads_before_the_table_count_outside_every_section() {
        let counters = PrlReadCounters::default();
        counters.record(0, 8);
        assert_eq!(counters.bytes_outside_sections(), 8);
        assert_eq!(counters.section_bytes(22), 0);
    }
}
