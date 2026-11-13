//! What the apps say about a TIN's state on the Buwiz account, and the
//! Queue gate over the account's list. The table below is the only copy of
//! the wording outside `src/tin_state.rs`.

use bir_app_core::bir_submit::{AccountTins, TinsError, TinsRead};
use bir_app_core::{ACCOUNT_SITE, BirApp, QueueGate, SendStage, TinKey, TinRefusal, TinState, TinStatement, TinStep, TinTone, Unknown};
use serde_json::{Value, json};

const NOW: i64 = 1_767_225_600;
const HEAD: &str = "000-123-453-00000";

/// A list read at `NOW` with `rows`, complete or not.
fn list(complete: bool, rows: Value) -> TinsRead {
    let mut body = json!({ "profiles": rows });
    if complete {
        body["include"] = "all".into();
    }
    Ok(AccountTins::parse(&body, NOW))
}

fn row(tin: &str, status: &str) -> Value {
    json!({"tin": tin, "registered_name": "SYNTHETIC TAXPAYER ONE", "status": status, "can_send": status == "approved"})
}

fn gate(tin: &str, read: TinsRead) -> QueueGate {
    BirApp::tin_gate(tin, &[read], NOW)
}

/// (state, chip, line, next, step, tone): one row of the wording table.
type Row = (TinState, &'static str, &'static str, &'static str, Option<TinStep>, TinTone);

fn table() -> Vec<Row> {
    vec![
        (
            TinState::NotOnAccount,
            "Not on your Buwiz account",
            "This TIN is not on your Buwiz account.",
            "Add the TIN to your Buwiz account, then queue the return again. Your answers are kept.",
            Some(TinStep::Add),
            TinTone::Danger,
        ),
        (
            TinState::NotVerified,
            "Not verified",
            "This TIN is on your Buwiz account but not verified yet.",
            "Verify the TIN on your Buwiz account, then queue the return again. Your answers are kept.",
            Some(TinStep::Verify),
            TinTone::Warning,
        ),
        (
            TinState::WaitingForReview,
            "Waiting for review",
            "Buwiz is reviewing this TIN. You can send it once it is approved.",
            "Wait for Buwiz to finish the review, then queue the return again. Your answers are kept.",
            None,
            TinTone::Info,
        ),
        (
            TinState::SentBack,
            "Sent back",
            "Buwiz sent this TIN back for a fix.",
            "Fix what Buwiz asked for on your Buwiz account, then queue the return again. Your answers are kept.",
            Some(TinStep::Fix),
            TinTone::Danger,
        ),
        (
            TinState::Paused,
            "Paused",
            "Buwiz staff paused this TIN. It can't send until they turn it back on.",
            "Write to support@buwiz.com to have it turned back on, then queue the return again. Your answers are kept.",
            None,
            TinTone::Warning,
        ),
        (
            TinState::AccountBanned,
            "Account banned",
            "This Buwiz account is banned, so nothing can be sent.",
            "Write to support@buwiz.com about the account. Your answers are kept.",
            None,
            TinTone::Danger,
        ),
        (
            TinState::NotAllowed,
            "Can't send",
            "This Buwiz account is not allowed to send this TIN.",
            "Only a Buwiz account allowed to send this TIN can send the return. Your answers are kept.",
            None,
            TinTone::Warning,
        ),
    ]
}

fn assert_row(statement: &TinStatement, want: &Row) {
    let (state, chip, line, next, step, tone) = want;
    assert_eq!(statement.state, *state);
    assert_eq!(statement.chip, *chip, "{state:?}");
    assert_eq!(statement.line, *line, "{state:?}");
    assert_eq!(statement.next, *next, "{state:?}");
    assert_eq!(statement.step, *step, "{state:?}");
    assert_eq!(statement.tone, *tone, "{state:?}");
}

#[test]
fn every_state_has_its_exact_line_chip_and_button() {
    let table = table();
    // From a stored or live refusal code.
    let codes = [
        (TinRefusal::NotActive, 0),
        (TinRefusal::Unverified, 1),
        (TinRefusal::Pending, 2),
        (TinRefusal::NeedsFix, 3),
        (TinRefusal::Paused, 4),
        (TinRefusal::AccountBanned, 5),
    ];
    for (code, at) in codes {
        assert_row(&BirApp::tin_statement_for_refusal(code, None), &table[at]);
    }
    // From the account's list.
    let live = [
        ("unverified", 1),
        ("pending", 2),
        ("needs_fix", 3),
        ("paused", 4),
    ];
    for (status, at) in live {
        let gate = gate(HEAD, list(true, json!([row(HEAD, status)])));
        assert!(!gate.can_queue, "{status}");
        assert_row(gate.statement.as_ref().expect(status), &table[at]);
    }
    let missing = gate(HEAD, list(true, json!([])));
    assert_row(missing.statement.as_ref().unwrap(), &table[0]);
    let banned = gate(HEAD, Err(TinsError::Banned));
    assert_row(banned.statement.as_ref().unwrap(), &table[5]);
    let not_allowed = json!([{"tin": HEAD, "registered_name": "X", "status": "approved", "can_send": false}]);
    let not_allowed = gate(HEAD, list(true, not_allowed));
    assert_row(not_allowed.statement.as_ref().unwrap(), &table[6]);
    // Active is quiet.
    let active = gate(HEAD, list(true, json!([row(HEAD, "approved")])));
    assert!(active.can_queue);
    assert_eq!(active.statement, None);
    assert_eq!(active.unknown, None);
    assert_eq!(active.checked_at, Some(NOW));

    let labels = [(TinStep::Add, "Add it", "add"), (TinStep::Verify, "Verify it", "verify"), (TinStep::Fix, "Fix it", "fix")];
    for (step, label, kind) in labels {
        assert_eq!((step.label(), step.kind()), (label, kind));
    }
    assert_eq!(BirApp::DRAFT_NOTE, "Back to a draft. Nothing was sent.");
}

#[test]
fn a_sent_back_line_names_the_fixed_reasons_and_the_note_stays_apart() {
    let stored = "Not sent (nothing reached BIR): Sent back for a fix: A photo is too blurry to read. (tin_needs_fix)";
    let statement = BirApp::tin_statement_for_refusal(TinRefusal::NeedsFix, Some(stored));
    assert_eq!(statement.line, "Buwiz sent this TIN back for a fix: A photo is too blurry to read.");
    assert_eq!(statement.staff_note, None, "a stored reason never carries the note");

    let fixed = json!([{
        "tin": HEAD, "registered_name": "X", "status": "needs_fix", "can_send": false,
        "reasons": ["A photo is too blurry to read.", "The name does not match."],
        "note": "JUANA DELA CRUZ, retake it in daylight.",
        "action": {"url": "https://account.buwiz.com/tins/claims/c5/fix"}
    }]);
    let gate = gate(HEAD, list(true, fixed));
    let statement = gate.statement.unwrap();
    assert_eq!(statement.line, "Buwiz sent this TIN back for a fix: A photo is too blurry to read. The name does not match.");
    assert_eq!(statement.staff_note.as_deref(), Some("JUANA DELA CRUZ, retake it in daylight."));
    for shown in [&statement.chip.to_owned(), &statement.line, &statement.next] {
        assert!(!shown.contains("JUANA"), "the note reached {shown}");
    }
    assert_eq!(statement.link(&TinKey::parse(HEAD).unwrap()).as_deref(), Some("https://account.buwiz.com/tins/claims/c5/fix"));
}

#[test]
fn sent_back_reason_is_sanitized() {
    let long = "x ".repeat(150);
    let stored = format!("Not sent (nothing reached BIR): Sent back for a fix: TIN 000-123-453 and   0001234530000\n differ. {long} (tin_needs_fix)");
    let line = BirApp::tin_statement_for_refusal(TinRefusal::NeedsFix, Some(&stored)).line;
    assert!(line.starts_with("Buwiz sent this TIN back for a fix: TIN [TIN] and [TIN] differ. x x"), "{line}");
    assert!(!line.contains("123"), "{line}");
    let note = json!([{"tin": HEAD, "registered_name": "X", "status": "needs_fix", "can_send": false, "note": format!("see   000123453 {}", "y".repeat(300))}]);
    let note = gate(HEAD, list(true, note)).statement.unwrap().staff_note.unwrap();
    assert!(note.starts_with("see [TIN] yyy"), "{note}");
    assert_eq!(note.chars().count(), 200);
    let short = json!([{"tin": HEAD, "registered_name": "X", "status": "needs_fix", "can_send": false, "note": "Retake it"}]);
    assert_eq!(gate(HEAD, list(true, short)).statement.unwrap().staff_note.as_deref(), Some("Retake it."));
}

#[test]
fn gate_blocks_each_non_active_state_and_allows_active() {
    for status in ["unverified", "pending", "needs_fix", "paused"] {
        let gate = gate(HEAD, list(false, json!([row(HEAD, status)])));
        assert!(!gate.can_queue, "{status} blocks, even on a list without the marker");
        assert_eq!(gate.unknown, None);
    }
    assert!(gate(HEAD, list(false, json!([row(HEAD, "approved")]))).can_queue);
}

#[test]
fn gate_fails_open_when_the_list_is_incomplete_or_unreadable() {
    let cases: [(TinsRead, Unknown); 5] = [
        (list(false, json!([row("000-123-454-00000", "approved")])), Unknown::Incomplete),
        (Err(TinsError::Offline), Unknown::Offline),
        (Err(TinsError::SignedOut), Unknown::SignedOut),
        (Err(TinsError::Server { code: "error".into(), message: "x".into() }), Unknown::Unreadable),
        (list(true, json!([row(HEAD, "frozen")])), Unknown::UnknownStatus),
    ];
    for (read, why) in cases {
        let gate = gate(HEAD, read);
        assert!(gate.can_queue, "{why:?}");
        assert_eq!(gate.statement, None, "{why:?}");
        assert_eq!(gate.unknown, Some(why));
    }
    // A list too old to trust, and no list at all.
    let mut old = AccountTins::parse(&json!({"include": "all", "profiles": [row(HEAD, "paused")]}), NOW - 601);
    old.stale = true;
    let stale = BirApp::tin_gate(HEAD, &[Ok(old.clone())], NOW);
    assert!(stale.can_queue);
    assert_eq!(stale.unknown, Some(Unknown::Stale));
    old.fetched_at = NOW - 599;
    assert!(!BirApp::tin_gate(HEAD, &[Ok(old)], NOW).can_queue, "a stale list under the limit still counts");
    let none = BirApp::tin_gate(HEAD, &[], NOW);
    assert!(none.can_queue);
    assert_eq!(none.unknown, Some(Unknown::SignedOut));
    let bad_tin = gate("12345", list(true, json!([])));
    assert!(bad_tin.can_queue);
    assert_eq!(bad_tin.unknown, Some(Unknown::Unreadable));
}

/// A list whose read time is ahead of now beyond a small skew (the clock
/// was set back since) is of no known age: stale, so it never blocks.
#[test]
fn a_list_read_ahead_of_now_is_stale() {
    let paused = |at: i64| -> TinsRead { Ok(AccountTins::parse(&json!({"include": "all", "profiles": [row(HEAD, "paused")]}), at)) };
    assert!(!BirApp::tin_gate(HEAD, &[paused(NOW + 30)], NOW).can_queue, "inside the skew it still counts");
    for ahead in [NOW + 120, NOW + 86_400] {
        let gate = BirApp::tin_gate(HEAD, &[paused(ahead)], NOW);
        assert!(gate.can_queue, "{ahead}");
        assert_eq!(gate.unknown, Some(Unknown::Stale));
        assert_eq!(gate.statement, None);
    }
}

/// The server's `can_send` decides: true never blocks, whatever the status
/// says (the state is then shown as information only); false on an
/// approved row blocks.
#[test]
fn can_send_decides_whatever_the_status_says() {
    let rows = [
        ("pending", TinState::WaitingForReview),
        ("unverified", TinState::NotVerified),
        ("needs_fix", TinState::SentBack),
        ("paused", TinState::Paused),
    ];
    for (status, state) in rows {
        let sends = json!([{"tin": HEAD, "registered_name": "X", "status": status, "can_send": true}]);
        let gate = gate(HEAD, list(true, sends));
        assert!(gate.can_queue, "{status} with can_send true");
        assert_eq!(gate.unknown, None, "{status}");
        assert_eq!(gate.statement.map(|s| s.state), Some(state), "{status}");
    }
    let other = json!([{"tin": HEAD, "registered_name": "X", "status": "frozen", "can_send": true}]);
    assert!(gate(HEAD, list(true, other)).can_queue);
    let held = json!([{"tin": HEAD, "registered_name": "X", "status": "approved", "can_send": false}]);
    let held = gate(HEAD, list(true, held));
    assert!(!held.can_queue);
    assert_eq!(held.statement.map(|s| s.state), Some(TinState::NotAllowed));
}

/// Two rows for one TIN: the one that can send wins; rows that disagree
/// otherwise make the TIN unknown, which never blocks.
#[test]
fn two_rows_for_one_tin_prefer_the_one_that_can_send() {
    for rows in [json!([row(HEAD, "pending"), row(HEAD, "approved")]), json!([row(HEAD, "approved"), row(HEAD, "pending")])] {
        let gate = gate(HEAD, list(true, rows.clone()));
        assert!(gate.can_queue, "{rows}");
        assert_eq!(gate.statement, None, "{rows}");
        assert_eq!(gate.unknown, None, "{rows}");
    }
    let conflict = gate(HEAD, list(true, json!([row(HEAD, "pending"), row("000123453", "paused")])));
    assert!(conflict.can_queue);
    assert_eq!(conflict.statement, None);
    assert_eq!(conflict.unknown, Some(Unknown::Unreadable));
    let agree = gate(HEAD, list(true, json!([row(HEAD, "paused"), row(HEAD, "paused")])));
    assert!(!agree.can_queue);
    assert_eq!(agree.statement.map(|s| s.state), Some(TinState::Paused));
}

/// Today's rule for reads of several sign-ins (a later phase gates on the
/// login that will send): a read that can send lets Queue through; then a
/// read that cannot tell; only when every read knows the TIN cannot send
/// does Queue close, with the first read's state.
#[test]
fn several_reads_follow_todays_rule() {
    let active = list(true, json!([row(HEAD, "approved")]));
    let paused = list(true, json!([row(HEAD, "paused")]));
    let pending = list(true, json!([row(HEAD, "pending")]));
    for reads in [vec![paused.clone(), active.clone()], vec![Err(TinsError::Banned), active.clone()], vec![active.clone(), Err(TinsError::Offline)]] {
        let gate = BirApp::tin_gate(HEAD, &reads, NOW);
        assert!(gate.can_queue);
        assert_eq!((gate.statement, gate.unknown), (None, None));
    }
    let unsure = BirApp::tin_gate(HEAD, &[paused.clone(), Err(TinsError::Offline)], NOW);
    assert!(unsure.can_queue);
    assert_eq!(unsure.unknown, Some(Unknown::Offline));
    let closed = BirApp::tin_gate(HEAD, &[pending, paused], NOW);
    assert!(!closed.can_queue);
    assert_eq!(closed.statement.map(|s| s.state), Some(TinState::WaitingForReview));
}

#[test]
fn a_missing_tin_in_a_complete_list_is_not_on_account() {
    let gate = gate(HEAD, list(true, json!([row("000-123-453-00002", "approved")])));
    assert!(!gate.can_queue);
    assert_eq!(gate.statement.unwrap().state, TinState::NotOnAccount);
}

#[test]
fn the_gate_compares_one_key_for_9_12_and_14_digit_tins() {
    let rows = json!([row("000-123-453-00000", "approved"), row("000123453002", "paused")]);
    for head in ["000123453", "000-123-453", "000123453000", "00012345300000", "000-123-453-00000"] {
        let gate = gate(head, list(true, rows.clone()));
        assert!(gate.can_queue, "{head}");
        assert_eq!(gate.unknown, None, "{head} is found, not unknown");
    }
    for branch in ["000-123-453-002", "00012345300002", "000-123-453-00002"] {
        let gate = gate(branch, list(true, rows.clone()));
        assert_eq!(gate.statement.map(|s| s.state), Some(TinState::Paused), "{branch}");
    }
    // A head office given as its root against a 14-digit row of a branch.
    let only_branch = json!([row("000-123-453-00002", "approved")]);
    assert_eq!(gate("000123453", list(true, only_branch)).statement.map(|s| s.state), Some(TinState::NotOnAccount));
}

#[test]
fn live_state_wins_over_a_stored_refusal() {
    let stored = BirApp::tin_statement_for_refusal(TinRefusal::NotActive, None);
    let live = gate(HEAD, list(false, json!([row(HEAD, "pending")])));
    assert_eq!(BirApp::effective_statement(Some(stored), &live).map(|s| s.state), Some(TinState::WaitingForReview));
}

#[test]
fn a_stored_refusal_stands_when_the_list_is_stale_or_missing() {
    let stored = BirApp::tin_statement_for_refusal(TinRefusal::Unverified, None);
    for read in [Err(TinsError::Offline), list(false, json!([])), Err(TinsError::SignedOut)] {
        let live = gate(HEAD, read);
        assert_eq!(BirApp::effective_statement(Some(stored.clone()), &live), Some(stored.clone()));
    }
    let live = BirApp::tin_gate(HEAD, &[], NOW);
    assert_eq!(BirApp::effective_statement(Some(stored.clone()), &live), Some(stored));
}

#[test]
fn a_stored_refusal_is_hidden_when_the_tin_is_now_active() {
    let stored = BirApp::tin_statement_for_refusal(TinRefusal::Paused, None);
    let live = gate(HEAD, list(true, json!([row(HEAD, "approved")])));
    assert_eq!(BirApp::effective_statement(Some(stored), &live), None);
}

/// A list read before the stored refusal says nothing about it: the
/// refusal stands. A list read after it decides, as before.
#[test]
fn a_list_read_before_the_refusal_does_not_hide_it() {
    let stored = BirApp::tin_statement_for_refusal(TinRefusal::Paused, None);
    let active = gate(HEAD, list(true, json!([row(HEAD, "approved")])));
    let pending = gate(HEAD, list(true, json!([row(HEAD, "pending")])));
    for refused_at in [NOW, NOW + 1, NOW + 3600] {
        assert_eq!(BirApp::effective_statement_since(Some(stored.clone()), Some(refused_at), &active), Some(stored.clone()), "{refused_at}");
        assert_eq!(BirApp::effective_statement_since(Some(stored.clone()), Some(refused_at), &pending), Some(stored.clone()), "{refused_at}");
    }
    assert_eq!(BirApp::effective_statement_since(Some(stored.clone()), Some(NOW - 1), &active), None);
    assert_eq!(BirApp::effective_statement_since(Some(stored.clone()), Some(NOW - 1), &pending).map(|s| s.state), Some(TinState::WaitingForReview));
    // A time not known: the live state decides, as `effective_statement`.
    assert_eq!(BirApp::effective_statement_since(Some(stored.clone()), None, &active), None);
    assert_eq!(BirApp::effective_statement(Some(stored.clone()), &active), None);
    // A ban read now carries no read time, and still shows.
    let banned = gate(HEAD, Err(TinsError::Banned));
    assert_eq!(BirApp::effective_statement_since(Some(stored), Some(NOW + 3600), &banned).map(|s| s.state), Some(TinState::AccountBanned));
}

#[test]
fn link_for_add_carries_root_and_branch() {
    let key = TinKey::parse("000-123-453-002").unwrap();
    assert_eq!(TinStep::Add.link(&key, None), "https://account.buwiz.com/tins/new?tin=000123453&branch=00002");
    assert_eq!(TinStep::Verify.link(&key, None), "https://account.buwiz.com/tins");
    assert_eq!(TinStep::Fix.link(&key, Some("https://account.buwiz.com/tins/claims/c1/fix")), "https://account.buwiz.com/tins/claims/c1/fix");
    // Only "Add it" carries the TIN.
    assert!(!TinStep::Verify.link(&key, Some("https://account.buwiz.com/tins/claims/c1/verify")).contains("123"));
}

/// A Verify or Fix link that is not a page of the account site opens the
/// account's TIN list instead; "Add it" is built on the same site.
#[test]
fn a_step_link_off_the_account_site_falls_back_to_the_tin_list() {
    let key = TinKey::parse(HEAD).unwrap();
    let tins = format!("{ACCOUNT_SITE}/tins");
    for url in [
        "https://account.buwiz.com.evil.example/tins/claims/c1/fix",
        "https://account.buwiz.com@evil.example/tins/claims/c1/fix",
        "https://evil.example/tins/claims/c1/fix",
        "http://account.buwiz.com/tins/claims/c1/fix",
        "https://user@account.buwiz.com/tins/claims/c1/fix",
        "https://account.buwiz.com:8443/tins/claims/c1/fix",
    ] {
        for step in [TinStep::Verify, TinStep::Fix] {
            assert_eq!(step.link(&key, Some(url)), tins, "{url}");
        }
        let statement = BirApp::tin_statement_for_refusal(TinRefusal::NeedsFix, None);
        let statement = TinStatement { action_url: Some(url.to_owned()), ..statement };
        assert_eq!(statement.link(&key).as_deref(), Some(tins.as_str()), "{url}");
    }
    assert!(TinStep::Add.link(&key, None).starts_with(&format!("{ACCOUNT_SITE}/tins/new?")));
    assert_eq!(ACCOUNT_SITE, bir_app_core::bir_submit::tins::ACCOUNT_SITE);
}

#[test]
fn stages_read_their_statement_and_a_maybe_sent_never_does() {
    let reason = "Not sent (nothing reached BIR): any wording (tin_paused)".to_owned();
    let refused = SendStage::Refused { code: TinRefusal::Paused, reason: reason.clone() };
    assert_eq!(BirApp::tin_statement_for_stage(&refused).map(|s| s.state), Some(TinState::Paused));
    let legacy = SendStage::Failed { reason: reason.clone(), attempts: 1, next_retry_at: None };
    assert_eq!(BirApp::tin_statement_for_stage(&legacy).map(|s| s.state), Some(TinState::Paused));
    assert_eq!(BirApp::tin_statement_for_stage(&SendStage::NeedsAttention { reason }), None);
    let other = SendStage::Failed { reason: "Not sent (nothing reached BIR): the relay closed early".into(), attempts: 1, next_retry_at: None };
    assert_eq!(BirApp::tin_statement_for_stage(&other), None);
}
