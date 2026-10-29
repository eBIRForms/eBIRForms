//! Shaping shared by the forms' print maps (`to_print_field_map`): the
//! `derived:` values a frozen print sheet needs that the upload map does
//! not carry in the sheet's shape. Print only; the upload never reads them.

use std::collections::BTreeMap;

/// `text` over a comb of `width` slots: the first line broken at the last
/// space that fits (else at `width`), then the rest.
pub fn split_print_line(text: &str, width: usize) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return (text.to_string(), String::new());
    }
    let cut = chars[..=width]
        .iter()
        .rposition(|ch| *ch == ' ')
        .filter(|at| *at > 0)
        .unwrap_or(width);
    let first: String = chars[..cut].iter().collect();
    let rest: String = chars[cut..].iter().collect();
    (first.trim_end().to_string(), rest.trim_start().to_string())
}

/// The digits of `fields[key]` for each key in order, joined: a later
/// page's one-comb TIN header from the page's TIN part keys.
pub fn digits_of(fields: &BTreeMap<String, String>, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|key| fields.get(*key))
        .flat_map(|part| part.chars().filter(char::is_ascii_digit))
        .collect()
}

/// A stored TIN (dashed or compact) as the 14 digits a header comb holds:
/// the nine TIN digits then the branch, zero-filled to five.
pub fn tin_digits(tin: &str) -> String {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    format!("{digits:0<14}")
}

/// A later page's header when the sheet draws its TIN and Taxpayer's Name
/// boxes as one 40-slot comb: the 14 TIN digits, then the name from slot 14
/// (under the Taxpayer's Name caption).
pub fn tin_name_header(tin: &str, name: &str) -> String {
    format!("{}{}", tin_digits(tin), name.trim().to_uppercase())
}

/// The printed names of an excise place's region, province and city codes
/// (the upload carries the codes); a region prints its short name
/// (`REGION IV-A`, `NCR`). Blank for the `00` placeholder or an unknown code.
pub fn place_names(region: &str, province: &str, city: &str) -> [String; 3] {
    let name = |code: &str, short: bool| {
        let code = code.trim();
        let Some(name) = super::excise_places::place_name(code).filter(|_| !code.is_empty()) else {
            return String::new();
        };
        let name = if short {
            name.split(" - ").next().unwrap_or(name)
        } else {
            name
        };
        name.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    [name(region, true), name(province, false), name(city, false)]
}

/// Item "If yes, specify" text for the official tax-relief dropdown
/// position (`1` special rate, `2` treaty, `3` both); blank otherwise.
pub fn tax_relief_label(code: &str) -> &'static str {
    match code.trim() {
        "1" => "SPECIAL RATE",
        "2" => "INTERNATIONAL TAX TREATY",
        "3" => "SPECIAL RATE AND TAX TREATY",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_the_last_space_that_fits() {
        assert_eq!(
            split_print_line("123 SAMPLE STREET, BARANGAY EXAMPLE, QUEZON CITY", 40),
            (
                "123 SAMPLE STREET, BARANGAY EXAMPLE,".to_string(),
                "QUEZON CITY".to_string()
            )
        );
        assert_eq!(
            split_print_line("SHORT", 40),
            ("SHORT".to_string(), String::new())
        );
        assert_eq!(
            split_print_line("ABCDEFGHIJ", 4),
            ("ABCD".to_string(), "EFGHIJ".to_string())
        );
    }

    #[test]
    fn header_puts_the_name_after_fourteen_tin_digits() {
        assert_eq!(tin_digits("123-456-788-000"), "12345678800000");
        assert_eq!(
            tin_name_header("123456788", " Sample Co "),
            "12345678800000SAMPLE CO"
        );
    }

    #[test]
    fn digits_follow_the_key_order() {
        let fields: BTreeMap<String, String> = [("a", "123"), ("b", "45-6"), ("c", "00000")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(
            digits_of(&fields, &["a", "b", "c", "missing"]),
            "12345600000"
        );
    }
}
