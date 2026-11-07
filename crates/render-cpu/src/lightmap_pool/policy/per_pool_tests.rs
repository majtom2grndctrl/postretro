// Every policy suite, run once per pool under one model.
//
// A suite names `POOL`, the pool under test, and drives blocks of one plane
// each, plane A and plane B in turn. The pool under test holds what a suite
// asks of it; the other pool has a layer per block, so it never needs a repack
// or a growth. Every pair still installs and leaves through both pools, and the
// mirror still checks both.

macro_rules! pool_suites {
    ($($suite:ident: $kind:expr;)*) => {$(
        mod $suite {
            use super::super::*;

            const POOL: PoolKind = $kind;

            mod pool_test_fixtures {
                include!("pool_test_fixtures.rs");
            }
            mod batch_tests {
                include!("batch_tests.rs");
            }
            mod compaction_proptests {
                include!("compaction_proptests.rs");
            }
            mod compaction_scale_tests {
                include!("compaction_scale_tests.rs");
            }
            mod compaction_tests {
                include!("compaction_tests.rs");
            }
            mod proptests {
                include!("proptests.rs");
            }
            mod reservation_tests {
                include!("reservation_tests.rs");
            }
            mod sizing_tests {
                include!("sizing_tests.rs");
            }
            mod tests {
                include!("tests.rs");
            }
            mod whole_demand_tests {
                include!("whole_demand_tests.rs");
            }
        }
    )*};
}

pool_suites! {
    lightmap_pool: PoolKind::Lightmap;
    mask_pool: PoolKind::Mask;
}
