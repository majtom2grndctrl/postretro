// The `prl-build` command the integration suites bake through, with the
// lightmap reach cull's verify mode on as in every test bake.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

#![allow(dead_code)] // Each suite uses a different subset.

use std::process::Command;

/// `1` turns the reach cull's verify mode on, `0` off. Verify mode reads
/// neither the lightmap layer cache nor the `lightmap_section` memo, so a
/// bake whose test asserts their hits sets it to `0`.
pub const REACH_VERIFY_ENV: &str = "POSTRETRO_LIGHTMAP_REACH_VERIFY";

/// `prl-build` with reach verify mode on, whatever the caller's shell sets.
pub fn prl_build() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
    command.env(REACH_VERIFY_ENV, "1");
    command
}
