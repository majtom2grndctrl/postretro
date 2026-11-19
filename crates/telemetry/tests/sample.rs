// The reducer's checked-in sample is current: every line parses as an
// envelope of this version, and each run's sequence starts at 0 and runs
// without a gap.
// See: context/lib/development_guide.md §6.5 · tools/telemetry_reduce.py

use std::collections::BTreeMap;

use postretro_telemetry::{Envelope, Kind};

#[test]
fn every_line_of_the_two_run_sample_parses_as_a_current_envelope() {
    let text = include_str!("fixtures/two_run_sample.jsonl");
    let mut runs: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut first_kinds: BTreeMap<String, Kind> = BTreeMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let envelope = Envelope::parse_line(line).unwrap_or_else(|e| panic!("{e}: {line}"));
        first_kinds
            .entry(envelope.run.clone())
            .or_insert(envelope.kind);
        runs.entry(envelope.run).or_default().push(envelope.seq);
    }
    assert_eq!(runs.len(), 2, "a bake run and an engine run");
    for (run, seqs) in &runs {
        assert_eq!(seqs, &(0..seqs.len() as u64).collect::<Vec<_>>(), "{run}");
        assert_eq!(first_kinds[run], Kind::RunStart, "{run}");
    }
}
