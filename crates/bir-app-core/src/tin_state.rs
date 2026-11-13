//! What the apps say about a TIN's state on the Buwiz account: the one
//! wording table (a state or a refusal code to its chip, line, next step and
//! button), and the Queue gate as a pure function over the account's list
//! the caller read ([`bir_submit::Submission::account_tins`]).
//!
//! - `line`, `chip` and `next` are safe for anyone, an assistant included:
//!   they hold the fixed sentences only. Staff's free-text note of a
//!   sent-back TIN is [`TinStatement::staff_note`], for app-only surfaces.
//! - The gate fails open: it blocks only on a known state that cannot send,
//!   or on a TIN missing from a list the server marked complete. The row's
//!   `can_send` decides: true never blocks, whatever its status.

pub use bir_core::forms::taxform::TinKey;
pub use bir_core::tin_refusal::TinRefusal;
pub use bir_submit::tins::ACCOUNT_SITE;
use bir_submit::tins::{STALE_LIMIT_SECS, pinned_account_url};
use bir_submit::{AccountTin, TinStatus, TinsError, TinsRead};

use crate::sending::SendStage;
use crate::{AppResult, BirApp, ReturnRow};

/// A TIN's state on the account, as the apps name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TinState {
    Active,
    NotOnAccount,
    NotVerified,
    WaitingForReview,
    SentBack,
    Paused,
    AccountBanned,
    /// On the account and active, but this account may not send it.
    NotAllowed,
}

impl TinState {
    /// The state's name in an app's JSON (`not_verified`, …).
    pub fn name(self) -> &'static str {
        match self {
            TinState::Active => "active",
            TinState::NotOnAccount => "not_on_account",
            TinState::NotVerified => "not_verified",
            TinState::WaitingForReview => "waiting_for_review",
            TinState::SentBack => "sent_back",
            TinState::Paused => "paused",
            TinState::AccountBanned => "account_banned",
            TinState::NotAllowed => "not_allowed",
        }
    }
}

/// What a person can do about a state, on the account site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TinStep {
    Add,
    Verify,
    Fix,
}

impl TinStep {
    /// The button's label.
    pub fn label(self) -> &'static str {
        match self {
            TinStep::Add => "Add it",
            TinStep::Verify => "Verify it",
            TinStep::Fix => "Fix it",
        }
    }

    /// The step's name in an app's JSON (`add`, `verify`, `fix`).
    pub fn kind(self) -> &'static str {
        match self {
            TinStep::Add => "add",
            TinStep::Verify => "verify",
            TinStep::Fix => "fix",
        }
    }

    /// Where the step opens, on [`ACCOUNT_SITE`]. Only "Add it" carries the
    /// TIN (`key`); Verify and Fix open the row's own page (`action_url`)
    /// when the list gave one that is a page of the account site
    /// ([`bir_submit::tins::pinned_account_url`]), else the account's TIN
    /// list.
    pub fn link(self, key: &TinKey, action_url: Option<&str>) -> String {
        match (self, action_url.and_then(pinned_account_url)) {
            (TinStep::Add, _) => format!("{ACCOUNT_SITE}/tins/new?tin={}&branch={}", key.root(), key.branch()),
            (_, Some(url)) => url,
            (_, None) => format!("{ACCOUNT_SITE}/tins"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TinTone {
    Danger,
    Warning,
    Info,
}

/// One state in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TinStatement {
    pub state: TinState,
    /// A short label for a chip.
    pub chip: &'static str,
    /// The one line that says what is wrong. Safe for anyone.
    pub line: String,
    /// What to do next, ending "Your answers are kept.". Safe for anyone.
    pub next: String,
    pub step: Option<TinStep>,
    pub tone: TinTone,
    /// Staff's note on a sent-back TIN (sanitized, at most 200 characters),
    /// from the account's list only. For app-only surfaces: never give it to
    /// an assistant, and never store it.
    pub staff_note: Option<String>,
    /// The row's own page for its step, from the account's list.
    pub action_url: Option<String>,
}

impl TinStatement {
    /// Where its step opens for the return's TIN `key` ([`TinStep::link`]).
    pub fn link(&self, key: &TinKey) -> Option<String> {
        self.step.map(|step| step.link(key, self.action_url.as_deref()))
    }
}

/// Why the gate could not tell a TIN's state; it then lets Queue through
/// and the send decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unknown {
    /// No one is signed in (the desktop queues and waits).
    SignedOut,
    Offline,
    /// The list is older than the limit ([`bir_submit::tins::STALE_LIMIT_SECS`]).
    Stale,
    /// The TIN is not in a list the server did not mark complete.
    Incomplete,
    /// The server named a state this build does not know.
    UnknownStatus,
    /// The list or the TIN could not be read.
    Unreadable,
}

/// The Queue gate's answer for one TIN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueGate {
    pub can_queue: bool,
    /// The known state that is not active (`None` when active or unknown).
    /// With `can_queue` true it is information only: the server says the
    /// TIN can send although its status is not approved.
    pub statement: Option<TinStatement>,
    pub unknown: Option<Unknown>,
    /// When the list that decided was read.
    pub checked_at: Option<i64>,
}

/// A return the server refused for its TIN, back to a draft.
#[derive(Clone, Debug, PartialEq)]
pub struct RefusedReturn {
    pub row: ReturnRow,
    /// Always [`SendStage::Refused`].
    pub stage: SendStage,
}

impl BirApp {
    /// Added to a refused return's line once it is a Draft.
    pub const DRAFT_NOTE: &'static str = "Back to a draft. Nothing was sent.";

    /// Whether the return's TIN (9, 12 or 14 digits) may be queued, from the
    /// account lists the caller read (one per sign-in; today one) at `now`
    /// (seconds since the epoch). A list read more than
    /// [`bir_submit::tins::STALE_LIMIT_SECS`] before `now`, or more than a
    /// minute after it (the clock moved back since), is stale.
    pub fn tin_gate(tin: &str, account: &[TinsRead], now: i64) -> QueueGate {
        let Some(key) = TinKey::parse(tin) else {
            return QueueGate::unknown(Unknown::Unreadable, None);
        };
        let reads: Vec<(Known, Option<i64>)> = account.iter().map(|read| known(&key, read, now)).collect();
        // Any sign-in that can send it lets it through; then any that cannot
        // tell; only when every one knows it cannot send does Queue close.
        if let Some((Known::State(state, facts, _), at)) = reads.iter().find(|(known, _)| matches!(known, Known::State(_, _, true))) {
            let statement = (*state != TinState::Active).then(|| statement(*state, facts.clone()));
            return QueueGate { can_queue: true, statement, unknown: None, checked_at: *at };
        }
        if let Some((Known::Unknown(why), at)) = reads.iter().find(|(known, _)| matches!(known, Known::Unknown(_))) {
            return QueueGate::unknown(*why, *at);
        }
        match reads.into_iter().next() {
            Some((Known::State(state, row, _), at)) => QueueGate { can_queue: false, statement: Some(statement(state, row)), unknown: None, checked_at: at },
            _ => QueueGate::unknown(Unknown::SignedOut, None),
        }
    }

    /// The words for a refusal code, from a live or stored `reason` (whose
    /// fixed fix reasons a sent-back line names).
    pub fn tin_statement_for_refusal(code: TinRefusal, reason: Option<&str>) -> TinStatement {
        let state = match code {
            TinRefusal::NotActive => TinState::NotOnAccount,
            TinRefusal::Unverified => TinState::NotVerified,
            TinRefusal::Pending => TinState::WaitingForReview,
            TinRefusal::NeedsFix => TinState::SentBack,
            TinRefusal::Paused => TinState::Paused,
            TinRefusal::AccountBanned => TinState::AccountBanned,
        };
        let reasons = reason.and_then(TinRefusal::fix_reasons);
        statement(state, Facts { reasons, note: None, action_url: None })
    }

    /// The words for a send stage that is a TIN refusal: a refused draft, or
    /// a queued failure whose reason carries a code. Never one for a return
    /// BIR may have.
    pub fn tin_statement_for_stage(stage: &SendStage) -> Option<TinStatement> {
        match stage {
            SendStage::Refused { code, reason } => Some(Self::tin_statement_for_refusal(*code, Some(reason))),
            SendStage::Failed { reason, .. } => TinRefusal::from_reason(reason).map(|code| Self::tin_statement_for_refusal(code, Some(reason))),
            _ => None,
        }
    }

    /// What to show for a return: the live state when the list knew the
    /// TIN (nothing when it is active now), else the stored refusal. It
    /// cannot tell which is newer: when the refusal's time is known
    /// ([`RefusedReturn::refused_at`]), use [`Self::effective_statement_since`].
    pub fn effective_statement(stored: Option<TinStatement>, live: &QueueGate) -> Option<TinStatement> {
        Self::effective_statement_since(stored, None, live)
    }

    /// [`Self::effective_statement`] for a refusal stored at `refused_at`
    /// (seconds since the epoch): a list read at or before that time says
    /// nothing about it, so the stored refusal stands. A banned read, which
    /// has no read time, is always newer.
    pub fn effective_statement_since(stored: Option<TinStatement>, refused_at: Option<i64>, live: &QueueGate) -> Option<TinStatement> {
        let newer = match (refused_at, live.checked_at) {
            (None, _) => true,
            (Some(refused_at), Some(checked_at)) => checked_at > refused_at,
            (Some(_), None) => live.statement.is_some(),
        };
        if live.unknown.is_none() && newer { live.statement.clone() } else { stored }
    }

    /// The returns the server refused for their TIN, now drafts.
    pub fn refused_returns(&self) -> AppResult<Vec<RefusedReturn>> {
        let stages = self.send_states()?;
        Ok(self
            .list_returns(crate::ReturnFilter::All)?
            .into_iter()
            .filter_map(|row| match stages.get(&row.id) {
                Some(stage @ SendStage::Refused { .. }) => Some(RefusedReturn { row, stage: stage.clone() }),
                _ => None,
            })
            .collect())
    }
}

impl RefusedReturn {
    /// When the return was last written (it went back to a draft then, or
    /// was edited since), in seconds since the epoch: the time to give
    /// [`BirApp::effective_statement_since`].
    pub fn refused_at(&self) -> Option<i64> {
        let at = self.row.updated_at.trim();
        chrono::DateTime::parse_from_rfc3339(at)
            .map(|at| at.timestamp())
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%d %H:%M:%S").map(|at| at.and_utc().timestamp()))
            .ok()
    }
}

impl QueueGate {
    fn unknown(why: Unknown, checked_at: Option<i64>) -> Self {
        QueueGate { can_queue: true, statement: None, unknown: Some(why), checked_at }
    }
}

/// What one account list says about a TIN.
enum Known {
    /// Its state, and whether the server says it can send (which decides).
    State(TinState, Facts, bool),
    Unknown(Unknown),
}

/// What a sent-back statement and a step's link take from the list's row.
#[derive(Clone, Default)]
struct Facts {
    /// The fixed reason sentences.
    reasons: Option<String>,
    note: Option<String>,
    action_url: Option<String>,
}

/// How far ahead of `now` a list's read time may be and still count: the
/// clock moved back by more than this since the read.
const CLOCK_SKEW_SECS: i64 = 60;

/// What `read` (read at its `fetched_at`) says about `key` at `now`, and
/// when it was read.
fn known(key: &TinKey, read: &TinsRead, now: i64) -> (Known, Option<i64>) {
    let list = match read {
        Ok(list) => list,
        Err(TinsError::Banned) => return (Known::State(TinState::AccountBanned, Facts::default(), false), None),
        Err(TinsError::SignedOut) => return (Known::Unknown(Unknown::SignedOut), None),
        Err(TinsError::Offline) => return (Known::Unknown(Unknown::Offline), None),
        Err(_) => return (Known::Unknown(Unknown::Unreadable), None),
    };
    let at = Some(list.fetched_at);
    if now - list.fetched_at >= STALE_LIMIT_SECS || list.fetched_at - now > CLOCK_SKEW_SECS {
        return (Known::Unknown(Unknown::Stale), at);
    }
    // Two rows for one TIN: the one that can send; rows that disagree
    // otherwise say nothing for sure.
    let rows: Vec<&AccountTin> = list.rows.iter().filter(|row| &row.key == key).collect();
    let row = match rows.as_slice() {
        [] => {
            let known = if list.complete { Known::State(TinState::NotOnAccount, Facts::default(), false) } else { Known::Unknown(Unknown::Incomplete) };
            return (known, at);
        }
        [row] => *row,
        many => match many.iter().find(|row| row.can_send) {
            Some(row) => *row,
            None if many.iter().all(|row| row.status == many[0].status) => many[0],
            None => return (Known::Unknown(Unknown::Unreadable), at),
        },
    };
    (row_state(row), at)
}

fn row_state(row: &AccountTin) -> Known {
    let state = match &row.status {
        TinStatus::Approved if row.can_send => TinState::Active,
        TinStatus::Approved => TinState::NotAllowed,
        TinStatus::Unverified => TinState::NotVerified,
        TinStatus::Pending => TinState::WaitingForReview,
        TinStatus::NeedsFix => TinState::SentBack,
        TinStatus::Paused => TinState::Paused,
        _ => return Known::Unknown(Unknown::UnknownStatus),
    };
    let reasons = (!row.reasons.is_empty()).then(|| row.reasons.join(" "));
    Known::State(state, Facts { reasons, note: row.note.clone(), action_url: row.action_url.clone() }, row.can_send)
}

/// The wording table.
fn statement(state: TinState, facts: Facts) -> TinStatement {
    const KEPT: &str = " Your answers are kept.";
    let (chip, line, next, step, tone): (&'static str, &str, &str, Option<TinStep>, TinTone) = match state {
        TinState::Active => ("", "", "", None, TinTone::Info),
        TinState::NotOnAccount => (
            "Not on your Buwiz account",
            "This TIN is not on your Buwiz account.",
            "Add the TIN to your Buwiz account, then queue the return again.",
            Some(TinStep::Add),
            TinTone::Danger,
        ),
        TinState::NotVerified => (
            "Not verified",
            "This TIN is on your Buwiz account but not verified yet.",
            "Verify the TIN on your Buwiz account, then queue the return again.",
            Some(TinStep::Verify),
            TinTone::Warning,
        ),
        TinState::WaitingForReview => (
            "Waiting for review",
            "Buwiz is reviewing this TIN. You can send it once it is approved.",
            "Wait for Buwiz to finish the review, then queue the return again.",
            None,
            TinTone::Info,
        ),
        TinState::SentBack => (
            "Sent back",
            "Buwiz sent this TIN back for a fix.",
            "Fix what Buwiz asked for on your Buwiz account, then queue the return again.",
            Some(TinStep::Fix),
            TinTone::Danger,
        ),
        TinState::Paused => (
            "Paused",
            "Buwiz staff paused this TIN. It can't send until they turn it back on.",
            "Write to support@buwiz.com to have it turned back on, then queue the return again.",
            None,
            TinTone::Warning,
        ),
        TinState::AccountBanned => (
            "Account banned",
            "This Buwiz account is banned, so nothing can be sent.",
            "Write to support@buwiz.com about the account.",
            None,
            TinTone::Danger,
        ),
        TinState::NotAllowed => (
            "Can't send",
            "This Buwiz account is not allowed to send this TIN.",
            "Only a Buwiz account allowed to send this TIN can send the return.",
            None,
            TinTone::Warning,
        ),
    };
    let line = match (state, facts.reasons.as_deref().map(sanitize)) {
        (TinState::SentBack, Some(reasons)) if !reasons.is_empty() => format!("Buwiz sent this TIN back for a fix: {reasons}"),
        _ => line.to_owned(),
    };
    TinStatement {
        state,
        chip,
        line,
        next: format!("{next}{KEPT}"),
        step,
        tone,
        staff_note: (state == TinState::SentBack).then(|| facts.note.as_deref().map(sanitize)).flatten().filter(|note| !note.is_empty()),
        action_url: facts.action_url,
    }
}

/// Text from the server or staff, made fit to show: whitespace collapsed,
/// any run of 9 or more digits (dashes allowed) masked as `[TIN]`, at most
/// 200 characters, ending in a full stop.
fn sanitize(text: &str) -> String {
    const CAP: usize = 200;
    let words: Vec<String> = text.split_whitespace().map(mask_tins).collect();
    let text = words.join(" ");
    if text.is_empty() {
        return text;
    }
    if text.chars().count() > CAP {
        let cut: String = text.chars().take(CAP - 1).collect();
        return format!("{}\u{2026}", cut.trim_end());
    }
    if text.ends_with(['.', '!', '?']) { text } else { format!("{text}.") }
}

/// `word` with each run of digits and dashes that holds 9 or more digits
/// replaced by `[TIN]`.
fn mask_tins(word: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        let digits = run.chars().filter(char::is_ascii_digit).count();
        if digits >= 9 {
            let trailing = run.len() - run.trim_end_matches('-').len();
            out.push_str("[TIN]");
            out.push_str(&"-".repeat(trailing));
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in word.chars() {
        if c.is_ascii_digit() || (c == '-' && !run.is_empty()) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}
