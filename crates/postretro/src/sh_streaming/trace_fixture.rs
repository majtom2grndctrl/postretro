//! Byte-exact comparison of an SH streaming trace against its committed
//! baseline in `testdata/`. See: context/lib/testing_guide.md
//!
//! The baselines were recorded from the SH controller and issuer before the
//! resource-neutral streaming layer was extracted. A replay that differs
//! means SH request order, drain budget, or read order changed.

use std::path::PathBuf;

/// Set to `1` to rewrite a baseline from the current code instead of
/// comparing. Only for recording a new baseline on purpose: a change that is
/// meant to preserve SH behaviour must pass without it.
pub(crate) const REGEN_ENV: &str = "POSTRETRO_REGEN_SH_TRACE";

fn baseline_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/sh_streaming/testdata")
        .join(file_name)
}

pub(crate) fn assert_matches_baseline(file_name: &str, actual: &str) {
    let path = baseline_path(file_name);
    if std::env::var_os(REGEN_ENV).is_some_and(|value| value == "1") {
        std::fs::write(&path, actual)
            .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading baseline {}: {error}", path.display()));
    if expected == actual {
        return;
    }
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    let mut line = 1;
    loop {
        match (expected_lines.next(), actual_lines.next()) {
            (Some(left), Some(right)) if left == right => line += 1,
            (left, right) => panic!(
                "SH trace diverges from baseline {} at line {line}\n  baseline: {}\n  replay:   {}\n\
                 full replay:\n{actual}",
                path.display(),
                left.unwrap_or("<end of file>"),
                right.unwrap_or("<end of trace>"),
            ),
        }
    }
}
