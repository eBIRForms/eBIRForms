//! Boxes that reformat as the filer types (amounts get commas, dates
//! slashes, TINs dashes) keep the caret where the filer was typing: after
//! the same digits, not at the end.

use nsh_core::prelude::Signal;

/// Puts `shown`, the text formatted from `raw` (what the box reported),
/// in `value`. A signal skips a write equal to what it holds, so a
/// keystroke the format refuses (a tenth TIN digit, a ninth date digit)
/// would stay on screen: `raw` goes in first, so the box takes `shown` back.
pub(crate) fn show_formatted(value: Signal<String>, raw: &str, shown: &str) {
    if shown != raw && value.get_untracked() == shown {
        value.set(raw.to_owned());
    }
    value.set(shown.to_owned());
}

/// The caret after `raw` (the box's text after a keystroke; it read `prev`
/// before) is shown as `formatted`: after as many characters that `keep`
/// counts as stood before the caret in `raw`. In UTF-16 units, as inputs
/// take selections.
pub(crate) fn caret_after(prev: &str, raw: &str, formatted: &str, keep: impl Fn(char) -> bool) -> u32 {
    let p: Vec<char> = prev.chars().collect();
    let r: Vec<char> = raw.chars().collect();
    let prefix = p.iter().zip(&r).take_while(|(a, b)| a == b).count();
    let room = p.len().min(r.len()) - prefix;
    let suffix = p.iter().rev().zip(r.iter().rev()).take(room).take_while(|(a, b)| a == b).count();
    let caret = r.len() - suffix;
    let want = r[..caret].iter().filter(|c| keep(**c)).count();
    if want == 0 {
        return 0;
    }
    let mut seen = 0;
    let mut at = 0u32;
    for c in formatted.chars() {
        at += c.len_utf16() as u32;
        if keep(c) {
            seen += 1;
            if seen == want {
                break;
            }
        }
    }
    at
}

/// Digits typed as a date, shown `MM/DD/YYYY` as they come: `01151990`
/// reads `01/15/1990`, `011` reads `01/1`.
pub(crate) fn slash_date(raw: &str) -> String {
    let digits: String = raw.chars().filter(char::is_ascii_digit).take(8).collect();
    let mut out = String::with_capacity(10);
    for (i, c) in digits.chars().enumerate() {
        if i == 2 || i == 4 {
            out.push('/');
        }
        out.push(c);
    }
    out
}

/// Digits typed as a TIN, `000-000-000-00000` as they come (at most 14).
pub(crate) fn dash_tin(raw: &str, max: usize) -> String {
    let digits: String = raw.chars().filter(char::is_ascii_digit).take(max).collect();
    let mut out = String::with_capacity(17);
    for (i, c) in digits.chars().enumerate() {
        if i == 3 || i == 6 || i == 9 {
            out.push('-');
        }
        out.push(c);
    }
    out
}

/// Whether `c` is a digit (what the caret counts in dates and TINs).
pub(crate) fn digit(c: char) -> bool {
    c.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_stays_after_the_digit_typed_mid_amount() {
        // "1,234" → typed 9 after the 1: raw "19,234" → "192,34"? formatted "192,34" is
        // what live commas give for "19234": "19,234".
        let c = caret_after("1,234", "19,234", "19,234", |c| c.is_ascii_digit());
        assert_eq!(c, 2);
    }

    #[test]
    fn date_slashes_and_caret_at_the_end() {
        assert_eq!(slash_date("01151990"), "01/15/1990");
        assert_eq!(caret_after("01/1", "01/15", &slash_date("01/15"), digit), 5);
        assert_eq!(caret_after("01/15", "01/151", &slash_date("01/151"), digit), 7);
    }
}
