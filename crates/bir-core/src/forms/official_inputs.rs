//! Small ports of the official eBIRForms input helpers (`js/string-util.js`
//! and per-form handlers) shared by the event-based capital gains, donor's and
//! estate tax returns (1706, 1707, 1707A, 1800, 1801).

use crate::official_xml::{official_amount, parse_official_amount};

/// `round(this, 2)` / `formatCurrency`: the value the official field holds.
pub fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

/// An optional amount after `round(this, 2)`; blank stays blank.
pub fn cents_opt(value: Option<f64>) -> Option<f64> {
    value.map(cents)
}

/// `round()` turns any amount with more than 12 integer digits into `0.00`.
pub const MAX_ROUNDED_AMOUNT: f64 = 1_000_000_000_000.0;

/// True when `round()` keeps the amount (at most 12 integer digits).
pub fn within_round_limit(value: f64) -> bool {
    value.is_finite() && value.abs() < MAX_ROUNDED_AMOUNT
}

/// JavaScript `Number.prototype.toFixed(digits)`: the exact binary value
/// rounded half up (`2.675.toFixed(2)` is `"2.67"`).
pub fn to_fixed_text(value: f64, digits: usize) -> String {
    if !value.is_finite() {
        return "NaN".to_string();
    }
    // Rust prints the exact decimal expansion for a large precision.
    let exact = format!("{:.60}", value.abs());
    let (whole, frac) = exact.split_once('.').unwrap_or((&exact, "0"));
    let mut kept: Vec<u8> = whole.bytes().chain(frac.bytes().take(digits)).collect();
    let round_up = frac.as_bytes().get(digits).is_some_and(|d| *d >= b'5');
    if round_up {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                break;
            }
            i -= 1;
            if kept[i] == b'9' {
                kept[i] = b'0';
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let text = String::from_utf8(kept).unwrap_or_default();
    let (int_part, dec_part) = text.split_at(text.len() - digits);
    let sign = if value < 0.0 { "-" } else { "" };
    if digits == 0 {
        format!("{sign}{int_part}")
    } else {
        format!("{sign}{int_part}.{dec_part}")
    }
}

/// [`to_fixed_text`] with two digits, as a number.
pub fn to_fixed_2(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    to_fixed_text(value, 2).parse().unwrap_or(0.0)
}

/// `formatCurrency(x.toFixed(2))`, the pattern most handlers use.
pub fn format_fixed(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        cents(to_fixed_2(value))
    }
}

/// Commas between thousands of the integer part, as the
/// `/\B(?=(\d{3})+(?!\d))/g` replacements write them.
pub fn group_thousands(text: &str) -> String {
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text),
    };
    let (int_part, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let mut grouped = String::new();
    for (i, ch) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(frac) => format!("{sign}{grouped}.{frac}"),
        None => format!("{sign}{grouped}"),
    }
}

/// `NumWithComma()` on a field that may be blank: blank is `NaN`.
pub fn js_num(value: Option<f64>) -> f64 {
    value.unwrap_or(f64::NAN)
}

/// `a >= b` with JavaScript `NaN` semantics (always false on `NaN`).
pub fn js_ge(a: f64, b: f64) -> bool {
    a >= b
}

/// Whether `value` is an amount in pesos and centavos: at most two
/// decimals. Decided from its decimal form (maintainer decision (two-decimal
/// amounts of any size pass),
/// 2026-10-19): the value is the stored double of a two-decimal amount when
/// rounding it to centavos and back gives the same double, so 8816764.47
/// and 1234567890.12 pass. The earlier float tolerance (`value * 100`
/// within 1e-7 of a whole number) refused those, since a double's error at
/// about 2,000,000 and up exceeds it. The tolerance is kept beside the
/// decimal check on purpose: a double that is not the nearest one to its
/// two-decimal amount (a sum such as 0.1 + 0.2, 0.30000000000000004, kept
/// in a stored draft) fails the round trip but passed the tolerance, and
/// no amount that passed before may be refused now.
pub fn has_cent_precision(value: f64) -> bool {
    if !value.is_finite() {
        return false;
    }
    let scaled = value * 100.0;
    let within_tolerance = (scaled - scaled.round()).abs() < 1e-7;
    within_tolerance || cents_round_trip(value)
}

/// `value` read back from its own amount at centavos (`round(value * 100) /
/// 100`) is `value` itself. Exact for every amount below 2^53 centavos,
/// which covers the official 12 integer digits.
fn cents_round_trip(value: f64) -> bool {
    let centavos = (value * 100.0).round();
    centavos.abs() < 9_007_199_254_740_992.0 && centavos / 100.0 == value
}

pub fn digits_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit())
}

/// Digits and `.` only: what `numbersonly` lets a filer type.
pub fn numbers_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Letters and digits only: what `letternumber` lets a filer type.
pub fn letters_and_digits(value: &str) -> bool {
    value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes.
/// A missing branch is padded to `00000`.
pub fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    let branch = digits.get(9..).unwrap_or("");
    (part(0..3), part(3..6), part(6..9), format!("{branch:0>5}"))
}

/// A TIN written as 9 digits plus an optional 3–5 digit branch, with or
/// without dashes.
pub fn tin_is_well_formed(tin: &str) -> bool {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    tin.chars().all(|c| c.is_ascii_digit() || c == '-') && digits.len() >= 9 && digits.len() <= 14
}

/// `capital()`: every text control except `txtEmail` is uppercased.
pub fn capital(value: &str) -> String {
    value.trim().to_uppercase()
}

/// An optional amount as the official control shows it: blank or `1,234.56`.
pub fn amount_text(value: Option<f64>) -> String {
    value.map(official_amount).unwrap_or_default()
}

/// A real calendar date with a 4-digit year.
pub fn is_calendar_date(year: u16, month: u8, day: u8) -> bool {
    (1000..=9999).contains(&year)
        && chrono::NaiveDate::from_ymd_opt(i32::from(year), u32::from(month), u32::from(day))
            .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_matches_javascript() {
        assert_eq!(to_fixed_2(2.675), 2.67);
        assert_eq!(to_fixed_2(1.005), 1.0);
        assert_eq!(to_fixed_2(0.125), 0.13);
        assert_eq!(to_fixed_2(1_000_000.005), 1_000_000.01);
        assert_eq!(to_fixed_2(99.999), 100.0);
        assert_eq!(to_fixed_2(-1.5), -1.5);
        assert_eq!(to_fixed_text(1234.5, 3), "1234.500");
        assert_eq!(group_thousands("1234567.125"), "1,234,567.125");
        assert_eq!(format_fixed(f64::NAN), 0.0);
    }

    #[test]
    fn helpers() {
        assert_eq!(cents(123_456.789), 123_456.79);
        assert_eq!(
            split_tin("123-456-788"),
            ("123".into(), "456".into(), "788".into(), "00000".into())
        );
        assert!(!js_ge(js_num(None), 0.0));
        assert!(is_calendar_date(2024, 2, 29));
        assert!(!is_calendar_date(2025, 2, 29));
        assert!(numbers_only("12.5"));
        assert!(!numbers_only("1,2"));
    }

    /// The check before maintainer decision (two-decimal amounts of any size
    /// pass), kept here as the oracle of what
    /// passed then.
    fn tolerance_only(value: f64) -> bool {
        value.is_finite() && ((value * 100.0) - (value * 100.0).round()).abs() < 1e-7
    }

    /// A deterministic spread of centavo counts below 10^14 (amounts below
    /// 1,000,000,000,000), weighted to every magnitude.
    fn centavo_counts() -> Vec<u64> {
        let mut out: Vec<u64> = (0..=100_000).collect();
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..200_000 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let digits = (seed >> 59) as u32 % 15;
            let bound = 10u64.pow(digits.max(1));
            out.push((seed >> 7) % bound);
        }
        out.push(99_999_999_999_999);
        out.push(881_676_447);
        out.push(123_456_789_012);
        out
    }

    fn typed(centavos: u64) -> f64 {
        format!("{}.{:02}", centavos / 100, centavos % 100)
            .parse()
            .unwrap()
    }

    /// maintainer decision (two-decimal amounts of any size pass): every
    /// two-decimal amount below 1,000,000,000,000 passes,
    /// decided from its decimal form; the ones the float tolerance refused
    /// (about 2,000,000 and up) now pass too.
    #[test]
    fn every_two_decimal_amount_has_cent_precision() {
        let mut newly = 0;
        for centavos in centavo_counts() {
            let value = typed(centavos);
            assert!(has_cent_precision(value), "{value}");
            assert!(has_cent_precision(-value), "-{value}");
            if !tolerance_only(value) {
                newly += 1;
            }
        }
        assert!(newly > 1_000, "{newly} amounts the tolerance refused");
        for (text, value) in [
            ("8816764.47", 8_816_764.47),
            ("1234567890.12", 1_234_567_890.12),
        ] {
            assert_eq!(text.parse::<f64>().unwrap(), value);
            assert!(!tolerance_only(value), "{text} was refused before");
            assert!(has_cent_precision(value), "{text}");
        }
        assert!(has_cent_precision(999_999_999_999.99));
    }

    /// maintainer decision (two-decimal amounts of any size pass): no value
    /// that passed the earlier check is refused now. The
    /// sample covers each two-decimal amount, its neighbouring doubles (as a
    /// sum of amounts may land on), sums of amounts and arbitrary doubles.
    #[test]
    fn nothing_that_passed_before_is_refused_now() {
        let mut values = Vec::new();
        for centavos in centavo_counts().into_iter().step_by(7) {
            let value = typed(centavos);
            let mut up = value;
            let mut down = value;
            for _ in 0..4 {
                up = up.next_up();
                down = down.next_down();
                values.extend([up, down]);
            }
            values.push(value + typed(centavos / 3));
            values.push(value * 3.0);
        }
        values.extend([0.1 + 0.2, 1.1 + 2.2, 0.0, -0.0, 1e-9, 3.000_000_000_1]);
        let mut seed: u64 = 42;
        for _ in 0..100_000 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            values.push(f64::from_bits(seed >> 2));
        }
        let mut passed = 0;
        for value in values {
            if tolerance_only(value) {
                passed += 1;
                assert!(has_cent_precision(value), "{value:?} passed before");
            }
        }
        assert!(passed > 10_000, "{passed}");
    }

    /// More than two decimals, and what is not a number, stay refused.
    #[test]
    fn more_decimals_stay_refused() {
        for text in [
            "10.005",
            "1.999",
            "0.125",
            "8816764.475",
            "1234567890.125",
            "0.001",
        ] {
            let value: f64 = text.parse().unwrap();
            assert!(!has_cent_precision(value), "{text}");
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(!has_cent_precision(value));
        }
    }
}

/// Rows a page adds at run time (1801 "Add row", the 1707 / 1707A pop-ups)
/// are written by the same upload loop (`saveEncryptedProfile`) in DOM order, so their
/// layout is the generated one with copies of a template row's entries
/// spliced in. Each insertion copies `template` entries under new keys
/// (`template` key → new key, same kind, codec, default and trailing text)
/// and places them right after the entry keyed `after`.
pub struct RowInsertion {
    pub after: String,
    pub copies: Vec<(String, String)>,
}

/// The generated layout extended with run-time rows. Insertions apply in
/// order; a later insertion may anchor on a key an earlier one added.
pub fn extend_layout(
    base: &crate::official_xml::OfficialLayout,
    insertions: &[RowInsertion],
) -> Result<crate::official_xml::OfficialLayout, String> {
    use crate::official_xml::{Entry, Part};
    let key_of = |entry: &Entry| match entry {
        Entry::Bool { key, .. } | Entry::Value { key, .. } => key.clone(),
    };
    let mut layout = base.clone();
    for insertion in insertions {
        let mut new_entries = Vec::new();
        for (template, new_key) in &insertion.copies {
            let source = layout
                .entries
                .iter()
                .find(|entry| key_of(entry) == *template)
                .ok_or_else(|| format!("no template entry {template}"))?;
            let copy = match source.clone() {
                Entry::Bool { default, after, .. } => Entry::Bool {
                    key: new_key.clone(),
                    default,
                    after,
                },
                Entry::Value {
                    parts,
                    default,
                    number,
                    after,
                    ..
                } => Entry::Value {
                    key: new_key.clone(),
                    number,
                    parts: parts
                        .into_iter()
                        .map(|part| match part {
                            Part::Source {
                                source,
                                codec,
                                uppercase,
                            } if source == *template => Part::Source {
                                source: new_key.clone(),
                                codec,
                                uppercase,
                            },
                            other => other,
                        })
                        .collect(),
                    default,
                    after,
                },
            };
            new_entries.push(copy);
        }
        let at = layout
            .entries
            .iter()
            .position(|entry| key_of(entry) == insertion.after)
            .ok_or_else(|| format!("no anchor entry {}", insertion.after))?;
        layout.entries.splice(at + 1..at + 1, new_entries);
    }
    Ok(layout)
}
