//! A mini-app reads what a late return costs through `BirApp`, with no
//! database: `None` on or before the due date, then the days late and each
//! part of the penalty.

use bir_app_core::{BirApp, PenaltyEstimate};
use chrono::NaiveDate;

#[test]
fn a_late_return_costs_surcharge_interest_and_compromise() {
    let due = NaiveDate::from_ymd_opt(2026, 4, 15).unwrap();
    assert_eq!(BirApp::penalty_estimate("1701Q", due, due, 10_000.0), None);

    let today = NaiveDate::from_ymd_opt(2026, 5, 15).unwrap();
    let PenaltyEstimate { days_late, surcharge, interest, compromise } =
        BirApp::penalty_estimate("1701Q", due, today, 10_000.0).expect("late");
    assert_eq!(days_late, 30);
    assert!(surcharge > 0.0 && interest > 0.0 && compromise > 0.0);
    let estimate = PenaltyEstimate { days_late, surcharge, interest, compromise };
    assert_eq!(estimate.total(), surcharge + interest + compromise);

    let nothing_due = BirApp::penalty_estimate("1601C", due, today, 0.0).expect("late");
    assert!(nothing_due.total() > 0.0, "a late return never reads as free");
}
