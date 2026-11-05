// Which clusters name each dense probe, as id-49 dense ranges sorted by start.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

/// One id-49 dense range: `start..end` probes named by `cluster`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DenseSpan {
    pub start: u32,
    pub end: u32,
    pub cluster: u32,
}

/// Every cluster's dense ranges. Clusters overlap by design, so a probe has no
/// single answer here: a query visits each cluster that names it. Bounded by
/// the longest span, a lookup walks back only as far as a covering span could
/// start, and it costs nothing per probe at rest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DenseSpans {
    spans: Vec<DenseSpan>,
    longest: u32,
}

impl DenseSpans {
    pub fn new(mut spans: Vec<DenseSpan>) -> Self {
        spans.sort_unstable_by_key(|span| (span.start, span.cluster));
        let longest = spans
            .iter()
            .map(|span| span.end - span.start)
            .max()
            .unwrap_or(0);
        Self { spans, longest }
    }

    /// Whether any cluster naming `dense` satisfies `accept`.
    pub fn any_cluster_naming(&self, dense: u32, mut accept: impl FnMut(u32) -> bool) -> bool {
        let earliest = dense.saturating_sub(self.longest);
        let after = self.spans.partition_point(|span| span.start <= dense);
        self.spans[..after]
            .iter()
            .rev()
            .take_while(|span| span.start >= earliest)
            .any(|span| dense < span.end && accept(span.cluster))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clusters_naming(spans: &DenseSpans, dense: u32) -> Vec<u32> {
        let mut clusters = Vec::new();
        spans.any_cluster_naming(dense, |cluster| {
            clusters.push(cluster);
            false
        });
        clusters.sort_unstable();
        clusters
    }

    #[test]
    fn a_probe_reports_every_overlapping_cluster_and_no_neighbor() {
        let spans = DenseSpans::new(vec![
            DenseSpan {
                start: 10,
                end: 20,
                cluster: 2,
            },
            DenseSpan {
                start: 0,
                end: 16,
                cluster: 0,
            },
            DenseSpan {
                start: 14,
                end: 15,
                cluster: 1,
            },
        ]);
        assert_eq!(clusters_naming(&spans, 14), [0, 1, 2]);
        assert_eq!(clusters_naming(&spans, 5), [0]);
        assert_eq!(clusters_naming(&spans, 19), [2]);
        assert!(clusters_naming(&spans, 20).is_empty());
    }

    #[test]
    fn an_empty_table_names_nothing() {
        assert!(clusters_naming(&DenseSpans::default(), 0).is_empty());
    }
}
