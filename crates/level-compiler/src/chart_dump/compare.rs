//! Compares two chart dumps chart by chart, per category, and names the first
//! differing texel of the first few differing charts.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::Category;
use super::format::{ChartKey, DumpKind, IndexEntry, PlaneKind};
use super::reader::{DumpReader, read_manifest};
use super::regions::category_texels;

/// Differing charts per category described in full; the rest are counted.
const EXAMPLES_PER_CATEGORY: usize = 10;
/// Missing or mismatched keys listed per kind.
const KEYS_LISTED: usize = 10;

/// Whether a padding-ring difference fails the comparison or is only
/// reported. The bit-identity AC is about the chart's own grid; the ring is
/// P8's evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RingPolicy {
    Fail,
    Report,
}

#[derive(Debug, Default)]
pub(crate) struct KindTally {
    pub(crate) baseline: usize,
    pub(crate) candidate: usize,
    pub(crate) missing_in_candidate: Vec<ChartKey>,
    pub(crate) missing_in_baseline: Vec<ChartKey>,
    /// Same key, different chart grid: the key no longer names one chart.
    pub(crate) fingerprint_mismatch: Vec<ChartKey>,
}

#[derive(Debug, Default)]
pub(crate) struct CategoryTally {
    pub(crate) compared: usize,
    pub(crate) identical: usize,
    pub(crate) differing: usize,
    /// Direction only: charts whose baseline origin is off the direction
    /// grid, so their direction texels group another chart's texels.
    pub(crate) skipped_unaligned: usize,
    pub(crate) examples: Vec<(ChartKey, String)>,
}

#[derive(Debug, Default)]
pub(crate) struct Comparison {
    pub(crate) kinds: BTreeMap<DumpKind, KindTally>,
    pub(crate) categories: BTreeMap<Category, CategoryTally>,
}

impl Category {
    pub(crate) const ALL: [Category; 8] = [
        Category::IrradianceGrid,
        Category::IrradianceRing,
        Category::Direction,
        Category::MaskAGrid,
        Category::MaskARing,
        Category::MaskBGrid,
        Category::MaskBRing,
        Category::AnimatedWeights,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Category::IrradianceGrid => "irradiance (chart grid)",
            Category::IrradianceRing => "irradiance (padding ring)",
            Category::Direction => "direction (half-res)",
            Category::MaskAGrid => "mask A (chart grid)",
            Category::MaskARing => "mask A (padding ring)",
            Category::MaskBGrid => "mask B (chart grid)",
            Category::MaskBRing => "mask B (padding ring)",
            Category::AnimatedWeights => "animated weights",
        }
    }

    /// Padding-ring categories: P8's evidence, not the bit-identity AC's.
    pub(crate) fn is_ring(self) -> bool {
        matches!(
            self,
            Category::IrradianceRing | Category::MaskARing | Category::MaskBRing
        )
    }
}

fn kind_planes(kind: DumpKind) -> &'static [PlaneKind] {
    match kind {
        DumpKind::Lightmap => &[PlaneKind::Irradiance, PlaneKind::Direction],
        DumpKind::Mask => &[PlaneKind::Mask],
        DumpKind::Animated => &[PlaneKind::AnimatedWeights],
    }
}

/// Compare every record kind of `candidate` against `baseline`.
pub(crate) fn compare_dumps(baseline: &Path, candidate: &Path) -> anyhow::Result<Comparison> {
    read_manifest(baseline)?;
    read_manifest(candidate)?;
    let mut comparison = Comparison::default();
    for kind in DumpKind::ALL {
        let base = DumpReader::open(baseline, kind)?;
        let cand = DumpReader::open(candidate, kind)?;
        let tally = comparison.kinds.entry(kind).or_default();
        tally.baseline = base.entries.len();
        tally.candidate = cand.entries.len();
        tally.missing_in_baseline = cand
            .entries
            .keys()
            .filter(|key| !base.entries.contains_key(key))
            .copied()
            .collect();
        for (key, base_entry) in &base.entries {
            let Some(cand_entry) = cand.entries.get(key) else {
                tally.missing_in_candidate.push(*key);
                continue;
            };
            if base_entry.meta.chart_fingerprint != cand_entry.meta.chart_fingerprint {
                tally.fingerprint_mismatch.push(*key);
                continue;
            }
            for &plane in kind_planes(kind) {
                for &category in plane.categories() {
                    let result = compare_category(category, base_entry, cand_entry);
                    let category_tally = comparison.categories.entry(category).or_default();
                    match result {
                        Outcome::Skipped => category_tally.skipped_unaligned += 1,
                        Outcome::Identical => {
                            category_tally.compared += 1;
                            category_tally.identical += 1;
                        }
                        Outcome::Differs(detail) => {
                            category_tally.compared += 1;
                            category_tally.differing += 1;
                            if category_tally.examples.len() < EXAMPLES_PER_CATEGORY {
                                category_tally.examples.push((*key, detail(&base, &cand)?));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(comparison)
}

type Detail<'a> = Box<dyn FnOnce(&DumpReader, &DumpReader) -> anyhow::Result<String> + 'a>;

enum Outcome<'a> {
    Skipped,
    Identical,
    /// The description is built only for the first few differing charts:
    /// it reads both charts' texels back from disk.
    Differs(Detail<'a>),
}

fn extent(category: Category, entry: &IndexEntry) -> ([u32; 2], [i32; 2]) {
    match category {
        Category::Direction => (entry.meta.direction_dims, [0, 0]),
        _ => ([entry.meta.width, entry.meta.height], entry.meta.origin),
    }
}

fn compare_category<'a>(
    category: Category,
    base: &'a IndexEntry,
    cand: &'a IndexEntry,
) -> Outcome<'a> {
    // Direction groups `scale²` texels; at an unaligned baseline origin its
    // texels mixed this chart with another, so the AC has no baseline there.
    if category == Category::Direction && !base.meta.direction_aligned() {
        return Outcome::Skipped;
    }
    let (base_extent, base_origin) = extent(category, base);
    let (cand_extent, cand_origin) = extent(category, cand);
    if (base_extent, base_origin) != (cand_extent, cand_origin) {
        return Outcome::Differs(Box::new(move |_, _| {
            Ok(format!(
                "grid {}x{} at {base_origin:?} vs {}x{} at {cand_origin:?}",
                base_extent[0], base_extent[1], cand_extent[0], cand_extent[1]
            ))
        }));
    }
    if base.hash(category) == cand.hash(category) {
        return Outcome::Identical;
    }
    Outcome::Differs(Box::new(move |base_reader, cand_reader| {
        first_difference(category, (base_reader, base), (cand_reader, cand))
    }))
}

fn first_difference(
    category: Category,
    (base_reader, base): (&DumpReader, &IndexEntry),
    (cand_reader, cand): (&DumpReader, &IndexEntry),
) -> anyhow::Result<String> {
    let plane = PlaneKind::of(category);
    let (Some(base_bytes), Some(cand_bytes)) = (
        base_reader.read_plane(base, plane)?,
        cand_reader.read_plane(cand, plane)?,
    ) else {
        return Ok("hash differs (a hashes-only dump has no texels to name)".to_owned());
    };
    let texels = |entry: &IndexEntry, bytes: &[u8]| {
        category_texels(category, &entry.meta, bytes)
            .map_err(|reason| anyhow::anyhow!("{} {plane:?}: {reason}", entry.meta.key))
    };
    let base_texels = texels(base, &base_bytes)?;
    let cand_texels = texels(cand, &cand_bytes)?;
    let mismatches = base_texels
        .iter()
        .zip(&cand_texels)
        .filter(|(b, c)| b.1 != c.1)
        .count();
    match base_texels
        .iter()
        .zip(&cand_texels)
        .find(|(b, c)| b.1 != c.1)
    {
        Some(((at, b), (_, c))) => Ok(format!(
            "{mismatches} texel(s) differ; first at ({}, {}): baseline {} candidate {}",
            at[0],
            at[1],
            render_texel(category, b),
            render_texel(category, c)
        )),
        None => Ok(format!(
            "{} texels vs {}",
            base_texels.len(),
            cand_texels.len()
        )),
    }
}

/// Irradiance prints as floats so a one-ulp drift reads as one; the rest
/// prints as bytes.
fn render_texel(category: Category, bytes: &[u8]) -> String {
    if matches!(
        category,
        Category::IrradianceGrid | Category::IrradianceRing
    ) {
        let floats: Vec<String> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| format!("{:e}", f32::from_le_bytes(*c)))
            .collect();
        return format!("[{}]", floats.join(", "));
    }
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl Comparison {
    pub(crate) fn compared_total(&self) -> usize {
        self.categories.values().map(|tally| tally.compared).sum()
    }

    /// Why the comparison fails; empty when it passes. Comparing nothing
    /// fails: an empty pass proves nothing.
    pub(crate) fn failures(&self, ring: RingPolicy) -> Vec<String> {
        let mut failures = Vec::new();
        if self.compared_total() == 0 {
            failures.push("no chart was compared".to_owned());
        }
        for (kind, tally) in &self.kinds {
            for (what, keys) in [
                ("missing in the candidate", &tally.missing_in_candidate),
                ("missing in the baseline", &tally.missing_in_baseline),
                ("with a different chart grid", &tally.fingerprint_mismatch),
            ] {
                if !keys.is_empty() {
                    failures.push(format!("{kind:?}: {} record(s) {what}", keys.len()));
                }
            }
        }
        for (category, tally) in &self.categories {
            if tally.differing > 0 && !(category.is_ring() && ring == RingPolicy::Report) {
                failures.push(format!(
                    "{}: {} chart(s) differ",
                    category.label(),
                    tally.differing
                ));
            }
        }
        failures
    }

    pub(crate) fn render(&self) -> String {
        let mut out = String::from("chart dump comparison\n");
        for (kind, tally) in &self.kinds {
            let _ = writeln!(
                out,
                "  {kind:?}: baseline {}, candidate {}, missing in candidate {}, missing in baseline {}, grid mismatch {}",
                tally.baseline,
                tally.candidate,
                tally.missing_in_candidate.len(),
                tally.missing_in_baseline.len(),
                tally.fingerprint_mismatch.len(),
            );
            for (what, keys) in [
                ("missing in candidate", &tally.missing_in_candidate),
                ("missing in baseline", &tally.missing_in_baseline),
                ("grid mismatch", &tally.fingerprint_mismatch),
            ] {
                if !keys.is_empty() {
                    let listed: Vec<String> = keys
                        .iter()
                        .take(KEYS_LISTED)
                        .map(ToString::to_string)
                        .collect();
                    let _ = writeln!(out, "    {what}: {}", listed.join(", "));
                }
            }
        }
        let _ = writeln!(
            out,
            "  {:<28} {:>9} {:>9} {:>9} {:>17}",
            "category", "compared", "identical", "differing", "odd-origin skips"
        );
        for category in Category::ALL {
            let Some(tally) = self.categories.get(&category) else {
                continue;
            };
            let _ = writeln!(
                out,
                "  {:<28} {:>9} {:>9} {:>9} {:>17}",
                category.label(),
                tally.compared,
                tally.identical,
                tally.differing,
                tally.skipped_unaligned
            );
        }
        for category in Category::ALL {
            let Some(tally) = self.categories.get(&category) else {
                continue;
            };
            for (key, detail) in &tally.examples {
                let _ = writeln!(out, "  {} {key}: {detail}", category.label());
            }
        }
        out
    }
}
