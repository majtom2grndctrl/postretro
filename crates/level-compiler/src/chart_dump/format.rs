//! Chart dump records: the index entry, the texel planes, and the writer and
//! reader every capture site and the comparator share.

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use super::regions::category_hashes;
use super::{CHART_DUMP_FORMAT_VERSION, Category, ChartDumpConfig};

pub(super) const INDEX_MAGIC: [u8; 4] = *b"PRCD";

/// The three record kinds, each written by its own stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum DumpKind {
    /// One record per chart: irradiance and direction (`push_layer`).
    Lightmap,
    /// One record per mask-owning chart: its four mask channels
    /// (`OwnershipFill::close_layer`).
    Mask,
    /// One record per animated chunk: its weight lists.
    Animated,
}

impl DumpKind {
    pub(crate) const ALL: [DumpKind; 3] = [DumpKind::Lightmap, DumpKind::Mask, DumpKind::Animated];

    fn stem(self) -> &'static str {
        match self {
            DumpKind::Lightmap => "lightmap",
            DumpKind::Mask => "mask",
            DumpKind::Animated => "animated",
        }
    }

    pub(crate) fn index_path(self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.index", self.stem()))
    }

    pub(crate) fn data_path(self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.data", self.stem()))
    }
}

/// A chart's identity independent of where it was packed: its id-17 face
/// index (post-cut chart order, fixed before packing), and for animated
/// records the chunk's ordinal among its face's chunks in id-24 order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct ChartKey {
    pub(crate) face: u32,
    pub(crate) sub: u32,
}

impl std::fmt::Display for ChartKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.sub == 0 {
            write!(f, "face {}", self.face)
        } else {
            write!(f, "face {} chunk {}", self.face, self.sub)
        }
    }
}

/// Where the record's chart sat. Informational: the comparator reads only
/// the block-local origin's alignment, to decide whether direction compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct Placement {
    /// Id-22 cell block holding the chart.
    pub(crate) block: u32,
    /// The padded chart rect's origin in its block.
    pub(crate) block_x: u32,
    pub(crate) block_y: u32,
    /// Internal bake layer and the origin there.
    pub(crate) layer: u32,
    pub(crate) layer_x: u32,
    pub(crate) layer_y: u32,
    /// Stored transposed. The planes are chart-local either way.
    pub(crate) transposed: bool,
}

/// Everything a record says about its chart besides the texels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChartMeta {
    pub(crate) key: ChartKey,
    /// blake3 of the chart's grid (axes, UV rect, extent, cut window): two
    /// dumps' records under one key describe the same chart only if this
    /// matches.
    pub(crate) chart_fingerprint: u64,
    /// Texel extent of the record's planes: the padded chart rect
    /// (`Chart::width_texels` × `height_texels`), or the animated chunk rect.
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Padding ring width inside `width × height`; 0 for animated records,
    /// which have no ring of their own.
    pub(crate) padding: u32,
    /// Animated: the chunk rect's origin in the padded chart rect. Else 0.
    pub(crate) origin: [i32; 2],
    pub(crate) placement: Placement,
    /// Lightmap: irradiance texels per direction texel per axis.
    pub(crate) direction_texel_scale: u32,
    /// Lightmap: the first whole direction texel's offset from the padded
    /// chart origin, in full-resolution texels (0 at an aligned origin), and
    /// the extent of whole direction texels.
    pub(crate) direction_offset: [u32; 2],
    pub(crate) direction_dims: [u32; 2],
    /// Mask: the face's owner per channel.
    pub(crate) owners: [u16; 4],
}

/// The texel planes a record carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum PlaneKind {
    /// `width × height` texels of four little-endian f32 (RGBA), the BC6H
    /// encoder's input.
    Irradiance,
    /// `direction_dims` texels of Rg8 octahedral direction, as stored.
    Direction,
    /// `width × height` texels of four u8 mask channels, as handed to BC5.
    Mask,
    /// `width × height` texels, each a u32 count and that many 12-byte
    /// entries: u32 light index, f32 weight bits, two u16 octahedral direction.
    AnimatedWeights,
}

impl PlaneKind {
    pub(crate) fn categories(self) -> &'static [Category] {
        match self {
            PlaneKind::Irradiance => &[Category::IrradianceGrid, Category::IrradianceRing],
            PlaneKind::Direction => &[Category::Direction],
            PlaneKind::Mask => &[
                Category::MaskAGrid,
                Category::MaskARing,
                Category::MaskBGrid,
                Category::MaskBRing,
            ],
            PlaneKind::AnimatedWeights => &[Category::AnimatedWeights],
        }
    }
}

/// One plane's bytes in the data file; `offset` and `len` are 0 in a
/// hashes-only dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlaneSpan {
    pub(crate) kind: PlaneKind,
    pub(crate) offset: u64,
    pub(crate) len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IndexEntry {
    pub(crate) meta: ChartMeta,
    pub(crate) planes: Vec<PlaneSpan>,
    pub(crate) hashes: Vec<(Category, [u8; 32])>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct IndexFile {
    pub(super) magic: [u8; 4],
    pub(super) format_version: u32,
    pub(super) kind: DumpKind,
    pub(super) texels: bool,
    /// Sorted by key; keys are unique.
    pub(crate) entries: Vec<IndexEntry>,
}

/// Streams one kind's records to disk as the bake produces them. Only the
/// index entries stay in memory; texels go straight to the data file.
pub(crate) struct DumpWriter {
    kind: DumpKind,
    dir: PathBuf,
    texels: bool,
    data: Option<BufWriter<File>>,
    offset: u64,
    entries: Vec<IndexEntry>,
    /// The first write failure. Capture sites cannot fail the bake they sit
    /// in, so the stage reports it at `finish`.
    error: Option<anyhow::Error>,
}

impl DumpWriter {
    pub(crate) fn create(config: &ChartDumpConfig, kind: DumpKind) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&config.dir)
            .with_context(|| format!("create chart dump directory {}", config.dir.display()))?;
        let data = if config.texels {
            let path = kind.data_path(&config.dir);
            Some(BufWriter::new(
                File::create(&path).with_context(|| format!("create {}", path.display()))?,
            ))
        } else {
            None
        };
        Ok(Self {
            kind,
            dir: config.dir.clone(),
            texels: config.texels,
            data,
            offset: 0,
            entries: Vec::new(),
            error: None,
        })
    }

    /// Record one chart. Each plane must be chart-local row-major in the
    /// layout [`PlaneKind`] names for `meta`'s extents.
    pub(crate) fn push(&mut self, meta: ChartMeta, planes: &[(PlaneKind, &[u8])]) {
        if self.error.is_some() {
            return;
        }
        let mut hashes = Vec::new();
        for &(kind, bytes) in planes {
            match category_hashes(kind, &meta, bytes) {
                Ok(plane_hashes) => hashes.extend(plane_hashes),
                Err(reason) => {
                    self.error = Some(anyhow::anyhow!(
                        "chart dump: {} {kind:?} plane is malformed: {reason}",
                        meta.key
                    ));
                    return;
                }
            }
        }
        let mut spans = Vec::with_capacity(planes.len());
        for &(kind, bytes) in planes {
            let (offset, len) = match self.data.as_mut() {
                Some(data) => {
                    if let Err(error) = data.write_all(bytes) {
                        self.error = Some(anyhow::Error::new(error).context(format!(
                            "write {}",
                            self.kind.data_path(&self.dir).display()
                        )));
                        return;
                    }
                    let offset = self.offset;
                    self.offset += bytes.len() as u64;
                    (offset, bytes.len() as u64)
                }
                None => (0, 0),
            };
            spans.push(PlaneSpan { kind, offset, len });
        }
        self.entries.push(IndexEntry {
            meta,
            planes: spans,
            hashes,
        });
    }

    /// Flush the texels and write the index, sorted by key. A key recorded
    /// twice is an error: every chart's final values are dumped once.
    pub(crate) fn finish(&mut self) -> anyhow::Result<()> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        if let Some(mut data) = self.data.take() {
            data.flush()
                .with_context(|| format!("flush {}", self.kind.data_path(&self.dir).display()))?;
        }
        let mut entries = std::mem::take(&mut self.entries);
        entries.sort_by_key(|entry| entry.meta.key);
        if let Some(pair) = entries
            .windows(2)
            .find(|pair| pair[0].meta.key == pair[1].meta.key)
        {
            anyhow::bail!(
                "chart dump: {} was recorded twice in the {:?} dump",
                pair[0].meta.key,
                self.kind
            );
        }
        let index = IndexFile {
            magic: INDEX_MAGIC,
            format_version: CHART_DUMP_FORMAT_VERSION,
            kind: self.kind,
            texels: self.texels,
            entries,
        };
        let path = self.kind.index_path(&self.dir);
        std::fs::write(&path, postcard::to_allocvec(&index)?)
            .with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }
}

/// A writer shared by a stage's capture sites, which run on whichever worker
/// holds the layer close, one at a time.
#[derive(Clone)]
pub(crate) struct SharedDumpWriter(Arc<Mutex<DumpWriter>>);

impl SharedDumpWriter {
    pub(crate) fn create(config: &ChartDumpConfig, kind: DumpKind) -> anyhow::Result<Self> {
        Ok(Self(Arc::new(Mutex::new(DumpWriter::create(
            config, kind,
        )?))))
    }

    pub(crate) fn push(&self, meta: ChartMeta, planes: &[(PlaneKind, &[u8])]) {
        self.0
            .lock()
            .expect("a chart dump capture panicked mid-write")
            .push(meta, planes);
    }

    pub(crate) fn finish(&self) -> anyhow::Result<()> {
        self.0
            .lock()
            .expect("a chart dump capture panicked mid-write")
            .finish()
    }
}
