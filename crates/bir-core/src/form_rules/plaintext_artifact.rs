//! Deterministic plaintext rendering and independent sequential validation.
//!
//! This module is intentionally not connected to final-copy payload production
//! yet. It provides the byte-exact prerequisite without accepting the legacy
//! pseudo-XML parser's envelope-noise compatibility behavior.

use std::collections::BTreeMap;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExpectedPlaintextRecord {
    PseudoXmlField {
        key: String,
        occurrence: usize,
        encoded_body: Vec<u8>,
    },
    MetadataElement {
        exact_tag: String,
        encoded_body: Vec<u8>,
    },
    ReviewedLiteral(Vec<u8>),
    Omitted,
    Accounting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParsedPlaintextRecord {
    PseudoXmlField {
        key: String,
        occurrence: usize,
        encoded_body: Vec<u8>,
    },
    MetadataElement {
        exact_tag: String,
        encoded_body: Vec<u8>,
    },
    ReviewedLiteral(Vec<u8>),
    Omitted,
    Accounting,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum PlaintextArtifactError {
    #[error("record {record_index} has invalid pseudo-XML key {key:?}")]
    InvalidKey { record_index: usize, key: String },
    #[error("record {record_index} has invalid metadata tag {tag:?}")]
    InvalidTag { record_index: usize, tag: String },
    #[error("record {record_index} encoded body contains markup delimiter at byte {body_offset}")]
    BodyContainsMarkupDelimiter {
        record_index: usize,
        body_offset: usize,
    },
    #[error(
        "record {record_index} encoded body contains repeated pseudo-XML delimiter {delimiter:?} at byte {body_offset}"
    )]
    BodyContainsRepeatedFieldDelimiter {
        record_index: usize,
        body_offset: usize,
        delimiter: String,
    },
    #[error(
        "record {record_index} declares occurrence {declared} for key {key:?}, expected {expected}"
    )]
    OccurrenceMismatch {
        record_index: usize,
        key: String,
        declared: usize,
        expected: usize,
    },
    #[error("record {record_index} is truncated at byte {offset}")]
    UnexpectedEnd { record_index: usize, offset: usize },
    #[error("record {record_index} is malformed at byte {offset}: {reason}")]
    MalformedRecord {
        record_index: usize,
        offset: usize,
        reason: String,
    },
    #[error("record {record_index} does not match its expected trace record")]
    TraceMismatch { record_index: usize },
    #[error("plaintext has {length} extra bytes beginning at byte {offset}")]
    ExtraBytes { offset: usize, length: usize },
}

pub(crate) fn render_expected_plaintext(
    records: &[ExpectedPlaintextRecord],
) -> Result<Vec<u8>, PlaintextArtifactError> {
    let mut rendered = Vec::new();
    let mut occurrences = BTreeMap::<&str, usize>::new();

    for (record_index, record) in records.iter().enumerate() {
        match record {
            ExpectedPlaintextRecord::PseudoXmlField {
                key,
                occurrence,
                encoded_body,
            } => {
                validate_key(record_index, key)?;
                let expected_occurrence = occurrences.entry(key.as_str()).or_default();
                *expected_occurrence += 1;
                if *occurrence != *expected_occurrence {
                    return Err(PlaintextArtifactError::OccurrenceMismatch {
                        record_index,
                        key: key.clone(),
                        declared: *occurrence,
                        expected: *expected_occurrence,
                    });
                }
                if let Some(body_offset) = encoded_body.iter().position(|byte| *byte == b'<') {
                    return Err(PlaintextArtifactError::BodyContainsMarkupDelimiter {
                        record_index,
                        body_offset,
                    });
                }
                let mut repeated_delimiter = Vec::with_capacity(key.len() + 1);
                repeated_delimiter.extend_from_slice(key.as_bytes());
                repeated_delimiter.push(b'=');
                if let Some(body_offset) = find_subslice(encoded_body, &repeated_delimiter) {
                    return Err(PlaintextArtifactError::BodyContainsRepeatedFieldDelimiter {
                        record_index,
                        body_offset,
                        delimiter: String::from_utf8(repeated_delimiter)
                            .expect("validated pseudo-XML keys are ASCII"),
                    });
                }

                rendered.extend_from_slice(b"<div>");
                rendered.extend_from_slice(key.as_bytes());
                rendered.push(b'=');
                rendered.extend_from_slice(encoded_body);
                rendered.extend_from_slice(key.as_bytes());
                rendered.extend_from_slice(b"=</div>");
            }
            ExpectedPlaintextRecord::MetadataElement {
                exact_tag,
                encoded_body,
            } => {
                validate_tag(record_index, exact_tag)?;
                if let Some(body_offset) = encoded_body.iter().position(|byte| *byte == b'<') {
                    return Err(PlaintextArtifactError::BodyContainsMarkupDelimiter {
                        record_index,
                        body_offset,
                    });
                }

                let closing_tag = metadata_closing_tag(exact_tag);
                rendered.push(b'<');
                rendered.extend_from_slice(exact_tag.as_bytes());
                rendered.push(b'>');
                rendered.extend_from_slice(encoded_body);
                rendered.extend_from_slice(&closing_tag);
            }
            ExpectedPlaintextRecord::ReviewedLiteral(bytes) => rendered.extend_from_slice(bytes),
            ExpectedPlaintextRecord::Omitted | ExpectedPlaintextRecord::Accounting => {}
        }
    }

    Ok(rendered)
}

pub(crate) fn parse_and_validate_plaintext(
    plaintext: &[u8],
    expected: &[ExpectedPlaintextRecord],
) -> Result<Vec<ParsedPlaintextRecord>, PlaintextArtifactError> {
    let mut cursor = 0;
    let mut occurrences = BTreeMap::<String, usize>::new();
    let mut parsed = Vec::with_capacity(expected.len());

    for (record_index, expected_record) in expected.iter().enumerate() {
        let parsed_record = match expected_record {
            ExpectedPlaintextRecord::PseudoXmlField { .. } => {
                parse_pseudo_xml_field(plaintext, &mut cursor, record_index, &mut occurrences)?
            }
            ExpectedPlaintextRecord::MetadataElement { .. } => {
                parse_metadata_element(plaintext, &mut cursor, record_index)?
            }
            ExpectedPlaintextRecord::ReviewedLiteral(expected_bytes) => {
                let end = cursor.checked_add(expected_bytes.len()).ok_or(
                    PlaintextArtifactError::UnexpectedEnd {
                        record_index,
                        offset: cursor,
                    },
                )?;
                let bytes =
                    plaintext
                        .get(cursor..end)
                        .ok_or(PlaintextArtifactError::UnexpectedEnd {
                            record_index,
                            offset: cursor,
                        })?;
                cursor = end;
                ParsedPlaintextRecord::ReviewedLiteral(bytes.to_vec())
            }
            ExpectedPlaintextRecord::Omitted => ParsedPlaintextRecord::Omitted,
            ExpectedPlaintextRecord::Accounting => ParsedPlaintextRecord::Accounting,
        };

        compare_trace_record(record_index, expected_record, &parsed_record)?;
        parsed.push(parsed_record);
    }

    if cursor != plaintext.len() {
        return Err(PlaintextArtifactError::ExtraBytes {
            offset: cursor,
            length: plaintext.len() - cursor,
        });
    }

    Ok(parsed)
}

fn parse_pseudo_xml_field(
    plaintext: &[u8],
    cursor: &mut usize,
    record_index: usize,
    occurrences: &mut BTreeMap<String, usize>,
) -> Result<ParsedPlaintextRecord, PlaintextArtifactError> {
    const OPENING: &[u8] = b"<div>";
    const CLOSING: &[u8] = b"</div>";

    let remaining = plaintext
        .get(*cursor..)
        .ok_or(PlaintextArtifactError::UnexpectedEnd {
            record_index,
            offset: *cursor,
        })?;
    if !remaining.starts_with(OPENING) {
        return Err(PlaintextArtifactError::MalformedRecord {
            record_index,
            offset: *cursor,
            reason: "expected canonical <div> opening".to_string(),
        });
    }

    let body_start = *cursor + OPENING.len();
    let closing_relative = find_subslice(&plaintext[body_start..], CLOSING).ok_or(
        PlaintextArtifactError::UnexpectedEnd {
            record_index,
            offset: body_start,
        },
    )?;
    let closing_start = body_start + closing_relative;
    let field = &plaintext[body_start..closing_start];
    let first_equals = field.iter().position(|byte| *byte == b'=').ok_or_else(|| {
        PlaintextArtifactError::MalformedRecord {
            record_index,
            offset: body_start,
            reason: "pseudo-XML field has no opening key delimiter".to_string(),
        }
    })?;
    let key_bytes = &field[..first_equals];
    let key =
        std::str::from_utf8(key_bytes).map_err(|_| PlaintextArtifactError::MalformedRecord {
            record_index,
            offset: body_start,
            reason: "pseudo-XML key is not UTF-8".to_string(),
        })?;
    validate_key(record_index, key)?;

    let mut repeated_delimiter = Vec::with_capacity(key_bytes.len() + 1);
    repeated_delimiter.extend_from_slice(key_bytes);
    repeated_delimiter.push(b'=');
    let value_with_delimiter = &field[first_equals + 1..];
    let encoded_body = value_with_delimiter
        .strip_suffix(repeated_delimiter.as_slice())
        .ok_or_else(|| PlaintextArtifactError::MalformedRecord {
            record_index,
            offset: body_start + first_equals + 1,
            reason: "pseudo-XML field has no repeated closing key delimiter".to_string(),
        })?
        .to_vec();

    let occurrence = occurrences.entry(key.to_string()).or_default();
    *occurrence += 1;
    *cursor = closing_start + CLOSING.len();

    Ok(ParsedPlaintextRecord::PseudoXmlField {
        key: key.to_string(),
        occurrence: *occurrence,
        encoded_body,
    })
}

fn parse_metadata_element(
    plaintext: &[u8],
    cursor: &mut usize,
    record_index: usize,
) -> Result<ParsedPlaintextRecord, PlaintextArtifactError> {
    let remaining = plaintext
        .get(*cursor..)
        .ok_or(PlaintextArtifactError::UnexpectedEnd {
            record_index,
            offset: *cursor,
        })?;
    if remaining.first() != Some(&b'<') || remaining.get(1) == Some(&b'/') {
        return Err(PlaintextArtifactError::MalformedRecord {
            record_index,
            offset: *cursor,
            reason: "expected metadata opening tag".to_string(),
        });
    }

    let opening_end_relative = remaining.iter().position(|byte| *byte == b'>').ok_or(
        PlaintextArtifactError::UnexpectedEnd {
            record_index,
            offset: *cursor,
        },
    )?;
    let opening_end = *cursor + opening_end_relative;
    let tag_bytes = &plaintext[*cursor + 1..opening_end];
    let tag =
        std::str::from_utf8(tag_bytes).map_err(|_| PlaintextArtifactError::MalformedRecord {
            record_index,
            offset: *cursor + 1,
            reason: "metadata tag is not UTF-8".to_string(),
        })?;
    validate_tag(record_index, tag)?;

    let closing_tag = metadata_closing_tag(tag);
    let body_start = opening_end + 1;
    let closing_relative = find_subslice(&plaintext[body_start..], &closing_tag).ok_or(
        PlaintextArtifactError::UnexpectedEnd {
            record_index,
            offset: body_start,
        },
    )?;
    let closing_start = body_start + closing_relative;
    let encoded_body = plaintext[body_start..closing_start].to_vec();
    *cursor = closing_start + closing_tag.len();

    Ok(ParsedPlaintextRecord::MetadataElement {
        exact_tag: tag.to_string(),
        encoded_body,
    })
}

fn compare_trace_record(
    record_index: usize,
    expected: &ExpectedPlaintextRecord,
    parsed: &ParsedPlaintextRecord,
) -> Result<(), PlaintextArtifactError> {
    match (expected, parsed) {
        (
            ExpectedPlaintextRecord::PseudoXmlField {
                key: expected_key,
                occurrence: expected_occurrence,
                encoded_body: expected_body,
            },
            ParsedPlaintextRecord::PseudoXmlField {
                key,
                occurrence,
                encoded_body,
            },
        ) => {
            if expected_occurrence != occurrence {
                return Err(PlaintextArtifactError::OccurrenceMismatch {
                    record_index,
                    key: key.clone(),
                    declared: *occurrence,
                    expected: *expected_occurrence,
                });
            }
            if expected_key == key && expected_body == encoded_body {
                Ok(())
            } else {
                Err(PlaintextArtifactError::TraceMismatch { record_index })
            }
        }
        (
            ExpectedPlaintextRecord::MetadataElement {
                exact_tag: expected_tag,
                encoded_body: expected_body,
            },
            ParsedPlaintextRecord::MetadataElement {
                exact_tag,
                encoded_body,
            },
        ) if expected_tag == exact_tag && expected_body == encoded_body => Ok(()),
        (
            ExpectedPlaintextRecord::ReviewedLiteral(expected),
            ParsedPlaintextRecord::ReviewedLiteral(actual),
        ) if expected == actual => Ok(()),
        (ExpectedPlaintextRecord::Omitted, ParsedPlaintextRecord::Omitted)
        | (ExpectedPlaintextRecord::Accounting, ParsedPlaintextRecord::Accounting) => Ok(()),
        _ => Err(PlaintextArtifactError::TraceMismatch { record_index }),
    }
}

fn validate_key(record_index: usize, key: &str) -> Result<(), PlaintextArtifactError> {
    if key.is_empty()
        || key.chars().any(|character| {
            !character.is_ascii()
                || character.is_ascii_whitespace()
                || matches!(character, '=' | '<' | '>' | '\'' | '"' | '/' | '\\')
        })
    {
        return Err(PlaintextArtifactError::InvalidKey {
            record_index,
            key: key.to_string(),
        });
    }
    Ok(())
}

fn validate_tag(record_index: usize, tag: &str) -> Result<(), PlaintextArtifactError> {
    let mut characters = tag.chars();
    let valid_first = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_');
    let valid_rest = characters.all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':')
    });
    if !valid_first || !valid_rest {
        return Err(PlaintextArtifactError::InvalidTag {
            record_index,
            tag: tag.to_string(),
        });
    }
    Ok(())
}

fn metadata_closing_tag(tag: &str) -> Vec<u8> {
    let mut closing = Vec::with_capacity(tag.len() + 3);
    closing.extend_from_slice(b"</");
    closing.extend_from_slice(tag.as_bytes());
    closing.push(b'>');
    closing
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &str, occurrence: usize, body: &[u8]) -> ExpectedPlaintextRecord {
        ExpectedPlaintextRecord::PseudoXmlField {
            key: key.to_string(),
            occurrence,
            encoded_body: body.to_vec(),
        }
    }

    fn metadata(tag: &str, body: &[u8]) -> ExpectedPlaintextRecord {
        ExpectedPlaintextRecord::MetadataElement {
            exact_tag: tag.to_string(),
            encoded_body: body.to_vec(),
        }
    }

    #[test]
    fn renders_and_independently_parses_all_record_kinds_in_order() {
        let records = vec![
            ExpectedPlaintextRecord::ReviewedLiteral(b"<?xml version='1.0'?>\n".to_vec()),
            field("name", 1, "PE%C3%91A 你好".as_bytes()),
            ExpectedPlaintextRecord::Omitted,
            field("name", 2, b"second%20value"),
            metadata("dateFiled", b"2026%2F07%2F24"),
            ExpectedPlaintextRecord::Accounting,
            ExpectedPlaintextRecord::ReviewedLiteral(b"\nEND\x00".to_vec()),
        ];

        let rendered = render_expected_plaintext(&records).unwrap();
        assert_eq!(
            rendered,
            concat!(
                "<?xml version='1.0'?>\n",
                "<div>name=PE%C3%91A 你好name=</div>",
                "<div>name=second%20valuename=</div>",
                "<dateFiled>2026%2F07%2F24</dateFiled>",
                "\nEND\0"
            )
            .as_bytes()
        );

        let parsed = parse_and_validate_plaintext(&rendered, &records).unwrap();
        assert_eq!(parsed.len(), records.len());
        assert_eq!(
            parsed[1],
            ParsedPlaintextRecord::PseudoXmlField {
                key: "name".to_string(),
                occurrence: 1,
                encoded_body: "PE%C3%91A 你好".as_bytes().to_vec(),
            }
        );
        assert_eq!(parsed[2], ParsedPlaintextRecord::Omitted);
        assert_eq!(parsed[5], ParsedPlaintextRecord::Accounting);
    }

    #[test]
    fn renderer_rejects_noncanonical_records_before_emitting() {
        assert!(matches!(
            render_expected_plaintext(&[field("repeat", 2, b"value")]),
            Err(PlaintextArtifactError::OccurrenceMismatch { .. })
        ));
        assert!(matches!(
            render_expected_plaintext(&[field("bad key", 1, b"value")]),
            Err(PlaintextArtifactError::InvalidKey { .. })
        ));
        assert!(matches!(
            render_expected_plaintext(&[metadata("bad tag", b"value")]),
            Err(PlaintextArtifactError::InvalidTag { .. })
        ));
        assert!(matches!(
            render_expected_plaintext(&[field("safe", 1, b"value</div>tail")]),
            Err(PlaintextArtifactError::BodyContainsMarkupDelimiter { .. })
        ));
        assert!(matches!(
            render_expected_plaintext(&[metadata("safe", b"value<other>tail")]),
            Err(PlaintextArtifactError::BodyContainsMarkupDelimiter { .. })
        ));
        assert!(matches!(
            render_expected_plaintext(&[field("safe", 1, b"value safe= tail")]),
            Err(PlaintextArtifactError::BodyContainsRepeatedFieldDelimiter { .. })
        ));
    }

    #[test]
    fn validator_rejects_wrong_key_tag_body_and_literal_bytes() {
        let cases = [
            (
                b"<div>other=valueother=</div>".to_vec(),
                vec![field("field", 1, b"value")],
            ),
            (
                b"<filedDate>value</filedDate>".to_vec(),
                vec![metadata("dateFiled", b"value")],
            ),
            (
                b"<div>field=tamperedfield=</div>".to_vec(),
                vec![field("field", 1, b"value")],
            ),
            (
                b"reviewed-literal-X".to_vec(),
                vec![ExpectedPlaintextRecord::ReviewedLiteral(
                    b"reviewed-literal-Y".to_vec(),
                )],
            ),
        ];

        for (plaintext, expected) in cases {
            assert!(matches!(
                parse_and_validate_plaintext(&plaintext, &expected),
                Err(PlaintextArtifactError::TraceMismatch { .. })
            ));
        }
    }

    #[test]
    fn validator_rejects_insertion_deletion_reorder_and_extra_bytes() {
        let expected = vec![
            field("first", 1, b"one"),
            metadata("stamp", b"two"),
            field("last", 1, b"three"),
        ];
        let rendered = render_expected_plaintext(&expected).unwrap();

        let mut inserted = rendered.clone();
        inserted.splice(0..0, b"X".iter().copied());
        assert!(parse_and_validate_plaintext(&inserted, &expected).is_err());

        let mut deleted = rendered.clone();
        deleted.remove(0);
        assert!(parse_and_validate_plaintext(&deleted, &expected).is_err());

        let reordered_records = vec![
            field("last", 1, b"three"),
            metadata("stamp", b"two"),
            field("first", 1, b"one"),
        ];
        let reordered = render_expected_plaintext(&reordered_records).unwrap();
        assert!(matches!(
            parse_and_validate_plaintext(&reordered, &expected),
            Err(PlaintextArtifactError::TraceMismatch { record_index: 0 })
        ));

        let mut extra = rendered;
        extra.extend_from_slice(b"extra");
        assert!(matches!(
            parse_and_validate_plaintext(&extra, &expected),
            Err(PlaintextArtifactError::ExtraBytes { .. })
        ));
    }

    #[test]
    fn validator_rejects_repeated_key_occurrence_mismatch() {
        let actual_records = vec![field("repeat", 1, b"first"), field("repeat", 2, b"second")];
        let rendered = render_expected_plaintext(&actual_records).unwrap();
        let expected = vec![field("repeat", 1, b"first"), field("repeat", 3, b"second")];

        assert!(matches!(
            parse_and_validate_plaintext(&rendered, &expected),
            Err(PlaintextArtifactError::OccurrenceMismatch {
                record_index: 1,
                declared: 2,
                expected: 3,
                ..
            })
        ));
    }
}
