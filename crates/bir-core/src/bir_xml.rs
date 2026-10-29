//! BIR pseudo-XML parser and generator.
//!
//! The BIR application uses a non-standard XML-like format:
//! ```xml
//! <?xml version='1.0'?>
//!     <div>key=url_encoded_valuekey=</div>
//! ```
//!
//! Field bodies use a per-occurrence mix of raw literals, legacy JavaScript
//! `escape()` over UTF-16, and RFC 3986 UTF-8 percent encoding. Keys appear
//! twice as delimiters. Official payloads are not guaranteed to contain line
//! breaks, so parsing is tag-oriented.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use bir_rules::serialization::BodyCodec;
use regex::Regex;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BirXmlParseError {
    #[error("BIR payload contains no field divs")]
    EmptyPayload,
    #[error("malformed BIR div at byte {offset}: {reason}")]
    MalformedDiv { offset: usize, reason: String },
    #[error("BIR div at byte {offset} has no field ID")]
    MissingFieldId { offset: usize },
    #[error(
        "BIR div at byte {offset} declares field ID {attribute_id:?} but its body uses {body_id:?}"
    )]
    MismatchedFieldId {
        offset: usize,
        attribute_id: String,
        body_id: String,
    },
    #[error("BIR payload contains duplicate field ID {field_id:?}")]
    DuplicateFieldId { field_id: String },
    #[error("BIR field {field_id:?} has invalid URL encoding")]
    InvalidEncoding { field_id: String },
    #[error("BIR field {field_id:?} has ambiguous URL encoding")]
    AmbiguousEncoding { field_id: String },
}

/// One encoded field occurrence from a BIR pseudo-XML payload.
///
/// Occurrences remain in source order. `occurrence` is assigned independently
/// for each field ID and starts at one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BirXmlEncodedOccurrence {
    pub field_id: String,
    pub occurrence: usize,
    pub encoded_value: String,
}

/// Parse every BIR field div and reject malformed, duplicate, or unidentified
/// fields. Text outside the divs is retained only as envelope noise; official
/// payloads commonly contain an XML declaration and a trailing copyright line.
pub fn parse_bir_xml_checked(content: &str) -> Result<BTreeMap<String, String>, BirXmlParseError> {
    parse_bir_xml_encoded_checked(content)?
        .into_iter()
        .map(|(field_id, encoded_value)| {
            let value =
                decode_bir_value_compatibility(&encoded_value).map_err(|error| match error {
                    CompatibilityDecodeError::InvalidEncoding => {
                        BirXmlParseError::InvalidEncoding {
                            field_id: field_id.clone(),
                        }
                    }
                    CompatibilityDecodeError::AmbiguousEncoding => {
                        BirXmlParseError::AmbiguousEncoding {
                            field_id: field_id.clone(),
                        }
                    }
                })?;
            Ok((field_id, value))
        })
        .collect()
}

/// Parse every BIR field div using one explicitly known body codec.
///
/// This is appropriate for whole documents with uniform, proven provenance,
/// including payloads emitted by [`generate_bir_xml`], which always use
/// [`BodyCodec::Utf8PercentRfc3986Unreserved`]. Unknown official imports may
/// mix codecs per occurrence and should remain on [`parse_bir_xml_checked`] or
/// parse raw occurrences with [`parse_bir_xml_encoded_occurrences_checked`].
pub fn parse_bir_xml_with_codec_checked(
    content: &str,
    codec: BodyCodec,
) -> Result<BTreeMap<String, String>, BirXmlParseError> {
    parse_bir_xml_encoded_checked(content)?
        .into_iter()
        .map(|(field_id, encoded_value)| {
            let value = decode_bir_value_with_codec(&encoded_value, codec).ok_or_else(|| {
                BirXmlParseError::InvalidEncoding {
                    field_id: field_id.clone(),
                }
            })?;
            Ok((field_id, value))
        })
        .collect()
}

/// Parse the pseudo-XML structure while retaining field bodies exactly as
/// emitted. This is needed for HTAs that apply JavaScript `escape()` to only a
/// documented subset of fields and leave literal percent signs in the rest.
pub fn parse_bir_xml_encoded_checked(
    content: &str,
) -> Result<BTreeMap<String, String>, BirXmlParseError> {
    let mut fields = BTreeMap::new();
    for field in parse_bir_xml_encoded_occurrences_checked(content)? {
        if fields
            .insert(field.field_id.clone(), field.encoded_value)
            .is_some()
        {
            return Err(BirXmlParseError::DuplicateFieldId {
                field_id: field.field_id,
            });
        }
    }
    Ok(fields)
}

/// Parse the pseudo-XML structure while retaining every encoded field
/// occurrence in source order.
///
/// Duplicate field IDs are preserved. Each occurrence is numbered from one
/// independently for its field ID, and encoded values are returned byte-for-
/// byte as they appeared between the repeated delimiters or div tags.
pub fn parse_bir_xml_encoded_occurrences_checked(
    content: &str,
) -> Result<Vec<BirXmlEncodedOccurrence>, BirXmlParseError> {
    let mut occurrence_counts = BTreeMap::new();
    let mut occurrences = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = content[cursor..].find("<div") {
        let start = cursor + relative_start;
        if content[cursor..start].contains("</div>") {
            return Err(BirXmlParseError::MalformedDiv {
                offset: cursor,
                reason: "closing tag appears before an opening tag".to_string(),
            });
        }

        let opening_end = content[start..]
            .find('>')
            .map(|index| start + index)
            .ok_or(BirXmlParseError::MalformedDiv {
                offset: start,
                reason: "opening tag is not terminated".to_string(),
            })?;
        let attributes = &content[start + "<div".len()..opening_end];
        let body_start = opening_end + 1;
        let closing_start = content[body_start..]
            .find("</div>")
            .map(|index| body_start + index)
            .ok_or(BirXmlParseError::MalformedDiv {
                offset: start,
                reason: "opening tag has no closing tag".to_string(),
            })?;

        let body = &content[body_start..closing_start];
        if body.contains("<div") {
            return Err(BirXmlParseError::MalformedDiv {
                offset: start,
                reason: "nested field divs are not supported".to_string(),
            });
        }

        let attribute_id = parse_id_attribute(attributes, start)?;
        let (field_id, encoded_value) = match attribute_id {
            Some(attribute_id) => parse_attributed_body(attribute_id, body),
            None => parse_delimited_body(body, start)?,
        };
        validate_field_id(&field_id, start)?;

        let occurrence = occurrence_counts.entry(field_id.clone()).or_insert(0);
        *occurrence += 1;
        occurrences.push(BirXmlEncodedOccurrence {
            field_id,
            occurrence: *occurrence,
            encoded_value,
        });

        cursor = closing_start + "</div>".len();
    }

    if content[cursor..].contains("</div>") {
        return Err(BirXmlParseError::MalformedDiv {
            offset: cursor,
            reason: "closing tag has no opening tag".to_string(),
        });
    }
    if occurrences.is_empty() {
        return Err(BirXmlParseError::EmptyPayload);
    }

    Ok(occurrences)
}

/// Decode a BIR field body with its explicitly reviewed serialization codec.
///
/// Raw literal bodies are returned byte-for-byte. Legacy JavaScript `escape()`
/// treats `%XX` escapes as UTF-16 code units in the Latin-1 range and `%uXXXX`
/// escapes as arbitrary UTF-16 code units. The RFC 3986 codec treats `%XX`
/// escapes as UTF-8 bytes and never gives `+` form-encoding semantics.
pub fn decode_bir_value_with_codec(value: &str, codec: BodyCodec) -> Option<String> {
    match codec {
        BodyCodec::RawLiteral => Some(value.to_string()),
        BodyCodec::LegacyJavaScriptEscape => decode_legacy_javascript_escape(value),
        BodyCodec::Utf8PercentRfc3986Unreserved => decode_utf8_percent_rfc3986(value),
    }
}

/// Compatibility decoder for historical callers that do not have a reviewed
/// per-occurrence codec.
///
/// The result is available only when the legacy JavaScript and RFC 3986
/// interpretations have the same semantic value, or when exactly one of them
/// is valid. Ambiguous and malformed encodings both return `None`; callers that
/// need the typed distinction should use [`parse_bir_xml_checked`].
pub fn decode_bir_value(value: &str) -> Option<String> {
    decode_bir_value_compatibility(value).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompatibilityDecodeError {
    InvalidEncoding,
    AmbiguousEncoding,
}

fn decode_bir_value_compatibility(value: &str) -> Result<String, CompatibilityDecodeError> {
    let legacy = decode_bir_value_with_codec(value, BodyCodec::LegacyJavaScriptEscape);
    let utf8 = decode_bir_value_with_codec(value, BodyCodec::Utf8PercentRfc3986Unreserved);

    match (legacy, utf8) {
        (None, None) => Err(CompatibilityDecodeError::InvalidEncoding),
        (Some(decoded), None) | (None, Some(decoded)) => Ok(decoded),
        (Some(legacy), Some(utf8)) if legacy == utf8 => Ok(legacy),
        (Some(_), Some(_)) => Err(CompatibilityDecodeError::AmbiguousEncoding),
    }
}

fn decode_legacy_javascript_escape(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut cursor = 0;
    let mut units = Vec::with_capacity(value.len());

    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            if bytes.get(cursor + 1) == Some(&b'u') {
                let escape = bytes.get(cursor + 2..cursor + 6)?;
                if !escape.iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                let unit = u16::from_str_radix(&value[cursor + 2..cursor + 6], 16).ok()?;
                units.push(unit);
                cursor += 6;
            } else {
                let escape = bytes.get(cursor + 1..cursor + 3)?;
                if !escape.iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                let byte = u8::from_str_radix(&value[cursor + 1..cursor + 3], 16).ok()?;
                units.push(u16::from(byte));
                cursor += 3;
            }
            continue;
        }

        let character = value[cursor..].chars().next()?;
        let mut encoded = [0; 2];
        units.extend(character.encode_utf16(&mut encoded).iter().copied());
        cursor += character.len_utf8();
    }

    String::from_utf16(&units).ok()
}

fn decode_utf8_percent_rfc3986(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut cursor = 0;
    let mut decoded = Vec::with_capacity(value.len());

    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            let escape = bytes.get(cursor + 1..cursor + 3)?;
            if !escape.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            decoded.push(u8::from_str_radix(&value[cursor + 1..cursor + 3], 16).ok()?);
            cursor += 3;
        } else {
            let character = value[cursor..].chars().next()?;
            let character_end = cursor + character.len_utf8();
            decoded.extend_from_slice(&bytes[cursor..character_end]);
            cursor = character_end;
        }
    }

    String::from_utf8(decoded).ok()
}

fn parse_id_attribute(attributes: &str, offset: usize) -> Result<Option<&str>, BirXmlParseError> {
    let attributes = attributes.trim();
    if attributes.is_empty() {
        return Ok(None);
    }

    static ID_ATTRIBUTE: OnceLock<Regex> = OnceLock::new();
    let expression = ID_ATTRIBUTE.get_or_init(|| {
        Regex::new(r#"(?i)(?:^|\s)id\s*=\s*(?:\"([^\"]*)\"|'([^']*)')"#)
            .expect("the field ID attribute regex is valid")
    });
    let mut matches = expression.captures_iter(attributes);
    let Some(captures) = matches.next() else {
        return Err(BirXmlParseError::MissingFieldId { offset });
    };
    if matches.next().is_some() {
        return Err(BirXmlParseError::MalformedDiv {
            offset,
            reason: "opening tag declares more than one ID attribute".to_string(),
        });
    }
    let id = captures
        .get(1)
        .or_else(|| captures.get(2))
        .map(|capture| capture.as_str())
        .unwrap_or_default();
    if id.is_empty() {
        return Err(BirXmlParseError::MissingFieldId { offset });
    }
    Ok(Some(id))
}

fn parse_attributed_body(attribute_id: &str, body: &str) -> (String, String) {
    let delimiter = format!("{attribute_id}=");
    let encoded_value = body
        .strip_prefix(&delimiter)
        .and_then(|value| value.strip_suffix(&delimiter))
        .unwrap_or(body);
    (attribute_id.to_string(), encoded_value.to_string())
}

fn parse_delimited_body(body: &str, offset: usize) -> Result<(String, String), BirXmlParseError> {
    let first_equals = body
        .find('=')
        .ok_or(BirXmlParseError::MissingFieldId { offset })?;
    let field_id = &body[..first_equals];
    if field_id.is_empty() {
        return Err(BirXmlParseError::MissingFieldId { offset });
    }
    let value_with_delimiter = &body[first_equals + 1..];
    let closing_delimiter = format!("{field_id}=");
    let encoded_value = value_with_delimiter
        .strip_suffix(&closing_delimiter)
        .ok_or(BirXmlParseError::MalformedDiv {
            offset,
            reason: format!("field {field_id:?} is missing its repeated closing delimiter"),
        })?;
    Ok((field_id.to_string(), encoded_value.to_string()))
}

fn validate_field_id(field_id: &str, offset: usize) -> Result<(), BirXmlParseError> {
    if field_id.is_empty()
        || field_id.chars().any(|character| {
            character.is_whitespace()
                || matches!(character, '=' | '<' | '>' | '\'' | '"' | '/' | '\\')
        })
    {
        return Err(BirXmlParseError::MalformedDiv {
            offset,
            reason: format!("invalid field ID {field_id:?}"),
        });
    }
    Ok(())
}

/// Generate BIR pseudo-XML from a key-value map.
pub fn generate_bir_xml(fields: &BTreeMap<String, String>) -> String {
    let mut output = String::from("<?xml version='1.0'?>\t\n");
    for (key, value) in fields {
        let encoded_value = urlencoding::encode(value);
        output.push_str(&format!(
            "            <div>{}={}{}=</div>\t\n",
            key, encoded_value, key
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_payload_round_trips_through_explicit_utf8_parser() {
        let mut fields = BTreeMap::new();
        fields.insert("frm:txtName".to_string(), "JOHN DOE".to_string());
        fields.insert("frm:txtTIN1".to_string(), "010".to_string());
        fields.insert(
            "frm:txtMemo".to_string(),
            "PEÑA 你好 😀 costs £100".to_string(),
        );

        let xml = generate_bir_xml(&fields);
        assert_eq!(
            parse_bir_xml_with_codec_checked(&xml, BodyCodec::Utf8PercentRfc3986Unreserved)
                .unwrap(),
            fields
        );
        assert!(matches!(
            parse_bir_xml_checked(&xml),
            Err(BirXmlParseError::AmbiguousEncoding { .. })
        ));
    }

    #[test]
    fn parses_xml_declaration_and_all_fields_on_one_line() {
        let xml = "<?xml version='1.0'?><div>frm0619E:txtMonth=04frm0619E:txtMonth=</div><div>frm0619E:txtTaxpayerName=JUAN%20DELA%20CRUZfrm0619E:txtTaxpayerName=</div>All Rights Reserved BIR 2012.0";

        let parsed = parse_bir_xml_checked(xml).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed["frm0619E:txtMonth"], "04");
        assert_eq!(parsed["frm0619E:txtTaxpayerName"], "JUAN DELA CRUZ");
    }

    #[test]
    fn parses_attributed_field_divs() {
        let xml =
            "<div id=\"frm0619E:txtMonth\">04</div><div id='txtEmail'>test%40example.com</div>";

        let parsed = parse_bir_xml_checked(xml).unwrap();
        assert_eq!(parsed["frm0619E:txtMonth"], "04");
        assert_eq!(parsed["txtEmail"], "test@example.com");
    }

    #[test]
    fn attributed_bodies_only_unwrap_the_declared_id_envelope() {
        let xml = concat!(
            "<div id=\"memo\">a=b</div>",
            "<div id=\"wrapped\">wrapped=a=bwrapped=</div>",
            "<div id=\"declared\">other=valueother=</div>"
        );

        assert_eq!(
            parse_bir_xml_encoded_occurrences_checked(xml).unwrap(),
            vec![
                BirXmlEncodedOccurrence {
                    field_id: "memo".to_string(),
                    occurrence: 1,
                    encoded_value: "a=b".to_string(),
                },
                BirXmlEncodedOccurrence {
                    field_id: "wrapped".to_string(),
                    occurrence: 1,
                    encoded_value: "a=b".to_string(),
                },
                BirXmlEncodedOccurrence {
                    field_id: "declared".to_string(),
                    occurrence: 1,
                    encoded_value: "other=valueother=".to_string(),
                },
            ]
        );
    }

    #[test]
    fn encoded_occurrences_preserve_source_order_duplicates_and_exact_values() {
        let xml = concat!(
            "<div>alpha=first%20valuealpha=</div>",
            "<div id=\"beta\">literal+%GG%25</div>",
            "<div>alpha=second%2Fvaluealpha=</div>",
            "<div id='alpha'>third%u00D1</div>",
            "<div>beta=last%3Dbeta=</div>"
        );

        assert_eq!(
            parse_bir_xml_encoded_occurrences_checked(xml).unwrap(),
            vec![
                BirXmlEncodedOccurrence {
                    field_id: "alpha".to_string(),
                    occurrence: 1,
                    encoded_value: "first%20value".to_string(),
                },
                BirXmlEncodedOccurrence {
                    field_id: "beta".to_string(),
                    occurrence: 1,
                    encoded_value: "literal+%GG%25".to_string(),
                },
                BirXmlEncodedOccurrence {
                    field_id: "alpha".to_string(),
                    occurrence: 2,
                    encoded_value: "second%2Fvalue".to_string(),
                },
                BirXmlEncodedOccurrence {
                    field_id: "alpha".to_string(),
                    occurrence: 3,
                    encoded_value: "third%u00D1".to_string(),
                },
                BirXmlEncodedOccurrence {
                    field_id: "beta".to_string(),
                    occurrence: 2,
                    encoded_value: "last%3D".to_string(),
                },
            ]
        );
    }

    #[test]
    fn encoded_occurrence_parser_rejects_malformed_delimiters_and_input() {
        assert!(matches!(
            parse_bir_xml_encoded_occurrences_checked("<div>alpha=valuebeta=</div>"),
            Err(BirXmlParseError::MalformedDiv { .. })
        ));
        assert!(matches!(
            parse_bir_xml_encoded_occurrences_checked("<div>alpha=valuealpha="),
            Err(BirXmlParseError::MalformedDiv { .. })
        ));
        assert!(matches!(
            parse_bir_xml_encoded_occurrences_checked("</div><div>alpha=valuealpha=</div>"),
            Err(BirXmlParseError::MalformedDiv { .. })
        ));
        assert!(matches!(
            parse_bir_xml_encoded_occurrences_checked("<div>alpha=valuealpha=</div></div>"),
            Err(BirXmlParseError::MalformedDiv { .. })
        ));
        assert!(matches!(
            parse_bir_xml_encoded_occurrences_checked("<div><div>alpha=valuealpha=</div></div>"),
            Err(BirXmlParseError::MalformedDiv { .. })
        ));
        assert_eq!(
            parse_bir_xml_encoded_occurrences_checked("envelope only"),
            Err(BirXmlParseError::EmptyPayload)
        );
    }

    #[test]
    fn rejects_duplicate_field_ids() {
        let xml = "<div>txtEmail=a%40example.comtxtEmail=</div><div id=\"txtEmail\">b%40example.com</div>";

        assert_eq!(
            parse_bir_xml_encoded_checked(xml),
            Err(BirXmlParseError::DuplicateFieldId {
                field_id: "txtEmail".to_string(),
            })
        );
        assert_eq!(
            parse_bir_xml_checked(xml),
            Err(BirXmlParseError::DuplicateFieldId {
                field_id: "txtEmail".to_string(),
            })
        );
    }

    #[test]
    fn rejects_malformed_and_unidentified_fields() {
        assert!(matches!(
            parse_bir_xml_checked("<div>txtEmail=value</div>"),
            Err(BirXmlParseError::MalformedDiv { .. })
        ));
        assert!(matches!(
            parse_bir_xml_checked("<div>=value=</div>"),
            Err(BirXmlParseError::MissingFieldId { .. })
        ));
        assert!(matches!(
            parse_bir_xml_checked("<div id=\"\">value</div>"),
            Err(BirXmlParseError::MissingFieldId { .. })
        ));
        assert!(matches!(
            parse_bir_xml_checked("<div>txtEmail=valuetxtEmail="),
            Err(BirXmlParseError::MalformedDiv { .. })
        ));
    }

    #[test]
    fn url_encoding_is_decoded_once_into_semantic_values() {
        let mut fields = BTreeMap::new();
        fields.insert("fn".to_string(), "TORRES, MARITA JOY".to_string());

        let xml = generate_bir_xml(&fields);
        assert!(xml.contains("TORRES%2C%20MARITA%20JOY"));
        assert_eq!(
            parse_bir_xml_with_codec_checked(&xml, BodyCodec::Utf8PercentRfc3986Unreserved)
                .unwrap(),
            fields
        );
    }

    #[test]
    fn legacy_javascript_escape_values_decode_as_utf16() {
        let xml = concat!(
            "<div>name=PE%D1A%20%u4F60%u597Dname=</div>",
            "<div>emoji=%uD83D%uDE00emoji=</div>"
        );

        let parsed = parse_bir_xml_checked(xml).unwrap();

        assert_eq!(parsed["name"], "PEÑA 你好");
        assert_eq!(parsed["emoji"], "😀");
    }

    #[test]
    fn explicit_body_codecs_decode_without_guessing() {
        assert_eq!(
            decode_bir_value_with_codec("%C2%A3", BodyCodec::RawLiteral),
            Some("%C2%A3".to_string())
        );
        assert_eq!(
            decode_bir_value_with_codec("%GG%", BodyCodec::RawLiteral),
            Some("%GG%".to_string())
        );
        assert_eq!(
            decode_bir_value_with_codec("%C2%A3", BodyCodec::LegacyJavaScriptEscape),
            Some("Â£".to_string())
        );
        assert_eq!(
            decode_bir_value_with_codec(
                "PE%D1A%20%u4F60%u597D%20%uD83D%uDE00",
                BodyCodec::LegacyJavaScriptEscape
            ),
            Some("PEÑA 你好 😀".to_string())
        );
        assert_eq!(
            decode_bir_value_with_codec("%C2%A3%20%2B+", BodyCodec::Utf8PercentRfc3986Unreserved),
            Some("£ ++".to_string())
        );
    }

    #[test]
    fn malformed_encoding_is_rejected_by_each_encoded_codec() {
        for encoded in ["%GG", "%", "%A", "%u123", "%u12GG", "%uD83D", "%uDE00"] {
            assert_eq!(
                decode_bir_value_with_codec(encoded, BodyCodec::LegacyJavaScriptEscape),
                None,
                "legacy JavaScript escape accepted {encoded:?}"
            );
        }
        for encoded in ["%GG", "%", "%A", "%u0041", "%D1", "%C2"] {
            assert_eq!(
                decode_bir_value_with_codec(encoded, BodyCodec::Utf8PercentRfc3986Unreserved),
                None,
                "UTF-8 percent codec accepted {encoded:?}"
            );
        }
    }

    #[test]
    fn compatibility_parser_rejects_malformed_encoding_with_field_context() {
        for encoded in ["%GG", "%", "%A", "%u123", "%u12GG", "%uD83D", "%uDE00"] {
            let xml = format!("<div>reviewer={encoded}reviewer=</div>");
            assert_eq!(
                parse_bir_xml_checked(&xml),
                Err(BirXmlParseError::InvalidEncoding {
                    field_id: "reviewer".to_string(),
                }),
                "semantic parser accepted {encoded:?}"
            );
            assert_eq!(decode_bir_value(encoded), None);
        }
    }

    #[test]
    fn compatibility_parser_rejects_different_valid_codec_interpretations() {
        let xml = "<div>currency=%C2%A3currency=</div>";

        assert_eq!(
            parse_bir_xml_checked(xml),
            Err(BirXmlParseError::AmbiguousEncoding {
                field_id: "currency".to_string(),
            })
        );
        assert_eq!(decode_bir_value("%C2%A3"), None);
    }

    #[test]
    fn compatibility_parser_preserves_unambiguous_ascii_and_percent_u_values() {
        let xml = concat!(
            "<div>ascii=A%20B%2FCascii=</div>",
            "<div>latin1=PE%D1Alatin1=</div>",
            "<div>unicode=%u4F60%u597Dunicode=</div>",
            "<div>emoji=%uD83D%uDE00emoji=</div>"
        );

        let parsed = parse_bir_xml_checked(xml).unwrap();

        assert_eq!(parsed["ascii"], "A B/C");
        assert_eq!(parsed["latin1"], "PEÑA");
        assert_eq!(parsed["unicode"], "你好");
        assert_eq!(parsed["emoji"], "😀");
    }
}
