//! Parses human-readable byte-budget options for the level compiler.
//! See: context/lib/build_pipeline.md §CLI flags.

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

    if number.bytes().all(|byte| byte.is_ascii_digit()) {
        if number.is_empty() {
            anyhow::bail!(
                "{option}: '{raw}' is not a valid size (e.g. 2GiB, 512MiB, or a byte count)"
            );
        }
        let significant_digits = number.trim_start_matches('0');
        if significant_digits.is_empty() {
            return Ok(0);
        }
        let value: u128 = significant_digits
            .parse()
            .map_err(|_| anyhow::anyhow!("{option} exceeds the maximum supported size"))?;
        let scaled = value
            .checked_mul(u128::from(multiplier))
            .ok_or_else(|| anyhow::anyhow!("{option} exceeds the maximum supported size"))?;
        if scaled > u128::from(u64::MAX) {
            anyhow::bail!(
                "{option} exceeds the maximum supported size of {} bytes",
                u64::MAX
            );
        }
        return Ok(scaled as u64);
    }

    let (whole, fractional) = number.split_once('.').ok_or_else(|| {
        anyhow::anyhow!(
            "{option}: '{raw}' is not a valid size (e.g. 2GiB, 512MiB, or a byte count)"
        )
    })?;
    if number.matches('.').count() != 1
        || (whole.is_empty() && fractional.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        anyhow::bail!("{option}: '{raw}' is not a valid size (e.g. 2GiB, 512MiB, or a byte count)");
    }

    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole
            .parse()
            .map_err(|_| anyhow::anyhow!("{option} exceeds the maximum supported size"))?
    };
    let (fractional_scaled, has_fractional_remainder) =
        decimal_fraction_scaled(fractional, multiplier);
    let scaled = whole
        .checked_mul(u128::from(multiplier))
        .and_then(|scaled| scaled.checked_add(u128::from(fractional_scaled)))
        .filter(|scaled| *scaled <= u128::from(u64::MAX));
    let Some(scaled) = scaled else {
        anyhow::bail!(
            "{option} exceeds the maximum supported size of {} bytes",
            u64::MAX
        );
    };
    if scaled == u128::from(u64::MAX) && has_fractional_remainder {
        anyhow::bail!(
            "{option} exceeds the maximum supported size of {} bytes",
            u64::MAX
        );
    }

    Ok(scaled as u64)
}

/// Convert a decimal fractional component to bytes without passing through `f64`.
/// Returns the truncated byte count and whether truncation discarded a remainder.
fn decimal_fraction_scaled(fractional: &str, multiplier: u64) -> (u64, bool) {
    debug_assert!(multiplier.is_power_of_two());
    let mut digits = fractional
        .bytes()
        .map(|byte| byte - b'0')
        .collect::<Vec<_>>();
    let mut scaled = 0;

    for _ in 0..multiplier.trailing_zeros() {
        let mut carry = 0;
        for digit in digits.iter_mut().rev() {
            let doubled = *digit * 2 + carry;
            *digit = doubled % 10;
            carry = doubled / 10;
        }
        scaled = (scaled << 1) | u64::from(carry);
    }

    (scaled, digits.iter().any(|&digit| digit != 0))
}

/// Render a byte budget with the largest exact binary unit for compiler help.
pub(crate) fn format_size_for_help(bytes: u64) -> String {
    const TIB: u64 = 1024 * 1024 * 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;

    for (unit, suffix) in [(TIB, "TiB"), (GIB, "GiB"), (MIB, "MiB"), (KIB, "KiB")] {
        if bytes >= unit && bytes % unit == 0 {
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
    fn parse_size_rejects_bare_and_unit_scaled_u64_overflow() {
        // Regression: f64-to-u64 conversion saturated these values to u64::MAX.
        assert!(parse_size("--sh-delta-working-set-max-size", "18446744073709551616B").is_err());
        assert!(parse_size("--sh-delta-working-set-max-size", "16777216TiB").is_err());
        assert!(parse_size("--sh-delta-working-set-max-size", "16777216.0TiB").is_err());
        assert!(parse_size("--sh-delta-working-set-max-size", "18446744073709551615.1B").is_err());
    }

    #[test]
    fn parse_size_accepts_largest_bare_byte_value() {
        assert_eq!(
            parse_size("--sh-delta-working-set-max-size", "18446744073709551615B").unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn parse_size_accepts_largest_decimal_byte_value() {
        assert_eq!(
            parse_size("--sh-delta-working-set-max-size", "18446744073709551615.0B").unwrap(),
            u64::MAX
        );
        assert_eq!(
            parse_size("--sh-delta-working-set-max-size", "18446744073709551614.9B").unwrap(),
            u64::MAX - 1
        );
    }

    #[test]
    fn format_size_for_help_uses_exact_binary_units() {
        assert_eq!(format_size_for_help(2 * 1024 * 1024 * 1024), "2 GiB");
        assert_eq!(format_size_for_help(256 * 1024 * 1024), "256 MiB");
        assert_eq!(format_size_for_help(1_024), "1 KiB");
        assert_eq!(format_size_for_help(12), "12 B");
    }
}
