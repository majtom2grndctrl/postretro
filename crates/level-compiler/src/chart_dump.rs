// Chart texel dump: per-chart lightmap, mask and weight-map texels before BC encoding, and their comparator.
// See: context/lib/build_pipeline.md §Build Cache (CLI flags, `--dump-chart-texels`)
//
// A measurement instrument, not a bake output: it reads the buffers the
// encoders are about to consume and never writes into them, so a build with
// the dump on writes the same PRL as one without it.
//
// Directory layout, one pair of files per record kind (`lightmap`, `mask`,
// `animated`), plus `chart-dump.json` naming the format version and mode:
// - `<kind>.index` — postcard [`format::IndexFile`]: one entry per chart in key
//   order, carrying the chart's placement and a blake3 hash per comparison
//   category ([`Category`]).
// - `<kind>.data` — texels only (`--dump-chart-texels`, absent under
//   `--dump-chart-hashes`): each record's planes back to back, in the order
//   the bake produced them, which is fixed for a layout whatever the worker
//   count. Every plane is chart-local row-major.
//
// Mask records cover only faces some light owns, in the lightmap layout the
// mask shares today. A kind whose stage had nothing to dump writes no index;
// readers treat a missing index as no records.

use std::path::PathBuf;

pub(crate) mod capture;
#[cfg(test)]
pub(crate) mod compare;
pub(crate) mod format;
#[cfg(test)]
pub(crate) mod reader;
mod regions;

#[cfg(test)]
mod tests;

pub(crate) use format::{DumpKind, SharedDumpWriter};

/// Format version of every file in a dump directory.
pub(crate) const CHART_DUMP_FORMAT_VERSION: u32 = 1;

/// Name of the manifest that marks a directory as a chart dump.
pub(crate) const MANIFEST_FILE: &str = "chart-dump.json";

/// `--dump-chart-texels <DIR>` or `--dump-chart-hashes <DIR>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartDumpConfig {
    pub dir: PathBuf,
    /// Write the texels as well as the per-category hashes. Hashes alone
    /// keep a large map's dump small; texels let the comparator name the
    /// first differing texel.
    pub texels: bool,
}

/// What a dump compares: every category is hashed on its own, so a
/// difference in the padding ring never hides behind, or masks, one in the
/// chart's own grid.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub(crate) enum Category {
    /// Pre-BC6H f32 RGBA irradiance inside the chart's padding.
    IrradianceGrid,
    /// The same plane's `CHART_PADDING_TEXELS` padding ring.
    IrradianceRing,
    /// Rg8 direction texels whose reduction footprint lies wholly inside the
    /// padded chart rect.
    Direction,
    /// Mask plane A (channels 0, 1), pre-BC5, inside the padding.
    MaskAGrid,
    MaskARing,
    /// Mask plane B (channels 2, 3).
    MaskBGrid,
    MaskBRing,
    /// One animated chunk's per-texel `(light, weight, direction)` lists.
    AnimatedWeights,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Manifest {
    pub(crate) format_version: u32,
    pub(crate) texels: bool,
}

/// Make `config.dir` a fresh dump directory: remove every file an earlier
/// dump left there (and nothing else), then write the manifest. Runs once
/// per build, before any stage writes a record.
pub(crate) fn prepare_dir(config: &ChartDumpConfig) -> anyhow::Result<()> {
    use anyhow::Context as _;
    std::fs::create_dir_all(&config.dir)
        .with_context(|| format!("create chart dump directory {}", config.dir.display()))?;
    for kind in DumpKind::ALL {
        for path in [kind.index_path(&config.dir), kind.data_path(&config.dir)] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("remove stale chart dump {}", path.display()));
                }
            }
        }
    }
    let manifest = Manifest {
        format_version: CHART_DUMP_FORMAT_VERSION,
        texels: config.texels,
    };
    let path = config.dir.join(MANIFEST_FILE);
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
