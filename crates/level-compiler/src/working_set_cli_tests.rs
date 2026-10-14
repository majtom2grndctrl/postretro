// Compiler working-set CLI contracts.
// See: context/lib/build_pipeline.md §Build Cache

use super::*;

#[test]
fn parse_args_sh_delta_working_set_max_size_defaults_to_16_gib() {
    let parsed = parse_args_from(["input.map"].into_iter().map(str::to_owned)).unwrap();

    assert_eq!(
        parsed.delta_section_config.max_working_set_bytes,
        16 * 1024 * 1024 * 1024
    );
}

#[test]
fn parse_args_sh_delta_working_set_max_size_accepts_existing_size_syntax() {
    let parsed = parse_args_from(
        ["input.map", "--sh-delta-working-set-max-size", "24GiB"]
            .into_iter()
            .map(str::to_owned),
    )
    .unwrap();

    assert_eq!(
        parsed.delta_section_config.max_working_set_bytes,
        24 * 1024 * 1024 * 1024
    );
}

#[test]
fn parse_args_sh_delta_working_set_max_size_requires_a_valid_value() {
    assert!(
        parse_args_from(
            ["input.map", "--sh-delta-working-set-max-size"]
                .into_iter()
                .map(str::to_owned),
        )
        .is_err()
    );
    assert!(
        parse_args_from(
            ["input.map", "--sh-delta-working-set-max-size", "12XB"]
                .into_iter()
                .map(str::to_owned),
        )
        .is_err()
    );
}

#[test]
fn parse_args_sh_delta_working_set_max_size_rejects_u64_overflow() {
    for value in ["18446744073709551616B", "16777216TiB"] {
        assert!(
            parse_args_from(
                ["input.map", "--sh-delta-working-set-max-size", value]
                    .into_iter()
                    .map(str::to_owned),
            )
            .is_err(),
            "{value} must not saturate into an admitted u64 budget",
        );
    }
}

