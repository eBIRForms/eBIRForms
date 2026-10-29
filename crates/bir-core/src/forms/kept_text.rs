//! A text field that remembers whether the stored draft had its key.
//!
//! Maintainer decision P2-4 (2026-10-18): a 1600PT or 1600VT draft stored
//! without one of its Part I text boxes (an older shape) opens with that box
//! blank, and Check asks for it, instead of failing to open. Saved again
//! without the box typed, it is written back without the key, so its stored
//! bytes stay the bytes it was read from ([`KeptText::is_absent`] with
//! `#[serde(default, skip_serializing_if = "KeptText::is_absent")]`), the
//! way [`super::form_1701q::PenaltySlot`] keeps 1701Q's penalty keys.
//!
//! The rule: a value is left out on save only while it is blank (empty or
//! spaces only) and absent. It is absent when the stored draft lacked the
//! key (the serde default) and nothing replaced it whole through [`From`];
//! a key read from the store ([`Deserialize`]) and a value built with
//! [`From`] (a new draft from the profile) are present, and stay so.
//! Editing in place through [`DerefMut`] (`*text = ...`, `push_str`,
//! `clear`; what the desktop editor's `set_typed_part_i` does on every
//! change) keeps the presence the value had, so a missing box files as
//! soon as it holds text and stays out while it is blank. The page's fill
//! writes the draft's JSON key (a cleared box as `""`), which would read
//! back present; for these two forms the host takes a blank key the
//! stored draft lacked out again after the fill
//! (bir-shell `rows_monthly::keep_absent_blank`), so on the page too a
//! missing box stays out while blank. A present value serializes as the
//! plain string, byte for byte what a `String` field wrote.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::ops::{Deref, DerefMut};

/// A `String` field of a stored draft that may be missing from an older
/// shape. Reads and writes as the `String` it holds.
#[derive(Clone, Default)]
pub struct KeptText {
    text: String,
    /// The stored draft carried the key, or the app wrote the value.
    present: bool,
}

impl KeptText {
    /// Missing from the stored draft and still blank (empty or spaces
    /// only): left out on save.
    pub fn is_absent(&self) -> bool {
        !self.present && self.text.trim().is_empty()
    }
}

impl From<String> for KeptText {
    fn from(text: String) -> Self {
        Self {
            text,
            present: true,
        }
    }
}

impl From<&str> for KeptText {
    fn from(text: &str) -> Self {
        Self::from(text.to_string())
    }
}

impl Deref for KeptText {
    type Target = String;
    fn deref(&self) -> &String {
        &self.text
    }
}

impl DerefMut for KeptText {
    fn deref_mut(&mut self) -> &mut String {
        &mut self.text
    }
}

/// The text alone: whether the key was stored is not part of the value.
impl PartialEq for KeptText {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl PartialEq<str> for KeptText {
    fn eq(&self, other: &str) -> bool {
        self.text == other
    }
}

impl PartialEq<&str> for KeptText {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl fmt::Debug for KeptText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.text, f)
    }
}

impl fmt::Display for KeptText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.text, f)
    }
}

impl Serialize for KeptText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.text.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for KeptText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::KeptText;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Draft {
        a: u8,
        #[serde(default, skip_serializing_if = "KeptText::is_absent")]
        email: KeptText,
        z: u8,
    }

    #[test]
    fn a_missing_key_stays_missing_until_typed() {
        let old = r#"{"a":1,"z":2}"#;
        let mut draft: Draft = serde_json::from_str(old).unwrap();
        assert_eq!(*draft.email, "");
        assert!(draft.email.is_absent());
        assert_eq!(serde_json::to_string(&draft).unwrap(), old);
        draft.email.push_str("a@b.c");
        assert_eq!(
            serde_json::to_string(&draft).unwrap(),
            r#"{"a":1,"email":"a@b.c","z":2}"#
        );
        // Cleared again, it is blank and missing, as it was read.
        draft.email.clear();
        assert_eq!(serde_json::to_string(&draft).unwrap(), old);
        // Spaces only are blank too.
        *draft.email = "   ".to_string();
        assert!(draft.email.is_absent());
        assert_eq!(serde_json::to_string(&draft).unwrap(), old);
    }

    #[test]
    fn a_stored_or_written_value_is_kept_even_blank() {
        let stored = r#"{"a":1,"email":"","z":2}"#;
        let draft: Draft = serde_json::from_str(stored).unwrap();
        assert!(!draft.email.is_absent());
        assert_eq!(serde_json::to_string(&draft).unwrap(), stored);
        let written = Draft {
            a: 1,
            email: String::new().into(),
            z: 2,
        };
        assert_eq!(serde_json::to_string(&written).unwrap(), stored);
        assert_eq!(written, draft);
    }
}
