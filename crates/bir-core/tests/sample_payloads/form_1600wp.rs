use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1600wp::{
    Form1600WpAgentCategory, Form1600WpDraft, Form1600WpScheduleRow,
};
use bir_core::forms::queueable::QueueableForm as _;

fn draft(month: u8, day: u8) -> Form1600WpDraft {
    let mut draft =
        Form1600WpDraft::new_from_profile(&dummy_profile("1600WP", "Corporation"), 2025, month);
    draft.day = day;
    draft
}

/// Government agent, both ATCs, an amended return and Schedule II rows.
fn sample_two_atcs() -> Form1600WpDraft {
    let mut draft = draft(6, 15);
    draft.is_amended = true;
    draft.set_taxes_withheld(true);
    draft.set_agent_category(Form1600WpAgentCategory::Government);
    draft.toggle_atc("WB191").unwrap();
    draft.toggle_atc("WB192").unwrap();
    draft.atc_rows[0].tax_base = 123_456.789;
    draft.atc_rows[1].tax_base = 50_000.015;
    draft.tax_remitted_previous = 1_000.0;
    draft.surcharge = 125.125;
    draft.interest = 10.0;
    draft.keep_entered_penalties();
    draft.schedule = vec![
        Form1600WpScheduleRow {
            tin: "987654321000".into(),
            payee_name: "Dela Cruz, Juan Sample".into(),
            atc_code: "WB191".into(),
            amount: 100_000.0,
            tax_withheld: 0.0,
        },
        Form1600WpScheduleRow {
            tin: "111222339000".into(),
            payee_name: "Sample Stables Inc".into(),
            atc_code: "WB192".into(),
            amount: 20_100.105,
            tax_withheld: 0.0,
        },
    ];
    draft.recompute();
    draft
}

/// Private agent, one ATC.
fn sample_one_atc() -> Form1600WpDraft {
    let mut draft = draft(7, 1);
    draft.set_taxes_withheld(true);
    draft.set_agent_category(Form1600WpAgentCategory::Private);
    draft.toggle_atc("WB194").unwrap();
    draft.atc_rows[0].tax_base = 7_777.775;
    draft.compromise = 1_000.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

/// No tax withheld.
fn sample_no_atc() -> Form1600WpDraft {
    let mut draft = draft(8, 31);
    draft.set_taxes_withheld(false);
    draft.set_agent_category(Form1600WpAgentCategory::Private);
    draft.recompute();
    draft
}

fn check(draft: Form1600WpDraft, stem: &str, layout: &str, sha: &str) {
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1600WP must validate: {errors:?}"));
    assert_eq!(draft.layout_id(), layout);
    check_sample(stem, layout, &payload, sha);
}

#[test]
fn form_1600wp_two_atc_sample_payload_is_current() {
    check(
        sample_two_atcs(),
        "1600WP-06152025",
        "1600wp-v2010-atc2",
        "360c19135e45008260922f62faf88b9053e4f08483f3cb20f09c9f56d1c27006",
    );
}

#[test]
fn form_1600wp_one_atc_sample_payload_is_current() {
    check(
        sample_one_atc(),
        "1600WP-07012025",
        "1600wp-v2010-atc1",
        "6b221aded987f55a89510528392d2b5ccc22afadad816591a670a067ac9713dd",
    );
}

#[test]
fn form_1600wp_no_atc_sample_payload_is_current() {
    check(
        sample_no_atc(),
        "1600WP-08312025",
        "1600wp-v2010-atc0",
        "ac0764b857e205d728a7177184badb6b0cd26c955599265fa8d5099740a9f1da",
    );
}

/// The calendar lists 1600-WP as event-based.
#[test]
fn form_1600wp_one_atc_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_one_atc,
        Deadline::None {
            assumed: day(2025, 12, 31),
        },
    );
}

/// D8c round 2, item 2: a queued return files the date it holds, typed or
/// not, so it is dated by that date; cancelling the queue makes it a Draft
/// again, and a Draft without the entered flag is undated.
#[test]
fn form_1600wp_a_cancelled_queue_is_undated_again() {
    let db = bir_core::db::Database::open_ephemeral().unwrap();
    let draft = sample_no_atc();
    assert!(!draft.lifecycle().trigger_date_entered);
    let id = db.save_queueable_draft(&draft).unwrap();
    assert_eq!(
        db.draft_event_trigger(id).unwrap(),
        None,
        "a Draft, not entered"
    );
    let mut queued = db
        .list_queueable_drafts::<Form1600WpDraft>(&draft.tin, 2025)
        .unwrap()
        .into_iter()
        .map(|(_, draft)| draft)
        .next()
        .unwrap();
    queued
        .queue(bir_core::filing_queue::QueueAuthSource::Gui)
        .expect("the sample queues");
    let id = db.save_queued_queueable(&queued).unwrap();
    assert_eq!(
        db.draft_event_trigger(id)
            .unwrap()
            .map(|trigger| trigger.date),
        chrono::NaiveDate::from_ymd_opt(2025, 8, 31),
        "queued: dated by the date it files"
    );
    let reverted = db.cancel_queued_queueable(&queued).unwrap();
    assert_eq!(
        reverted.lifecycle().status,
        bir_core::forms::FilingStatus::Draft
    );
    assert_eq!(
        db.draft_event_trigger(id).unwrap(),
        None,
        "cancelled: a Draft again, undated"
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_two_atcs()).expect("sample serializes"),
        serde_json::to_value(sample_one_atc()).expect("sample serializes"),
        serde_json::to_value(sample_no_atc()).expect("sample serializes"),
    ]
}
