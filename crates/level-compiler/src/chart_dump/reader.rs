//! Reads a chart dump back: the comparator's side of `format`. Only the
//! comparator reads dumps, and it runs as a test.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use super::format::{
    ChartKey, ChartMeta, DumpKind, INDEX_MAGIC, IndexEntry, IndexFile, PlaneKind, PlaneSpan,
};
use super::{CHART_DUMP_FORMAT_VERSION, Category, MANIFEST_FILE, Manifest};

impl ChartMeta {
    /// Whether the chart's origin sits on the direction grid, so its
    /// direction texels group only its own irradiance texels.
    pub(crate) fn direction_aligned(&self) -> bool {
        let scale = self.direction_texel_scale.max(1);
        self.placement.block_x.is_multiple_of(scale) && self.placement.block_y.is_multiple_of(scale)
    }
}

impl IndexEntry {
    pub(crate) fn hash(&self, category: Category) -> Option<&[u8; 32]> {
        self.hashes
            .iter()
            .find(|(c, _)| *c == category)
            .map(|(_, hash)| hash)
    }

    pub(crate) fn plane(&self, kind: PlaneKind) -> Option<PlaneSpan> {
        self.planes.iter().copied().find(|span| span.kind == kind)
    }
}

impl PlaneKind {
    pub(crate) fn of(category: Category) -> PlaneKind {
        match category {
            Category::IrradianceGrid | Category::IrradianceRing => PlaneKind::Irradiance,
            Category::Direction => PlaneKind::Direction,
            Category::MaskAGrid
            | Category::MaskARing
            | Category::MaskBGrid
            | Category::MaskBRing => PlaneKind::Mask,
            Category::AnimatedWeights => PlaneKind::AnimatedWeights,
        }
    }
}

/// One kind's records from a dump directory. Only the comparator reads
/// dumps, and it runs as a test.
pub(crate) struct DumpReader {
    data_path: PathBuf,
    texels: bool,
    pub(crate) entries: BTreeMap<ChartKey, IndexEntry>,
}

impl DumpReader {
    /// A missing index reads as no records: its stage had nothing to dump.
    pub(crate) fn open(dir: &Path, kind: DumpKind) -> anyhow::Result<Self> {
        let index_path = kind.index_path(dir);
        let data_path = kind.data_path(dir);
        if !index_path.exists() {
            return Ok(Self {
                data_path,
                texels: false,
                entries: BTreeMap::new(),
            });
        }
        let bytes =
            std::fs::read(&index_path).with_context(|| format!("read {}", index_path.display()))?;
        let index: IndexFile = postcard::from_bytes(&bytes)
            .with_context(|| format!("parse {}", index_path.display()))?;
        anyhow::ensure!(
            index.magic == INDEX_MAGIC && index.format_version == CHART_DUMP_FORMAT_VERSION,
            "{} is not a chart dump index of format {CHART_DUMP_FORMAT_VERSION}",
            index_path.display()
        );
        anyhow::ensure!(
            index.kind == kind,
            "{} holds {:?} records, expected {kind:?}",
            index_path.display(),
            index.kind
        );
        Ok(Self {
            data_path,
            texels: index.texels,
            entries: index
                .entries
                .into_iter()
                .map(|entry| (entry.meta.key, entry))
                .collect(),
        })
    }

    /// One plane's bytes, or `None` in a hashes-only dump.
    pub(crate) fn read_plane(
        &self,
        entry: &IndexEntry,
        kind: PlaneKind,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(span) = entry.plane(kind).filter(|_| self.texels) else {
            return Ok(None);
        };
        let mut file = File::open(&self.data_path)
            .with_context(|| format!("open {}", self.data_path.display()))?;
        file.seek(SeekFrom::Start(span.offset))?;
        let mut bytes = vec![0; usize::try_from(span.len)?];
        file.read_exact(&mut bytes).with_context(|| {
            format!(
                "read {} {kind:?} from {}",
                entry.meta.key,
                self.data_path.display()
            )
        })?;
        Ok(Some(bytes))
    }
}

/// Read a dump directory's manifest: whether it holds texels. Fails on a
/// directory that is not a dump, or one of another format version.
pub(crate) fn read_manifest(dir: &Path) -> anyhow::Result<bool> {
    let path = dir.join(MANIFEST_FILE);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("{} is not a chart dump: no {MANIFEST_FILE}", dir.display()))?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    anyhow::ensure!(
        manifest.format_version == CHART_DUMP_FORMAT_VERSION,
        "{} is chart dump format {}, this build reads {CHART_DUMP_FORMAT_VERSION}",
        dir.display(),
        manifest.format_version
    );
    Ok(manifest.texels)
}
