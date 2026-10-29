//! RFC 5545 iCalendar output for one taxpayer profile's filing deadlines.
//!
//! It renders the [`DesiredCalendarEvent`] set that
//! `calendar_events::build_desired_events` builds, the single source of truth
//! for which obligations count as a deadline. Every platform can open an
//! `.ics`.

use std::path::{Path, PathBuf};

use crate::calendar_events::DesiredCalendarEvent;
use crate::profile::TaxpayerProfile;

/// RFC 5545 caps a content line at 75 octets, continuing with CRLF plus one
/// leading space. Long BIR form descriptions exceed that routinely.
const MAX_LINE_OCTETS: usize = 75;

/// Renders a complete VCALENDAR for `profile`.
///
/// `stamp` is the `DTSTAMP` value in iCalendar UTC form (`YYYYMMDDTHHMMSSZ`).
/// It is a parameter rather than read from the clock so the output is
/// deterministic and testable.
pub fn profile_calendar_ics(
    profile: &TaxpayerProfile,
    events: &[DesiredCalendarEvent],
    stamp: &str,
) -> String {
    let mut out = String::new();
    push_line(&mut out, "BEGIN:VCALENDAR");
    push_line(&mut out, "VERSION:2.0");
    push_line(&mut out, "PRODID:-//Goldcoders//eBIRForms//EN");
    push_line(&mut out, "CALSCALE:GREGORIAN");
    // Publish, not request: importers must not email invitations to anyone.
    push_line(&mut out, "METHOD:PUBLISH");
    push_line(
        &mut out,
        &format!("X-WR-CALNAME:{}", escape_text(&calendar_name(profile))),
    );

    for event in events {
        // An event with no resolved date cannot be represented as a VEVENT.
        // `build_desired_events` counts these as `excluded_undated`; skip
        // them rather than emitting an invalid entry.
        let (Some(start), Some(end)) = (event_date(event, "start"), event_date(event, "end"))
        else {
            continue;
        };

        push_line(&mut out, "BEGIN:VEVENT");
        push_line(
            &mut out,
            &format!("UID:{}", escape_text(&event_uid(profile, event))),
        );
        push_line(&mut out, &format!("DTSTAMP:{stamp}"));
        // All-day events use VALUE=DATE, and DTEND is exclusive. The event
        // bodies already carry an end one day after the deadline, which is
        // exactly the exclusive form, so the dates pass through unchanged.
        push_line(&mut out, &format!("DTSTART;VALUE=DATE:{}", compact(&start)));
        push_line(&mut out, &format!("DTEND;VALUE=DATE:{}", compact(&end)));
        push_line(
            &mut out,
            &format!("SUMMARY:{}", escape_text(&event_text(event, "summary"))),
        );
        let description = event_text(event, "description");
        if !description.is_empty() {
            push_line(
                &mut out,
                &format!("DESCRIPTION:{}", escape_text(&description)),
            );
        }
        // A filing deadline should not make the user look busy.
        push_line(&mut out, "TRANSP:TRANSPARENT");
        push_line(&mut out, "END:VEVENT");
    }

    push_line(&mut out, "END:VCALENDAR");
    out
}

/// The file name offered to the OS calendar app.
pub fn ics_file_name(profile: &TaxpayerProfile) -> String {
    format!("BIR_Deadlines_{}.ics", profile.tin.full())
}

/// Writes the calendar beside the application's own data and returns its path,
/// ready to hand to the platform's default calendar application.
pub fn write_profile_calendar_ics(
    directory: &Path,
    profile: &TaxpayerProfile,
    events: &[DesiredCalendarEvent],
    stamp: &str,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let path = directory.join(ics_file_name(profile));
    std::fs::write(&path, profile_calendar_ics(profile, events, stamp))?;
    Ok(path)
}

fn calendar_name(profile: &TaxpayerProfile) -> String {
    format!("BIR Deadlines - {}", profile.full_name)
}

/// A UID must be stable across exports so a re-import updates the existing entry
/// instead of duplicating it - and it must be unique across *profiles*.
///
/// `obligation_key` is only `{year}:{form}:{period}`; it carries no taxpayer
/// identity, so two profiles filing the same form for the same period would
/// otherwise emit byte-identical UIDs and the second import would overwrite the
/// first taxpayer's deadlines inside the calendar. The profile's TIN hash scopes
/// it without writing a raw TIN into a file that may be shared.
fn event_uid(profile: &TaxpayerProfile, event: &DesiredCalendarEvent) -> String {
    format!(
        "{}-{}@ebirforms.goldcoders.dev",
        crate::calendar_events::profile_key(&profile.tin.full()),
        event.obligation_key
    )
}

fn event_text(event: &DesiredCalendarEvent, key: &str) -> String {
    event
        .body
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

fn event_date(event: &DesiredCalendarEvent, key: &str) -> Option<String> {
    event
        .body
        .get(key)?
        .get("date")?
        .as_str()
        .map(str::to_string)
}

/// `2026-07-27` becomes `20260727`.
fn compact(date: &str) -> String {
    date.chars().filter(|c| *c != '-').collect()
}

/// RFC 5545 escaping for TEXT values. A raw comma or semicolon would end the
/// value early and a raw newline would end the line, so an unescaped BIR
/// description could silently truncate every field after it.
fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(character),
        }
    }
    out
}

/// Appends one content line, folded to the 75-octet limit with CRLF endings.
///
/// Folding counts octets, not characters, and must not split a UTF-8
/// sequence - a continuation byte on its own is invalid UTF-8 and importers
/// reject the file.
fn push_line(out: &mut String, line: &str) {
    let bytes = line.len();
    if bytes <= MAX_LINE_OCTETS {
        out.push_str(line);
        out.push_str("\r\n");
        return;
    }

    let mut remaining = line;
    let mut budget = MAX_LINE_OCTETS;
    loop {
        if remaining.len() <= budget {
            out.push_str(remaining);
            out.push_str("\r\n");
            return;
        }
        // Walk back to a character boundary so the split stays valid UTF-8.
        let mut cut = budget;
        while cut > 0 && !remaining.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == 0 {
            // One character alone exceeds the budget; emit it whole rather
            // than looping forever or slicing it apart.
            cut = remaining
                .char_indices()
                .nth(1)
                .map(|(index, _)| index)
                .unwrap_or(remaining.len());
        }
        out.push_str(&remaining[..cut]);
        out.push_str("\r\n ");
        remaining = &remaining[cut..];
        // Continuation lines carry a leading space that counts toward the limit.
        budget = MAX_LINE_OCTETS - 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(summary: &str, description: &str, start: &str, end: &str) -> DesiredCalendarEvent {
        DesiredCalendarEvent {
            obligation_key: "2026:2551Q:Q2".to_string(),
            taxable_year: 2026,
            form_code: "2551Q".to_string(),
            period_label: "Q2 2026".to_string(),
            content_hash: "hash".to_string(),
            body: json!({
                "summary": summary,
                "description": description,
                "start": {"date": start},
                "end": {"date": end},
            }),
        }
    }

    fn profile() -> TaxpayerProfile {
        serde_json::from_value(json!({
            "id": null,
            "full_name": "Juan Dela Cruz",
            "tin": {"segment1": "000", "segment2": "000", "segment3": "000", "branch": "00000"},
            "rdo_code": "018",
            "line_of_business": "Services",
            "registered_address": "Manila",
            "zip_code": "1000",
            "phone": "09123456789",
            "email": "a@b.com",
            "default_form_type": "2551Qv2018",
            "taxpayer_type": "Individual"
        }))
        .unwrap()
    }

    fn unfold(ics: &str) -> String {
        ics.replace("\r\n ", "")
    }

    #[test]
    fn all_day_events_use_value_date_and_a_compact_form() {
        let ics = profile_calendar_ics(
            &profile(),
            &[event("[BIR] 2551Q", "", "2026-07-27", "2026-07-28")],
            "20260729T000000Z",
        );
        assert!(ics.contains("DTSTART;VALUE=DATE:20260727"), "{ics}");
        assert!(ics.contains("DTEND;VALUE=DATE:20260728"), "{ics}");
    }

    /// A raw comma or semicolon ends an iCalendar value early, so an unescaped
    /// description would truncate the fields after it.
    #[test]
    fn commas_semicolons_and_newlines_are_escaped() {
        let ics = profile_calendar_ics(
            &profile(),
            &[event(
                "A,B;C",
                "line one\nline two",
                "2026-07-27",
                "2026-07-28",
            )],
            "20260729T000000Z",
        );
        let flat = unfold(&ics);
        assert!(flat.contains("SUMMARY:A\\,B\\;C"), "{flat}");
        assert!(flat.contains("DESCRIPTION:line one\\nline two"), "{flat}");
    }

    #[test]
    fn backslashes_are_escaped_before_anything_else() {
        let ics = profile_calendar_ics(
            &profile(),
            &[event("path\\to", "", "2026-07-27", "2026-07-28")],
            "20260729T000000Z",
        );
        assert!(unfold(&ics).contains("SUMMARY:path\\\\to"));
    }

    #[test]
    fn no_content_line_exceeds_seventy_five_octets() {
        let ics = profile_calendar_ics(
            &profile(),
            &[event(
                &"Quarterly Percentage Tax Return for a very long taxpayer name ".repeat(4),
                &"Filing guidance that runs well past the folding limit. ".repeat(4),
                "2026-07-27",
                "2026-07-28",
            )],
            "20260729T000000Z",
        );
        for line in ics.split("\r\n") {
            assert!(
                line.len() <= MAX_LINE_OCTETS,
                "line of {} octets: {line}",
                line.len()
            );
        }
    }

    /// Folding counts octets. Splitting mid-sequence yields invalid UTF-8 and
    /// importers reject the whole file.
    #[test]
    fn folding_never_splits_a_multibyte_character() {
        let ics = profile_calendar_ics(
            &profile(),
            &[event(&"ñ".repeat(80), "", "2026-07-27", "2026-07-28")],
            "20260729T000000Z",
        );
        // Round-tripping through bytes proves no sequence was cut apart.
        assert_eq!(String::from_utf8(ics.clone().into_bytes()).unwrap(), ics);
        assert_eq!(unfold(&ics).matches('ñ').count(), 80);
    }

    #[test]
    fn folding_restores_the_full_value_when_unfolded() {
        let summary = "Withholding Tax Remittance Return for the taxable period ending December";
        let ics = profile_calendar_ics(
            &profile(),
            &[event(summary, "", "2026-07-27", "2026-07-28")],
            "20260729T000000Z",
        );
        assert!(unfold(&ics).contains(&format!("SUMMARY:{summary}")));
    }

    /// A re-import must update the existing entry, not add a second one.
    #[test]
    fn the_uid_is_stable_across_exports_of_the_same_profile() {
        let events = [event("x", "", "2026-07-27", "2026-07-28")];
        let first = profile_calendar_ics(&profile(), &events, "20260729T000000Z");
        // A later export, different DTSTAMP: the UID must not move.
        let second = profile_calendar_ics(&profile(), &events, "20270101T120000Z");
        let uid_of = |ics: &str| {
            unfold(ics)
                .lines()
                .find(|l| l.starts_with("UID:"))
                .unwrap()
                .to_string()
        };
        assert_eq!(uid_of(&first), uid_of(&second));
        assert!(uid_of(&first).ends_with("2026:2551Q:Q2@ebirforms.goldcoders.dev"));
    }

    /// Two taxpayers filing the same form for the same period share an
    /// `obligation_key`. Without profile scoping their UIDs collide, and
    /// importing the second calendar overwrites the first taxpayer's deadlines.
    #[test]
    fn two_profiles_never_emit_the_same_uid_for_the_same_obligation() {
        let events = [event("x", "", "2026-07-27", "2026-07-28")];
        let mut other = profile();
        other.full_name = "Other Taxpayer".to_string();
        other.tin = serde_json::from_value(serde_json::json!({
            "segment1": "111", "segment2": "222", "segment3": "333", "branch": "00000"
        }))
        .unwrap();

        let uid_of = |p: &TaxpayerProfile| {
            unfold(&profile_calendar_ics(p, &events, "20260729T000000Z"))
                .lines()
                .find(|l| l.starts_with("UID:"))
                .unwrap()
                .to_string()
        };
        assert_ne!(uid_of(&profile()), uid_of(&other));
    }

    /// The UID travels into a calendar the user may share, so it must not carry
    /// the raw TIN.
    #[test]
    fn the_uid_does_not_leak_the_raw_tin() {
        let p = profile();
        let ics = profile_calendar_ics(
            &p,
            &[event("x", "", "2026-07-27", "2026-07-28")],
            "20260729T000000Z",
        );
        let uid = unfold(&ics)
            .lines()
            .find(|l| l.starts_with("UID:"))
            .unwrap()
            .to_string();
        assert!(!uid.contains(&p.tin.full()), "{uid}");
    }

    #[test]
    fn an_event_without_a_resolved_date_is_skipped_not_emitted_invalid() {
        let mut undated = event("x", "", "2026-07-27", "2026-07-28");
        undated.body = json!({"summary": "x"});
        let ics = profile_calendar_ics(&profile(), &[undated], "20260729T000000Z");
        assert!(!ics.contains("BEGIN:VEVENT"), "{ics}");
        assert!(ics.contains("END:VCALENDAR"));
    }

    #[test]
    fn an_empty_deadline_set_still_produces_a_valid_empty_calendar() {
        let ics = profile_calendar_ics(&profile(), &[], "20260729T000000Z");
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(ics.trim_end().ends_with("END:VCALENDAR"));
        assert!(!ics.contains("BEGIN:VEVENT"));
    }

    /// A bare LF is not a valid iCalendar line ending; strict importers reject
    /// the file rather than guessing.
    #[test]
    fn every_line_ends_with_crlf_and_never_a_bare_lf() {
        let ics = profile_calendar_ics(
            &profile(),
            &[event("x", "y", "2026-07-27", "2026-07-28")],
            "20260729T000000Z",
        );
        let bytes = ics.as_bytes();
        let bare_lf = bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| *byte == b'\n' && (index == 0 || bytes[index - 1] != b'\r'));
        assert!(!bare_lf, "{ics:?}");
    }

    #[test]
    fn the_file_name_identifies_the_profile() {
        assert_eq!(
            ics_file_name(&profile()),
            "BIR_Deadlines_00000000000000.ics"
        );
    }
}
