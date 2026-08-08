//! Parses human-readable byte-budget options for the level compiler.
//! See: context/lib/build_pipeline.md §Build Cache.

/// Parse a byte-budget CLI value. Accepts a plain integer (bytes) or a decimal
/// value with a binary unit suffix: `B`, `KiB`, `MiB`, `GiB`, `TiB`
/// (case-insensitive; a bare `K`/`M`/`G`/`T` is treated as the binary unit).
pub(crate) fn parse_size(option: &str, raw: &str) -> anyhow::Result<u64> {
    let value_text = raw.trim();
    if value_text.is_empty() {
        anyhow::bail!("{option} requires a value");
    }

    let split = value_text
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(value_text.len());
    let (number, unit) = value_text.split_at(split);
    let value: f64 = number.parse().map_err(|_| {
        anyhow::anyhow!(
            "{option}: '{raw}' is not a valid size (e.g. 2GiB, 512MiB, or a byte count)"
        )
    })?;
    if !value.is_finite() || value < 0.0 {
        anyhow::bail!("{option} must be a non-negative size");
    }

    let multiplier: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kib" => 1024,
        "m" | "mib" => 1024 * 1024,
        "g" | "gib" => 1024 * 1024 * 1024,
        "t" | "tib" => 1024u64 * 1024 * 1024 * 1024,
        other => {
            anyhow::bail!("{option}: unknown unit '{other}' (use B, KiB, MiB, GiB, or TiB)")
        }
    };

    Ok((value * multiplier as f64) as u64)
}

/// Render a byte budget with the largest exact binary unit for compiler help.
pub(crate) fn format_size_for_help(bytes: u64) -> String {
    const TIB: u64 = 1024 * 1024 * 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;

    for (unit, suffix) in [(TIB, "TiB"), (GIB, "GiB"), (MIB, "MiB"), (KIB, "KiB")] {
        if bytes >= unit && bytes.is_multiple_of(unit) {
            return format!("{} {suffix}", bytes / unit);
        }
    }
    format!("{bytes} B")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_handles_units_and_bytes() {
        assert_eq!(
            parse_size("--cache-max-size", "2147483648").unwrap(),
            2 * 1024 * 1024 * 1024
        );
        assert_eq!(
            parse_size("--cache-max-size", "2GiB").unwrap(),
            2 * 1024 * 1024 * 1024
        );
        assert_eq!(
            parse_size("--cache-max-size", "2gib").unwrap(),
            2 * 1024 * 1024 * 1024
        );
        assert_eq!(
            parse_size("--cache-max-size", "1536MiB").unwrap(),
            1536 * 1024 * 1024
        );
        assert_eq!(
            parse_size("--cache-max-size", "1.5GiB").unwrap(),
            1536 * 1024 * 1024
        );
        assert_eq!(
            parse_size("--cache-max-size", "4G").unwrap(),
            4u64 * 1024 * 1024 * 1024
        );
        assert_eq!(parse_size("--cache-max-size", "0").unwrap(), 0);
    }

    #[test]
    fn parse_size_rejects_garbage_and_unknown_units() {
        assert!(parse_size("--cache-max-size", "").is_err());
        assert!(parse_size("--cache-max-size", "abc").is_err());
        assert!(parse_size("--cache-max-size", "12XB").is_err());
        assert!(parse_size("--cache-max-size", "-5GiB").is_err());
    }

    #[test]
    fn format_size_for_help_uses_exact_binary_units() {
        assert_eq!(format_size_for_help(2 * 1024 * 1024 * 1024), "2 GiB");
        assert_eq!(format_size_for_help(256 * 1024 * 1024), "256 MiB");
        assert_eq!(format_size_for_help(1_024), "1 KiB");
        assert_eq!(format_size_for_help(12), "12 B");
    }
}
