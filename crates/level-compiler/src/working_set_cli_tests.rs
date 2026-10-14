// Compiler working-set CLI contracts.
// See: context/lib/build_pipeline.md §Build Cache

use super::*;

#[test]
fn parse_args_working_set_max_size_defaults_to_16_gib() {
    let parsed = parse_args_from(["input.map"].into_iter().map(str::to_owned)).unwrap();

    assert_eq!(parsed.working_set_max_bytes, 16 * 1024 * 1024 * 1024);
}

#[test]
fn parse_args_working_set_max_size_accepts_existing_size_syntax() {
    let parsed = parse_args_from(
        ["input.map", "--working-set-max-size", "24GiB"]
            .into_iter()
            .map(str::to_owned),
    )
    .unwrap();

    assert_eq!(parsed.working_set_max_bytes, 24 * 1024 * 1024 * 1024);
}

#[test]
fn parse_args_working_set_max_size_requires_a_valid_value() {
    assert!(
        parse_args_from(
            ["input.map", "--working-set-max-size"]
                .into_iter()
                .map(str::to_owned),
        )
        .is_err()
    );
    assert!(
        parse_args_from(
            ["input.map", "--working-set-max-size", "12XB"]
                .into_iter()
                .map(str::to_owned),
        )
        .is_err()
    );
}

#[test]
fn parse_args_working_set_max_size_rejects_u64_overflow() {
    for value in ["18446744073709551616B", "16777216TiB"] {
        assert!(
            parse_args_from(
                ["input.map", "--working-set-max-size", value]
                    .into_iter()
                    .map(str::to_owned),
            )
            .is_err(),
            "{value} must not saturate into an admitted u64 budget",
        );
    }
}

#[test]
fn retired_delta_only_working_set_flag_is_unknown() {
    let retired = ["--sh-delta", "working-set-max-size"].join("-");
    // Regression: an unknown option before the input became its path.
    for args in [
        vec![retired.as_str(), "16GiB", "input.map"],
        vec!["input.map", retired.as_str(), "16GiB"],
        vec![retired.as_str()],
    ] {
        let error = parse_args_from(args.into_iter().map(str::to_owned))
            .expect_err("retired flag must be rejected");
        assert_eq!(error.to_string(), format!("unexpected argument: {retired}"));
    }
}
