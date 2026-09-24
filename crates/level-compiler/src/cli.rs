//! Command-line value parsing and terminal-mode selection.
//! See: context/lib/build_pipeline.md §Progress reporting, controls, and logging

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TuiPreference {
    Auto,
    Force,
    Disable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReporterMode {
    Plain,
    Tui,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TerminalStreams {
    pub(super) stdin: bool,
    pub(super) stdout: bool,
    pub(super) stderr: bool,
}

pub(super) fn select_reporter_mode(
    preference: TuiPreference,
    streams: TerminalStreams,
) -> anyhow::Result<ReporterMode> {
    let all_terminals = streams.stdin && streams.stdout && streams.stderr;
    match (preference, all_terminals) {
        (TuiPreference::Disable, _) => Ok(ReporterMode::Plain),
        (TuiPreference::Force, true) | (TuiPreference::Auto, true) => Ok(ReporterMode::Tui),
        (TuiPreference::Auto, false) => Ok(ReporterMode::Plain),
        (TuiPreference::Force, false) => anyhow::bail!(
            "--tui requires stdin, stdout, and stderr to all be attached to terminals"
        ),
    }
}

pub(super) fn default_jobs_for(logical_cores: usize) -> usize {
    match logical_cores {
        0 | 1 => 1,
        2..=8 => logical_cores - 1,
        _ => logical_cores - 2,
    }
}

pub(super) fn default_jobs() -> usize {
    default_jobs_for(
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1),
    )
}

/// Parse a `--sh-protect-aabb minx,miny,minz,maxx,maxy,maxz` value.
pub(super) fn parse_protect_aabb(spec: &str) -> anyhow::Result<[f32; 6]> {
    let parts: Vec<&str> = spec.split(',').collect();
    if parts.len() != 6 {
        anyhow::bail!(
            "--sh-protect-aabb expects 6 comma-separated numbers \
             (minx,miny,minz,maxx,maxy,maxz), got {}",
            parts.len()
        );
    }
    let mut values = [0.0f32; 6];
    for (index, part) in parts.iter().enumerate() {
        let parsed: f32 = part.trim().parse().map_err(|_| {
            anyhow::anyhow!(
                "--sh-protect-aabb field {} is not a number: {part:?}",
                index + 1
            )
        })?;
        if !parsed.is_finite() {
            anyhow::bail!("--sh-protect-aabb field {} must be finite", index + 1);
        }
        values[index] = parsed;
    }
    for axis in 0..3 {
        if values[axis + 3] < values[axis] {
            anyhow::bail!(
                "--sh-protect-aabb max[{axis}] ({}) must be >= min[{axis}] ({})",
                values[axis + 3],
                values[axis]
            );
        }
    }
    Ok(values)
}
