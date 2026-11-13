//! Why Buwiz would not send a return for its TIN. The server ends every
//! refusal with its code in brackets (`"{message} ({code})"`), and the queue
//! stores that text as the return's reason; [`TinRefusal::from_reason`] is
//! the one reader of that code, for live errors and stored reasons alike.
//! Nothing here matches on the English of a message.

/// A refusal because of the TIN's state on the Buwiz account, or a banned
/// account: nothing reached BIR, and sending again cannot help until the
/// person fixes it on the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TinRefusal {
    /// `tin_not_active`: not on this account (or the server says no more).
    NotActive,
    /// `tin_unverified`: on the account, not verified yet.
    Unverified,
    /// `tin_pending`: its papers are being checked.
    Pending,
    /// `tin_needs_fix`: staff sent it back for a fix.
    NeedsFix,
    /// `tin_paused`: staff paused sending for it.
    Paused,
    /// `account_banned`: the account can send nothing.
    AccountBanned,
}

/// What the server's `tin_needs_fix` message begins with, before the fixed
/// reason sentences.
const FIX_PREFIX: &str = "Sent back for a fix: ";

/// Where the queue says a reason came from; [`TinRefusal::message_of`]
/// takes them off.
const SOURCE_PREFIXES: [&str; 3] = [
    "Needs attention: ",
    "Not sent (nothing reached BIR): ",
    "Submission session failed before upload (no PUT attempted): ",
];

impl TinRefusal {
    pub const ALL: [TinRefusal; 6] = [
        TinRefusal::NotActive,
        TinRefusal::Unverified,
        TinRefusal::Pending,
        TinRefusal::NeedsFix,
        TinRefusal::Paused,
        TinRefusal::AccountBanned,
    ];

    /// The refusal a server code names, exactly; `None` for any other code.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|refusal| refusal.code() == code)
    }

    /// The server's code.
    pub fn code(self) -> &'static str {
        match self {
            TinRefusal::NotActive => "tin_not_active",
            TinRefusal::Unverified => "tin_unverified",
            TinRefusal::Pending => "tin_pending",
            TinRefusal::NeedsFix => "tin_needs_fix",
            TinRefusal::Paused => "tin_paused",
            TinRefusal::AccountBanned => "account_banned",
        }
    }

    /// The one decoder: the trailing ` (code)` of a live or stored reason,
    /// matched against the closed set. `None` when the text does not end in
    /// one of the six codes.
    pub fn from_reason(reason: &str) -> Option<Self> {
        trailing_code(reason).and_then(|(_, code)| Self::from_code(code))
    }

    /// The server's sentence of a reason: the queue's prefix, the
    /// `Sent back for a fix: ` prefix and the trailing code taken off.
    pub fn message_of(reason: &str) -> String {
        let mut rest = reason.trim();
        for prefix in SOURCE_PREFIXES {
            if let Some(after) = rest.strip_prefix(prefix) {
                rest = after.trim();
            }
        }
        if let Some((message, _)) = trailing_code(rest) {
            rest = message;
        }
        rest.strip_prefix(FIX_PREFIX)
            .unwrap_or(rest)
            .trim()
            .to_owned()
    }

    /// `reason` with the text before its trailing code cut to `max_chars`
    /// characters (an ellipsis marks the cut), the code kept; text without
    /// a code is cut the same way.
    pub(crate) fn capped(reason: &str, max_chars: usize) -> String {
        let cut = |text: &str| match text.char_indices().nth(max_chars) {
            Some((end, _)) => format!("{}\u{2026}", &text[..end]),
            None => text.to_owned(),
        };
        match trailing_code(reason) {
            Some((message, code)) if message.is_empty() => format!("({code})"),
            Some((message, code)) => format!("{} ({code})", cut(message)),
            None => cut(reason),
        }
    }

    /// The fixed reason sentences of a `tin_needs_fix` reason, when the
    /// server named any (`Sent back for a fix: <sentences> (tin_needs_fix)`);
    /// `None` for any other reason.
    pub fn fix_reasons(reason: &str) -> Option<String> {
        if Self::from_reason(reason) != Some(TinRefusal::NeedsFix) {
            return None;
        }
        let mut rest = reason.trim();
        for prefix in SOURCE_PREFIXES {
            if let Some(after) = rest.strip_prefix(prefix) {
                rest = after.trim();
            }
        }
        let (message, _) = trailing_code(rest)?;
        let sentences = message.strip_prefix(FIX_PREFIX)?.trim();
        (!sentences.is_empty()).then(|| sentences.to_owned())
    }
}

/// `"{message} ({code})"` split at its trailing code, when it has one: the
/// text ends in `)`, the last `(` opens the text or follows whitespace (an
/// empty message reads `" (code)"`, which is `"(code)"` trimmed), and the
/// brackets hold only lowercase letters and `_`. A prefix the queue adds in
/// front never changes the answer.
fn trailing_code(reason: &str) -> Option<(&str, &str)> {
    let reason = reason.trim();
    let inner = reason.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let message = &inner[..open];
    if !(message.is_empty() || message.ends_with(char::is_whitespace)) {
        return None;
    }
    let code = &inner[open + 1..];
    let is_code = !code.is_empty()
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_');
    is_code.then(|| (message.trim_end(), code))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODES: [&str; 6] = [
        "tin_not_active",
        "tin_unverified",
        "tin_pending",
        "tin_needs_fix",
        "tin_paused",
        "account_banned",
    ];

    #[test]
    fn each_code_names_one_refusal_and_back() {
        for (refusal, code) in TinRefusal::ALL.into_iter().zip(CODES) {
            assert_eq!(refusal.code(), code);
            assert_eq!(TinRefusal::from_code(code), Some(refusal), "{code}");
        }
        for other in [
            "",
            "already_submitted",
            "relay_refused",
            "TIN_NOT_ACTIVE",
            "tin_not_active ",
            "tin",
        ] {
            assert_eq!(TinRefusal::from_code(other), None, "{other:?}");
        }
    }

    #[test]
    fn a_live_or_stored_reason_decodes_by_its_trailing_code() {
        for (refusal, code) in TinRefusal::ALL.into_iter().zip(CODES) {
            for reason in [
                format!("Some sentence the server chose. ({code})"),
                format!("Not sent (nothing reached BIR): another wording entirely ({code})"),
                format!("Submission session failed before upload (no PUT attempted): x ({code})"),
                format!("  trailing space ({code})  "),
            ] {
                assert_eq!(TinRefusal::from_reason(&reason), Some(refusal), "{reason}");
            }
        }
    }

    #[test]
    fn an_old_stored_reason_still_decodes() {
        assert_eq!(
            TinRefusal::from_reason(
                "Not sent (nothing reached BIR): this TIN is not active on your account (tin_not_active)"
            ),
            Some(TinRefusal::NotActive)
        );
    }

    #[test]
    fn a_message_with_parentheses_is_not_a_code() {
        for reason in [
            "the relay closed early (relay_closed)",
            "the connection ended (see the log)",
            "BIR may have it (tin_not_active) so check your email",
            "Not sent (nothing reached BIR): ",
            "(tin_not_active",
            "tin_not_active",
            "x(tin_not_active)",
            "x (TIN_NOT_ACTIVE)",
            "x (tin_not_active )",
            "",
        ] {
            assert_eq!(TinRefusal::from_reason(reason), None, "{reason:?}");
        }
    }

    /// An empty server message displays as `" (code)"`, and a relay error
    /// can be just `"(code)"`: the live text and the stored reason built
    /// from it decode alike.
    #[test]
    fn a_live_error_and_its_stored_reason_decode_alike() {
        for live in [
            " (tin_paused)",
            "(tin_paused)",
            "\t(tin_paused)",
            "Sending is paused. (tin_paused)",
            "x(tin_paused)",
            "(tin_paused",
            "",
            "the relay closed early",
        ] {
            for prefix in SOURCE_PREFIXES {
                let stored = format!("{prefix}{live}");
                assert_eq!(
                    TinRefusal::from_reason(live),
                    TinRefusal::from_reason(&stored),
                    "{live:?} / {stored:?}"
                );
            }
        }
        for live in [" (tin_paused)", "(tin_paused)", "\t(tin_paused)"] {
            assert_eq!(
                TinRefusal::from_reason(live),
                Some(TinRefusal::Paused),
                "{live:?}"
            );
        }
    }

    #[test]
    fn a_capped_reason_keeps_its_code_and_short_text_whole() {
        let long = format!(
            "Not sent (nothing reached BIR): {} (tin_paused)",
            "y".repeat(500)
        );
        let capped = TinRefusal::capped(&long, 40);
        assert_eq!(
            capped,
            format!(
                "Not sent (nothing reached BIR): {}\u{2026} (tin_paused)",
                "y".repeat(8)
            )
        );
        assert_eq!(TinRefusal::from_reason(&capped), Some(TinRefusal::Paused));
        let short = "Not sent (nothing reached BIR): Paused. (tin_paused)";
        assert_eq!(TinRefusal::capped(short, 40), short);
        assert_eq!(
            TinRefusal::capped(" (tin_paused)", 40),
            "(tin_paused)",
            "an empty message stays empty"
        );
        assert_eq!(TinRefusal::capped("abcdef", 3), "abc\u{2026}");
    }

    #[test]
    fn the_message_is_the_servers_sentence_alone() {
        assert_eq!(
            TinRefusal::message_of(
                "Not sent (nothing reached BIR): This TIN is not verified yet. (tin_unverified)"
            ),
            "This TIN is not verified yet."
        );
        assert_eq!(
            TinRefusal::message_of(
                "Not sent (nothing reached BIR): Sent back for a fix: A photo is too blurry to read. (tin_needs_fix)"
            ),
            "A photo is too blurry to read."
        );
        assert_eq!(
            TinRefusal::message_of(
                "Submission session failed before upload (no PUT attempted): This Buwiz account is banned. (account_banned)"
            ),
            "This Buwiz account is banned."
        );
        assert_eq!(TinRefusal::message_of("no code here"), "no code here");
    }

    #[test]
    fn fix_reasons_are_only_the_fixed_sentences_after_the_prefix() {
        assert_eq!(
            TinRefusal::fix_reasons("Not sent (nothing reached BIR): Sent back for a fix: A photo is too blurry to read. The name does not match. (tin_needs_fix)").as_deref(),
            Some("A photo is too blurry to read. The name does not match.")
        );
        // The server's sentence when it named no reason.
        assert_eq!(
            TinRefusal::fix_reasons(
                "Not sent (nothing reached BIR): Sent back for a fix. See what to change on My TINs at account.buwiz.com. (tin_needs_fix)"
            ),
            None
        );
        assert_eq!(
            TinRefusal::fix_reasons(
                "Not sent (nothing reached BIR): Sent back for a fix:  (tin_needs_fix)"
            ),
            None
        );
        // Another code's text is never read as reasons.
        assert_eq!(
            TinRefusal::fix_reasons("Sent back for a fix: x (tin_paused)"),
            None
        );
        assert_eq!(TinRefusal::fix_reasons("Sent back for a fix: x"), None);
    }

    #[test]
    fn a_transport_error_reads_its_refusal_from_its_text() {
        use crate::background_cron::TransportError;
        let refused = TransportError(
            "Sending is paused for this TIN. Write to support@buwiz.com. (tin_paused)".into(),
        );
        assert_eq!(refused.refusal(), Some(TinRefusal::Paused));
        assert_eq!(
            TransportError("could not reach the Buwiz server: connection refused".into()).refusal(),
            None
        );
    }
}
