use super::*;
use crate::{
    CompiledRuleSet, ContextValue, EvaluationError, InputRevision, InputSnapshotError,
    OfficialPackageVersion, RawFieldValue, RuleSetId, Sha256Digest,
};

const EMPTY_NORMALIZATION: &[NormalizationStep] = &[];
const SAVE: &[ValidationPhase] = &[ValidationPhase::Save];
const VALIDATE: &[ValidationPhase] = &[ValidationPhase::Validate];
const SAVE_AND_VALIDATE: &[ValidationPhase] = &[ValidationPhase::Save, ValidationPhase::Validate];
const TRUE: Predicate = Predicate::Constant(true);

fn leaked<T>(value: T) -> &'static T {
    Box::leak(Box::new(value))
}

fn leaked_slice<T>(values: Vec<T>) -> &'static [T] {
    Box::leak(values.into_boxed_slice())
}

fn blocking_rule(
    rule_id: &'static str,
    order: u32,
    phases: &'static [ValidationPhase],
    message: &'static str,
) -> RuleSpec {
    let effects = leaked_slice(vec![Effect::EmitIssue {
        severity: RuleSeverity::Blocking,
        message,
        official_message: Some(message),
        assessment: RuleAssessment::VerifiedCorrect,
        fields: &[],
    }]);
    RuleSpec {
        rule_id,
        scope: EvaluationScope::Singleton,
        order,
        phases,
        trigger_field_ids: &[],
        profiles: executable(RuleBranch {
            predicate: &TRUE,
            effects,
        }),
    }
}

fn executable<T: Copy>(value: T) -> Profiled<Branch<T>> {
    Profiled {
        official: Branch::Executable(value),
        filing_safe: Branch::Executable(value),
    }
}

#[test]
fn javascript_parse_int_radix10_matches_prefix_and_whitespace_semantics() {
    let parse = |value: &str| {
        evaluate_javascript_parse_int_radix10(CanonicalValue::Text(value.to_owned()))
            .expect("bounded JavaScript integer")
    };

    assert_eq!(parse("  +2025suffix"), CanonicalValue::Integer(2025));
    assert_eq!(parse("\u{feff}-0012/ignored"), CanonicalValue::Integer(-12));
    assert_eq!(parse("0x10"), CanonicalValue::Integer(0));
    assert_eq!(parse("12.99"), CanonicalValue::Integer(12));
    assert_eq!(parse("+"), CanonicalValue::Absent);
    assert_eq!(parse("\u{180e}2025"), CanonicalValue::Absent);
    assert_eq!(parse(&"9".repeat(10_000)), CanonicalValue::Absent);
    assert_eq!(
        evaluate_javascript_parse_int_radix10(CanonicalValue::Blank)
            .expect("blank is parseInt NaN"),
        CanonicalValue::Absent
    );
}

#[test]
fn javascript_parse_float_matches_longest_prefix_and_ieee_754_categories() {
    assert!(javascript_parse_float("").is_nan());
    assert!(javascript_parse_float("\u{feff}  ").is_nan());
    assert!(javascript_parse_float("+").is_nan());
    assert!(javascript_parse_float("NaN").is_nan());
    assert!(javascript_parse_float("\u{180e}1.5").is_nan());
    assert_eq!(javascript_parse_float("  +1.25suffix"), 1.25);
    assert_eq!(javascript_parse_float(".5e2/ignored"), 50.0);
    assert_eq!(javascript_parse_float("1."), 1.0);
    assert_eq!(javascript_parse_float("1.e2tail"), 100.0);
    assert_eq!(javascript_parse_float("1e"), 1.0);
    assert_eq!(javascript_parse_float("1e+"), 1.0);
    assert_eq!(javascript_parse_float("0x10"), 0.0);
    assert_eq!(javascript_parse_float("1,000.00"), 1.0);
    assert_eq!(javascript_parse_float("+Infinitytail"), f64::INFINITY);
    assert_eq!(javascript_parse_float("-Infinitytail"), f64::NEG_INFINITY);
    assert_eq!(
        javascript_parse_float("1.7976931348623159e308"),
        f64::INFINITY
    );
    let negative_zero = javascript_parse_float("-1e-9999");
    assert_eq!(negative_zero, 0.0);
    assert!(negative_zero.is_sign_negative());
}

#[test]
fn javascript_parse_float_predicate_preserves_nan_zero_and_infinity_comparisons() {
    let zero = DecimalLiteral {
        coefficient: 0,
        scale: 0,
    };
    let thousand = DecimalLiteral {
        coefficient: 1000,
        scale: 0,
    };
    let evaluate = |operator, input: &str, operand| {
        evaluate_javascript_parse_float_predicate(
            operator,
            CanonicalValue::Text(input.to_owned()),
            operand,
        )
        .expect("well-typed JavaScript parseFloat predicate")
    };

    assert!(evaluate(JavaScriptParseFloatOperator::IsNaN, "", None));
    assert!(!evaluate(
        JavaScriptParseFloatOperator::IsNaN,
        "Infinity",
        None
    ));
    assert!(evaluate(
        JavaScriptParseFloatOperator::StrictEqual,
        "-0",
        Some(zero)
    ));
    assert!(!evaluate(
        JavaScriptParseFloatOperator::StrictEqual,
        "not-a-number",
        Some(zero)
    ));
    assert!(evaluate(
        JavaScriptParseFloatOperator::GreaterThan,
        "Infinity",
        Some(thousand)
    ));
    assert!(!evaluate(
        JavaScriptParseFloatOperator::GreaterThan,
        "-Infinity",
        Some(zero)
    ));
}

#[test]
fn javascript_global_number_grammar_is_closed_and_exact() {
    let exact_integer = |value: i128| LegacyJavaScriptNumber::Finite {
        coefficient: BigInt::from(value),
        decimal_exponent: BigInt::from(0_u8),
    };
    let compare = |input: &str, expected: LegacyJavaScriptNumber| {
        legacy_javascript_number(input).compare(&expected)
    };

    for input in [
        "+0x10",
        "-0x10",
        "0x",
        "0xg",
        "0b10",
        "0o10",
        "1suffix",
        "1e",
        "NaN",
        "\u{180e}1",
    ] {
        assert!(
            matches!(legacy_javascript_number(input), LegacyJavaScriptNumber::NaN),
            "{input:?} must be NaN"
        );
    }
    assert_eq!(compare("0x10", exact_integer(16)), Some(Ordering::Equal));
    assert_eq!(compare("0Xff", exact_integer(255)), Some(Ordering::Equal));
    assert_eq!(
        compare("\u{feff} +2.5e2 \u{3000}", exact_integer(250)),
        Some(Ordering::Equal)
    );
    assert_eq!(
        compare("9007199254740993", exact_integer(9_007_199_254_740_993)),
        Some(Ordering::Equal),
        "the reviewed abstraction must not round through binary64"
    );
    assert_eq!(
        compare("1e-100000000000000000000", exact_integer(0)),
        Some(Ordering::Greater)
    );
    assert_eq!(
        legacy_javascript_number("-Infinity").compare(&exact_integer(-1)),
        Some(Ordering::Less)
    );
    assert_eq!(
        legacy_javascript_number("+Infinity").compare(&exact_integer(1)),
        Some(Ordering::Greater)
    );
    assert_eq!(
        legacy_javascript_number("not-a-number").compare(&exact_integer(0)),
        None
    );
}

#[test]
fn offline_ebir_money_round_v1_matches_hash_pinned_finite_observations() {
    let cases = [
        ("", "0.00"),
        (" ", "0.00"),
        ("0", "0.00"),
        ("0.00", "0.00"),
        ("12x", "0.00"),
        ("1,234.50", "1,234.50"),
        ("$1,234.50", "1,234.50"),
        ("1,,2", "12.00"),
        (",0", "0.00"),
        ("1e3", "1,000.00"),
        ("1e12", "1,000,000,000,000.00"),
        ("1e20", "100,000,000,000,000,000,000.00"),
        ("1e21", "999,999,999,999,999,900,000.92"),
        ("1e", "0.00"),
        ("0x10", "16.00"),
        ("123456789012", "123,456,789,012.00"),
        ("1234567890123", "0.00"),
        ("-12345678901", "-12,345,678,901.00"),
        ("-123456789012", "0.00"),
        (".495", "0.50"),
        ("-.495", "-0.50"),
        ("0.0049999999998999999999", "0.01"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            offline_ebir_money_round_v1(input).unwrap(),
            expected,
            "input {input:?}"
        );
    }

    for input in [
        "Infinity",
        "-Infinity",
        "+Infinity",
        "1e37",
        "9e99",
        "1e308",
    ] {
        assert!(matches!(
            offline_ebir_money_round_v1(input),
            Err(InterpreterError::Overflow {
                operation: ExecutionOperation::NormalizeField,
            })
        ));
    }
    assert_eq!(
        offline_ebir_money_round_v1(&"1".repeat(26)).unwrap(),
        "0.00"
    );
    assert!(matches!(
        offline_ebir_money_round_v1(&"1".repeat(4_097)),
        Err(InterpreterError::Overflow {
            operation: ExecutionOperation::NormalizeField,
        })
    ));
}

#[test]
fn offline_ebir_parse_float_fixed_zero_v1_matches_hash_pinned_observations() {
    let hundred_digits = format!("1{}", "0".repeat(99));
    let cases = [
        ("", ""),
        (" ", ""),
        ("not-a-number", ""),
        ("12x", "12"),
        ("1e3", "1000"),
        ("0x10", "0"),
        (".5", "1"),
        ("-.5", "-1"),
        ("-0.1", "-0"),
        ("0.49999999999999994", "0"),
        ("0.49999999999999999", "1"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            offline_ebir_parse_float_fixed_zero_v1(input).unwrap(),
            expected,
            "input {input:?}"
        );
    }
    assert_eq!(
        offline_ebir_parse_float_fixed_zero_v1("1e99").unwrap(),
        hundred_digits
    );
    for input in ["Infinity", "-Infinity", "+Infinity", "1e999"] {
        assert!(matches!(
            offline_ebir_parse_float_fixed_zero_v1(input),
            Err(InterpreterError::Overflow {
                operation: ExecutionOperation::NormalizeField,
            })
        ));
    }
    assert!(matches!(
        offline_ebir_parse_float_fixed_zero_v1(&"1".repeat(4_097)),
        Err(InterpreterError::Overflow {
            operation: ExecutionOperation::NormalizeField,
        })
    ));
}

#[test]
fn javascript_global_is_nan_logical_or_uses_javascript_truthiness_and_final_falsy_value() {
    let selected = |values: Vec<CanonicalValue>| {
        select_javascript_logical_or_value(
            &values,
            ExecutionOperation::JavaScriptGlobalIsNaNLogicalOr,
        )
        .expect("string/null logical-or inputs")
        .clone()
    };

    assert_eq!(
        selected(vec![
            CanonicalValue::Text(String::new()),
            CanonicalValue::Absent,
            CanonicalValue::Text("bad".to_owned()),
            CanonicalValue::Text("10".to_owned()),
        ]),
        CanonicalValue::Text("bad".to_owned())
    );
    assert_eq!(
        selected(vec![
            CanonicalValue::Text(String::new()),
            CanonicalValue::Text("   ".to_owned()),
            CanonicalValue::Text("bad".to_owned()),
        ]),
        CanonicalValue::Text("   ".to_owned()),
        "a nonempty whitespace string is truthy even though Number(string) is zero"
    );
    assert_eq!(
        selected(vec![
            CanonicalValue::Text(String::new()),
            CanonicalValue::Blank,
            CanonicalValue::Absent,
        ]),
        CanonicalValue::Absent
    );
}

#[test]
fn javascript_number_compare_orders_exact_values_and_rejects_nan() {
    let evaluate = |operator, input: CanonicalValue, operand: CanonicalValue| {
        evaluate_javascript_number_compare(operator, input, operand)
            .expect("well-typed JavaScript Number comparison")
    };

    assert!(evaluate(
        JavaScriptNumberCompareOperator::StrictEqual,
        CanonicalValue::Text("2.5e2".to_owned()),
        CanonicalValue::Integer(250),
    ));
    assert!(evaluate(
        JavaScriptNumberCompareOperator::LessThan,
        CanonicalValue::Text("2024".to_owned()),
        CanonicalValue::Integer(2025),
    ));
    assert!(evaluate(
        JavaScriptNumberCompareOperator::GreaterThan,
        CanonicalValue::Text("Infinity".to_owned()),
        CanonicalValue::Decimal("999999999.99".parse().unwrap()),
    ));
    assert!(evaluate(
        JavaScriptNumberCompareOperator::StrictEqual,
        CanonicalValue::Absent,
        CanonicalValue::Integer(0),
    ));
    for operator in [
        JavaScriptNumberCompareOperator::LessThan,
        JavaScriptNumberCompareOperator::GreaterThan,
        JavaScriptNumberCompareOperator::StrictEqual,
    ] {
        assert!(!evaluate(
            operator,
            CanonicalValue::Text("0b10".to_owned()),
            CanonicalValue::Integer(2),
        ));
    }
}

#[test]
fn javascript_number_predicates_execute_with_string_fields_and_integer_context() {
    let logical_inputs = leaked_slice(vec![
        Expression::Field {
            result_type: ValueType::String,
            field: FieldRef {
                field_id: "first",
                instance: FieldInstanceSelector::Singleton,
            },
        },
        Expression::Field {
            result_type: ValueType::String,
            field: FieldRef {
                field_id: "second",
                instance: FieldInstanceSelector::Singleton,
            },
        },
    ]);
    let year_input = leaked(Expression::Field {
        result_type: ValueType::String,
        field: FieldRef {
            field_id: "year-text",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    let current_year = leaked(Expression::Context {
        result_type: ValueType::Integer,
        context_value_id: "current-calendar-year",
    });
    let issue = |message: &'static str| {
        leaked_slice(vec![Effect::EmitIssue {
            severity: RuleSeverity::Advisory,
            message,
            official_message: Some(message),
            assessment: RuleAssessment::VerifiedCorrect,
            fields: &[],
        }])
    };
    let rules = leaked_slice(vec![
        RuleSpec {
            rule_id: "logical-or-is-nan",
            scope: EvaluationScope::Singleton,
            order: 1,
            phases: VALIDATE,
            trigger_field_ids: &[],
            profiles: executable(RuleBranch {
                predicate: leaked(Predicate::JavaScriptGlobalIsNaNLogicalOr {
                    inputs: logical_inputs,
                }),
                effects: issue("logical-or selected invalid number"),
            }),
        },
        RuleSpec {
            rule_id: "year-before-current",
            scope: EvaluationScope::Singleton,
            order: 2,
            phases: VALIDATE,
            trigger_field_ids: &[],
            profiles: executable(RuleBranch {
                predicate: leaked(Predicate::JavaScriptNumberCompare {
                    operator: JavaScriptNumberCompareOperator::LessThan,
                    input: year_input,
                    operand: current_year,
                }),
                effects: issue("year is before current year"),
            }),
        },
    ]);
    let specification = spec(
        leaked_slice(vec![ContextValueSpec {
            context_value_id: "current-calendar-year",
            value_type: ValueType::Integer,
            required: true,
        }]),
        &[],
        leaked_slice(vec![
            string_field("first"),
            string_field("second"),
            string_field("year-text"),
        ]),
        &[],
        &[],
        rules,
        all_effects(),
    );
    let result = evaluator(specification)
        .evaluate(&request(
            ValidationContext::new(
                ValidationPhase::Validate,
                BehaviorProfile::OfficialCompatibility,
            ),
            vec![ContextValue::new(
                ContextValueId::parse("current-calendar-year").unwrap(),
                CanonicalValue::Integer(2025),
            )],
            vec![],
            vec![
                raw_singleton("first", RawValue::Text(String::new())),
                raw_singleton("second", RawValue::Text("invalid".to_owned())),
                raw_singleton("year-text", RawValue::Text("2024".to_owned())),
            ],
        ))
        .unwrap();

    assert_eq!(result.report().violations().len(), 2);
    assert_eq!(
        result.report().violations()[0].message(),
        "logical-or selected invalid number"
    );
    assert_eq!(
        result.report().violations()[1].message(),
        "year is before current year"
    );
}

#[test]
fn javascript_number_predicates_reject_non_string_inputs_and_non_numeric_operands() {
    let invalid_predicate = leaked(Predicate::JavaScriptNumberCompare {
        operator: JavaScriptNumberCompareOperator::StrictEqual,
        input: leaked(Expression::Literal(TypedValue::String("2025"))),
        operand: leaked(Expression::Literal(TypedValue::String("2025"))),
    });
    let invalid_rule = RuleSpec {
        rule_id: "invalid-number-operand",
        scope: EvaluationScope::Singleton,
        order: 1,
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(RuleBranch {
            predicate: invalid_predicate,
            effects: leaked_slice(vec![Effect::EmitIssue {
                severity: RuleSeverity::Advisory,
                message: "invalid",
                official_message: None,
                assessment: RuleAssessment::VerifiedCorrect,
                fields: &[],
            }]),
        }),
    };
    let specification = spec(
        &[],
        &[],
        &[],
        &[],
        &[],
        leaked_slice(vec![invalid_rule]),
        all_effects(),
    );

    assert!(matches!(
        validate_static_spec(specification),
        Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::TypeMismatch {
                operation: ExecutionOperation::JavaScriptNumberCompare,
                ..
            }
        ))
    ));

    for (predicate, expected_empty) in [
        (
            Predicate::JavaScriptGlobalIsNaNLogicalOr { inputs: &[] },
            true,
        ),
        (
            Predicate::JavaScriptGlobalIsNaNLogicalOr {
                inputs: leaked_slice(vec![Expression::Literal(TypedValue::Integer(1))]),
            },
            false,
        ),
    ] {
        let rule = RuleSpec {
            rule_id: if expected_empty {
                "empty-logical-or"
            } else {
                "numeric-logical-or"
            },
            scope: EvaluationScope::Singleton,
            order: 1,
            phases: VALIDATE,
            trigger_field_ids: &[],
            profiles: executable(RuleBranch {
                predicate: leaked(predicate),
                effects: leaked_slice(vec![Effect::EmitIssue {
                    severity: RuleSeverity::Advisory,
                    message: "invalid",
                    official_message: None,
                    assessment: RuleAssessment::VerifiedCorrect,
                    fields: &[],
                }]),
            }),
        };
        let specification = spec(
            &[],
            &[],
            &[],
            &[],
            &[],
            leaked_slice(vec![rule]),
            all_effects(),
        );
        let error = validate_static_spec(specification)
            .expect_err("invalid JavaScript logical-or shape must fail");
        if expected_empty {
            assert!(matches!(
                error,
                InterpreterError::InvalidStaticSpec(StaticSpecError::EmptyRequiredList {
                    value: "javascript-global-is-nan-logical-or",
                    ..
                })
            ));
        } else {
            assert!(matches!(
                error,
                InterpreterError::InvalidStaticSpec(StaticSpecError::TypeMismatch {
                    operation: ExecutionOperation::JavaScriptGlobalIsNaNLogicalOr,
                    ..
                })
            ));
        }
    }
}

#[test]
fn offline_ebir_tin_v1_matches_pinned_helper_cases_and_legacy_interval() {
    let valid = |value: &str| {
        evaluate_checksum_predicate(
            ChecksumAlgorithm::OfflineEbirTinV1,
            CanonicalValue::Text(value.to_owned()),
        )
        .expect("string checksum input")
    };

    for value in [
        "000000000",
        "100000006",
        "010000008",
        "001000009",
        "000100004",
        "000010003",
        "000001001",
        "000000105",
        "000000018",
        "123456788",
    ] {
        assert!(valid(value), "expected helper-compatible TIN {value}");
    }
    for value in [
        "",
        "12345678",
        "12345678x",
        "123456787",
        "999999999",
        "900000009",
        "900000019",
        "900000080",
        "905179999",
        "905180008",
        "+00000005",
        "-00000005",
    ] {
        assert!(!valid(value), "expected rejected TIN {value}");
    }

    // After a direct mismatch, unsigned values beginning with 9 inside the
    // strict legacy interval retry n - 1 for a nonzero suffix and n + 9 for a
    // zero suffix. The endpoints still use only the direct path.
    for value in [
        "900000005",
        "900000070",
        "900000100",
        "904999999",
        "905180000",
    ] {
        assert!(valid(value), "expected legacy-retry TIN {value}");
    }
    assert!(valid("900000004"));
    assert!(valid("905180009"));

    // The helper's integer gate accepts a sign plus eight digits. Its
    // per-character conversion turns either sign into digit one; signed
    // inputs never enter the unsigned retry branch.
    assert!(valid("+00000006"));
    assert!(valid("-00000006"));
}

#[test]
fn checksum_predicate_is_typed_and_treats_absence_as_not_valid() {
    assert!(
        !evaluate_checksum_predicate(ChecksumAlgorithm::OfflineEbirTinV1, CanonicalValue::Absent,)
            .expect("absence is an invalid checksum value, not an interpreter failure")
    );
    assert!(matches!(
        evaluate_checksum_predicate(
            ChecksumAlgorithm::OfflineEbirTinV1,
            CanonicalValue::Integer(123456788),
        ),
        Err(InterpreterError::TypeMismatch {
            operation: ExecutionOperation::Checksum,
            expected: ValueType::String,
            actual: ValueKind::Integer,
        })
    ));
}

#[test]
fn split_component_preserves_empty_parts_and_marks_missing_parts_absent() {
    let split = |value: CanonicalValue, index| {
        evaluate_split_component(value, "/", index).expect("string split")
    };

    assert_eq!(
        split(CanonicalValue::Text("11//2024/extra".to_owned()), 1),
        CanonicalValue::Text(String::new())
    );
    assert_eq!(
        split(CanonicalValue::Text("11/30/2024/extra".to_owned()), 2),
        CanonicalValue::Text("2024".to_owned())
    );
    assert_eq!(
        split(CanonicalValue::Text("11/30".to_owned()), 2),
        CanonicalValue::Absent
    );
    assert_eq!(
        split(CanonicalValue::Blank, 0),
        CanonicalValue::Text(String::new())
    );
}

#[test]
fn javascript_date_local_day_normalizes_constructor_components_and_legacy_years() {
    let js_day = |year, month_index, day| {
        evaluate_javascript_date_local_day(
            CanonicalValue::Integer(year),
            CanonicalValue::Integer(month_index),
            CanonicalValue::Integer(day),
        )
        .expect("bounded JavaScript date")
    };
    let canonical_day = |year, month, day| {
        evaluate_canonical_local_date_day(CanonicalValue::Date(
            CanonicalDate::try_new(year, month, day).expect("canonical date"),
        ))
        .expect("canonical ordinal")
    };

    assert_eq!(js_day(2024, 12, 1), canonical_day(2025, 1, 1));
    assert_eq!(js_day(2025, 0, 0), canonical_day(2024, 12, 31));
    assert_eq!(js_day(2024, 1, 30), canonical_day(2024, 3, 1));
    assert_eq!(js_day(24, 0, 1), canonical_day(1924, 1, 1));
    assert_eq!(js_day(0, 0, 1), canonical_day(1900, 1, 1));
    assert_eq!(
        evaluate_javascript_date_local_day(
            CanonicalValue::Absent,
            CanonicalValue::Integer(0),
            CanonicalValue::Integer(1),
        )
        .expect("Invalid Date is represented as absent"),
        CanonicalValue::Absent
    );
}

fn profile_status() -> Profiled<Branch<()>> {
    executable(())
}

fn string_field(field_id: &'static str) -> FieldSpec {
    FieldSpec {
        field_id,
        value_type: ValueType::String,
        group_id: None,
        calculation_id: None,
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: &[],
            coercion: Coercion::String {
                on_empty: StringEmptyPolicy::Null,
            },
        }),
    }
}

fn grouped_string_field(field_id: &'static str, group_id: &'static str) -> FieldSpec {
    FieldSpec {
        field_id,
        value_type: ValueType::String,
        group_id: Some(group_id),
        calculation_id: None,
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: &[],
            coercion: Coercion::String {
                on_empty: StringEmptyPolicy::Null,
            },
        }),
    }
}

fn decimal_policy() -> DecimalPolicy {
    DecimalPolicy {
        precision: 38,
        scale: 18,
        division_scale: 18,
        rounding: Rounding {
            mode: RoundingMode::None,
            scale: 18,
        },
        overflow: OverflowPolicy::Error,
    }
}

fn division_policy(scale: u32, rounding: RoundingMode) -> DecimalDivisionPolicy {
    DecimalDivisionPolicy { scale, rounding }
}

fn decimal_literal(coefficient: i128, scale: u32) -> &'static Expression {
    leaked(Expression::Literal(TypedValue::Decimal(DecimalLiteral {
        coefficient,
        scale,
    })))
}

fn decimal_field(field_id: &'static str, group_id: Option<&'static str>) -> FieldSpec {
    FieldSpec {
        field_id,
        value_type: ValueType::Decimal,
        group_id,
        calculation_id: None,
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: &[],
            coercion: Coercion::Decimal {
                decimal: decimal_policy(),
                grouping: InputGrouping::Forbidden,
                on_empty: NumericEmptyPolicy::Null,
                on_invalid: InvalidValuePolicy::Error,
            },
        }),
    }
}

fn integer_field(field_id: &'static str, group_id: Option<&'static str>) -> FieldSpec {
    FieldSpec {
        field_id,
        value_type: ValueType::Integer,
        group_id,
        calculation_id: None,
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: &[],
            coercion: Coercion::Integer {
                on_empty: NumericEmptyPolicy::Null,
                on_invalid: InvalidValuePolicy::Error,
            },
        }),
    }
}

fn spec(
    context_values: &'static [ContextValueSpec],
    field_groups: &'static [FieldGroupSpec],
    fields: &'static [FieldSpec],
    evaluation_order: &'static [&'static str],
    calculations: &'static [CalculationSpec],
    rules: &'static [RuleSpec],
    effect_mode: Profiled<Branch<EffectEvaluationMode>>,
) -> &'static StaticRuleSetSpec {
    leaked(StaticRuleSetSpec {
        profile_status: profile_status(),
        effect_mode,
        serialization: &crate::StaticSerializationContract::EMPTY_V1,
        context_values,
        field_groups,
        fields,
        field_event_programs: &[],
        evaluation_order,
        calculations,
        rules,
        workflow: Branch::Unresolved,
    })
}

fn spec_with_field_event_programs(
    field_groups: &'static [FieldGroupSpec],
    fields: &'static [FieldSpec],
    evaluation_order: &'static [&'static str],
    calculations: &'static [CalculationSpec],
    rules: &'static [RuleSpec],
    field_event_programs: &'static [FieldEventProgramSpec],
    effect_mode: Profiled<Branch<EffectEvaluationMode>>,
) -> &'static StaticRuleSetSpec {
    leaked(StaticRuleSetSpec {
        profile_status: profile_status(),
        effect_mode,
        serialization: &crate::StaticSerializationContract::EMPTY_V1,
        context_values: &[],
        field_groups,
        fields,
        field_event_programs,
        evaluation_order,
        calculations,
        rules,
        workflow: Branch::Unresolved,
    })
}

fn all_effects() -> Profiled<Branch<EffectEvaluationMode>> {
    executable(EffectEvaluationMode::ApplyAll)
}

fn profiled_effects(
    official: EffectEvaluationMode,
    filing_safe: EffectEvaluationMode,
) -> Profiled<Branch<EffectEvaluationMode>> {
    Profiled {
        official: Branch::Executable(official),
        filing_safe: Branch::Executable(filing_safe),
    }
}

fn unavailable_filing_safe_effects(
    official: EffectEvaluationMode,
) -> Profiled<Branch<EffectEvaluationMode>> {
    Profiled {
        official: Branch::Executable(official),
        filing_safe: Branch::Unresolved,
    }
}

fn identity() -> FormRevisionKey {
    FormRevisionKey::new(
        RuleSetId::parse("static-test-v1-p1").unwrap(),
        crate::FormCode::parse("TEST").unwrap(),
        crate::FormRevision::parse("v1").unwrap(),
        OfficialPackageVersion::parse("p1").unwrap(),
        Sha256Digest::from_bytes([42; 32]),
    )
}

fn request(
    context: ValidationContext,
    context_values: Vec<ContextValue>,
    groups: Vec<RepeatedGroupInstance>,
    fields: Vec<RawFieldValue>,
) -> EvaluationRequest {
    EvaluationRequest::try_new(
        identity(),
        context,
        InputRevision::new(1),
        context_values,
        groups,
        fields,
    )
    .unwrap()
}

fn field_event_request(
    context: ValidationContext,
    event_field: FieldInstance,
    groups: Vec<RepeatedGroupInstance>,
    fields: Vec<RawFieldValue>,
) -> EvaluationRequest {
    EvaluationRequest::try_new_for_field_event(
        identity(),
        context,
        InputRevision::new(1),
        event_field,
        Vec::new(),
        groups,
        fields,
    )
    .unwrap()
}

fn raw_singleton(field_id: &str, value: RawValue) -> RawFieldValue {
    RawFieldValue::new(
        FieldInstance::singleton(FieldId::parse(field_id).unwrap()),
        value,
    )
}

fn evaluator(spec: &'static StaticRuleSetSpec) -> StaticCompiledRuleSet {
    StaticCompiledRuleSet::new(identity(), spec)
}

fn spec_with_serialization(
    serialization: &'static crate::StaticSerializationContract,
    field_groups: &'static [FieldGroupSpec],
    fields: &'static [FieldSpec],
) -> &'static StaticRuleSetSpec {
    leaked(StaticRuleSetSpec {
        profile_status: profile_status(),
        effect_mode: all_effects(),
        serialization,
        context_values: &[],
        field_groups,
        fields,
        field_event_programs: &[],
        evaluation_order: &[],
        calculations: &[],
        rules: &[],
        workflow: Branch::Unresolved,
    })
}

fn text_semantic(
    absent: crate::serialization::AbsentValuePolicy,
) -> crate::serialization_contract::SerializationSemanticFormat {
    crate::serialization_contract::SerializationSemanticFormat {
        absent,
        blank: crate::serialization::BlankValuePolicy::EmitEmptyBody,
        present: crate::serialization_contract::SerializationPresentFormat::Text,
    }
}

fn artifact_identity(
    target: crate::serialization::SerializationArtifactTarget,
    variant: &str,
) -> crate::serialization::SerializationArtifactIdentity {
    crate::serialization::SerializationArtifactIdentity::new(
        target,
        crate::serialization::ArtifactVariantId::parse(variant).unwrap(),
    )
}

fn contract(
    target: crate::serialization::SerializationArtifactTarget,
    variant: &'static str,
    digest: Option<&'static str>,
    branches: Profiled<Branch<crate::serialization_contract::SerializationPlan>>,
) -> &'static crate::StaticSerializationContract {
    leaked(crate::StaticSerializationContract {
        contract_version: "1.0.0",
        canonical_sha256: digest,
        artifacts: leaked_slice(vec![
            crate::serialization_contract::SerializationArtifactSpec {
                artifact_id: "artifact",
                target,
                variant_id: variant,
                branches,
            },
        ]),
    })
}

fn pseudo_node(
    ordinal: u32,
    key: &'static str,
    occurrence: u32,
    field: &'static str,
    absent: crate::serialization::AbsentValuePolicy,
    presence: crate::serialization_contract::SerializationPresence,
) -> crate::serialization_contract::SerializationNode {
    crate::serialization_contract::SerializationNode::PseudoXmlField(
        crate::serialization_contract::PseudoXmlFieldNode {
            ordinal,
            key_projection: crate::serialization_contract::SerializationKeyProjection::Exact(key),
            occurrence_projection:
                crate::serialization_contract::SerializationOccurrenceProjection::Fixed(occurrence),
            value_projection: crate::serialization_contract::SerializationValueProjection::Field(
                FieldRef {
                    field_id: field,
                    instance: FieldInstanceSelector::Singleton,
                },
            ),
            semantic_format: text_semantic(absent),
            body_codec: crate::serialization::BodyCodec::RawLiteral,
            presence,
        },
    )
}

const CONTRACT_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn materialization_selects_exact_artifact_and_preserves_semantic_omission_and_fanout() {
    let nodes = leaked_slice(vec![
        pseudo_node(
            1,
            "Name",
            1,
            "name",
            crate::serialization::AbsentValuePolicy::OmitOccurrence,
            crate::serialization_contract::SerializationPresence::Always,
        ),
        pseudo_node(
            2,
            "NameCopy",
            1,
            "name",
            crate::serialization::AbsentValuePolicy::OmitOccurrence,
            crate::serialization_contract::SerializationPresence::When(Predicate::Constant(false)),
        ),
    ]);
    let plan = crate::serialization_contract::SerializationPlan { nodes };
    let serialization = contract(
        crate::serialization::SerializationArtifactTarget::EditableSave,
        "save",
        Some(CONTRACT_DIGEST),
        executable(plan),
    );
    let rules = evaluator(spec_with_serialization(
        serialization,
        &[],
        leaked_slice(vec![string_field("name")]),
    ));
    let request = request(
        ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Absent)],
    );
    let materialized = rules
        .materialize_serialization(
            &request,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::EditableSave,
                "save",
            ),
        )
        .unwrap();
    assert_eq!(materialized.artifact_id(), "artifact");
    assert_eq!(materialized.context(), request.context());
    let records = materialized.records().collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].omission(),
        crate::MaterializedOmissionView::SemanticAbsent
    );
    assert_eq!(records[0].semantic_value(), Some(&CanonicalValue::Absent));
    assert_eq!(
        records[1].omission(),
        crate::MaterializedOmissionView::PresenceFalse
    );
    assert!(matches!(
        rules.materialize_serialization(
            &request,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::EditableSave,
                "other",
            ),
        ),
        Err(crate::MaterializationError::ArtifactSelection { matches: 0 })
    ));
}

#[test]
fn materialization_projects_group_scoped_derived_values_by_exact_instance() {
    use crate::serialization_contract::{
        DynamicGroupNode, IndexedKeyProjection, PseudoXmlFieldNode,
        SerializationGroupInstanceOrder, SerializationKeyProjection, SerializationNode,
        SerializationOccurrenceProjection, SerializationPlan, SerializationPresence,
        SerializationValueProjection,
    };

    let row_expression = leaked(Expression::Field {
        result_type: ValueType::String,
        field: FieldRef {
            field_id: "row-value",
            instance: FieldInstanceSelector::CurrentGroupInstance,
        },
    });
    let calculation = CalculationSpec {
        calculation_id: "row-calculation",
        scope: EvaluationScope::EachGroup("rows"),
        depends_on: &[],
        phases: &[ValidationPhase::Save],
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "row-output",
                value: row_expression,
                rounding: None,
                writeback: None,
            }]),
        }),
    };
    let nodes = leaked_slice(vec![SerializationNode::DynamicGroup(DynamicGroupNode {
        ordinal: 1,
        group_id: "rows",
        instance_order: SerializationGroupInstanceOrder::StableInstanceIdAscending,
        min_occurs: 0,
        max_occurs: Some(2),
        nodes: leaked_slice(vec![SerializationNode::PseudoXmlField(
            PseudoXmlFieldNode {
                ordinal: 2,
                key_projection: SerializationKeyProjection::GroupIndexed(IndexedKeyProjection {
                    group_id: "rows",
                    index_base: 1,
                    index_step: 1,
                    padding: 0,
                    prefix: "Row",
                    suffix: "",
                }),
                occurrence_projection: SerializationOccurrenceProjection::Fixed(1),
                value_projection: SerializationValueProjection::Derived {
                    calculation_id: "row-calculation",
                    output_id: "row-output",
                    instance: DerivedInstanceSelector::CurrentGroupInstance,
                },
                semantic_format: text_semantic(crate::serialization::AbsentValuePolicy::Reject),
                body_codec: crate::serialization::BodyCodec::RawLiteral,
                presence: SerializationPresence::Always,
            },
        )]),
    })]);
    let serialization = contract(
        crate::serialization::SerializationArtifactTarget::EditableSave,
        "group-save",
        Some(CONTRACT_DIGEST),
        executable(SerializationPlan { nodes }),
    );
    let specification = leaked(StaticRuleSetSpec {
        profile_status: profile_status(),
        effect_mode: all_effects(),
        serialization,
        context_values: &[],
        field_groups: leaked_slice(vec![FieldGroupSpec {
            group_id: "rows",
            min_occurs: 0,
            max_occurs: Some(2),
            members: &["row-value"],
        }]),
        fields: leaked_slice(vec![FieldSpec {
            field_id: "row-value",
            value_type: ValueType::String,
            group_id: Some("rows"),
            calculation_id: None,
            behavior: executable(FieldBehavior {
                normalization: EMPTY_NORMALIZATION,
                event_normalization: &[],
                coercion: Coercion::String {
                    on_empty: StringEmptyPolicy::Null,
                },
            }),
        }]),
        field_event_programs: &[],
        evaluation_order: &["row-calculation"],
        calculations: leaked_slice(vec![calculation]),
        rules: &[],
        workflow: Branch::Unresolved,
    });
    let row_a = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows").unwrap(),
        StableInstanceId::parse("row-a").unwrap(),
    );
    let row_b = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows").unwrap(),
        StableInstanceId::parse("row-b").unwrap(),
    );
    let field_a =
        FieldInstance::try_new(FieldId::parse("row-value").unwrap(), vec![row_a.clone()]).unwrap();
    let field_b =
        FieldInstance::try_new(FieldId::parse("row-value").unwrap(), vec![row_b.clone()]).unwrap();
    let request = request(
        ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
        vec![],
        vec![row_b.clone(), row_a.clone()],
        vec![
            RawFieldValue::new(field_b, RawValue::Text("B".into())),
            RawFieldValue::new(field_a, RawValue::Text("A".into())),
        ],
    );
    let materialized = evaluator(specification)
        .materialize_serialization(
            &request,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::EditableSave,
                "group-save",
            ),
        )
        .unwrap();
    let records = materialized.records().collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert!(matches!(
        records[0].value_source(),
        crate::MaterializedValueSourceView::Derived {
            instance: Some(instance),
            ..
        } if instance == &row_a
    ));
    assert!(matches!(
        records[1].value_source(),
        crate::MaterializedValueSourceView::Derived {
            instance: Some(instance),
            ..
        } if instance == &row_b
    ));
    assert_eq!(records[0].semantic_body(), Some("A"));
    assert_eq!(records[1].semantic_body(), Some("B"));
}

#[test]
fn serialization_inspector_evaluates_each_row_and_resolves_full_group_identity() {
    let current_value = leaked(Expression::Field {
        result_type: ValueType::String,
        field: FieldRef {
            field_id: "rows-a-value",
            instance: FieldInstanceSelector::CurrentGroupInstance,
        },
    });
    let emit_literal = leaked(Expression::Literal(TypedValue::String("emit")));
    let presence = SerializationPresence::When(Predicate::Compare {
        operator: CompareOperator::Equal,
        left: current_value,
        right: emit_literal,
    });
    let calculation = CalculationSpec {
        calculation_id: "rows-a-calculation",
        scope: EvaluationScope::EachGroup("rows-a"),
        depends_on: &[],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "row-output",
                value: current_value,
                rounding: None,
                writeback: None,
            }]),
        }),
    };
    let specification = spec(
        &[],
        leaked_slice(vec![
            FieldGroupSpec {
                group_id: "rows-a",
                min_occurs: 0,
                max_occurs: None,
                members: &["rows-a-value"],
            },
            FieldGroupSpec {
                group_id: "rows-b",
                min_occurs: 0,
                max_occurs: None,
                members: &["rows-b-value"],
            },
        ]),
        leaked_slice(vec![
            grouped_string_field("rows-a-value", "rows-a"),
            grouped_string_field("rows-b-value", "rows-b"),
        ]),
        &["rows-a-calculation"],
        leaked_slice(vec![calculation]),
        &[],
        all_effects(),
    );
    let rows_a_shared = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows-a").unwrap(),
        StableInstanceId::parse("shared").unwrap(),
    );
    let rows_a_other = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows-a").unwrap(),
        StableInstanceId::parse("other").unwrap(),
    );
    let rows_b_shared = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows-b").unwrap(),
        StableInstanceId::parse("shared").unwrap(),
    );
    let field = |field_id: &str, instance: RepeatedGroupInstance| {
        FieldInstance::try_new(FieldId::parse(field_id).unwrap(), vec![instance]).unwrap()
    };
    let request = request(
        ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
        vec![],
        vec![
            rows_b_shared.clone(),
            rows_a_shared.clone(),
            rows_a_other.clone(),
        ],
        vec![
            RawFieldValue::new(
                field("rows-b-value", rows_b_shared.clone()),
                RawValue::Text("foreign".into()),
            ),
            RawFieldValue::new(
                field("rows-a-value", rows_a_other.clone()),
                RawValue::Text("skip".into()),
            ),
            RawFieldValue::new(
                field("rows-a-value", rows_a_shared.clone()),
                RawValue::Text("emit".into()),
            ),
        ],
    );
    let compiled = evaluator(specification);
    let result = compiled.evaluate(&request).unwrap();
    let mut inspector = compiled.serialization_inspector(&request, &result).unwrap();

    assert!(
        inspector
            .evaluate_presence(presence, Some(&rows_a_shared))
            .unwrap()
    );
    assert!(
        !inspector
            .evaluate_presence(presence, Some(&rows_a_other))
            .unwrap()
    );
    assert!(matches!(
        inspector.evaluate_presence(presence, Some(&rows_b_shared)),
        Err(SerializationInspectionError::Interpreter(
            InterpreterError::FieldScopeMismatch { .. }
        ))
    ));

    let field_source = inspector
        .resolve_value_source(
            SerializationValueProjection::Field(FieldRef {
                field_id: "rows-a-value",
                instance: FieldInstanceSelector::StableInstanceId("shared"),
            }),
            None,
        )
        .unwrap();
    assert!(matches!(
        field_source,
        crate::MaterializedValueSourceView::Field { field }
            if field.group_path() == std::slice::from_ref(&rows_a_shared)
    ));

    let derived_source = inspector
        .resolve_value_source(
            SerializationValueProjection::Derived {
                calculation_id: "rows-a-calculation",
                output_id: "row-output",
                instance: DerivedInstanceSelector::StableInstanceId("shared"),
            },
            None,
        )
        .unwrap();
    assert!(matches!(
        derived_source,
        crate::MaterializedValueSourceView::Derived {
            calculation_id,
            output_id,
            instance: Some(instance),
        } if calculation_id.as_str() == "rows-a-calculation"
            && output_id.as_str() == "row-output"
            && instance == rows_a_shared
            && instance != rows_b_shared
    ));
}

#[test]
fn materialization_rejects_digest_phase_history_and_profile_fallback() {
    let plan = crate::serialization_contract::SerializationPlan {
        nodes: leaked_slice(vec![pseudo_node(
            1,
            "Name",
            1,
            "name",
            crate::serialization::AbsentValuePolicy::Reject,
            crate::serialization_contract::SerializationPresence::Always,
        )]),
    };
    let fields = leaked_slice(vec![string_field("name")]);
    let raw = || vec![raw_singleton("name", RawValue::Text("Taxpayer".into()))];

    let missing_digest = evaluator(spec_with_serialization(
        contract(
            crate::serialization::SerializationArtifactTarget::EditableSave,
            "save",
            None,
            executable(plan),
        ),
        &[],
        fields,
    ));
    let save_request = request(
        ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        raw(),
    );
    assert!(matches!(
        missing_digest.materialize_serialization(
            &save_request,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::EditableSave,
                "save",
            ),
        ),
        Err(crate::MaterializationError::MissingContractDigest)
    ));

    let branches = Profiled {
        official: Branch::Executable(plan),
        filing_safe: Branch::Unresolved,
    };
    let unavailable = evaluator(spec_with_serialization(
        contract(
            crate::serialization::SerializationArtifactTarget::EditableSave,
            "save",
            Some(CONTRACT_DIGEST),
            branches,
        ),
        &[],
        fields,
    ));
    assert!(matches!(
        unavailable.materialize_serialization(
            &save_request,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::EditableSave,
                "save",
            ),
        ),
        Err(crate::MaterializationError::BranchUnavailable {
            profile: BehaviorProfile::FilingSafe
        })
    ));

    let wrong_phase = request(
        ValidationContext::new(ValidationPhase::Submit, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        raw(),
    );
    assert!(matches!(
        unavailable.materialize_serialization(
            &wrong_phase,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::EditableSave,
                "save",
            ),
        ),
        Err(crate::MaterializationError::PhaseMismatch { .. })
    ));
    assert!(matches!(
        unavailable.materialize_serialization(
            &save_request,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::HistoricalImportCompatibility,
                "import",
            ),
        ),
        Err(crate::MaterializationError::HistoricalImportUnsupported)
    ));
}

#[test]
fn materialization_rejects_occurrence_gaps_and_duplicate_emission_ids() {
    let gap_plan = crate::serialization_contract::SerializationPlan {
        nodes: leaked_slice(vec![pseudo_node(
            1,
            "Name",
            2,
            "name",
            crate::serialization::AbsentValuePolicy::Reject,
            crate::serialization_contract::SerializationPresence::Always,
        )]),
    };
    let rules = evaluator(spec_with_serialization(
        contract(
            crate::serialization::SerializationArtifactTarget::EditableSave,
            "save",
            Some(CONTRACT_DIGEST),
            executable(gap_plan),
        ),
        &[],
        leaked_slice(vec![string_field("name")]),
    ));
    let save_request = request(
        ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("Taxpayer".to_owned()))],
    );
    assert!(matches!(
        rules.materialize_serialization(
            &save_request,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::EditableSave,
                "save",
            ),
        ),
        Err(crate::MaterializationError::OccurrenceGap {
            expected: 1,
            actual: 2,
            ..
        })
    ));

    let literals = leaked_slice(vec![
        crate::serialization_contract::SerializationNode::ReviewedLiteral(
            crate::serialization_contract::ReviewedLiteralNode {
                ordinal: 1,
                exact_bytes: b"a",
            },
        ),
        crate::serialization_contract::SerializationNode::ReviewedLiteral(
            crate::serialization_contract::ReviewedLiteralNode {
                ordinal: 1,
                exact_bytes: b"b",
            },
        ),
    ]);
    let duplicate = evaluator(spec_with_serialization(
        contract(
            crate::serialization::SerializationArtifactTarget::EditableSave,
            "save",
            Some(CONTRACT_DIGEST),
            executable(crate::serialization_contract::SerializationPlan { nodes: literals }),
        ),
        &[],
        &[],
    ));
    let empty_request = request(
        ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![],
    );
    assert!(matches!(
        duplicate.materialize_serialization(
            &empty_request,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::EditableSave,
                "save",
            ),
        ),
        Err(crate::MaterializationError::InvalidContractStructure { ordinal: 1 })
    ));
}

#[test]
fn materialization_prevalidates_children_of_zero_occurrence_groups() {
    let nested = crate::serialization_contract::SerializationNode::DynamicGroup(
        crate::serialization_contract::DynamicGroupNode {
            ordinal: 2,
            group_id: "child-rows",
            instance_order:
                crate::serialization_contract::SerializationGroupInstanceOrder::StableInstanceIdAscending,
            min_occurs: 0,
            max_occurs: Some(0),
            nodes: &[],
        },
    );
    let parent = crate::serialization_contract::SerializationNode::DynamicGroup(
        crate::serialization_contract::DynamicGroupNode {
            ordinal: 1,
            group_id: "parent-rows",
            instance_order:
                crate::serialization_contract::SerializationGroupInstanceOrder::StableInstanceIdAscending,
            min_occurs: 0,
            max_occurs: Some(0),
            nodes: leaked_slice(vec![nested]),
        },
    );
    let rules = evaluator(spec_with_serialization(
        contract(
            crate::serialization::SerializationArtifactTarget::EditableSave,
            "save",
            Some(CONTRACT_DIGEST),
            executable(crate::serialization_contract::SerializationPlan {
                nodes: leaked_slice(vec![parent]),
            }),
        ),
        leaked_slice(vec![FieldGroupSpec {
            group_id: "parent-rows",
            min_occurs: 0,
            max_occurs: Some(0),
            members: &[],
        }]),
        &[],
    ));
    let empty_request = request(
        ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![],
    );

    assert!(matches!(
        rules.materialize_serialization(
            &empty_request,
            &artifact_identity(
                crate::serialization::SerializationArtifactTarget::EditableSave,
                "save",
            ),
        ),
        Err(crate::MaterializationError::NestedDynamicGroup)
    ));
}

#[test]
fn materialization_accounts_for_zero_and_stably_ordered_group_instances() {
    let child = crate::serialization_contract::SerializationNode::PseudoXmlField(
        crate::serialization_contract::PseudoXmlFieldNode {
            ordinal: 2,
            key_projection: crate::serialization_contract::SerializationKeyProjection::GroupIndexed(
                crate::serialization_contract::IndexedKeyProjection {
                    group_id: "rows",
                    index_base: 1,
                    index_step: 1,
                    padding: 2,
                    prefix: "Row",
                    suffix: "",
                },
            ),
            occurrence_projection:
                crate::serialization_contract::SerializationOccurrenceProjection::Fixed(1),
            value_projection: crate::serialization_contract::SerializationValueProjection::Field(
                FieldRef {
                    field_id: "row-value",
                    instance: FieldInstanceSelector::CurrentGroupInstance,
                },
            ),
            semantic_format: text_semantic(crate::serialization::AbsentValuePolicy::Reject),
            body_codec: crate::serialization::BodyCodec::Utf8PercentRfc3986Unreserved,
            presence: crate::serialization_contract::SerializationPresence::Always,
        },
    );
    let repeated_literal = crate::serialization_contract::SerializationNode::ReviewedLiteral(
        crate::serialization_contract::ReviewedLiteralNode {
            ordinal: 3,
            exact_bytes: b",",
        },
    );
    let group_node = crate::serialization_contract::SerializationNode::DynamicGroup(
        crate::serialization_contract::DynamicGroupNode {
            ordinal: 1,
            group_id: "rows",
            instance_order:
                crate::serialization_contract::SerializationGroupInstanceOrder::StableInstanceIdAscending,
            min_occurs: 0,
            max_occurs: Some(2),
            nodes: leaked_slice(vec![child, repeated_literal]),
        },
    );
    let serialization = contract(
        crate::serialization::SerializationArtifactTarget::EditableSave,
        "save",
        Some(CONTRACT_DIGEST),
        executable(crate::serialization_contract::SerializationPlan {
            nodes: leaked_slice(vec![group_node]),
        }),
    );
    let groups = leaked_slice(vec![FieldGroupSpec {
        group_id: "rows",
        min_occurs: 0,
        max_occurs: Some(2),
        members: &["row-value"],
    }]);
    let fields = leaked_slice(vec![FieldSpec {
        field_id: "row-value",
        value_type: ValueType::String,
        group_id: Some("rows"),
        calculation_id: None,
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: &[],
            coercion: Coercion::String {
                on_empty: StringEmptyPolicy::Null,
            },
        }),
    }]);
    let rules = evaluator(spec_with_serialization(serialization, groups, fields));
    let selected = artifact_identity(
        crate::serialization::SerializationArtifactTarget::EditableSave,
        "save",
    );

    let zero = rules
        .materialize_serialization(
            &request(
                ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
                vec![],
                vec![],
                vec![],
            ),
            &selected,
        )
        .unwrap();
    assert_eq!(zero.trace().len(), 1);
    assert_eq!(zero.group_accounting().next().unwrap().instances().len(), 0);

    let instance = |id: &str| {
        RepeatedGroupInstance::new(
            RepeatedGroupId::parse("rows").unwrap(),
            StableInstanceId::parse(id).unwrap(),
        )
    };
    let row_a = instance("row-a");
    let row_b = instance("row-b");
    let field = |row: RepeatedGroupInstance, value: &str| {
        RawFieldValue::new(
            FieldInstance::try_new(FieldId::parse("row-value").unwrap(), vec![row]).unwrap(),
            RawValue::Text(value.to_owned()),
        )
    };
    let many = rules
        .materialize_serialization(
            &request(
                ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
                vec![],
                vec![row_b.clone(), row_a.clone()],
                vec![field(row_b, "B B"), field(row_a, "A A")],
            ),
            &selected,
        )
        .unwrap();
    let records = many
        .records()
        .filter(|record| {
            matches!(
                record.binding(),
                crate::MaterializedBindingView::PseudoXmlField { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].emission_id().group_path()[0], instance("row-a"));
    assert_eq!(records[0].encoded_body(), Some("A%20A"));
    assert_eq!(records[1].emission_id().group_path()[0], instance("row-b"));
    let literals = many
        .records()
        .filter(|record| {
            matches!(
                record.binding(),
                crate::MaterializedBindingView::ReviewedLiteral { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(literals.len(), 2);
    assert_eq!(literals[0].emission_id().group_path()[0], instance("row-a"));
    assert_eq!(literals[1].emission_id().group_path()[0], instance("row-b"));
}

#[test]
fn canonicalization_preserves_absent_and_present_blank() {
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        &[],
        all_effects(),
    );
    let evaluator = evaluator(spec);
    let context = ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe);

    let absent = evaluator
        .evaluate(&request(
            context,
            vec![],
            vec![],
            vec![raw_singleton("name", RawValue::Absent)],
        ))
        .unwrap();
    let blank = evaluator
        .evaluate(&request(
            context,
            vec![],
            vec![],
            vec![raw_singleton("name", RawValue::Text(String::new()))],
        ))
        .unwrap();

    assert_eq!(absent.canonical_inputs()[0].raw(), &RawValue::Absent);
    assert_eq!(
        blank.canonical_inputs()[0].raw(),
        &RawValue::Text(String::new())
    );
    assert_eq!(
        absent.canonical_inputs()[0].canonical(),
        &CanonicalValue::Absent
    );
    assert_eq!(
        blank.canonical_inputs()[0].canonical(),
        &CanonicalValue::Blank
    );
}

#[test]
fn exact_blur_normalization_precedes_calculations_and_is_event_scoped() {
    let blur_pipeline = leaked_slice(vec![NormalizationStep::OfflineEbirMoneyRoundV1]);
    let field = FieldSpec {
        field_id: "amount",
        value_type: ValueType::String,
        group_id: None,
        calculation_id: Some("normalized-amount"),
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: leaked_slice(vec![FieldEventNormalization {
                phase: ValidationPhase::Blur,
                normalization: blur_pipeline,
            }]),
            coercion: Coercion::String {
                on_empty: StringEmptyPolicy::Null,
            },
        }),
    };
    let field_expression = leaked(Expression::Field {
        result_type: ValueType::String,
        field: FieldRef {
            field_id: "amount",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    let decimal_expression = leaked(Expression::Coerce {
        result_type: ValueType::Decimal,
        input: field_expression,
        coercion: Coercion::Decimal {
            decimal: DecimalPolicy {
                precision: 38,
                scale: 2,
                division_scale: 18,
                rounding: Rounding {
                    mode: RoundingMode::None,
                    scale: 2,
                },
                overflow: OverflowPolicy::Error,
            },
            grouping: InputGrouping::Comma,
            on_empty: NumericEmptyPolicy::Zero,
            on_invalid: InvalidValuePolicy::Error,
        },
    });
    let blur = leaked_slice(vec![ValidationPhase::Blur]);
    let calculation = CalculationSpec {
        calculation_id: "normalized-amount",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: blur,
        trigger_field_ids: &["amount"],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "amount-decimal",
                value: decimal_expression,
                rounding: None,
                writeback: Some(CalculationWriteback {
                    field: FieldRef {
                        field_id: "amount",
                        instance: FieldInstanceSelector::Singleton,
                    },
                    format: CalculationWriteFormat::OfflineEbirFormatCurrencyV1,
                }),
            }]),
        }),
    };
    let programs = leaked_slice(vec![
        FieldEventProgramSpec {
            phase: ValidationPhase::Blur,
            trigger_field_id: "amount",
            profiles: executable(FieldEventProgram {
                steps: leaked_slice(vec![FieldEventStep::Calculation {
                    calculation_id: "normalized-amount",
                    output_ids: &["amount-decimal"],
                    write_mode: ScheduledOutputWriteMode::Insert,
                }]),
            }),
        },
        FieldEventProgramSpec {
            phase: ValidationPhase::Change,
            trigger_field_id: "amount",
            profiles: executable(FieldEventProgram { steps: &[] }),
        },
    ]);
    let evaluator = evaluator(spec_with_field_event_programs(
        &[],
        leaked_slice(vec![field]),
        &["normalized-amount"],
        leaked_slice(vec![calculation]),
        &[],
        programs,
        all_effects(),
    ));
    let amount = FieldInstance::singleton(FieldId::parse("amount").unwrap());

    for (raw, normalized, expected_decimal) in [
        ("1234567890123", "0.00", "0.00"),
        ("1e3", "1,000.00", "1000.00"),
    ] {
        let result = evaluator
            .evaluate(&field_event_request(
                ValidationContext::new(
                    ValidationPhase::Blur,
                    BehaviorProfile::OfficialCompatibility,
                ),
                amount.clone(),
                vec![],
                vec![raw_singleton("amount", RawValue::Text(raw.to_owned()))],
            ))
            .unwrap();
        assert_eq!(
            result.canonical_inputs()[0].raw(),
            &RawValue::Text(raw.to_owned())
        );
        assert_eq!(
            result.canonical_inputs()[0].canonical(),
            &CanonicalValue::Text(normalized.to_owned())
        );
        assert_eq!(
            result.derived_outputs()[0].value(),
            &CanonicalValue::Decimal(expected_decimal.parse().unwrap())
        );
    }

    let change = evaluator
        .evaluate(&field_event_request(
            ValidationContext::new(
                ValidationPhase::Change,
                BehaviorProfile::OfficialCompatibility,
            ),
            amount,
            vec![],
            vec![raw_singleton("amount", RawValue::Text("1e3".to_owned()))],
        ))
        .unwrap();
    assert_eq!(
        change.canonical_inputs()[0].canonical(),
        &CanonicalValue::Text("1e3".to_owned())
    );
    assert!(change.derived_outputs().is_empty());

    let absent = evaluator.evaluate(&field_event_request(
        ValidationContext::new(
            ValidationPhase::Blur,
            BehaviorProfile::OfficialCompatibility,
        ),
        FieldInstance::singleton(FieldId::parse("amount").unwrap()),
        vec![],
        vec![raw_singleton("amount", RawValue::Absent)],
    ));
    assert!(matches!(
        absent,
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidCoercion {
                target: ValueType::String,
                reason: CoercionFailure::Empty,
            }
        ))
    ));
}

#[test]
fn exact_field_event_program_interleaves_writeback_reset_and_recalculation() {
    let decimal_coercion = Coercion::Decimal {
        decimal: DecimalPolicy {
            precision: 38,
            scale: 2,
            division_scale: 18,
            rounding: Rounding {
                mode: RoundingMode::None,
                scale: 2,
            },
            overflow: OverflowPolicy::Error,
        },
        grouping: InputGrouping::Comma,
        on_empty: NumericEmptyPolicy::Zero,
        on_invalid: InvalidValuePolicy::Error,
    };
    let amount = FieldSpec {
        field_id: "amount",
        value_type: ValueType::String,
        group_id: None,
        calculation_id: None,
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: leaked_slice(vec![FieldEventNormalization {
                phase: ValidationPhase::Blur,
                normalization: leaked_slice(vec![NormalizationStep::OfflineEbirMoneyRoundV1]),
            }]),
            coercion: Coercion::String {
                on_empty: StringEmptyPolicy::Null,
            },
        }),
    };
    let mirror = FieldSpec {
        field_id: "mirror",
        value_type: ValueType::String,
        group_id: None,
        calculation_id: Some("snapshot"),
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: &[],
            coercion: Coercion::String {
                on_empty: StringEmptyPolicy::Null,
            },
        }),
    };
    let amount_text = leaked(Expression::Field {
        result_type: ValueType::String,
        field: FieldRef {
            field_id: "amount",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    let amount_decimal = leaked(Expression::Coerce {
        result_type: ValueType::Decimal,
        input: amount_text,
        coercion: decimal_coercion,
    });
    let mirror_text = leaked(Expression::Field {
        result_type: ValueType::String,
        field: FieldRef {
            field_id: "mirror",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    let mirror_decimal = leaked(Expression::Coerce {
        result_type: ValueType::Decimal,
        input: mirror_text,
        coercion: decimal_coercion,
    });
    let calculation = CalculationSpec {
        calculation_id: "snapshot",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: &[ValidationPhase::Blur],
        trigger_field_ids: &["amount"],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "working",
                value: amount_decimal,
                rounding: None,
                writeback: Some(CalculationWriteback {
                    field: FieldRef {
                        field_id: "mirror",
                        instance: FieldInstanceSelector::Singleton,
                    },
                    format: CalculationWriteFormat::OfflineEbirFormatCurrencyV1,
                }),
            }]),
        }),
    };
    let cap_rule = RuleSpec {
        rule_id: "cap-reset",
        scope: EvaluationScope::Singleton,
        order: 10,
        phases: &[ValidationPhase::Blur],
        trigger_field_ids: &["amount"],
        profiles: executable(RuleBranch {
            predicate: leaked(Predicate::Compare {
                operator: CompareOperator::GreaterThan,
                left: mirror_decimal,
                right: decimal_literal(250_000, 0),
            }),
            effects: leaked_slice(vec![
                Effect::EmitIssue {
                    severity: RuleSeverity::Blocking,
                    message: "amount exceeds cap",
                    official_message: Some("amount exceeds cap"),
                    assessment: RuleAssessment::VerifiedCorrect,
                    fields: &[],
                },
                Effect::SetRawFieldValue {
                    field: FieldRef {
                        field_id: "amount",
                        instance: FieldInstanceSelector::Singleton,
                    },
                    value: StaticRawValue::Text("0.00"),
                },
            ]),
        }),
    };
    let program = FieldEventProgramSpec {
        phase: ValidationPhase::Blur,
        trigger_field_id: "amount",
        profiles: executable(FieldEventProgram {
            steps: leaked_slice(vec![
                FieldEventStep::Calculation {
                    calculation_id: "snapshot",
                    output_ids: &["working"],
                    write_mode: ScheduledOutputWriteMode::Insert,
                },
                FieldEventStep::Rule {
                    rule_id: "cap-reset",
                },
                FieldEventStep::Calculation {
                    calculation_id: "snapshot",
                    output_ids: &["working"],
                    write_mode: ScheduledOutputWriteMode::Replace,
                },
            ]),
        }),
    };
    let evaluator = evaluator(spec_with_field_event_programs(
        &[],
        leaked_slice(vec![amount, mirror]),
        &["snapshot"],
        leaked_slice(vec![calculation]),
        leaked_slice(vec![cap_rule]),
        leaked_slice(vec![program]),
        executable(EffectEvaluationMode::StopEffectsAfterFirstBlockingIssue),
    ));
    let result = evaluator
        .evaluate(&field_event_request(
            ValidationContext::new(
                ValidationPhase::Blur,
                BehaviorProfile::OfficialCompatibility,
            ),
            FieldInstance::singleton(FieldId::parse("amount").unwrap()),
            vec![],
            vec![
                raw_singleton("amount", RawValue::Text("300001".into())),
                raw_singleton("mirror", RawValue::Text("1.00".into())),
            ],
        ))
        .unwrap();

    assert_eq!(result.report().violations().len(), 1);
    assert_eq!(result.field_value_assignments().len(), 1);
    assert_eq!(result.expected_outputs().len(), 1);
    assert_eq!(result.derived_outputs().len(), 1);
    assert_eq!(
        result.derived_outputs()[0].value(),
        &CanonicalValue::Decimal("0.00".parse().unwrap())
    );
    let final_value = |field_id| {
        result
            .canonical_inputs()
            .iter()
            .find(|input| input.field().field_id().as_str() == field_id)
            .unwrap()
    };
    assert_eq!(
        final_value("amount").raw(),
        &RawValue::Text("300001".into())
    );
    assert_eq!(final_value("mirror").raw(), &RawValue::Text("1.00".into()));
    assert_eq!(
        final_value("amount").canonical(),
        &CanonicalValue::Text("0.00".into())
    );
    assert_eq!(
        final_value("mirror").canonical(),
        &CanonicalValue::Text("0.00".into())
    );
}

#[test]
fn offline_ebir_currency_writeback_matches_the_finite_source_matrix_and_fails_closed() {
    for (value, expected) in [
        (CanonicalValue::Integer(1_234), "1,234.00"),
        (
            CanonicalValue::Decimal("1234.50".parse().unwrap()),
            "1,234.50",
        ),
        (
            CanonicalValue::Decimal("-1234.50".parse().unwrap()),
            "-1,234.50",
        ),
        (CanonicalValue::Integer(0), "0.00"),
        (
            CanonicalValue::Integer(1_234_567_890_123_456),
            "123,456,789,012,345.00",
        ),
        (
            CanonicalValue::Decimal("1234567890123456.50".parse().unwrap()),
            "123,456,789,012,345.50",
        ),
        (
            CanonicalValue::Decimal("-1234567890123456.50".parse().unwrap()),
            "-12,345,678,901,234.50",
        ),
        (
            CanonicalValue::Integer(-1_234_567_890_123_456),
            "-1,234,567,890,123,456.00",
        ),
        (
            CanonicalValue::Decimal("0.0049999999999".parse().unwrap()),
            "0.01",
        ),
        (
            CanonicalValue::Decimal("-0.0049999999999".parse().unwrap()),
            "-0.01",
        ),
    ] {
        assert_eq!(
            offline_ebir_format_currency_v1(&value).unwrap(),
            expected,
            "source value {value:?}"
        );
    }
    for (value, expected_kind) in [
        (CanonicalValue::Absent, ValueKind::Absent),
        (CanonicalValue::Blank, ValueKind::Blank),
        (CanonicalValue::Text(String::new()), ValueKind::String),
        (CanonicalValue::Text("NaN".into()), ValueKind::String),
        (CanonicalValue::Text("Infinity".into()), ValueKind::String),
        (CanonicalValue::Boolean(true), ValueKind::Boolean),
    ] {
        assert!(matches!(
            offline_ebir_format_currency_v1(&value),
            Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::CalculationWriteback,
                expected: ValueType::Decimal,
                actual,
            }) if actual == expected_kind
        ));
    }
    for value in [
        CanonicalValue::Integer(1_000_000_000_000_000_000_000),
        CanonicalValue::Integer(-1_000_000_000_000_000_000_000),
        CanonicalValue::Decimal("1000000000000000000000.00".parse().unwrap()),
        CanonicalValue::Decimal("-1000000000000000000000.00".parse().unwrap()),
    ] {
        assert!(matches!(
            offline_ebir_format_currency_v1(&value),
            Err(InterpreterError::Overflow {
                operation: ExecutionOperation::CalculationWriteback,
            })
        ));
    }
}

#[test]
fn calculation_writeback_distinguishes_source_no_write_from_evaluated_absence() {
    let build_evaluator = |condition: &'static Predicate| {
        let field = FieldSpec {
            field_id: "amount",
            value_type: ValueType::String,
            group_id: None,
            calculation_id: Some("parse-amount"),
            behavior: executable(FieldBehavior {
                normalization: EMPTY_NORMALIZATION,
                event_normalization: &[],
                coercion: Coercion::String {
                    on_empty: StringEmptyPolicy::Null,
                },
            }),
        };
        let raw_amount = leaked(Expression::Field {
            result_type: ValueType::String,
            field: FieldRef {
                field_id: "amount",
                instance: FieldInstanceSelector::Singleton,
            },
        });
        let parsed_amount = leaked(Expression::JavaScriptParseIntRadix10 {
            result_type: ValueType::Integer,
            input: raw_amount,
        });
        let calculation = CalculationSpec {
            calculation_id: "parse-amount",
            scope: EvaluationScope::Singleton,
            depends_on: &[],
            phases: &[ValidationPhase::Blur],
            trigger_field_ids: &["amount"],
            profiles: executable(CalculationBranch {
                condition,
                outputs: leaked_slice(vec![CalculationOutput {
                    output_id: "parsed",
                    value: parsed_amount,
                    rounding: None,
                    writeback: Some(CalculationWriteback {
                        field: FieldRef {
                            field_id: "amount",
                            instance: FieldInstanceSelector::Singleton,
                        },
                        format: CalculationWriteFormat::OfflineEbirFormatCurrencyV1,
                    }),
                }]),
            }),
        };
        let programs = leaked_slice(vec![FieldEventProgramSpec {
            phase: ValidationPhase::Blur,
            trigger_field_id: "amount",
            profiles: executable(FieldEventProgram {
                steps: leaked_slice(vec![FieldEventStep::Calculation {
                    calculation_id: "parse-amount",
                    output_ids: &["parsed"],
                    write_mode: ScheduledOutputWriteMode::Insert,
                }]),
            }),
        }]);
        evaluator(spec_with_field_event_programs(
            &[],
            leaked_slice(vec![field]),
            &["parse-amount"],
            leaked_slice(vec![calculation]),
            &[],
            programs,
            all_effects(),
        ))
    };
    let request_with = |value: &str| {
        field_event_request(
            ValidationContext::new(
                ValidationPhase::Blur,
                BehaviorProfile::OfficialCompatibility,
            ),
            FieldInstance::singleton(FieldId::parse("amount").unwrap()),
            vec![],
            vec![raw_singleton("amount", RawValue::Text(value.to_owned()))],
        )
    };

    for invalid in ["", "NaN", "Infinity"] {
        assert!(matches!(
            build_evaluator(&TRUE).evaluate(&request_with(invalid)),
            Err(EvaluationError::Interpreter(
                InterpreterError::TypeMismatch {
                    operation: ExecutionOperation::CalculationWriteback,
                    expected: ValueType::Decimal,
                    actual: ValueKind::Absent,
                }
            ))
        ));
    }

    let false_condition = leaked(Predicate::Constant(false));
    let no_write = build_evaluator(false_condition)
        .evaluate(&request_with("NaN"))
        .expect("a reviewed false condition is a source no-write");
    assert_eq!(
        no_write.canonical_inputs()[0].canonical(),
        &CanonicalValue::Text("NaN".to_owned())
    );
    assert_eq!(
        no_write.derived_outputs()[0].value(),
        &CanonicalValue::Absent
    );
}

#[test]
fn static_validation_rejects_an_unscheduled_calculation_writeback_output() {
    let target_field = |field_id| FieldSpec {
        field_id,
        value_type: ValueType::String,
        group_id: None,
        calculation_id: Some("two-output-calc"),
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: &[],
            coercion: Coercion::String {
                on_empty: StringEmptyPolicy::Null,
            },
        }),
    };
    let writeback = |field_id| {
        Some(CalculationWriteback {
            field: FieldRef {
                field_id,
                instance: FieldInstanceSelector::Singleton,
            },
            format: CalculationWriteFormat::OfflineEbirFormatCurrencyV1,
        })
    };
    let calculation = CalculationSpec {
        calculation_id: "two-output-calc",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: &[ValidationPhase::Blur],
        trigger_field_ids: &["trigger"],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![
                CalculationOutput {
                    output_id: "scheduled",
                    value: decimal_literal(1, 0),
                    rounding: None,
                    writeback: writeback("target-a"),
                },
                CalculationOutput {
                    output_id: "silently-omitted",
                    value: decimal_literal(2, 0),
                    rounding: None,
                    writeback: writeback("target-b"),
                },
            ]),
        }),
    };
    let program = FieldEventProgramSpec {
        phase: ValidationPhase::Blur,
        trigger_field_id: "trigger",
        profiles: executable(FieldEventProgram {
            steps: leaked_slice(vec![FieldEventStep::Calculation {
                calculation_id: "two-output-calc",
                output_ids: &["scheduled"],
                write_mode: ScheduledOutputWriteMode::Insert,
            }]),
        }),
    };
    let specification = spec_with_field_event_programs(
        &[],
        leaked_slice(vec![
            string_field("trigger"),
            target_field("target-a"),
            target_field("target-b"),
        ]),
        &["two-output-calc"],
        leaked_slice(vec![calculation]),
        &[],
        leaked_slice(vec![program]),
        all_effects(),
    );

    assert!(matches!(
        validate_static_spec(specification),
        Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::InvalidEventBinding {
                kind: SpecItemKind::Output,
                value: "silently-omitted",
            }
        ))
    ));
}

#[test]
fn static_validation_rejects_writeback_on_a_non_event_calculation() {
    let target = FieldSpec {
        field_id: "batch-target",
        value_type: ValueType::String,
        group_id: None,
        calculation_id: Some("batch-calc"),
        behavior: executable(FieldBehavior {
            normalization: EMPTY_NORMALIZATION,
            event_normalization: &[],
            coercion: Coercion::String {
                on_empty: StringEmptyPolicy::Null,
            },
        }),
    };
    let calculation = CalculationSpec {
        calculation_id: "batch-calc",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "batch-output",
                value: decimal_literal(1, 0),
                rounding: None,
                writeback: Some(CalculationWriteback {
                    field: FieldRef {
                        field_id: "batch-target",
                        instance: FieldInstanceSelector::Singleton,
                    },
                    format: CalculationWriteFormat::OfflineEbirFormatCurrencyV1,
                }),
            }]),
        }),
    };
    let specification = spec(
        &[],
        &[],
        leaked_slice(vec![target]),
        &["batch-calc"],
        leaked_slice(vec![calculation]),
        &[],
        all_effects(),
    );

    assert!(matches!(
        validate_static_spec(specification),
        Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::InvalidEventBinding {
                kind: SpecItemKind::Output,
                value: "batch-output",
            }
        ))
    ));
}

#[test]
fn coercion_failed_predicate_only_matches_preserved_invalid_input() {
    fn matched(value_type: ValueType, coercion: Coercion, raw: RawValue) -> bool {
        let field = FieldSpec {
            field_id: "value",
            value_type,
            group_id: None,
            calculation_id: None,
            behavior: executable(FieldBehavior {
                normalization: EMPTY_NORMALIZATION,
                event_normalization: &[],
                coercion,
            }),
        };
        let predicate = leaked(Predicate::CoercionFailed {
            field: FieldRef {
                field_id: "value",
                instance: FieldInstanceSelector::Singleton,
            },
        });
        let rule = RuleSpec {
            rule_id: "invalid-value",
            scope: EvaluationScope::Singleton,
            order: 1,
            phases: VALIDATE,
            trigger_field_ids: &[],
            profiles: executable(RuleBranch {
                predicate,
                effects: leaked_slice(vec![Effect::EmitIssue {
                    severity: RuleSeverity::Advisory,
                    message: "invalid value",
                    official_message: None,
                    assessment: RuleAssessment::VerifiedCorrect,
                    fields: &[],
                }]),
            }),
        };
        let specification = spec(
            &[],
            &[],
            leaked_slice(vec![field]),
            &[],
            &[],
            leaked_slice(vec![rule]),
            all_effects(),
        );
        let result = evaluator(specification)
            .evaluate(&request(
                ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
                vec![],
                vec![],
                vec![raw_singleton("value", raw)],
            ))
            .unwrap();
        result.report().violations().len() == 1
    }

    let cases = [
        (
            ValueType::Decimal,
            Coercion::Decimal {
                decimal: decimal_policy(),
                grouping: InputGrouping::Forbidden,
                on_empty: NumericEmptyPolicy::Null,
                on_invalid: InvalidValuePolicy::PreserveRaw,
            },
            "not-a-decimal",
            "12.5",
        ),
        (
            ValueType::Integer,
            Coercion::Integer {
                on_empty: NumericEmptyPolicy::Null,
                on_invalid: InvalidValuePolicy::PreserveRaw,
            },
            "not-an-integer",
            "12",
        ),
        (
            ValueType::Boolean,
            Coercion::Boolean {
                true_values: &["Y"],
                false_values: &["N"],
                on_empty: BooleanEmptyPolicy::Null,
                on_invalid: InvalidValuePolicy::PreserveRaw,
            },
            "maybe",
            "Y",
        ),
        (
            ValueType::Date,
            Coercion::Date {
                accepted_formats: &[DateFormat::YearMonthDay],
                on_empty: DateEmptyPolicy::Null,
                on_invalid: InvalidValuePolicy::PreserveRaw,
            },
            "not-a-date",
            "2024-02-29",
        ),
    ];

    for (value_type, coercion, invalid, valid) in cases {
        assert!(matched(
            value_type,
            coercion,
            RawValue::Text(invalid.to_owned())
        ));
        assert!(!matched(
            value_type,
            coercion,
            RawValue::Text(valid.to_owned())
        ));
    }
    let integer = Coercion::Integer {
        on_empty: NumericEmptyPolicy::Null,
        on_invalid: InvalidValuePolicy::PreserveRaw,
    };
    assert!(!matched(
        ValueType::Integer,
        integer,
        RawValue::Text(String::new())
    ));
    assert!(!matched(ValueType::Integer, integer, RawValue::Absent));
}

#[test]
fn decimal_coercion_enforces_reviewed_scale_and_precision() {
    let rounded_policy = DecimalPolicy {
        precision: 5,
        scale: 2,
        division_scale: 2,
        rounding: Rounding {
            mode: RoundingMode::HalfUp,
            scale: 2,
        },
        overflow: OverflowPolicy::Error,
    };
    assert_eq!(
        coerce_decimal(
            NormalizedInput::Text("1.234"),
            rounded_policy,
            InputGrouping::Forbidden,
            NumericEmptyPolicy::Null,
            InvalidValuePolicy::Error,
        )
        .unwrap(),
        CanonicalValue::Decimal("1.23".parse().unwrap())
    );

    assert!(matches!(
        coerce_decimal(
            NormalizedInput::Text("99999"),
            rounded_policy,
            InputGrouping::Forbidden,
            NumericEmptyPolicy::Null,
            InvalidValuePolicy::Error,
        ),
        Err(InterpreterError::InvalidCoercion {
            target: ValueType::Decimal,
            reason: CoercionFailure::PrecisionOverflow,
        })
    ));

    let clamped_policy = DecimalPolicy {
        overflow: OverflowPolicy::Clamp,
        ..rounded_policy
    };
    assert_eq!(
        coerce_decimal(
            NormalizedInput::Text("99999"),
            clamped_policy,
            InputGrouping::Forbidden,
            NumericEmptyPolicy::Null,
            InvalidValuePolicy::Error,
        )
        .unwrap(),
        CanonicalValue::Decimal("999.99".parse().unwrap())
    );

    let no_rounding_policy = DecimalPolicy {
        rounding: Rounding {
            mode: RoundingMode::None,
            scale: 2,
        },
        ..rounded_policy
    };
    assert!(matches!(
        coerce_decimal(
            NormalizedInput::Text("1.234"),
            no_rounding_policy,
            InputGrouping::Forbidden,
            NumericEmptyPolicy::Null,
            InvalidValuePolicy::Error,
        ),
        Err(InterpreterError::InvalidCoercion {
            target: ValueType::Decimal,
            reason: CoercionFailure::PrecisionOverflow,
        })
    ));

    assert!(matches!(
        validate_decimal_policy(DecimalPolicy {
            precision: 2,
            scale: 3,
            division_scale: 2,
            rounding: Rounding {
                mode: RoundingMode::HalfUp,
                scale: 2,
            },
            overflow: OverflowPolicy::Error,
        }),
        Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::InvalidDecimalPolicy {
                precision: 2,
                scale: 3,
                division_scale: 2,
            }
        ))
    ));
}

#[test]
fn static_validation_rejects_overlapping_boolean_coercion_tokens() {
    assert!(matches!(
        validate_coercion(Coercion::Boolean {
            true_values: &["Y", "1"],
            false_values: &["N", "Y"],
            on_empty: BooleanEmptyPolicy::Null,
            on_invalid: InvalidValuePolicy::Error,
        }),
        Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::AmbiguousBooleanCoercionValue { value: "Y" }
        ))
    ));
}

#[test]
fn first_error_mode_still_reports_complete_rule_coverage_in_reviewed_order() {
    let first_effects = leaked_slice(vec![Effect::EmitIssue {
        severity: RuleSeverity::Blocking,
        message: "first",
        official_message: Some("official first"),
        assessment: RuleAssessment::VerifiedCorrect,
        fields: &[],
    }]);
    let second_effects = leaked_slice(vec![Effect::EmitIssue {
        severity: RuleSeverity::Blocking,
        message: "second",
        official_message: Some("official second"),
        assessment: RuleAssessment::VerifiedCorrect,
        fields: &[],
    }]);
    let rules = leaked_slice(vec![
        RuleSpec {
            rule_id: "second",
            scope: EvaluationScope::Singleton,
            order: 20,
            phases: VALIDATE,
            trigger_field_ids: &[],
            profiles: executable(RuleBranch {
                predicate: &TRUE,
                effects: second_effects,
            }),
        },
        RuleSpec {
            rule_id: "first",
            scope: EvaluationScope::Singleton,
            order: 10,
            phases: VALIDATE,
            trigger_field_ids: &[],
            profiles: executable(RuleBranch {
                predicate: &TRUE,
                effects: first_effects,
            }),
        },
    ]);
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        rules,
        profiled_effects(
            EffectEvaluationMode::ApplyAll,
            EffectEvaluationMode::StopEffectsAfterFirstBlockingIssue,
        ),
    );
    let result = evaluator(spec)
        .evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![],
            vec![],
            vec![raw_singleton("name", RawValue::Text("x".into()))],
        ))
        .unwrap();

    assert_eq!(result.report().expected_rules().len(), 2);
    assert_eq!(
        result
            .report()
            .evaluated_rules()
            .iter()
            .map(|execution| execution.rule_id().as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    assert_eq!(result.report().violations().len(), 1);
    assert_eq!(result.report().violations()[0].rule_id().as_str(), "first");
}

#[test]
fn exact_field_event_inventory_isolates_singleton_triggers() {
    let issue = |message| {
        leaked_slice(vec![Effect::EmitIssue {
            severity: RuleSeverity::Advisory,
            message,
            official_message: None,
            assessment: RuleAssessment::VerifiedCorrect,
            fields: &[],
        }])
    };
    let rules = leaked_slice(vec![
        RuleSpec {
            rule_id: "alpha-blur",
            scope: EvaluationScope::Singleton,
            order: 10,
            phases: &[ValidationPhase::Blur],
            trigger_field_ids: &["alpha"],
            profiles: executable(RuleBranch {
                predicate: &TRUE,
                effects: issue("alpha"),
            }),
        },
        RuleSpec {
            rule_id: "beta-blur",
            scope: EvaluationScope::Singleton,
            order: 20,
            phases: &[ValidationPhase::Blur],
            trigger_field_ids: &["beta"],
            profiles: executable(RuleBranch {
                predicate: &TRUE,
                effects: issue("beta"),
            }),
        },
    ]);
    let programs = leaked_slice(vec![
        FieldEventProgramSpec {
            phase: ValidationPhase::Blur,
            trigger_field_id: "alpha",
            profiles: executable(FieldEventProgram {
                steps: leaked_slice(vec![FieldEventStep::Rule {
                    rule_id: "alpha-blur",
                }]),
            }),
        },
        FieldEventProgramSpec {
            phase: ValidationPhase::Blur,
            trigger_field_id: "beta",
            profiles: executable(FieldEventProgram {
                steps: leaked_slice(vec![FieldEventStep::Rule {
                    rule_id: "beta-blur",
                }]),
            }),
        },
    ]);
    let spec = spec_with_field_event_programs(
        &[],
        leaked_slice(vec![string_field("alpha"), string_field("beta")]),
        &[],
        &[],
        rules,
        programs,
        all_effects(),
    );
    let event_field = FieldInstance::singleton(FieldId::parse("alpha").unwrap());
    let result = evaluator(spec)
        .evaluate(&field_event_request(
            ValidationContext::new(
                ValidationPhase::Blur,
                BehaviorProfile::OfficialCompatibility,
            ),
            event_field.clone(),
            Vec::new(),
            vec![
                raw_singleton("alpha", RawValue::Text("a".into())),
                raw_singleton("beta", RawValue::Text("b".into())),
            ],
        ))
        .unwrap();

    assert_eq!(result.event_field(), Some(&event_field));
    assert_eq!(result.report().expected_rules().len(), 1);
    assert_eq!(
        result.report().expected_rules()[0].rule_id().as_str(),
        "alpha-blur"
    );
    assert_eq!(result.report().violations()[0].message(), "alpha");
}

#[test]
fn event_program_pairs_are_authoritative_without_inventing_a_cross_product() {
    let rule = RuleSpec {
        rule_id: "sparse-pairs",
        scope: EvaluationScope::Singleton,
        order: 10,
        phases: &[
            ValidationPhase::Blur,
            ValidationPhase::Change,
            ValidationPhase::Validate,
        ],
        trigger_field_ids: &["alpha", "beta"],
        profiles: executable(RuleBranch {
            predicate: &TRUE,
            effects: leaked_slice(vec![Effect::EmitIssue {
                severity: RuleSeverity::Advisory,
                message: "matched",
                official_message: None,
                assessment: RuleAssessment::VerifiedCorrect,
                fields: &[],
            }]),
        }),
    };
    let blur_alpha = FieldEventProgramSpec {
        phase: ValidationPhase::Blur,
        trigger_field_id: "alpha",
        profiles: executable(FieldEventProgram {
            steps: leaked_slice(vec![FieldEventStep::Rule {
                rule_id: "sparse-pairs",
            }]),
        }),
    };
    let change_beta = FieldEventProgramSpec {
        phase: ValidationPhase::Change,
        trigger_field_id: "beta",
        profiles: executable(FieldEventProgram {
            steps: leaked_slice(vec![FieldEventStep::Rule {
                rule_id: "sparse-pairs",
            }]),
        }),
    };
    let fields = leaked_slice(vec![string_field("alpha"), string_field("beta")]);
    let rules = leaked_slice(vec![rule]);
    let specification = spec_with_field_event_programs(
        &[],
        fields,
        &[],
        &[],
        rules,
        leaked_slice(vec![blur_alpha, change_beta]),
        all_effects(),
    );
    validate_static_spec(specification).unwrap();

    let values = || {
        vec![
            raw_singleton("alpha", RawValue::Text("a".into())),
            raw_singleton("beta", RawValue::Text("b".into())),
        ]
    };
    let exact = evaluator(specification)
        .evaluate(&field_event_request(
            ValidationContext::new(
                ValidationPhase::Blur,
                BehaviorProfile::OfficialCompatibility,
            ),
            FieldInstance::singleton(FieldId::parse("alpha").unwrap()),
            vec![],
            values(),
        ))
        .unwrap();
    assert_eq!(exact.report().violations()[0].message(), "matched");

    let missing_cross_product = evaluator(specification).evaluate(&field_event_request(
        ValidationContext::new(
            ValidationPhase::Blur,
            BehaviorProfile::OfficialCompatibility,
        ),
        FieldInstance::singleton(FieldId::parse("beta").unwrap()),
        vec![],
        values(),
    ));
    assert!(matches!(
        missing_cross_product,
        Err(EvaluationError::Interpreter(
            InterpreterError::MissingFieldEventProgram {
                phase: ValidationPhase::Blur,
                ..
            }
        ))
    ));

    let batch = evaluator(specification)
        .evaluate(&request(
            ValidationContext::new(
                ValidationPhase::Validate,
                BehaviorProfile::OfficialCompatibility,
            ),
            vec![],
            vec![],
            values(),
        ))
        .unwrap();
    assert_eq!(batch.report().violations()[0].message(), "matched");

    let missing_projection = spec_with_field_event_programs(
        &[],
        fields,
        &[],
        &[],
        rules,
        leaked_slice(vec![blur_alpha]),
        all_effects(),
    );
    assert!(matches!(
        validate_static_spec(missing_projection),
        Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::InvalidEventBinding {
                kind: SpecItemKind::Rule,
                value: "sparse-pairs",
            }
        ))
    ));
}

#[test]
fn exact_group_field_event_executes_only_the_matching_stable_instance() {
    let group = FieldGroupSpec {
        group_id: "rows",
        min_occurs: 2,
        max_occurs: Some(2),
        members: &["row-value"],
    };
    let effects = leaked_slice(vec![Effect::EmitIssue {
        severity: RuleSeverity::Advisory,
        message: "row",
        official_message: None,
        assessment: RuleAssessment::VerifiedCorrect,
        fields: &[],
    }]);
    let rule = RuleSpec {
        rule_id: "row-change",
        scope: EvaluationScope::EachGroup("rows"),
        order: 10,
        phases: &[ValidationPhase::Change],
        trigger_field_ids: &["row-value"],
        profiles: executable(RuleBranch {
            predicate: &TRUE,
            effects,
        }),
    };
    let programs = leaked_slice(vec![FieldEventProgramSpec {
        phase: ValidationPhase::Change,
        trigger_field_id: "row-value",
        profiles: executable(FieldEventProgram {
            steps: leaked_slice(vec![FieldEventStep::Rule {
                rule_id: "row-change",
            }]),
        }),
    }]);
    let spec = spec_with_field_event_programs(
        leaked_slice(vec![group]),
        leaked_slice(vec![grouped_string_field("row-value", "rows")]),
        &[],
        &[],
        leaked_slice(vec![rule]),
        programs,
        all_effects(),
    );
    let row_a = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows").unwrap(),
        StableInstanceId::parse("row-a").unwrap(),
    );
    let row_b = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows").unwrap(),
        StableInstanceId::parse("row-b").unwrap(),
    );
    let field_id = FieldId::parse("row-value").unwrap();
    let field_a = FieldInstance::try_new(field_id.clone(), vec![row_a.clone()]).unwrap();
    let field_b = FieldInstance::try_new(field_id, vec![row_b.clone()]).unwrap();
    let result = evaluator(spec)
        .evaluate(&field_event_request(
            ValidationContext::new(
                ValidationPhase::Change,
                BehaviorProfile::OfficialCompatibility,
            ),
            field_b.clone(),
            vec![row_a, row_b.clone()],
            vec![
                RawFieldValue::new(field_a, RawValue::Text("a".into())),
                RawFieldValue::new(field_b, RawValue::Text("b".into())),
            ],
        ))
        .unwrap();

    assert_eq!(result.report().expected_rules().len(), 1);
    assert_eq!(result.report().expected_rules()[0].instance(), Some(&row_b));
}

#[test]
fn first_blocking_issue_keeps_current_rule_resets_but_suppresses_later_rules() {
    let singleton = |field_id| FieldRef {
        field_id,
        instance: FieldInstanceSelector::Singleton,
    };
    let first_effects = leaked_slice(vec![
        Effect::EmitIssue {
            severity: RuleSeverity::Blocking,
            message: "blocking",
            official_message: Some("blocking"),
            assessment: RuleAssessment::VerifiedCorrect,
            fields: &[],
        },
        Effect::EmitIssue {
            severity: RuleSeverity::Advisory,
            message: "suppressed",
            official_message: None,
            assessment: RuleAssessment::VerifiedCorrect,
            fields: &[],
        },
        Effect::SetRawFieldValue {
            field: singleton("reset-a"),
            value: StaticRawValue::Text("0.00"),
        },
        Effect::SetRawFieldValue {
            field: singleton("reset-b"),
            value: StaticRawValue::Absent,
        },
    ]);
    let later_effects = leaked_slice(vec![Effect::SetRawFieldValue {
        field: singleton("reset-a"),
        value: StaticRawValue::Text("forged-later"),
    }]);
    let rules = leaked_slice(vec![
        RuleSpec {
            rule_id: "blocking-reset",
            scope: EvaluationScope::Singleton,
            order: 10,
            phases: &[ValidationPhase::Blur],
            trigger_field_ids: &["trigger"],
            profiles: executable(RuleBranch {
                predicate: &TRUE,
                effects: first_effects,
            }),
        },
        RuleSpec {
            rule_id: "later-reset",
            scope: EvaluationScope::Singleton,
            order: 20,
            phases: &[ValidationPhase::Blur],
            trigger_field_ids: &["trigger"],
            profiles: executable(RuleBranch {
                predicate: &TRUE,
                effects: later_effects,
            }),
        },
    ]);
    let programs = leaked_slice(vec![FieldEventProgramSpec {
        phase: ValidationPhase::Blur,
        trigger_field_id: "trigger",
        profiles: executable(FieldEventProgram {
            steps: leaked_slice(vec![
                FieldEventStep::Rule {
                    rule_id: "blocking-reset",
                },
                FieldEventStep::Rule {
                    rule_id: "later-reset",
                },
            ]),
        }),
    }]);
    let spec = spec_with_field_event_programs(
        &[],
        leaked_slice(vec![
            string_field("trigger"),
            string_field("reset-a"),
            string_field("reset-b"),
        ]),
        &[],
        &[],
        rules,
        programs,
        executable(EffectEvaluationMode::StopEffectsAfterFirstBlockingIssue),
    );
    let event_field = FieldInstance::singleton(FieldId::parse("trigger").unwrap());
    let request = field_event_request(
        ValidationContext::new(
            ValidationPhase::Blur,
            BehaviorProfile::OfficialCompatibility,
        ),
        event_field,
        Vec::new(),
        vec![
            raw_singleton("trigger", RawValue::Text("300000".into())),
            raw_singleton("reset-a", RawValue::Text("17".into())),
            raw_singleton("reset-b", RawValue::Text("18".into())),
        ],
    );
    let result = evaluator(spec).evaluate(&request).unwrap();

    assert_eq!(result.report().violations().len(), 1);
    assert_eq!(result.expected_field_value_assignments().len(), 3);
    assert_eq!(result.field_value_assignments().len(), 2);
    assert_eq!(result.field_value_assignments()[0].effect_index(), 2);
    assert_eq!(result.field_value_assignments()[1].effect_index(), 3);
    assert_eq!(
        result.field_value_assignments()[0].value(),
        &RawValue::Text("0.00".into())
    );
    assert_eq!(
        result.field_value_assignments()[1].value(),
        &RawValue::Absent
    );

    let canonical_reset_a = result
        .canonical_inputs()
        .iter()
        .find(|input| input.field().field_id().as_str() == "reset-a")
        .unwrap();
    assert_eq!(canonical_reset_a.raw(), &RawValue::Text("17".into()));
    assert_eq!(
        canonical_reset_a.canonical(),
        &CanonicalValue::Text("0.00".into())
    );

    let mut wire = serde_json::to_value(&result).unwrap();
    assert_eq!(
        wire["report"]["event_field"]["field_id"],
        serde_json::Value::String("trigger".into())
    );
    wire["field_value_assignments"][0]["effect_index"] = serde_json::Value::from(99);
    assert!(
        serde_json::from_value::<EvaluationResult>(wire)
            .unwrap_err()
            .to_string()
            .contains("does not match compiled effect")
    );

    let mut missing_event = serde_json::to_value(&result).unwrap();
    missing_event["report"]
        .as_object_mut()
        .unwrap()
        .remove("event_field");
    assert!(
        serde_json::from_value::<EvaluationResult>(missing_event)
            .unwrap_err()
            .to_string()
            .contains("missing its event field")
    );
}

#[test]
fn phase_local_rule_orders_allow_disjoint_phases_and_evaluate_only_the_requested_phase() {
    let rules = leaked_slice(vec![
        blocking_rule("validate-first", 1, VALIDATE, "validate issue"),
        blocking_rule("save-first", 1, SAVE, "save issue"),
    ]);
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        rules,
        all_effects(),
    );

    let evaluate_phase = |phase| {
        evaluator(spec)
            .evaluate(&request(
                ValidationContext::new(phase, BehaviorProfile::OfficialCompatibility),
                vec![],
                vec![],
                vec![raw_singleton("name", RawValue::Text("x".into()))],
            ))
            .expect("disjoint phase-local orders evaluate")
    };

    let save = evaluate_phase(ValidationPhase::Save);
    assert_eq!(save.report().expected_rules().len(), 1);
    assert_eq!(
        save.report().expected_rules()[0]
            .execution()
            .rule_id()
            .as_str(),
        "save-first"
    );
    assert_eq!(save.report().violations()[0].message(), "save issue");

    let validate = evaluate_phase(ValidationPhase::Validate);
    assert_eq!(validate.report().expected_rules().len(), 1);
    assert_eq!(
        validate.report().expected_rules()[0]
            .execution()
            .rule_id()
            .as_str(),
        "validate-first"
    );
    assert_eq!(
        validate.report().violations()[0].message(),
        "validate issue"
    );
}

#[test]
fn phase_local_rule_orders_reject_same_phase_collisions() {
    let rules = leaked_slice(vec![
        blocking_rule("validate-a", 1, VALIDATE, "a"),
        blocking_rule("validate-b", 1, VALIDATE, "b"),
    ]);
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        rules,
        all_effects(),
    );

    assert!(matches!(
        validate_static_spec(spec),
        Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::DuplicateRuleOrder {
                phase: ValidationPhase::Validate,
                order: 1,
            }
        ))
    ));
}

#[test]
fn phase_local_rule_orders_reject_overlapping_multi_phase_collisions() {
    let rules = leaked_slice(vec![
        blocking_rule("save-and-validate", 1, SAVE_AND_VALIDATE, "both"),
        blocking_rule("validate-only", 1, VALIDATE, "validate"),
    ]);
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        rules,
        all_effects(),
    );

    assert!(matches!(
        validate_static_spec(spec),
        Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::DuplicateRuleOrder {
                phase: ValidationPhase::Validate,
                order: 1,
            }
        ))
    ));
}

#[test]
fn calculation_order_uses_evaluation_order_and_exact_decimal_values() {
    let amount = leaked(Expression::Field {
        result_type: ValueType::Decimal,
        field: FieldRef {
            field_id: "amount",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    let rate = leaked(Expression::Context {
        result_type: ValueType::Decimal,
        context_value_id: "rate",
    });
    let tax_value = leaked(Expression::Binary {
        result_type: ValueType::Decimal,
        operator: BinaryOperator::Multiply,
        division_policy: None,
        left: amount,
        right: rate,
    });
    let tax_derived = leaked(Expression::Derived {
        result_type: ValueType::Decimal,
        calculation_id: "tax",
        output_id: "due",
        instance: DerivedInstanceSelector::Singleton,
    });
    let two = leaked(Expression::Literal(TypedValue::Decimal(DecimalLiteral {
        coefficient: 2,
        scale: 0,
    })));
    let double_value = leaked(Expression::Binary {
        result_type: ValueType::Decimal,
        operator: BinaryOperator::Multiply,
        division_policy: None,
        left: tax_derived,
        right: two,
    });
    let tax = CalculationSpec {
        calculation_id: "tax",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "due",
                value: tax_value,
                rounding: Some(&[Rounding {
                    mode: RoundingMode::HalfUp,
                    scale: 2,
                }]),
                writeback: None,
            }]),
        }),
    };
    let double = CalculationSpec {
        calculation_id: "double",
        scope: EvaluationScope::Singleton,
        depends_on: &["tax"],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "total",
                value: double_value,
                rounding: None,
                writeback: None,
            }]),
        }),
    };
    let spec = spec(
        leaked_slice(vec![ContextValueSpec {
            context_value_id: "rate",
            value_type: ValueType::Decimal,
            required: true,
        }]),
        &[],
        leaked_slice(vec![decimal_field("amount", None)]),
        &["tax", "double"],
        leaked_slice(vec![double, tax]),
        &[],
        all_effects(),
    );
    let result = evaluator(spec)
        .evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![ContextValue::new(
                ContextValueId::parse("rate").unwrap(),
                CanonicalValue::Decimal("0.125".parse().unwrap()),
            )],
            vec![],
            vec![raw_singleton("amount", RawValue::Text("100.00".into()))],
        ))
        .unwrap();

    assert_eq!(
        result
            .expected_outputs()
            .iter()
            .map(|output| (
                output.calculation_id().as_str(),
                output.output_id().as_str()
            ))
            .collect::<Vec<_>>(),
        vec![("tax", "due"), ("double", "total")]
    );
    assert_eq!(
        result
            .derived_outputs()
            .iter()
            .map(|output| output.value().clone())
            .collect::<Vec<_>>(),
        vec![
            CanonicalValue::Decimal("12.5".parse().unwrap()),
            CanonicalValue::Decimal("25".parse().unwrap())
        ]
    );
}

#[test]
fn calculation_rounding_pipeline_applies_half_ceiling_steps_in_declared_order() {
    let pipeline = leaked_slice(vec![
        Rounding {
            mode: RoundingMode::HalfUp,
            scale: 2,
        },
        Rounding {
            mode: RoundingMode::HalfCeiling,
            scale: 0,
        },
    ]);
    let rounded = |coefficient| {
        evaluate_one_output_with_rounding(decimal_literal(coefficient, 3), Some(pipeline))
            .unwrap()
            .derived_outputs()[0]
            .value()
            .clone()
    };

    for (coefficient, expected) in [
        (494, "0"),
        (499, "1"),
        (-499, "0"),
        (-501, "0"),
        (-506, "-1"),
        (-1499, "-1"),
        (-1506, "-2"),
    ] {
        assert_eq!(
            rounded(coefficient),
            CanonicalValue::Decimal(expected.parse().unwrap()),
            "pipeline result for coefficient {coefficient}"
        );
    }

    let reversed = leaked_slice(vec![pipeline[1], pipeline[0]]);
    assert_eq!(
        evaluate_one_output_with_rounding(decimal_literal(499, 3), Some(reversed))
            .unwrap()
            .derived_outputs()[0]
            .value(),
        &CanonicalValue::Decimal("0".parse().unwrap()),
        "reversing the steps must change the result"
    );
}

#[test]
fn static_validation_rejects_empty_or_partially_invalid_rounding_pipelines() {
    assert!(matches!(
        evaluate_one_output_with_rounding(decimal_literal(499, 3), Some(&[])),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::EmptyRequiredList {
                kind: SpecItemKind::Output,
                value: "output",
            })
        ))
    ));

    let invalid_second_step = leaked_slice(vec![
        Rounding {
            mode: RoundingMode::HalfUp,
            scale: 2,
        },
        Rounding {
            mode: RoundingMode::HalfCeiling,
            scale: 19,
        },
    ]);
    assert!(matches!(
        evaluate_one_output_with_rounding(decimal_literal(499, 3), Some(invalid_second_step)),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidRoundingScale {
                scale: 19,
            })
        ))
    ));
}

#[test]
fn false_calculation_condition_still_emits_every_expected_output_as_absent() {
    let false_predicate = leaked(Predicate::Constant(false));
    let one = leaked(Expression::Literal(TypedValue::Decimal(DecimalLiteral {
        coefficient: 1,
        scale: 0,
    })));
    let zero = leaked(Expression::Literal(TypedValue::Decimal(DecimalLiteral {
        coefficient: 0,
        scale: 0,
    })));
    let would_fail = leaked(Expression::Binary {
        result_type: ValueType::Decimal,
        operator: BinaryOperator::Divide,
        division_policy: Some(division_policy(2, RoundingMode::HalfUp)),
        left: one,
        right: zero,
    });
    let calculation = CalculationSpec {
        calculation_id: "conditional",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: false_predicate,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "still-covered",
                value: would_fail,
                rounding: None,
                writeback: None,
            }]),
        }),
    };
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &["conditional"],
        leaked_slice(vec![calculation]),
        &[],
        all_effects(),
    );
    let result = evaluator(spec)
        .evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![],
            vec![],
            vec![raw_singleton("name", RawValue::Text("x".into()))],
        ))
        .unwrap();

    assert_eq!(result.expected_outputs().len(), 1);
    assert_eq!(result.derived_outputs().len(), 1);
    assert_eq!(result.derived_outputs()[0].value(), &CanonicalValue::Absent);
}

#[test]
fn repeated_group_lookup_and_aggregate_are_stable_by_instance_identity() {
    let row_amount = leaked(Expression::Field {
        result_type: ValueType::Decimal,
        field: FieldRef {
            field_id: "row-amount",
            instance: FieldInstanceSelector::CurrentGroupInstance,
        },
    });
    let aggregate = leaked(Expression::GroupAggregate {
        result_type: ValueType::Decimal,
        operator: GroupAggregateOperator::Sum,
        group_id: "rows",
        value: row_amount,
    });
    let calculation = CalculationSpec {
        calculation_id: "sum-rows",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "total",
                value: aggregate,
                rounding: None,
                writeback: None,
            }]),
        }),
    };
    let spec = spec(
        &[],
        leaked_slice(vec![FieldGroupSpec {
            group_id: "rows",
            min_occurs: 0,
            max_occurs: None,
            members: &["row-amount"],
        }]),
        leaked_slice(vec![decimal_field("row-amount", Some("rows"))]),
        &["sum-rows"],
        leaked_slice(vec![calculation]),
        &[],
        all_effects(),
    );
    let row_a = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows").unwrap(),
        StableInstanceId::parse("row-a").unwrap(),
    );
    let row_b = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows").unwrap(),
        StableInstanceId::parse("row-b").unwrap(),
    );
    let field_a =
        FieldInstance::try_new(FieldId::parse("row-amount").unwrap(), vec![row_a.clone()]).unwrap();
    let field_b =
        FieldInstance::try_new(FieldId::parse("row-amount").unwrap(), vec![row_b.clone()]).unwrap();
    let result = evaluator(spec)
        .evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![],
            vec![row_b, row_a],
            vec![
                RawFieldValue::new(field_b, RawValue::Text("2.25".into())),
                RawFieldValue::new(field_a, RawValue::Text("1.75".into())),
            ],
        ))
        .unwrap();

    assert_eq!(
        result.derived_outputs()[0].value(),
        &CanonicalValue::Decimal("4".parse().unwrap())
    );
}

#[test]
fn group_scoped_outputs_rules_and_derived_aggregate_share_stable_instances() {
    let row_value = leaked(Expression::Field {
        result_type: ValueType::Decimal,
        field: FieldRef {
            field_id: "row-value",
            instance: FieldInstanceSelector::CurrentGroupInstance,
        },
    });
    let row_derived = leaked(Expression::Derived {
        result_type: ValueType::Decimal,
        calculation_id: "row-calculation",
        output_id: "row-output",
        instance: DerivedInstanceSelector::CurrentGroupInstance,
    });
    let total_value = leaked(Expression::GroupAggregate {
        result_type: ValueType::Decimal,
        operator: GroupAggregateOperator::Sum,
        group_id: "rows",
        value: row_derived,
    });
    let calculations = leaked_slice(vec![
        CalculationSpec {
            calculation_id: "row-calculation",
            scope: EvaluationScope::EachGroup("rows"),
            depends_on: &[],
            phases: VALIDATE,
            trigger_field_ids: &[],
            profiles: executable(CalculationBranch {
                condition: &TRUE,
                outputs: leaked_slice(vec![CalculationOutput {
                    output_id: "row-output",
                    value: row_value,
                    rounding: None,
                    writeback: None,
                }]),
            }),
        },
        CalculationSpec {
            calculation_id: "total-calculation",
            scope: EvaluationScope::Singleton,
            depends_on: &["row-calculation"],
            phases: VALIDATE,
            trigger_field_ids: &[],
            profiles: executable(CalculationBranch {
                condition: &TRUE,
                outputs: leaked_slice(vec![CalculationOutput {
                    output_id: "total",
                    value: total_value,
                    rounding: None,
                    writeback: None,
                }]),
            }),
        },
    ]);
    let rules = leaked_slice(vec![RuleSpec {
        rule_id: "row-rule",
        scope: EvaluationScope::EachGroup("rows"),
        order: 1,
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(RuleBranch {
            predicate: &TRUE,
            effects: leaked_slice(vec![Effect::EmitIssue {
                severity: RuleSeverity::Advisory,
                message: "row observed",
                official_message: None,
                assessment: RuleAssessment::VerifiedCorrect,
                fields: leaked_slice(vec![FieldRef {
                    field_id: "row-value",
                    instance: FieldInstanceSelector::CurrentGroupInstance,
                }]),
            }]),
        }),
    }]);
    let specification = spec(
        &[],
        leaked_slice(vec![FieldGroupSpec {
            group_id: "rows",
            min_occurs: 0,
            max_occurs: None,
            members: &["row-value"],
        }]),
        leaked_slice(vec![decimal_field("row-value", Some("rows"))]),
        &["row-calculation", "total-calculation"],
        calculations,
        rules,
        all_effects(),
    );
    let row_a = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows").unwrap(),
        StableInstanceId::parse("row-a").unwrap(),
    );
    let row_b = RepeatedGroupInstance::new(
        RepeatedGroupId::parse("rows").unwrap(),
        StableInstanceId::parse("row-b").unwrap(),
    );
    let field_a =
        FieldInstance::try_new(FieldId::parse("row-value").unwrap(), vec![row_a.clone()]).unwrap();
    let field_b =
        FieldInstance::try_new(FieldId::parse("row-value").unwrap(), vec![row_b.clone()]).unwrap();
    let raw_a = RawFieldValue::new(field_a, RawValue::Text("1.75".into()));
    let raw_b = RawFieldValue::new(field_b, RawValue::Text("2.25".into()));

    let result = evaluator(specification)
        .evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![],
            vec![row_b.clone(), row_a.clone()],
            vec![raw_b.clone(), raw_a.clone()],
        ))
        .unwrap();
    let permuted = evaluator(specification)
        .evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![],
            vec![row_a.clone(), row_b.clone()],
            vec![raw_a, raw_b],
        ))
        .unwrap();
    assert_eq!(result, permuted);

    assert_eq!(result.derived_outputs().len(), 3);
    assert_eq!(result.derived_outputs()[0].instance(), Some(&row_a));
    assert_eq!(result.derived_outputs()[1].instance(), Some(&row_b));
    assert_eq!(result.derived_outputs()[2].instance(), None);
    assert_eq!(
        result.derived_outputs()[2].value(),
        &CanonicalValue::Decimal("4".parse().unwrap())
    );
    assert_eq!(result.report().expected_rules().len(), 2);
    assert_eq!(result.report().expected_rules()[0].instance(), Some(&row_a));
    assert_eq!(result.report().expected_rules()[1].instance(), Some(&row_b));
    assert_eq!(result.report().violations().len(), 2);
    assert_eq!(result.report().violations()[0].instance(), Some(&row_a));
    assert_eq!(result.report().violations()[1].instance(), Some(&row_b));
    assert_eq!(result.report().violations()[0].order().occurrence(), 0);
    assert_eq!(result.report().violations()[1].order().occurrence(), 1);
}

#[test]
fn empty_group_sum_returns_zero_with_the_member_numeric_type() {
    for (field, result_type, expected) in [
        (
            decimal_field("row-value", Some("rows")),
            ValueType::Decimal,
            CanonicalValue::Decimal("0".parse().unwrap()),
        ),
        (
            integer_field("row-value", Some("rows")),
            ValueType::Integer,
            CanonicalValue::Integer(0),
        ),
    ] {
        let row_value = leaked(Expression::Field {
            result_type,
            field: FieldRef {
                field_id: "row-value",
                instance: FieldInstanceSelector::CurrentGroupInstance,
            },
        });
        let aggregate = leaked(Expression::GroupAggregate {
            result_type,
            operator: GroupAggregateOperator::Sum,
            group_id: "rows",
            value: row_value,
        });
        let calculation = CalculationSpec {
            calculation_id: "sum-rows",
            scope: EvaluationScope::Singleton,
            depends_on: &[],
            phases: VALIDATE,
            trigger_field_ids: &[],
            profiles: executable(CalculationBranch {
                condition: &TRUE,
                outputs: leaked_slice(vec![CalculationOutput {
                    output_id: "total",
                    value: aggregate,
                    rounding: None,
                    writeback: None,
                }]),
            }),
        };
        let specification = spec(
            &[],
            leaked_slice(vec![FieldGroupSpec {
                group_id: "rows",
                min_occurs: 0,
                max_occurs: None,
                members: &["row-value"],
            }]),
            leaked_slice(vec![field]),
            &["sum-rows"],
            leaked_slice(vec![calculation]),
            &[],
            all_effects(),
        );
        let result = evaluator(specification)
            .evaluate(&request(
                ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
                vec![],
                vec![],
                vec![],
            ))
            .unwrap();

        assert_eq!(result.derived_outputs()[0].value(), &expected);
    }
}

#[test]
fn nested_group_aggregate_is_rejected_before_any_row_is_evaluated() {
    let row_value = leaked(Expression::Field {
        result_type: ValueType::Integer,
        field: FieldRef {
            field_id: "row-value",
            instance: FieldInstanceSelector::CurrentGroupInstance,
        },
    });
    let inner = leaked(Expression::GroupAggregate {
        result_type: ValueType::Integer,
        operator: GroupAggregateOperator::Sum,
        group_id: "rows",
        value: row_value,
    });
    let outer = leaked(Expression::GroupAggregate {
        result_type: ValueType::Integer,
        operator: GroupAggregateOperator::Sum,
        group_id: "rows",
        value: inner,
    });
    let calculation = CalculationSpec {
        calculation_id: "nested",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "total",
                value: outer,
                rounding: None,
                writeback: None,
            }]),
        }),
    };
    let specification = spec(
        &[],
        leaked_slice(vec![FieldGroupSpec {
            group_id: "rows",
            min_occurs: 0,
            max_occurs: None,
            members: &["row-value"],
        }]),
        leaked_slice(vec![integer_field("row-value", Some("rows"))]),
        &["nested"],
        leaked_slice(vec![calculation]),
        &[],
        all_effects(),
    );

    assert!(matches!(
        evaluator(specification).evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![],
            vec![],
            vec![],
        )),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
                target: "nested-group-aggregate",
                ..
            })
        ))
    ));
}

#[test]
fn exact_profile_and_phase_selection_never_falls_back() {
    let rule = RuleSpec {
        rule_id: "official-only",
        scope: EvaluationScope::Singleton,
        order: 1,
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: Profiled {
            official: Branch::Executable(RuleBranch {
                predicate: &TRUE,
                effects: leaked_slice(vec![Effect::EmitIssue {
                    severity: RuleSeverity::Advisory,
                    message: "official",
                    official_message: Some("official"),
                    assessment: RuleAssessment::OfficialBugCompatible,
                    fields: &[],
                }]),
            }),
            filing_safe: Branch::Unresolved,
        },
    };
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        leaked_slice(vec![rule]),
        all_effects(),
    );
    let evaluator = evaluator(spec);
    let filing_safe = evaluator.evaluate(&request(
        ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("x".into()))],
    ));
    assert!(matches!(
        filing_safe,
        Err(EvaluationError::Interpreter(
            InterpreterError::BranchUnavailable {
                kind: SpecItemKind::Rule,
                state: BranchState::Unresolved,
                ..
            }
        ))
    ));

    let different_phase = evaluator
        .evaluate(&request(
            ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
            vec![],
            vec![],
            vec![raw_singleton("name", RawValue::Text("x".into()))],
        ))
        .unwrap();
    assert!(different_phase.report().expected_rules().is_empty());
}

#[test]
fn unresolved_profiled_effect_policy_fails_instead_of_defaulting() {
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        &[],
        unavailable_filing_safe_effects(EffectEvaluationMode::ApplyAll),
    );
    let result = evaluator(spec).evaluate(&request(
        ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("x".into()))],
    ));

    assert!(matches!(
        result,
        Err(EvaluationError::Interpreter(
            InterpreterError::BranchUnavailable {
                kind: SpecItemKind::EvaluationPolicy,
                state: BranchState::Unresolved,
                ..
            }
        ))
    ));
}

#[test]
fn missing_duplicate_and_invalid_inputs_have_distinct_typed_errors() {
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![decimal_field("amount", None)]),
        &[],
        &[],
        &[],
        all_effects(),
    );
    let evaluator = evaluator(spec);
    let context = ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe);

    let missing = evaluator.evaluate(&request(context, vec![], vec![], vec![]));
    assert!(matches!(
        missing,
        Err(EvaluationError::Interpreter(
            InterpreterError::MissingInput { .. }
        ))
    ));

    let duplicate = EvaluationRequest::try_new(
        identity(),
        context,
        InputRevision::new(1),
        vec![],
        vec![],
        vec![
            raw_singleton("amount", RawValue::Text("1".into())),
            raw_singleton("amount", RawValue::Text("2".into())),
        ],
    );
    assert!(matches!(
        duplicate,
        Err(EvaluationError::InvalidInputSnapshot(
            InputSnapshotError::DuplicateFieldInstance { .. }
        ))
    ));

    let invalid = evaluator.evaluate(&request(
        context,
        vec![],
        vec![],
        vec![raw_singleton(
            "amount",
            RawValue::Text("not-a-number".into()),
        )],
    ));
    assert!(matches!(
        invalid,
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidCoercion {
                target: ValueType::Decimal,
                reason: CoercionFailure::InvalidSyntax,
            }
        ))
    ));
}

fn one_output_spec_with_rounding(
    expression: &'static Expression,
    rounding: Option<&'static [Rounding]>,
) -> &'static StaticRuleSetSpec {
    let calculation = CalculationSpec {
        calculation_id: "calculation",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "output",
                value: expression,
                rounding,
                writeback: None,
            }]),
        }),
    };
    spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &["calculation"],
        leaked_slice(vec![calculation]),
        &[],
        all_effects(),
    )
}

fn one_output_spec(expression: &'static Expression) -> &'static StaticRuleSetSpec {
    one_output_spec_with_rounding(expression, None)
}

fn evaluate_one_output(
    expression: &'static Expression,
) -> Result<crate::EvaluationResult, EvaluationError> {
    evaluator(one_output_spec(expression)).evaluate(&request(
        ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("x".into()))],
    ))
}

fn evaluate_one_output_with_rounding(
    expression: &'static Expression,
    rounding: Option<&'static [Rounding]>,
) -> Result<crate::EvaluationResult, EvaluationError> {
    evaluator(one_output_spec_with_rounding(expression, rounding)).evaluate(&request(
        ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("x".into()))],
    ))
}

fn evaluate_one_rule(
    predicate: &'static Predicate,
) -> Result<crate::EvaluationResult, EvaluationError> {
    let rule = RuleSpec {
        rule_id: "rule",
        scope: EvaluationScope::Singleton,
        order: 1,
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(RuleBranch {
            predicate,
            effects: leaked_slice(vec![Effect::EmitIssue {
                severity: RuleSeverity::Advisory,
                message: "matched",
                official_message: None,
                assessment: RuleAssessment::VerifiedCorrect,
                fields: &[],
            }]),
        }),
    };
    let specification = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        leaked_slice(vec![rule]),
        all_effects(),
    );
    evaluator(specification).evaluate(&request(
        ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("x".into()))],
    ))
}

#[test]
fn arithmetic_failures_are_typed_and_never_use_floating_point() {
    let one = decimal_literal(1, 0);
    let zero = decimal_literal(0, 0);
    let divide_zero = leaked(Expression::Binary {
        result_type: ValueType::Decimal,
        operator: BinaryOperator::Divide,
        division_policy: Some(division_policy(2, RoundingMode::HalfUp)),
        left: one,
        right: zero,
    });
    assert!(matches!(
        evaluate_one_output(divide_zero),
        Err(EvaluationError::Interpreter(
            InterpreterError::DivisionByZero {
                operation: ExecutionOperation::Divide,
            }
        ))
    ));

    let three = decimal_literal(3, 0);
    let non_terminating = leaked(Expression::Binary {
        result_type: ValueType::Decimal,
        operator: BinaryOperator::Divide,
        division_policy: Some(division_policy(2, RoundingMode::None)),
        left: one,
        right: three,
    });
    assert!(matches!(
        evaluate_one_output(non_terminating),
        Err(EvaluationError::Interpreter(
            InterpreterError::NonTerminatingDecimalDivision
        ))
    ));

    let maximum = leaked(Expression::Literal(TypedValue::Integer(i128::MAX)));
    let integer_one = leaked(Expression::Literal(TypedValue::Integer(1)));
    let overflow = leaked(Expression::Binary {
        result_type: ValueType::Integer,
        operator: BinaryOperator::Add,
        division_policy: None,
        left: maximum,
        right: integer_one,
    });
    assert!(matches!(
        evaluate_one_output(overflow),
        Err(EvaluationError::Interpreter(InterpreterError::Overflow {
            operation: ExecutionOperation::Add,
        }))
    ));

    let decimal_maximum = decimal_literal(i128::MAX, 0);
    let division_overflow = leaked(Expression::Binary {
        result_type: ValueType::Decimal,
        operator: BinaryOperator::Divide,
        division_policy: Some(division_policy(18, RoundingMode::HalfUp)),
        left: decimal_maximum,
        right: one,
    });
    assert!(matches!(
        evaluate_one_output(division_overflow),
        Err(EvaluationError::Interpreter(InterpreterError::Overflow {
            operation: ExecutionOperation::Divide,
        }))
    ));
}

#[test]
fn decimal_division_rounds_repeating_ratios_and_ties_deterministically() {
    fn divide(
        numerator: i128,
        denominator: i128,
        scale: u32,
        rounding: RoundingMode,
    ) -> CanonicalValue {
        let expression = leaked(Expression::Binary {
            result_type: ValueType::Decimal,
            operator: BinaryOperator::Divide,
            division_policy: Some(division_policy(scale, rounding)),
            left: decimal_literal(numerator, 0),
            right: decimal_literal(denominator, 0),
        });
        evaluate_one_output(expression).unwrap().derived_outputs()[0]
            .value()
            .clone()
    }

    assert_eq!(
        divide(1, 3, 2, RoundingMode::HalfUp),
        CanonicalValue::Decimal("0.33".parse().unwrap())
    );
    assert_eq!(
        divide(1, 8, 2, RoundingMode::HalfUp),
        CanonicalValue::Decimal("0.13".parse().unwrap())
    );
    assert_eq!(
        divide(1, 8, 2, RoundingMode::HalfEven),
        CanonicalValue::Decimal("0.12".parse().unwrap())
    );
    assert_eq!(
        divide(3, 8, 2, RoundingMode::HalfEven),
        CanonicalValue::Decimal("0.38".parse().unwrap())
    );
}

#[test]
fn decimal_division_accepts_representable_results_after_wide_intermediates() {
    fn divide(
        numerator: i128,
        denominator: i128,
        scale: u32,
        rounding: RoundingMode,
    ) -> CanonicalValue {
        let expression = leaked(Expression::Binary {
            result_type: ValueType::Decimal,
            operator: BinaryOperator::Divide,
            division_policy: Some(division_policy(scale, rounding)),
            left: decimal_literal(numerator, 0),
            right: decimal_literal(denominator, 0),
        });
        evaluate_one_output(expression).unwrap().derived_outputs()[0]
            .value()
            .clone()
    }

    assert_eq!(
        divide(i128::MAX - 1, i128::MAX, 18, RoundingMode::TowardZero),
        CanonicalValue::Decimal("0.999999999999999999".parse().unwrap())
    );
    assert_eq!(
        divide(i128::MIN, i128::MIN, 0, RoundingMode::None),
        CanonicalValue::Decimal("1".parse().unwrap())
    );
}

#[test]
fn decimal_division_rejects_true_final_coefficient_overflow() {
    fn assert_overflow(numerator: i128, denominator: i128, scale: u32, rounding: RoundingMode) {
        let expression = leaked(Expression::Binary {
            result_type: ValueType::Decimal,
            operator: BinaryOperator::Divide,
            division_policy: Some(division_policy(scale, rounding)),
            left: decimal_literal(numerator, 0),
            right: decimal_literal(denominator, 0),
        });
        assert!(matches!(
            evaluate_one_output(expression),
            Err(EvaluationError::Interpreter(InterpreterError::Overflow {
                operation: ExecutionOperation::Divide,
            }))
        ));
    }

    assert_overflow(i128::MAX, 1, 1, RoundingMode::TowardZero);
    assert_overflow(i128::MIN, -1, 0, RoundingMode::TowardZero);
}

#[test]
fn static_validation_enforces_decimal_division_policy_and_types() {
    let one = decimal_literal(1, 0);
    let two = decimal_literal(2, 0);
    let missing = leaked(Expression::Binary {
        result_type: ValueType::Decimal,
        operator: BinaryOperator::Divide,
        division_policy: None,
        left: one,
        right: two,
    });
    assert!(matches!(
        evaluate_one_output(missing),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::MissingDecimalDivisionPolicy)
        ))
    ));

    let unexpected = leaked(Expression::Binary {
        result_type: ValueType::Decimal,
        operator: BinaryOperator::Add,
        division_policy: Some(division_policy(2, RoundingMode::HalfUp)),
        left: one,
        right: two,
    });
    assert!(matches!(
        evaluate_one_output(unexpected),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::UnexpectedDecimalDivisionPolicy {
                operator: BinaryOperator::Add,
            })
        ))
    ));

    let invalid_scale = leaked(Expression::Binary {
        result_type: ValueType::Decimal,
        operator: BinaryOperator::Divide,
        division_policy: Some(division_policy(19, RoundingMode::HalfUp)),
        left: one,
        right: two,
    });
    assert!(matches!(
        evaluate_one_output(invalid_scale),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidDecimalDivisionScale {
                scale: 19,
            })
        ))
    ));

    let integer_one = leaked(Expression::Literal(TypedValue::Integer(1)));
    let integer_two = leaked(Expression::Literal(TypedValue::Integer(2)));
    let integer_divide = leaked(Expression::Binary {
        result_type: ValueType::Integer,
        operator: BinaryOperator::Divide,
        division_policy: Some(division_policy(0, RoundingMode::TowardZero)),
        left: integer_one,
        right: integer_two,
    });
    assert!(matches!(
        evaluate_one_output(integer_divide),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::TypeMismatch {
                operation: ExecutionOperation::Divide,
                expected: ValueType::Decimal,
                actual: ValueType::Integer,
            })
        ))
    ));
}

#[test]
fn static_validation_reports_missing_context_derived_and_type_mismatch() {
    let context_expression = leaked(Expression::Context {
        result_type: ValueType::Decimal,
        context_value_id: "missing-rate",
    });
    assert!(matches!(
        evaluate_one_output(context_expression),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
                target: "missing-rate",
                ..
            })
        ))
    ));

    let derived_expression = leaked(Expression::Derived {
        result_type: ValueType::Decimal,
        calculation_id: "other",
        output_id: "missing",
        instance: DerivedInstanceSelector::Singleton,
    });
    assert!(matches!(
        evaluate_one_output(derived_expression),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
                target: "other",
                ..
            })
        ))
    ));

    let text = leaked(Expression::Literal(TypedValue::String("text")));
    let invalid_add = leaked(Expression::Binary {
        result_type: ValueType::String,
        operator: BinaryOperator::Add,
        division_policy: None,
        left: text,
        right: text,
    });
    assert!(matches!(
        evaluate_one_output(invalid_add),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::TypeMismatch {
                operation: ExecutionOperation::Add,
                ..
            })
        ))
    ));
}

#[test]
fn static_validation_rejects_invalid_reference_in_dead_conditional_branch() {
    let never = leaked(Predicate::Constant(false));
    let invalid = leaked(Expression::Field {
        result_type: ValueType::String,
        field: FieldRef {
            field_id: "missing-field",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    let fallback = leaked(Expression::Literal(TypedValue::String("safe")));
    let conditional = leaked(Expression::Conditional {
        result_type: ValueType::String,
        condition: never,
        when_true: invalid,
        when_false: fallback,
    });

    assert!(matches!(
        evaluate_one_output(conditional),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
                target: "missing-field",
                ..
            })
        ))
    ));
}

#[test]
fn static_validation_closes_coercion_failed_field_and_profile_scope() {
    let string_predicate = leaked(Predicate::CoercionFailed {
        field: FieldRef {
            field_id: "name",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    assert!(matches!(
        evaluate_one_rule(string_predicate),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidCoercionFailedPredicate {
                field_id: "name",
                profile: BehaviorProfile::OfficialCompatibility,
            })
        ))
    ));

    let preserve_raw = FieldBehavior {
        normalization: EMPTY_NORMALIZATION,
        event_normalization: &[],
        coercion: Coercion::Integer {
            on_empty: NumericEmptyPolicy::Null,
            on_invalid: InvalidValuePolicy::PreserveRaw,
        },
    };
    let reject_invalid = FieldBehavior {
        normalization: EMPTY_NORMALIZATION,
        event_normalization: &[],
        coercion: Coercion::Integer {
            on_empty: NumericEmptyPolicy::Null,
            on_invalid: InvalidValuePolicy::Error,
        },
    };
    let field = FieldSpec {
        field_id: "amount",
        value_type: ValueType::Integer,
        group_id: None,
        calculation_id: None,
        behavior: Profiled {
            official: Branch::Executable(reject_invalid),
            filing_safe: Branch::Executable(preserve_raw),
        },
    };
    let predicate = leaked(Predicate::CoercionFailed {
        field: FieldRef {
            field_id: "amount",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    let rule = RuleSpec {
        rule_id: "invalid-amount",
        scope: EvaluationScope::Singleton,
        order: 1,
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: Profiled {
            official: Branch::Unresolved,
            filing_safe: Branch::Executable(RuleBranch {
                predicate,
                effects: leaked_slice(vec![Effect::EmitIssue {
                    severity: RuleSeverity::Advisory,
                    message: "invalid amount",
                    official_message: None,
                    assessment: RuleAssessment::VerifiedCorrect,
                    fields: &[],
                }]),
            }),
        },
    };
    let profile_local_spec = spec(
        &[],
        &[],
        leaked_slice(vec![field]),
        &[],
        &[],
        leaked_slice(vec![rule]),
        all_effects(),
    );
    let result = evaluator(profile_local_spec)
        .evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![],
            vec![],
            vec![raw_singleton(
                "amount",
                RawValue::Text("invalid".to_owned()),
            )],
        ))
        .expect("filing-safe validation must ignore the unresolved official rule branch");
    assert_eq!(result.report().violations().len(), 1);

    let missing = leaked(Predicate::CoercionFailed {
        field: FieldRef {
            field_id: "missing",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    assert!(matches!(
        evaluate_one_rule(missing),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
                target: "missing",
                ..
            })
        ))
    ));
}

#[test]
fn static_validation_rejects_unknown_group_quantifier() {
    let predicate = leaked(Predicate::GroupQuantifier {
        quantifier: GroupQuantifier::All,
        group_id: "missing-rows",
        predicate: &TRUE,
    });

    assert!(matches!(
        evaluate_one_rule(predicate),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
                target: "missing-rows",
                ..
            })
        ))
    ));
}

#[test]
fn static_validation_rejects_cross_phase_and_profile_dependencies() {
    let one = leaked(Expression::Literal(TypedValue::Integer(1)));
    let derived = leaked(Expression::Derived {
        result_type: ValueType::Integer,
        calculation_id: "base",
        output_id: "value",
        instance: DerivedInstanceSelector::Singleton,
    });
    let base_branch = CalculationBranch {
        condition: &TRUE,
        outputs: leaked_slice(vec![CalculationOutput {
            output_id: "value",
            value: one,
            rounding: None,
            writeback: None,
        }]),
    };
    let dependent_branch = CalculationBranch {
        condition: &TRUE,
        outputs: leaked_slice(vec![CalculationOutput {
            output_id: "result",
            value: derived,
            rounding: None,
            writeback: None,
        }]),
    };
    let cross_phase = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &["base", "dependent"],
        leaked_slice(vec![
            CalculationSpec {
                calculation_id: "base",
                scope: EvaluationScope::Singleton,
                depends_on: &[],
                phases: SAVE,
                trigger_field_ids: &[],
                profiles: executable(base_branch),
            },
            CalculationSpec {
                calculation_id: "dependent",
                scope: EvaluationScope::Singleton,
                depends_on: &["base"],
                phases: VALIDATE,
                trigger_field_ids: &[],
                profiles: executable(dependent_branch),
            },
        ]),
        &[],
        all_effects(),
    );
    let evaluate = |specification| {
        evaluator(specification).evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![],
            vec![],
            vec![raw_singleton("name", RawValue::Text("x".into()))],
        ))
    };
    assert!(matches!(
        evaluate(cross_phase),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
                value: "dependent",
                target: "base",
                ..
            })
        ))
    ));

    let cross_profile = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &["base", "dependent"],
        leaked_slice(vec![
            CalculationSpec {
                calculation_id: "base",
                scope: EvaluationScope::Singleton,
                depends_on: &[],
                phases: VALIDATE,
                trigger_field_ids: &[],
                profiles: Profiled {
                    official: Branch::Executable(base_branch),
                    filing_safe: Branch::Unresolved,
                },
            },
            CalculationSpec {
                calculation_id: "dependent",
                scope: EvaluationScope::Singleton,
                depends_on: &["base"],
                phases: VALIDATE,
                trigger_field_ids: &[],
                profiles: Profiled {
                    official: Branch::Unresolved,
                    filing_safe: Branch::Executable(dependent_branch),
                },
            },
        ]),
        &[],
        all_effects(),
    );
    assert!(matches!(
        evaluate(cross_profile),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
                value: "dependent",
                target: "base",
                ..
            })
        ))
    ));
}

#[test]
fn static_validation_rejects_equality_type_mismatch() {
    let integer = leaked(Expression::Literal(TypedValue::Integer(1)));
    let text = leaked(Expression::Literal(TypedValue::String("1")));
    let predicate = leaked(Predicate::Compare {
        operator: CompareOperator::Equal,
        left: integer,
        right: text,
    });

    assert!(matches!(
        evaluate_one_rule(predicate),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::TypeMismatch {
                operation: ExecutionOperation::Compare,
                expected: ValueType::Integer,
                actual: ValueType::String,
            })
        ))
    ));
}

#[test]
fn static_validation_rejects_declared_expression_result_type_mismatch() {
    let expression = leaked(Expression::Field {
        result_type: ValueType::Decimal,
        field: FieldRef {
            field_id: "name",
            instance: FieldInstanceSelector::Singleton,
        },
    });

    assert!(matches!(
        evaluate_one_output(expression),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::TypeMismatch {
                operation: ExecutionOperation::FieldLookup,
                expected: ValueType::String,
                actual: ValueType::Decimal,
            })
        ))
    ));
}

#[test]
fn static_validation_enforces_group_field_selector_scope() {
    let invalid = leaked(Expression::Field {
        result_type: ValueType::Decimal,
        field: FieldRef {
            field_id: "row-value",
            instance: FieldInstanceSelector::Singleton,
        },
    });
    let calculation = CalculationSpec {
        calculation_id: "calculation",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "output",
                value: invalid,
                rounding: None,
                writeback: None,
            }]),
        }),
    };
    let groups = leaked_slice(vec![FieldGroupSpec {
        group_id: "rows",
        min_occurs: 0,
        max_occurs: None,
        members: &["row-value"],
    }]);
    let fields = leaked_slice(vec![decimal_field("row-value", Some("rows"))]);
    let invalid_spec = spec(
        &[],
        groups,
        fields,
        &["calculation"],
        leaked_slice(vec![calculation]),
        &[],
        all_effects(),
    );
    let context = ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe);
    assert!(matches!(
        evaluator(invalid_spec).evaluate(&request(context, vec![], vec![], vec![])),
        Err(EvaluationError::Interpreter(
            InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
                target: "row-value",
                ..
            })
        ))
    ));

    let current = leaked(Expression::Field {
        result_type: ValueType::Decimal,
        field: FieldRef {
            field_id: "row-value",
            instance: FieldInstanceSelector::CurrentGroupInstance,
        },
    });
    let present = leaked(Predicate::Presence {
        operator: PresenceOperator::IsPresent,
        value: current,
    });
    let quantified = leaked(Predicate::GroupQuantifier {
        quantifier: GroupQuantifier::Any,
        group_id: "rows",
        predicate: present,
    });
    let rule = RuleSpec {
        rule_id: "row-present",
        scope: EvaluationScope::Singleton,
        order: 1,
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(RuleBranch {
            predicate: quantified,
            effects: leaked_slice(vec![Effect::EmitIssue {
                severity: RuleSeverity::Advisory,
                message: "row present",
                official_message: None,
                assessment: RuleAssessment::VerifiedCorrect,
                fields: &[],
            }]),
        }),
    };
    let valid_spec = spec(
        &[],
        groups,
        fields,
        &[],
        &[],
        leaked_slice(vec![rule]),
        all_effects(),
    );
    let result = evaluator(valid_spec)
        .evaluate(&request(context, vec![], vec![], vec![]))
        .unwrap();
    assert!(result.report().violations().is_empty());
}

#[test]
fn mutating_effects_fail_closed_before_rule_execution() {
    let output = leaked(Expression::Literal(TypedValue::String("old")));
    let replacement = leaked(Expression::Literal(TypedValue::String("new")));
    let calculation = CalculationSpec {
        calculation_id: "calculation",
        scope: EvaluationScope::Singleton,
        depends_on: &[],
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(CalculationBranch {
            condition: &TRUE,
            outputs: leaked_slice(vec![CalculationOutput {
                output_id: "output",
                value: output,
                rounding: None,
                writeback: None,
            }]),
        }),
    };
    let set_derived = RuleSpec {
        rule_id: "set-derived",
        scope: EvaluationScope::Singleton,
        order: 1,
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(RuleBranch {
            predicate: &TRUE,
            effects: leaked_slice(vec![Effect::SetDerived {
                output_id: "output",
                value: replacement,
            }]),
        }),
    };
    let set_derived_spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &["calculation"],
        leaked_slice(vec![calculation]),
        leaked_slice(vec![set_derived]),
        all_effects(),
    );
    let evaluate = |specification| {
        evaluator(specification).evaluate(&request(
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            vec![],
            vec![],
            vec![raw_singleton("name", RawValue::Text(" x ".into()))],
        ))
    };
    assert!(matches!(
        evaluate(set_derived_spec),
        Err(EvaluationError::Interpreter(
            InterpreterError::UnsupportedEffect {
                kind: EffectKind::SetDerived,
                ..
            }
        ))
    ));

    let normalize = RuleSpec {
        rule_id: "normalize",
        scope: EvaluationScope::Singleton,
        order: 1,
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(RuleBranch {
            predicate: &TRUE,
            effects: leaked_slice(vec![Effect::NormalizeField {
                field: FieldRef {
                    field_id: "name",
                    instance: FieldInstanceSelector::Singleton,
                },
                normalization: leaked_slice(vec![NormalizationStep::Trim {
                    side: TrimSide::Both,
                }]),
            }]),
        }),
    };
    let normalize_spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        leaked_slice(vec![normalize]),
        all_effects(),
    );
    assert!(matches!(
        evaluate(normalize_spec),
        Err(EvaluationError::Interpreter(
            InterpreterError::UnsupportedEffect {
                kind: EffectKind::NormalizeField,
                ..
            }
        ))
    ));
}

#[test]
fn workflow_state_effect_fails_closed_until_result_contract_can_carry_it() {
    let rule = RuleSpec {
        rule_id: "workflow",
        scope: EvaluationScope::Singleton,
        order: 1,
        phases: VALIDATE,
        trigger_field_ids: &[],
        profiles: executable(RuleBranch {
            predicate: &TRUE,
            effects: leaked_slice(vec![Effect::SetWorkflowState {
                state_id: "validated",
            }]),
        }),
    };
    let spec = spec(
        &[],
        &[],
        leaked_slice(vec![string_field("name")]),
        &[],
        &[],
        leaked_slice(vec![rule]),
        all_effects(),
    );
    let result = evaluator(spec).evaluate(&request(
        ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("x".into()))],
    ));

    assert!(matches!(
        result,
        Err(EvaluationError::Interpreter(
            InterpreterError::UnsupportedEffect {
                kind: EffectKind::SetWorkflowState,
                ..
            }
        ))
    ));
}

fn explicit_validate_workflow_spec(rules: &'static [RuleSpec]) -> &'static StaticRuleSetSpec {
    let effects = leaked_slice(vec![
        Effect::SetWorkflowState {
            state_id: "validated",
        },
        Effect::EmitNotification {
            channel: WorkflowNotificationChannel::Alert,
            message: "Validation successful.",
            official_message: Some("Validation successful."),
        },
    ]);
    leaked(StaticRuleSetSpec {
        profile_status: profile_status(),
        effect_mode: all_effects(),
        serialization: &crate::StaticSerializationContract::EMPTY_V1,
        context_values: &[],
        field_groups: &[],
        fields: leaked_slice(vec![string_field("name")]),
        field_event_programs: &[],
        evaluation_order: &[],
        calculations: &[],
        rules,
        workflow: Branch::Executable(StaticWorkflowSpec {
            initial_state: "edit",
            states: leaked_slice(vec![
                WorkflowStateSpec {
                    state_id: "edit",
                    terminal: false,
                },
                WorkflowStateSpec {
                    state_id: "validated",
                    terminal: false,
                },
            ]),
            transitions: leaked_slice(vec![
                WorkflowTransitionSpec {
                    transition_id: "validate-success",
                    from_state: "edit",
                    action: WorkflowAction::Validate,
                    evaluation_phase: ValidationPhase::Validate,
                    to_state: "validated",
                    profiles: Profiled {
                        official: Branch::Executable(WorkflowTransitionBranch {
                            guard: &TRUE,
                            effects,
                        }),
                        filing_safe: Branch::Unresolved,
                    },
                },
                WorkflowTransitionSpec {
                    transition_id: "edit-after-validation",
                    from_state: "validated",
                    action: WorkflowAction::Edit,
                    evaluation_phase: ValidationPhase::Validate,
                    to_state: "edit",
                    profiles: Profiled {
                        official: Branch::Executable(WorkflowTransitionBranch {
                            guard: &TRUE,
                            effects: leaked_slice(vec![
                                Effect::SetWorkflowState { state_id: "edit" },
                                Effect::EmitNotification {
                                    channel: WorkflowNotificationChannel::Alert,
                                    message: "You can now modify your entries.",
                                    official_message: Some("You can now modify your entries."),
                                },
                            ]),
                        }),
                        filing_safe: Branch::Unresolved,
                    },
                },
            ]),
        }),
    })
}

#[test]
fn explicit_workflow_transition_is_bound_to_valid_evaluation_state_and_action() {
    let specification = explicit_validate_workflow_spec(&[]);
    let provider = evaluator(specification);
    let exact_request = request(
        ValidationContext::new(
            ValidationPhase::Validate,
            BehaviorProfile::OfficialCompatibility,
        ),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("Taxpayer".into()))],
    );
    let exact_result = provider.evaluate(&exact_request).unwrap();
    let edit = WorkflowStateId::parse("edit").unwrap();
    let transition = provider
        .transition_workflow(
            &exact_request,
            &exact_result,
            &edit,
            WorkflowAction::Validate,
        )
        .unwrap();
    assert_eq!(transition.transition_id().as_str(), "validate-success");
    assert_eq!(transition.from_state().as_str(), "edit");
    assert_eq!(transition.to_state().as_str(), "validated");
    assert_eq!(transition.notifications().len(), 1);
    assert_eq!(
        transition.notifications()[0].official_message(),
        Some("Validation successful.")
    );
    let validated = WorkflowStateId::parse("validated").unwrap();
    let edit_transition = provider
        .transition_workflow(
            &exact_request,
            &exact_result,
            &validated,
            WorkflowAction::Edit,
        )
        .unwrap();
    assert_eq!(
        edit_transition.transition_id().as_str(),
        "edit-after-validation"
    );
    assert_eq!(edit_transition.context(), exact_result.context());
    assert_eq!(edit_transition.from_state().as_str(), "validated");
    assert_eq!(edit_transition.to_state().as_str(), "edit");
    assert_eq!(
        edit_transition.notifications()[0].official_message(),
        Some("You can now modify your entries.")
    );

    assert!(matches!(
        provider.transition_workflow(
            &exact_request,
            &exact_result,
            &validated,
            WorkflowAction::Validate,
        ),
        Err(WorkflowTransitionError::TransitionSelection { matches: 0 })
    ));
    assert!(matches!(
        provider.transition_workflow(&exact_request, &exact_result, &edit, WorkflowAction::Save,),
        Err(WorkflowTransitionError::TransitionSelection { matches: 0 })
    ));
    let wrong_phase_request = request(
        ValidationContext::new(
            ValidationPhase::DraftPreview,
            BehaviorProfile::OfficialCompatibility,
        ),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("Taxpayer".into()))],
    );
    let wrong_phase_result = provider.evaluate(&wrong_phase_request).unwrap();
    assert!(matches!(
        provider.transition_workflow(
            &wrong_phase_request,
            &wrong_phase_result,
            &edit,
            WorkflowAction::Validate,
        ),
        Err(WorkflowTransitionError::InvalidActionPhase { .. })
    ));

    let different_request = request(
        ValidationContext::new(
            ValidationPhase::Validate,
            BehaviorProfile::OfficialCompatibility,
        ),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("Different".into()))],
    );
    assert!(matches!(
        provider.transition_workflow(
            &different_request,
            &exact_result,
            &edit,
            WorkflowAction::Validate,
        ),
        Err(WorkflowTransitionError::BindingMismatch {
            field: "evaluation_result"
        })
    ));
}

#[test]
fn invalid_evaluation_cannot_activate_workflow_transition() {
    let specification = explicit_validate_workflow_spec(leaked_slice(vec![blocking_rule(
        "always-invalid",
        1,
        VALIDATE,
        "Invalid.",
    )]));
    let provider = evaluator(specification);
    let exact_request = request(
        ValidationContext::new(
            ValidationPhase::Validate,
            BehaviorProfile::OfficialCompatibility,
        ),
        vec![],
        vec![],
        vec![raw_singleton("name", RawValue::Text("Taxpayer".into()))],
    );
    let result = provider.evaluate(&exact_request).unwrap();
    assert!(!result.is_valid());
    assert!(matches!(
        provider.transition_workflow(
            &exact_request,
            &result,
            &WorkflowStateId::parse("edit").unwrap(),
            WorkflowAction::Validate,
        ),
        Err(WorkflowTransitionError::EvaluationNotValid)
    ));
}
