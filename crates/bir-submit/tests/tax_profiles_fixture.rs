//! The golden `include=all` answer both repos parse
//! (`tests/fixtures/tax_profiles_all.json`, identical in the server's
//! repo): every state, the complete-list marker, and a needs-fix row's
//! reasons, note and id-based link.

use bir_core::forms::taxform::TinKey;
use bir_submit::{AccountTins, TinStatus};

#[test]
fn the_golden_include_all_answer_parses() {
    let body: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/tax_profiles_all.json"))
            .expect("the fixture is JSON");
    let list = AccountTins::parse(&body, 1_767_225_600);
    assert!(list.complete, "the server marked the list complete");
    assert!(!list.stale);
    assert_eq!(list.fetched_at, 1_767_225_600);

    let row = |tin: &str| {
        list.find(&TinKey::parse(tin).unwrap())
            .unwrap_or_else(|| panic!("{tin} listed"))
    };
    let states: Vec<(TinStatus, bool)> = list
        .rows
        .iter()
        .map(|row| (row.status.clone(), row.can_send))
        .collect();
    assert_eq!(
        states,
        [
            (TinStatus::Approved, true),
            (TinStatus::Paused, false),
            (TinStatus::Unverified, false),
            (TinStatus::Pending, false),
            (TinStatus::NeedsFix, false),
        ]
    );

    // A head office given as its 9-digit root finds the 14-digit row.
    let active = row("000123453");
    assert_eq!(active.registered_name, "SYNTHETIC TAXPAYER ONE");
    assert!(active.action_url.is_none());

    let unverified = row("000-123-455");
    assert_eq!(
        unverified.action_url.as_deref(),
        Some("https://account.buwiz.com/tins/claims/33333333-3333-4333-8333-333333333333/verify")
    );

    let fix = row("000-123-457-00000");
    assert_eq!(fix.reasons, ["A photo is too blurry to read."]);
    assert_eq!(fix.note.as_deref(), Some("Retake it in daylight."));
    assert_eq!(
        fix.action_url.as_deref(),
        Some("https://account.buwiz.com/tins/claims/55555555-5555-4555-8555-555555555555/fix")
    );
    assert!(row("000123454").reasons.is_empty());
    assert!(list.find(&TinKey::parse("000123459").unwrap()).is_none());
}
