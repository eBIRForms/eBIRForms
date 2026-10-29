use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{error::Error, fmt, str::FromStr};

const MAX_STABLE_ID_LEN: usize = 255;

/// Why a runtime identity could not be accepted.
///
/// Runtime identities are deliberately stricter than arbitrary corpus text.
/// Code generation must map reviewed source names to this stable alphabet
/// instead of allowing whitespace, control characters, or machine-local
/// locators into packaged identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    Empty {
        kind: &'static str,
    },
    TooLong {
        kind: &'static str,
        max: usize,
    },
    InvalidCharacter {
        kind: &'static str,
        index: usize,
        character: char,
    },
    InvalidBoundary {
        kind: &'static str,
    },
    InvalidFormCode,
    InvalidSha256Length {
        actual: usize,
    },
    InvalidSha256Character {
        index: usize,
        character: char,
    },
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { kind } => write!(formatter, "{kind} must not be empty"),
            Self::TooLong { kind, max } => {
                write!(formatter, "{kind} must be no longer than {max} bytes")
            }
            Self::InvalidCharacter {
                kind,
                index,
                character,
            } => write!(
                formatter,
                "{kind} contains invalid character {character:?} at byte {index}"
            ),
            Self::InvalidBoundary { kind } => {
                write!(
                    formatter,
                    "{kind} must start and end with an ASCII letter or digit"
                )
            }
            Self::InvalidFormCode => write!(
                formatter,
                "form code must contain only uppercase ASCII letters and digits"
            ),
            Self::InvalidSha256Length { actual } => write!(
                formatter,
                "SHA-256 digest must contain exactly 64 lowercase hexadecimal characters, got {actual}"
            ),
            Self::InvalidSha256Character { index, character } => write!(
                formatter,
                "SHA-256 digest contains non-lowercase-hex character {character:?} at byte {index}"
            ),
        }
    }
}

impl Error for IdentityError {}

fn validate_stable_id(kind: &'static str, value: &str) -> Result<(), IdentityError> {
    if value.is_empty() {
        return Err(IdentityError::Empty { kind });
    }
    if value.len() > MAX_STABLE_ID_LEN {
        return Err(IdentityError::TooLong {
            kind,
            max: MAX_STABLE_ID_LEN,
        });
    }
    if !value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(IdentityError::InvalidBoundary { kind });
    }

    for (index, character) in value.char_indices() {
        let valid =
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':' | '/');
        if !valid {
            return Err(IdentityError::InvalidCharacter {
                kind,
                index,
                character,
            });
        }
    }
    Ok(())
}

macro_rules! stable_id {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, IdentityError> {
                let value = value.into();
                validate_stable_id($kind, &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = IdentityError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdentityError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdentityError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::parse(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(de::Error::custom)
            }
        }
    };
}

stable_id!(RuleSetId, "rule-set ID");
stable_id!(RuleId, "rule ID");
stable_id!(CalculationId, "calculation ID");
stable_id!(OutputId, "calculation output ID");
stable_id!(ContextValueId, "context value ID");
stable_id!(FieldId, "field ID");
stable_id!(RepeatedGroupId, "repeated-group ID");
stable_id!(StableInstanceId, "stable instance ID");
stable_id!(WorkflowStateId, "workflow state ID");
stable_id!(WorkflowTransitionId, "workflow transition ID");
stable_id!(FormRevision, "form revision");
stable_id!(OfficialPackageVersion, "official package version");
stable_id!(XmlKey, "XML key");

/// Printed BIR form code, validated independently from general stable IDs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct FormCode(String);

impl FormCode {
    pub fn parse(value: impl Into<String>) -> Result<Self, IdentityError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdentityError::Empty { kind: "form code" });
        }
        if value.len() > MAX_STABLE_ID_LEN {
            return Err(IdentityError::TooLong {
                kind: "form code",
                max: MAX_STABLE_ID_LEN,
            });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Err(IdentityError::InvalidFormCode);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for FormCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for FormCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for FormCode {
    type Err = IdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<String> for FormCode {
    type Error = IdentityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for FormCode {
    type Error = IdentityError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for FormCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// A decoded SHA-256 digest.
///
/// JSON uses the canonical lowercase hexadecimal form. Uppercase, prefixes,
/// shortened values, and other algorithms fail closed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn parse(value: &str) -> Result<Self, IdentityError> {
        if value.len() != 64 {
            return Err(IdentityError::InvalidSha256Length {
                actual: value.len(),
            });
        }

        let mut bytes = [0_u8; 32];
        let source = value.as_bytes();
        for index in 0..32 {
            let high = decode_lower_hex(source[index * 2], index * 2)?;
            let low = decode_lower_hex(source[index * 2 + 1], index * 2 + 1)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut result = String::with_capacity(64);
        for byte in self.0 {
            result.push(char::from(HEX[usize::from(byte >> 4)]));
            result.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        result
    }
}

fn decode_lower_hex(byte: u8, index: usize) -> Result<u8, IdentityError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(IdentityError::InvalidSha256Character {
            index,
            character: char::from(byte),
        }),
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Sha256Digest")
            .field(&self.to_hex())
            .finish()
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for Sha256Digest {
    type Err = IdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

/// Exact identity of one reviewed compiled rule-set snapshot.
///
/// Form code alone is never a selection key. All five components participate
/// in equality, hashing, registry lookup, and evaluation-request checks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormRevisionKey {
    rule_set_id: RuleSetId,
    form_code: FormCode,
    form_revision: FormRevision,
    official_package_version: OfficialPackageVersion,
    source_set_sha256: Sha256Digest,
}

impl FormRevisionKey {
    pub const fn new(
        rule_set_id: RuleSetId,
        form_code: FormCode,
        form_revision: FormRevision,
        official_package_version: OfficialPackageVersion,
        source_set_sha256: Sha256Digest,
    ) -> Self {
        Self {
            rule_set_id,
            form_code,
            form_revision,
            official_package_version,
            source_set_sha256,
        }
    }

    pub fn parse(
        rule_set_id: impl Into<String>,
        form_code: impl Into<String>,
        form_revision: impl Into<String>,
        official_package_version: impl Into<String>,
        source_set_sha256: &str,
    ) -> Result<Self, IdentityError> {
        Ok(Self::new(
            RuleSetId::parse(rule_set_id)?,
            FormCode::parse(form_code)?,
            FormRevision::parse(form_revision)?,
            OfficialPackageVersion::parse(official_package_version)?,
            Sha256Digest::parse(source_set_sha256)?,
        ))
    }

    pub fn rule_set_id(&self) -> &RuleSetId {
        &self.rule_set_id
    }

    pub fn form_code(&self) -> &FormCode {
        &self.form_code
    }

    pub fn form_revision(&self) -> &FormRevision {
        &self.form_revision
    }

    pub fn official_package_version(&self) -> &OfficialPackageVersion {
        &self.official_package_version
    }

    pub const fn source_set_sha256(&self) -> Sha256Digest {
        self.source_set_sha256
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_reject_ambiguous_or_machine_local_text() {
        assert!(RuleId::parse("2550q-validate-tin").is_ok());
        assert!(FieldId::parse("frm2550qv2024:txtTIN1").is_ok());
        assert!(RuleId::parse("").is_err());
        assert!(RuleId::parse(" leading").is_err());
        assert!(RuleId::parse("source#C:\\temp\\form.hta").is_err());
    }

    #[test]
    fn form_code_is_canonical_and_digest_is_exact() {
        assert!(FormCode::parse("2550Q").is_ok());
        assert!(FormCode::parse("2550q").is_err());

        let lowercase = "0123456789abcdef".repeat(4);
        let digest = Sha256Digest::parse(&lowercase).expect("valid digest");
        assert_eq!(digest.to_string(), lowercase);
        assert!(Sha256Digest::parse(&"A".repeat(64)).is_err());
        assert!(Sha256Digest::parse("00").is_err());
    }

    #[test]
    fn form_revision_key_cannot_be_built_from_partial_identity() {
        let digest = "ab".repeat(32);
        let key =
            FormRevisionKey::parse("2550q-v2024-p7.9.6", "2550Q", "2024-04", "7.9.6", &digest)
                .expect("valid exact identity");

        assert_eq!(key.form_code().as_str(), "2550Q");
        assert_eq!(key.source_set_sha256().to_string(), digest);
    }
}
