//! How the screens show amounts, dates and periods. Amounts stay decimal
//! text end to end: these functions work on the digits, never on `f64`.

use chrono::Datelike as _;
use serde::{Deserialize, Serialize};

const MONTHS_LONG: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September",
    "October", "November", "December",
];

/// An amount as text without `₱`, commas or spaces: `(negative, whole
/// digits, fraction digits)`, or `None` when it is not a number.
fn amount_parts(text: &str) -> Option<(bool, String, String)> {
    let clean: String = text
        .chars()
        .filter(|c| !matches!(c, '₱' | ',' | ' ' | '\u{a0}'))
        .collect();
    let (negative, digits) = match clean.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, clean.strip_prefix('+').unwrap_or(&clean)),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let numeric = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty()) || !numeric(whole) || !numeric(fraction) {
        return None;
    }
    let whole = whole.trim_start_matches('0');
    Some((
        negative,
        if whole.is_empty() { "0".into() } else { whole.into() },
        fraction.into(),
    ))
}

/// Add one to a string of digits.
fn increment(digits: &str) -> String {
    let mut out: Vec<u8> = digits.bytes().collect();
    for byte in out.iter_mut().rev() {
        if *byte == b'9' {
            *byte = b'0';
        } else {
            *byte += 1;
            return String::from_utf8(out).unwrap_or_default();
        }
    }
    format!("1{}", String::from_utf8(out).unwrap_or_default())
}

fn group_thousands(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// An amount as the form shows it: `1,234.50`, or `1,235` for a whole-peso
/// box (rounded half away from zero). Text that is not a number, and empty
/// text, comes back as it was.
pub fn format_amount(text: &str, whole_pesos: bool) -> String {
    let Some((negative, mut whole, fraction)) = amount_parts(text) else {
        return text.to_string();
    };
    let places = if whole_pesos { 0 } else { 2 };
    let mut kept: String = fraction.chars().take(places).collect();
    while kept.len() < places {
        kept.push('0');
    }
    if fraction.as_bytes().get(places).is_some_and(|d| *d >= b'5') {
        let joined = increment(&format!("{whole}{kept}"));
        let split = joined.len() - places;
        whole = joined[..split].to_string();
        kept = joined[split..].to_string();
    }
    let zero = whole.chars().all(|c| c == '0') && kept.chars().all(|c| c == '0');
    let sign = if negative && !zero { "-" } else { "" };
    let grouped = group_thousands(&whole);
    if whole_pesos {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{kept}")
    }
}

/// Amounts added exactly (to the centavo), as `format_amount` shows the
/// sum. Blank or non-numeric text counts as zero.
pub fn sum_amounts<'a>(texts: impl IntoIterator<Item = &'a str>) -> String {
    let mut centavos: i128 = 0;
    for text in texts {
        let Some((negative, whole, fraction)) = amount_parts(text) else {
            continue;
        };
        let mut cents: String = fraction.chars().take(2).collect();
        while cents.len() < 2 {
            cents.push('0');
        }
        let value: i128 = format!("{whole}{cents}").parse().unwrap_or(0);
        centavos += if negative { -value } else { value };
    }
    let negative = centavos < 0;
    let abs = centavos.unsigned_abs();
    let text = format!("{}{}.{:02}", if negative { "-" } else { "" }, abs / 100, abs % 100);
    format_amount(&text, false)
}

/// A date the filer typed: `MMDDYYYY`, `YYYY-MM-DD`, `M/D/YYYY`,
/// `M-D-YYYY` or `Jan 5, 2025`. `None` when it is not a real date.
pub fn parse_typed_date(text: &str) -> Option<chrono::NaiveDate> {
    let v = text.trim();
    if v.is_empty() {
        return None;
    }
    let num = |s: &str| s.parse::<u32>().ok();
    let (y, m, d) = if v.len() == 8 && v.chars().all(|c| c.is_ascii_digit()) {
        (num(&v[4..])?, num(&v[..2])?, num(&v[2..4])?)
    } else if let Some((y, rest)) = v.split_once('-').filter(|(y, _)| y.len() == 4) {
        let (m, d) = rest.split_once('-')?;
        (num(y)?, num(m)?, num(d)?)
    } else if let Some(parts) = split3(v, &['/', '-']) {
        (num(parts.2)?, num(parts.0)?, num(parts.1)?)
    } else {
        let mut words = v.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty());
        let month = words.next()?.trim_end_matches('.').to_ascii_lowercase();
        let m = MONTHS_LONG
            .iter()
            .position(|name| month.len() >= 3 && name.to_ascii_lowercase().starts_with(&month))?;
        let d = num(words.next()?)?;
        let y = num(words.next()?)?;
        (y, m as u32 + 1, d)
    };
    if !(1900..=2100).contains(&y) {
        return None;
    }
    chrono::NaiveDate::from_ymd_opt(y as i32, m, d)
}

fn split3<'a>(v: &'a str, seps: &[char]) -> Option<(&'a str, &'a str, &'a str)> {
    let mut parts = v.split(|c| seps.contains(&c));
    let out = (parts.next()?, parts.next()?, parts.next()?);
    (parts.next().is_none() && out.2.len() == 4).then_some(out)
}

/// `MM/DD/YYYY`, the form's date text.
pub fn format_form_date(date: chrono::NaiveDate) -> String {
    format!("{:02}/{:02}/{}", date.month(), date.day(), date.year())
}

/// A period a return can be opened for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodChoice {
    /// Month 1-12, quarter 1-4, `0` for an annual return, `1` for the
    /// first as-needed return.
    pub period: i64,
    pub label: String,
}

/// The periods of `year` a form of `frequency` can be opened for, newest
/// first, up to today's (every period of a past year).
pub fn period_choices(frequency: &str, year: i64) -> Vec<PeriodChoice> {
    let today = chrono::Local::now().date_naive();
    let this_year = i64::from(today.year());
    let last_month = match year.cmp(&this_year) {
        std::cmp::Ordering::Less => 12,
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Equal => i64::from(today.month()),
    };
    match frequency {
        "Monthly" => (1..=last_month)
            .rev()
            .map(|period| PeriodChoice { period, label: period_label(frequency, year, period) })
            .collect(),
        "Quarterly" => (1..=(last_month + 2) / 3)
            .rev()
            .map(|period| PeriodChoice { period, label: period_label(frequency, year, period) })
            .collect(),
        "As needed" => vec![PeriodChoice { period: 1, label: format!("{year}, first return") }],
        _ => vec![PeriodChoice { period: 0, label: period_label(frequency, year, 0) }],
    }
}

/// `March 2025`, `Q2 2025`, `2025 #1` or `TY 2025`.
pub fn period_label(frequency: &str, year: i64, period: i64) -> String {
    match frequency {
        "Monthly" if (1..=12).contains(&period) => {
            format!("{} {year}", MONTHS_LONG[(period - 1) as usize])
        }
        "Quarterly" if (1..=4).contains(&period) => format!("Q{period} {year}"),
        "As needed" => format!("{year} #{}", period.max(1)),
        _ => format!("TY {year}"),
    }
}

/// The current calendar year, local time.
pub fn current_year() -> i64 {
    i64::from(chrono::Local::now().year())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_format_as_the_form_shows_them() {
        assert_eq!(format_amount("1000", false), "1,000.00");
        assert_eq!(format_amount("₱1,234.565", false), "1,234.57");
        assert_eq!(format_amount("999.995", false), "1,000.00");
        assert_eq!(format_amount("-12.5", true), "-13");
        assert_eq!(format_amount("-0.001", false), "0.00");
        assert_eq!(format_amount("abc", false), "abc");
        assert_eq!(format_amount("", false), "");
        assert_eq!(sum_amounts(["1,000.50", "", "25", "-0.5"]), "1,025.00");
    }

    #[test]
    fn typed_dates_parse() {
        let d = chrono::NaiveDate::from_ymd_opt(2020, 12, 1).unwrap();
        for text in ["12012020", "2020-12-01", "12/1/2020", "Dec 1, 2020", "december 1 2020"] {
            assert_eq!(parse_typed_date(text), Some(d), "{text}");
        }
        assert_eq!(parse_typed_date("02/30/2020"), None);
        assert_eq!(format_form_date(d), "12/01/2020");
    }
}
