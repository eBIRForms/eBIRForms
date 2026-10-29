//! The official eBIRForms submit plaintext, reproduced byte for byte.
//!
//! Each form's `saveXMLsubmit()` walks its page controls and writes one
//! `<div>key=valuekey=</div>` per control, with per-form quirks: page order,
//! `escape()` on some fields only, a few fields written twice or folded into
//! another, form-specific whitespace and the `All Rights Reserved BIR 2012.0`
//! trailer. `tools/official-xml` runs that official loop in jsdom and
//! records the result for each form in `data/official-xml/<form_id>.json`.
//! [`write`] replays such a layout over a form's field map, so the form code
//! only supplies values.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct OfficialLayout {
    pub form_id: String,
    pub official_hta: String,
    pub official_hta_sha256: String,
    pub header: String,
    /// Text between the header and the first `<div>`.
    pub lead: String,
    pub entries: Vec<Entry>,
}

/// One `<div>` occurrence, in official order.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Entry {
    /// A radio button or checkbox: `true` or `false`.
    Bool {
        key: String,
        default: String,
        /// Exact text the official code writes after this `<div>`.
        after: String,
    },
    /// A text or select control, possibly with literal text or with another
    /// control's value folded in (1601C Address 2).
    Value {
        key: String,
        parts: Vec<Part>,
        default: String,
        /// A value-dependent rule the official loop applies to this amount.
        #[serde(default)]
        number: Option<NumberRule>,
        after: String,
    },
}

/// 1702MX `numbertext` amounts: the official loop strips commas and turns
/// `(5.00)` into `-5.00` (`NumWithParenthesis`). The upload loop
/// (`saveEncryptedProfile`) stops there; the older `saveXMLsubmit` loop also
/// writes no `<div>` (and no separator) when the result is numerically zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum NumberRule {
    #[serde(rename = "normalize")]
    Normalize,
    #[serde(rename = "omit-zero")]
    OmitZero,
}

/// The official `numbertext` normalization: the first `(` becomes `-`, the
/// first `)` goes, and commas are removed.
pub fn official_number_text(value: &str) -> String {
    if value.contains('(') && value.contains(')') {
        value
            .replacen('(', "-", 1)
            .replacen(')', "", 1)
            .replace(',', "")
    } else {
        value.replace(',', "")
    }
}

/// JavaScript's `(value * 1) === 0`: blank is zero, non-numbers are not.
fn js_is_zero(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed.parse::<f64>().is_ok_and(|n| n == 0.0)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Part {
    Literal {
        literal: String,
    },
    Source {
        source: String,
        codec: Codec,
        #[serde(default)]
        uppercase: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Codec {
    #[serde(rename = "raw")]
    Raw,
    /// JavaScript's legacy `escape()`.
    #[serde(rename = "js-escape")]
    JsEscape,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OfficialXmlError {
    #[error("no official layout for form {0}")]
    UnknownForm(String),
    #[error("{0} is not a control on the official form")]
    UnknownKey(String),
    #[error("{key} must be true or false, got {value:?}")]
    NotBoolean { key: String, value: String },
    #[error("plaintext does not follow the official layout at byte {at}: expected {expected:?}")]
    Layout { at: usize, expected: String },
    #[error("{key} is written twice with different values")]
    Inconsistent { key: String },
    #[error("{key} holds an invalid escape() sequence")]
    InvalidEscape { key: String },
}

macro_rules! layouts {
    ($($id:literal),* $(,)?) => {
        &[$(($id, include_str!(concat!("../data/official-xml/", $id, ".json")))),*]
    };
}

/// Layouts of the forms the app serializes today. Add a form here when its
/// field map moves to [`write`].
const LAYOUTS: &[(&str, &str)] = layouts![
    "0605-v2003",
    "0619e-v2018",
    "0619f-v2018",
    "1600pt-v2018-government",
    "1600pt-v2018-private",
    "1600vt-v2018-government",
    "1600vt-v2018-private",
    "1600wp-v2010-atc0",
    "1600wp-v2010-atc1",
    "1600wp-v2010-atc2",
    "1601c-v2018",
    "1601eq-v2018",
    "1601fq-v2018",
    "1602q-v2018",
    "1603q-v2018",
    "1604c-v2018",
    "1604e-v2018",
    "1604f-v2018",
    "1606-v2018",
    "1700-v2013",
    "1701-v2018",
    "1701a-v2018",
    "1701ms-v2024",
    "1701q-v2018",
    "1702ex-v2018c",
    "1702mx-v2018c",
    "1702q-v2018c",
    "1702rt-v2018c",
    "1706-v2018",
    "1707-v2021",
    "1707a-v2021",
    "1800-v2018",
    "1801-v2018",
    "2000-v2018",
    "2000ot-v2018",
    "2200a-v2020",
    "2200an-v2018",
    "2200c-v2018",
    "2200m-v2018",
    "2200p-v2020",
    "2200s-v2018",
    "2200t-v2020",
    "2550m-v2007",
    "2550q-v2024",
    "2551q-v2018",
    "2552-v2018",
    "2553-v1999",
];

/// The official layout for a rule-package form id such as `"2551q-v2018"`.
pub fn layout(form_id: &str) -> Result<&'static OfficialLayout, OfficialXmlError> {
    static PARSED: OnceLock<BTreeMap<&'static str, OfficialLayout>> = OnceLock::new();
    PARSED
        .get_or_init(|| {
            LAYOUTS
                .iter()
                .map(|(id, json)| {
                    let layout: OfficialLayout = serde_json::from_str(json)
                        .unwrap_or_else(|error| panic!("invalid official layout {id}: {error}"));
                    (*id, layout)
                })
                .collect()
        })
        .get(form_id)
        .ok_or_else(|| OfficialXmlError::UnknownForm(form_id.to_string()))
}

impl OfficialLayout {
    /// Every key the official form serializes, including folded-in sources.
    pub fn keys(&self) -> BTreeSet<&str> {
        let mut keys = BTreeSet::new();
        for entry in &self.entries {
            match entry {
                Entry::Bool { key, .. } => {
                    keys.insert(key.as_str());
                }
                Entry::Value { key, parts, .. } => {
                    keys.insert(key.as_str());
                    for part in parts {
                        if let Part::Source { source, .. } = part {
                            keys.insert(source.as_str());
                        }
                    }
                }
            }
        }
        keys
    }
}

/// Write the official plaintext for `values`, keyed by official control id.
///
/// A control with no value keeps the official page default. A key that is not
/// a control on the official form is an error, so a field map cannot invent
/// keys the official app never sends.
pub fn write(
    layout: &OfficialLayout,
    values: &BTreeMap<String, String>,
) -> Result<String, OfficialXmlError> {
    let known = layout.keys();
    if let Some(unknown) = values.keys().find(|key| !known.contains(key.as_str())) {
        return Err(OfficialXmlError::UnknownKey(unknown.clone()));
    }

    let mut out = String::with_capacity(8 * 1024);
    out.push_str(&layout.header);
    out.push_str(&layout.lead);
    for entry in &layout.entries {
        let (key, body, after) = match entry {
            Entry::Bool {
                key,
                default,
                after,
            } => {
                let body = match values.get(key) {
                    Some(value) if value == "true" || value == "false" => value.clone(),
                    Some(value) => {
                        return Err(OfficialXmlError::NotBoolean {
                            key: key.clone(),
                            value: value.clone(),
                        });
                    }
                    None => default.clone(),
                };
                (key, body, after)
            }
            Entry::Value {
                key,
                parts,
                default,
                number,
                after,
            } => {
                let any_source = parts.iter().any(
                    |part| matches!(part, Part::Source { source, .. } if values.contains_key(source)),
                );
                let body = if any_source {
                    parts
                        .iter()
                        .map(|part| match part {
                            Part::Literal { literal } => literal.clone(),
                            Part::Source {
                                source,
                                codec,
                                uppercase,
                            } => {
                                let raw = values.get(source).map(String::as_str).unwrap_or("");
                                let value = if *uppercase {
                                    raw.to_uppercase()
                                } else {
                                    raw.to_string()
                                };
                                match codec {
                                    Codec::Raw => value,
                                    Codec::JsEscape => js_escape(&value),
                                }
                            }
                        })
                        .collect()
                } else {
                    default.clone()
                };
                let body = match number {
                    Some(NumberRule::Normalize) => official_number_text(&body),
                    Some(NumberRule::OmitZero) => {
                        let normalized = official_number_text(&body);
                        if js_is_zero(&normalized) {
                            continue;
                        }
                        normalized
                    }
                    None => body,
                };
                (key, body, after)
            }
        };
        out.push_str("<div>");
        out.push_str(key);
        out.push('=');
        out.push_str(&body);
        out.push_str(key);
        out.push_str("=</div>");
        out.push_str(after);
    }
    Ok(out)
}

/// Read an official plaintext back into a field map, checking every byte of
/// structure against `layout`: header, order, keys and the text after each
/// `<div>`. Values are decoded per part (`escape()` or raw). A control written
/// twice (2551Q) must hold the same value both times. A folded value (1601C
/// Address 2 inside `txtAddress`) cannot be split, so it goes to the first
/// source.
pub fn read(
    layout: &OfficialLayout,
    plaintext: &str,
) -> Result<BTreeMap<String, String>, OfficialXmlError> {
    fn expect(text: &str, at: &mut usize, token: &str) -> Result<(), OfficialXmlError> {
        if text[*at..].starts_with(token) {
            *at += token.len();
            Ok(())
        } else {
            Err(OfficialXmlError::Layout {
                at: *at,
                expected: token.to_string(),
            })
        }
    }
    fn store(
        values: &mut BTreeMap<String, String>,
        key: &str,
        value: String,
    ) -> Result<(), OfficialXmlError> {
        match values.get(key) {
            Some(existing) if *existing != value => Err(OfficialXmlError::Inconsistent {
                key: key.to_string(),
            }),
            _ => {
                values.insert(key.to_string(), value);
                Ok(())
            }
        }
    }

    let mut values = BTreeMap::new();
    let mut at = 0;
    expect(plaintext, &mut at, &layout.header)?;
    expect(plaintext, &mut at, &layout.lead)?;
    for entry in &layout.entries {
        let (key, after) = match entry {
            Entry::Bool { key, after, .. } | Entry::Value { key, after, .. } => (key, after),
        };
        let open = format!("<div>{key}=");
        if let Entry::Value {
            parts,
            number: Some(NumberRule::OmitZero),
            ..
        } = entry
        {
            // `write` drops a zero amount and its separator; read it as "0",
            // which writes back the same way.
            if !plaintext[at..].starts_with(&open) {
                if let Some(Part::Source { source, .. }) = parts
                    .iter()
                    .find(|part| matches!(part, Part::Source { .. }))
                {
                    store(&mut values, source, "0".to_string())?;
                }
                continue;
            }
        }
        expect(plaintext, &mut at, &open)?;
        let close = format!("{key}=</div>");
        let Some(len) = plaintext[at..].find(&close) else {
            return Err(OfficialXmlError::Layout {
                at,
                expected: close,
            });
        };
        let body = &plaintext[at..at + len];
        at += len + close.len();
        match entry {
            Entry::Bool { .. } => {
                if body != "true" && body != "false" {
                    return Err(OfficialXmlError::NotBoolean {
                        key: key.clone(),
                        value: body.to_string(),
                    });
                }
                store(&mut values, key, body.to_string())?;
            }
            Entry::Value { parts, .. } => {
                let mut rest = body;
                let mut first: Option<(&str, Codec)> = None;
                for part in parts {
                    match part {
                        Part::Literal { literal } => {
                            // A literal can only be checked where it leads.
                            if first.is_none() {
                                rest = rest.strip_prefix(literal.as_str()).ok_or_else(|| {
                                    OfficialXmlError::Layout {
                                        at,
                                        expected: literal.clone(),
                                    }
                                })?;
                            }
                        }
                        Part::Source { source, codec, .. } => match first {
                            None => first = Some((source, *codec)),
                            Some(_) => store(&mut values, source, String::new())?,
                        },
                    }
                }
                if let Some((source, codec)) = first {
                    let value = match codec {
                        Codec::Raw => rest.to_string(),
                        Codec::JsEscape => js_unescape(rest)
                            .ok_or_else(|| OfficialXmlError::InvalidEscape { key: key.clone() })?,
                    };
                    store(&mut values, source, value)?;
                }
            }
        }
        expect(plaintext, &mut at, after)?;
    }
    if at != plaintext.len() {
        return Err(OfficialXmlError::Layout {
            at,
            expected: "end of plaintext".to_string(),
        });
    }
    Ok(values)
}

/// Inverse of [`js_escape`]: JavaScript's legacy `unescape()` for well-formed
/// input. `None` for a malformed `%` sequence or a lone surrogate.
pub fn js_unescape(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut units: Vec<u16> = Vec::with_capacity(value.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if bytes.get(i + 1) == Some(&b'u') {
                let hex = value.get(i + 2..i + 6)?;
                units.push(u16::from_str_radix(hex, 16).ok()?);
                i += 6;
            } else {
                let hex = value.get(i + 1..i + 3)?;
                units.push(u16::from(u8::from_str_radix(hex, 16).ok()?));
                i += 3;
            }
        } else {
            let ch = value[i..].chars().next()?;
            let mut buf = [0u16; 2];
            units.extend_from_slice(ch.encode_utf16(&mut buf));
            i += ch.len_utf8();
        }
    }
    String::from_utf16(&units).ok()
}

/// An amount as the official form holds it at submit time: `formatCurrency()`
/// and `round()` (`js/string-util.js`) give two decimals rounded half up with
/// `floor(x * 100 + 0.50000000001)`, thousands separated by commas, and a
/// leading `-` when negative. Every official money field passes through one
/// of them on blur or compute, so the plaintext carries `1,234,567.89`.
pub fn official_amount(value: f64) -> String {
    if !value.is_finite() {
        return "0.00".to_string();
    }
    let negative = value < 0.0;
    let cents_total = (value.abs() * 100.0 + 0.500_000_000_01).floor();
    let cents = (cents_total % 100.0) as u64;
    let whole = format!("{:.0}", (cents_total / 100.0).floor());
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    // Both official formatters keep the sign of any negative value, even
    // one whose cents round to zero: formatCurrency(-0.004) and
    // (-0.004).toFixed(2) give "-0.00" (checked in the official runtime).
    // -0.0 is not negative and prints "0.00", as both do.
    let sign = if negative { "-" } else { "" };
    format!("{sign}{grouped}.{cents:02}")
}

/// Parse an official amount back: commas removed, then a plain decimal.
pub fn parse_official_amount(value: &str) -> Option<f64> {
    let cleaned: String = value.chars().filter(|ch| *ch != ',').collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return Some(0.0);
    }
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// JavaScript's legacy global `escape()`: keeps `A-Z a-z 0-9 @ * _ + - . /`,
/// writes other UTF-16 code units below 256 as `%XX` and the rest as `%uXXXX`.
pub fn js_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for unit in value.encode_utf16() {
        match u8::try_from(unit) {
            Ok(byte)
                if byte.is_ascii_alphanumeric()
                    || matches!(byte, b'@' | b'*' | b'_' | b'+' | b'-' | b'.' | b'/') =>
            {
                out.push(char::from(byte));
            }
            Ok(byte) => out.push_str(&format!("%{byte:02X}")),
            Err(_) => out.push_str(&format!("%u{unit:04X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_escape_matches_the_javascript_builtin() {
        // Expected values are node's escape() output.
        for (input, expected) in [
            ("JUAN DELA CRUZ", "JUAN%20DELA%20CRUZ"),
            ("a@b.com", "a@b.com"),
            ("1,234.50", "1%2C234.50"),
            ("A*_+-./Z", "A*_+-./Z"),
            ("PEÑA", "PE%D1A"),
            ("Ñ ~!#$&'()", "%D1%20%7E%21%23%24%26%27%28%29"),
            ("你好", "%u4F60%u597D"),
            ("😀", "%uD83D%uDE00"),
            ("", ""),
        ] {
            assert_eq!(js_escape(input), expected, "{input:?}");
        }
    }

    #[test]
    fn every_packaged_layout_parses_and_ends_with_the_official_trailer() {
        for (id, _) in LAYOUTS {
            let layout = layout(id).expect("layout");
            assert_eq!(layout.form_id, *id);
            let last = layout.entries.last().expect("entries");
            let after = match last {
                Entry::Bool { after, .. } | Entry::Value { after, .. } => after,
            };
            // The year in the trailer is per form (2012; 2014 on newer pages).
            let year = after
                .rsplit("All Rights Reserved BIR ")
                .next()
                .filter(|_| after.contains("All Rights Reserved BIR "));
            assert!(
                year.is_some_and(|y| y.len() == 6 && y.ends_with(".0") && y.starts_with("20")),
                "{id}: {after:?}"
            );
        }
    }

    #[test]
    fn tiny_negative_keeps_its_minus_like_the_official_formatters() {
        // Official runtime (BIR-Form2200Tv2020.hta): formatCurrency(-0.004),
        // amtFormat(-0.004) and formatCurrency(-1e-17) all give "-0.00";
        // formatCurrency(-0) gives "0.00".
        assert_eq!(official_amount(-0.004), "-0.00");
        assert_eq!(official_amount(-1e-17), "-0.00");
        assert_eq!(official_amount(-0.0), "0.00");
        assert_eq!(official_amount(0.004), "0.00");
    }

    #[test]
    fn official_amount_matches_format_currency() {
        // Expected values are node running the official formatCurrency().
        for (input, expected) in [
            (0.0, "0.00"),
            (-0.0, "0.00"),
            (0.004, "0.00"),
            (0.005, "0.01"),
            (0.015, "0.02"),
            (1.005, "1.01"),
            (999.995, "1,000.00"),
            (1000.0, "1,000.00"),
            (1234.5, "1,234.50"),
            (50000.0, "50,000.00"),
            (1500.125, "1,500.13"),
            (-250.5, "-250.50"),
            (-0.005, "-0.01"),
            (-1234.565, "-1,234.57"),
            (1_234_567.891, "1,234,567.89"),
            (100_000_000_000.01, "100,000,000,000.01"),
            (12.3456, "12.35"),
        ] {
            assert_eq!(official_amount(input), expected, "{input}");
            assert_eq!(
                parse_official_amount(expected)
                    .map(official_amount)
                    .as_deref(),
                Some(expected),
                "{expected} round trip"
            );
        }
    }

    #[test]
    fn js_unescape_inverts_js_escape() {
        for input in [
            "JUAN DELA CRUZ",
            "PEÑA",
            "你好 😀",
            "1,234.50",
            "a@b.com",
            "",
        ] {
            assert_eq!(js_unescape(&js_escape(input)).as_deref(), Some(input));
        }
        assert_eq!(js_unescape("%G1"), None);
        assert_eq!(js_unescape("%uD83D"), None);
    }

    #[test]
    fn read_inverts_write_and_checks_structure() {
        let layout = layout("2551q-v2018").unwrap();
        let mut values = BTreeMap::new();
        values.insert(
            "frm2551Qv2018:registeredName".to_string(),
            "PEÑA, JUAN".to_string(),
        );
        values.insert("frm2551Qv2018:rtnMonth".to_string(), "06".to_string());
        values.insert("frm2551Qv2018:forThe_2".to_string(), "true".to_string());
        let plain = write(layout, &values).unwrap();
        let read_back = read(layout, &plain).unwrap();
        for (key, value) in &values {
            assert_eq!(read_back.get(key), Some(value), "{key}");
        }

        // Raw values (the upload loop doesn't escape): a value is read back as written.
        assert!(plain.contains(
            "<div>frm2551Qv2018:registeredName=PEÑA, JUANfrm2551Qv2018:registeredName=</div>"
        ));
        assert!(matches!(
            read(layout, &plain.replace("\t\t<div>", "\n<div>")),
            Err(OfficialXmlError::Layout { .. })
        ));
    }

    #[test]
    fn number_rule_normalizes_1702mx_amounts_and_keeps_zeros() {
        // The saveEncryptedProfile loop: commas stripped, "(5.00)" -> "-5.00",
        // zero amounts written like any other value.
        let layout = layout("1702mx-v2018c").unwrap();
        let values: BTreeMap<String, String> = [
            ("frm1702MX:txtPg1Pt2I14TotalIncome", "1,234.50"),
            ("frm1702MX:txtPg1Pt2I15LessTotalTax", "(5.00)"),
            ("frm1702MX:txtPg1Pt2I16NetTaxPayable", "0"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let ours = write(layout, &values).unwrap();
        assert!(ours.contains("<div>frm1702MX:txtPg1Pt2I14TotalIncome=1234.50"));
        assert!(ours.contains("<div>frm1702MX:txtPg1Pt2I15LessTotalTax=-5.00"));
        assert!(ours.contains("<div>frm1702MX:txtPg1Pt2I16NetTaxPayable=0frm"));
        let back = read(layout, &ours).unwrap();
        assert_eq!(back["frm1702MX:txtPg1Pt2I14TotalIncome"], "1234.50");
        assert_eq!(back["frm1702MX:txtPg1Pt2I16NetTaxPayable"], "0");
        assert_eq!(official_number_text("(1,234.50)"), "-1234.50");
    }

    #[test]
    fn read_accepts_an_omitted_zero_amount() {
        let layout: OfficialLayout = serde_json::from_str(
            r#"{"form_id":"t","official_hta":"","official_hta_sha256":"","header":"H","lead":"","entries":[
                {"kind":"value","key":"a","parts":[{"source":"a","codec":"raw"}],"default":"0","number":"omit-zero","after":"|"},
                {"kind":"value","key":"b","parts":[{"source":"b","codec":"raw"}],"default":"","after":""}]}"#,
        )
        .unwrap();
        let values: BTreeMap<String, String> = [("a", "0.00"), ("b", "x")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let plain = write(&layout, &values).unwrap();
        assert_eq!(plain, "H<div>b=xb=</div>");
        let back = read(&layout, &plain).unwrap();
        assert_eq!(back["a"], "0");
        assert_eq!(write(&layout, &back).unwrap(), plain);
    }

    #[test]
    fn unknown_keys_and_non_boolean_radios_are_rejected() {
        let layout = layout("2551q-v2018").unwrap();
        let mut values = BTreeMap::new();
        values.insert("__year_ended".to_string(), "122025".to_string());
        assert_eq!(
            write(layout, &values),
            Err(OfficialXmlError::UnknownKey("__year_ended".to_string()))
        );

        let mut values = BTreeMap::new();
        values.insert("frm2551Qv2018:forThe_1".to_string(), "yes".to_string());
        assert!(matches!(
            write(layout, &values),
            Err(OfficialXmlError::NotBoolean { .. })
        ));
    }

    #[test]
    fn empty_values_reproduce_the_official_page_defaults() {
        let layout = layout("2551q-v2018").unwrap();
        let plain = write(layout, &BTreeMap::new()).unwrap();
        // IE drops the newline after <xmp>, so the separator is "\t\t".
        assert!(plain.starts_with("<?xml version='1.0'?>\t\t<div>frm2551Qv2018:forThe_1="));
        assert!(plain.contains(
            "<div>frm2551Qv2018:txtTaxReliefSpecify=0frm2551Qv2018:txtTaxReliefSpecify=</div>"
        ));
        assert!(plain.ends_with("All Rights Reserved BIR 2012.0"));
    }
}
