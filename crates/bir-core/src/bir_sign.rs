//! The BIR sign as an app hears it: the server's word on whether returns
//! get in, carried on calls the app already makes. Pure: parsing, and
//! keeping the newest one seen. Nothing here decides a send; the smart
//! queue ([`crate::smart_queue`]) reads it, and only ever to send less.
//!
//! - [`Sign::from_header`] reads the compact `buwiz-bir` header, an RFC
//!   8941 dictionary (`v=1, seq=4182, state=offline, …`).
//! - [`Sign::from_json`] reads the full sign object (a hold's body `bir`,
//!   a relay `result`'s `bir`).
//! - [`SignCell`] keeps the sign with the highest `seq`; an older one (a
//!   cached response) is ignored.
//!
//! A server that sends no sign (every server today, and any older one)
//! leaves the cell empty, and the queue behaves exactly as without it.

use std::sync::Mutex;

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::Value;

use crate::smart_queue::QueuePolicy;

/// The header the sign rides on.
pub const SIGN_HEADER: &str = "buwiz-bir";

/// The contract version this app reads. A sign with another `v` is ignored.
pub const SIGN_VERSION: u64 = 1;

/// One light, or the overall state. An unknown word reads as
/// [`SignState::NoRecentNews`], which claims neither up nor down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignState {
    Online,
    Busy,
    Offline,
    NoRecentNews,
}

impl SignState {
    pub fn from_wire(word: &str) -> Self {
        match word {
            "online" => Self::Online,
            "busy" => Self::Busy,
            "offline" => Self::Offline,
            _ => Self::NoRecentNews,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Busy => "busy",
            Self::Offline => "offline",
            Self::NoRecentNews => "no_recent_news",
        }
    }
}

/// Whether Buwiz itself is sending. Only `paused` pauses; any other word
/// reads as `ok`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relay {
    Ok,
    Paused,
}

impl Relay {
    fn from_wire(word: &str) -> Self {
        if word == "paused" { Self::Paused } else { Self::Ok }
    }
}

/// What the app keeps of one sign. Fields the app does not use (the words,
/// the note's text, the links) are not kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Sign {
    pub seq: u64,
    pub state: SignState,
    pub dispatcher: Option<SignState>,
    pub upload: Option<SignState>,
    pub relay: Relay,
    pub since: Option<DateTime<Utc>>,
    /// `wait_until`: never before it, unless a turn says so.
    pub wait_until: Option<DateTime<Utc>>,
    pub scout_at: Option<DateTime<Utc>>,
    /// Tickets need a turn (Busy, or waking after Offline).
    pub admission: bool,
    /// A hand-set note is in force.
    pub note: bool,
    /// The queue numbers, already clamped ([`QueuePolicy::from_json`]).
    /// Only the full object carries them.
    pub queue: Option<QueuePolicy>,
}

impl Sign {
    /// The compact `buwiz-bir` header (API §2.3). `None` without `v=1` and
    /// a `seq`. Members it does not know, or cannot read, are skipped.
    pub fn from_header(value: &str) -> Option<Self> {
        let members = parse_dictionary(value);
        let get = |key: &str| members.iter().find(|(k, _)| k == key).map(|(_, v)| v);
        if get("v")?.as_int()? != SIGN_VERSION as i64 {
            return None;
        }
        let seq = u64::try_from(get("seq")?.as_int()?).ok()?;
        let time = |key: &str| get(key).and_then(Item::as_int).and_then(unix);
        let word = |key: &str| get(key).and_then(Item::as_word);
        Some(Self {
            seq,
            state: word("state").map_or(SignState::NoRecentNews, SignState::from_wire),
            dispatcher: word("dispatcher").map(SignState::from_wire),
            upload: word("upload").map(SignState::from_wire),
            relay: word("relay").map_or(Relay::Ok, Relay::from_wire),
            since: time("since"),
            wait_until: time("until"),
            scout_at: time("scout"),
            admission: get("admit").and_then(Item::as_bool).unwrap_or(false),
            note: get("note").and_then(Item::as_bool).unwrap_or(false),
            queue: None,
        })
    }

    /// The full sign object (API §2.2). `None` without `"v": 1` and a
    /// `seq`.
    pub fn from_json(value: &Value) -> Option<Self> {
        if value.get("v")?.as_u64()? != SIGN_VERSION {
            return None;
        }
        let seq = value.get("seq")?.as_u64()?;
        let word = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
        let time = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                .map(|at| at.with_timezone(&Utc))
        };
        let light = |name: &str| {
            value
                .get("windows")
                .and_then(|windows| windows.get(name))
                .and_then(|window| word(window, "state"))
                .map(|state| SignState::from_wire(&state))
        };
        Some(Self {
            seq,
            state: word(value, "state").map_or(SignState::NoRecentNews, |s| SignState::from_wire(&s)),
            dispatcher: light("dispatcher"),
            upload: light("upload"),
            relay: word(value, "relay").map_or(Relay::Ok, |r| Relay::from_wire(&r)),
            since: time("since"),
            wait_until: time("wait_until"),
            scout_at: time("scout_at"),
            admission: value.get("admission").and_then(Value::as_bool).unwrap_or(false),
            note: value.get("note").is_some_and(|note| !note.is_null()),
            queue: value.get("queue").map(QueuePolicy::from_json),
        })
    }
}

fn unix(seconds: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(seconds, 0).single()
}

/// A sign and when this app saw it, by the wall clock (so a sign read
/// before the computer slept is not taken as fresh after it wakes).
#[derive(Clone, Debug, PartialEq)]
pub struct SeenSign {
    pub sign: Sign,
    pub seen_at: DateTime<Utc>,
}

impl SeenSign {
    /// Still believed: seen less than `stale_after_s` ago. A sign seen "in
    /// the future" (the clock went back) is not fresh.
    pub fn is_fresh(&self, now: DateTime<Utc>, policy: &QueuePolicy) -> bool {
        let age = now - self.seen_at;
        age >= Duration::zero() && age < Duration::seconds(i64::from(policy.stale_after_s))
    }
}

/// The newest sign this process has seen, by `seq`. Shared by every
/// request of one client.
#[derive(Debug, Default)]
pub struct SignCell(Mutex<Option<SeenSign>>);

impl SignCell {
    /// Keep `sign` if its `seq` is higher than the one held, or equal and it
    /// carries the queue numbers the held one lacks (the same sign, read in
    /// full). An older sign is ignored. `true` when kept.
    pub fn observe(&self, sign: Sign, seen_at: DateTime<Utc>) -> bool {
        let Ok(mut held) = self.0.lock() else {
            return false;
        };
        let newer = match held.as_ref() {
            None => true,
            Some(old) => {
                sign.seq > old.sign.seq
                    || (sign.seq == old.sign.seq && (sign.queue.is_some() || old.sign.queue.is_none()))
            }
        };
        if newer {
            *held = Some(SeenSign { sign, seen_at });
        }
        newer
    }

    pub fn latest(&self) -> Option<SeenSign> {
        self.0.lock().ok().and_then(|held| held.clone())
    }

    pub fn clear(&self) {
        if let Ok(mut held) = self.0.lock() {
            *held = None;
        }
    }
}

// ---------------------------------------------------------------- RFC 8941

/// One bare item of a structured-field dictionary member.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Int(i64),
    Bool(bool),
    Token(String),
    Text(String),
    Other,
}

impl Item {
    fn as_int(&self) -> Option<i64> {
        match self {
            Item::Int(n) => Some(*n),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Item::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn as_word(&self) -> Option<&str> {
        match self {
            Item::Token(word) | Item::Text(word) => Some(word),
            _ => None,
        }
    }
}

/// The members of a dictionary, in order. Parameters (`;x=1`) are dropped;
/// a member without `=` is `true`, as RFC 8941 says. A member that does not
/// parse is skipped, never the whole header.
fn parse_dictionary(input: &str) -> Vec<(String, Item)> {
    split_members(input)
        .into_iter()
        .filter_map(|member| {
            let member = member.trim();
            let (key, rest) = match member.find('=') {
                Some(at) => (&member[..at], Some(&member[at + 1..])),
                None => (member.split(';').next().unwrap_or_default(), None),
            };
            let key = key.trim();
            let valid_key = key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c == '*')
                && key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.*".contains(c));
            if !valid_key {
                return None;
            }
            let item = match rest {
                None => Item::Bool(true),
                Some(value) => bare_item(value.trim()),
            };
            Some((key.to_owned(), item))
        })
        .collect()
}

/// Commas outside a quoted string end a member.
fn split_members(input: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in input.chars() {
        if quoted {
            current.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                current.push(c);
            }
            ',' => members.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    members.push(current);
    members
}

fn bare_item(value: &str) -> Item {
    if let Some(text) = value.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some(next) => out.push(next),
                    None => return Item::Other,
                },
                '"' => return Item::Text(out),
                _ => out.push(c),
            }
        }
        return Item::Other;
    }
    // Parameters follow a `;` and are not used here.
    let value = value.split(';').next().unwrap_or_default().trim();
    match value {
        "?1" => Item::Bool(true),
        "?0" => Item::Bool(false),
        _ if value.starts_with(|c: char| c.is_ascii_digit() || c == '-') => {
            if value.contains('.') {
                Item::Other
            } else {
                value.parse().map_or(Item::Other, Item::Int)
            }
        }
        _ if value.starts_with(|c: char| c.is_ascii_alphabetic() || c == '*') => {
            Item::Token(value.to_owned())
        }
        _ => Item::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// API.md §2.3, verbatim.
    const API_2_3_HEADER: &str = "v=1, seq=4182, state=offline, dispatcher=online, upload=offline, relay=ok, since=1762845000, until=1762852421, scout=1762852421, admit=?0, note=?0";

    #[test]
    fn the_compact_header_of_api_2_3_reads_every_member() {
        let sign = Sign::from_header(API_2_3_HEADER).expect("a v=1 sign");
        assert_eq!(sign.seq, 4182);
        assert_eq!(sign.state, SignState::Offline);
        assert_eq!(sign.dispatcher, Some(SignState::Online));
        assert_eq!(sign.upload, Some(SignState::Offline));
        assert_eq!(sign.relay, Relay::Ok);
        assert_eq!(sign.since, unix(1_762_845_000));
        assert_eq!(sign.wait_until, unix(1_762_852_421));
        assert_eq!(sign.scout_at, unix(1_762_852_421));
        assert!(!sign.admission);
        assert!(!sign.note);
        assert_eq!(sign.queue, None);
    }

    #[test]
    fn absent_members_are_null_and_unknown_words_claim_nothing() {
        let sign = Sign::from_header("v=1, seq=7, state=sideways, relay=paused, admit").unwrap();
        assert_eq!(sign.state, SignState::NoRecentNews);
        assert_eq!(sign.relay, Relay::Paused);
        assert!(sign.admission, "a bare member is true");
        assert_eq!((sign.since, sign.wait_until, sign.scout_at), (None, None, None));
        assert_eq!((sign.dispatcher, sign.upload), (None, None));
        let unknown_relay = Sign::from_header("v=1, seq=7, relay=maybe").unwrap();
        assert_eq!(unknown_relay.relay, Relay::Ok);
    }

    #[test]
    fn a_header_without_v1_or_seq_is_no_sign() {
        for header in [
            "",
            "seq=4182, state=online",
            "v=2, seq=4182, state=online",
            "v=1, state=online",
            "v=1, seq=-3",
            "v=1, seq=12.5",
            "garbage ;;; ,,,",
        ] {
            assert_eq!(Sign::from_header(header), None, "{header:?}");
        }
    }

    #[test]
    fn a_member_that_does_not_parse_is_skipped_not_the_header() {
        let sign = Sign::from_header(
            r#"v=1, seq=9, Bad=1, note="a, quoted; text", state=busy;x=1, until=nope, admit=?1"#,
        )
        .unwrap();
        assert_eq!(sign.state, SignState::Busy);
        assert_eq!(sign.wait_until, None);
        assert!(sign.admission);
        // A string where a boolean belongs is no note.
        assert!(!sign.note);
    }

    /// API.md §2.2, with the links pointed at example.test.
    fn api_2_2_sign_object() -> Value {
        json!({
          "v": 1,
          "seq": 4182,
          "state": "offline",
          "since": "2026-11-11T07:10:00Z",
          "evaluated_at": "2026-11-11T09:00:00Z",
          "source": "tries",
          "relay": "ok",
          "windows": {
            "dispatcher": { "state": "online",  "since": "2026-11-11T01:00:00Z", "last_ok_at": "2026-11-11T08:55:00Z" },
            "upload":     { "state": "offline", "since": "2026-11-11T07:10:00Z", "last_ok_at": "2026-11-11T07:00:00Z" }
          },
          "last_success_at": "2026-11-11T07:00:00Z",
          "last_news_at": "2026-11-11T08:55:00Z",
          "last_state": null,
          "scout_at": "2026-11-11T09:13:41Z",
          "wait_until": "2026-11-11T09:13:41Z",
          "admission": false,
          "note": null,
          "words": { "title": "BIR is offline", "line": "The BIR's mailbox is offline since 3:10 PM." },
          "queue": { "waits_s": [300, 1800, 3600, 7200], "repeat_s": 7200, "spread": 0.5,
                     "notice_after_s": 86400, "sign_read_s": 60, "report_s": 300, "stale_after_s": 600 },
          "links": { "status_page": "https://example.test/bir-status" }
        })
    }

    #[test]
    fn the_sign_object_of_api_2_2_reads_its_lights_times_and_queue() {
        let sign = Sign::from_json(&api_2_2_sign_object()).unwrap();
        assert_eq!(sign.seq, 4182);
        assert_eq!(sign.state, SignState::Offline);
        assert_eq!(sign.dispatcher, Some(SignState::Online));
        assert_eq!(sign.upload, Some(SignState::Offline));
        assert_eq!(sign.wait_until.unwrap().to_rfc3339(), "2026-11-11T09:13:41+00:00");
        assert_eq!(sign.scout_at, sign.wait_until);
        assert!(!sign.note);
        assert_eq!(sign.queue, Some(QueuePolicy::default()));
        let mut with_note = api_2_2_sign_object();
        with_note["note"] = json!({"kind": "outage", "text": "x"});
        assert!(Sign::from_json(&with_note).unwrap().note);
        for broken in [json!({}), json!({"v": 2, "seq": 1}), json!({"v": 1}), json!("v=1")] {
            assert_eq!(Sign::from_json(&broken), None, "{broken}");
        }
    }

    #[test]
    fn the_cell_keeps_the_highest_seq_and_ignores_an_older_sign() {
        let cell = SignCell::default();
        let at = Utc.with_ymd_and_hms(2026, 11, 11, 9, 0, 0).unwrap();
        assert_eq!(cell.latest(), None, "no sign until one is heard");
        let new = Sign::from_header("v=1, seq=10, state=online").unwrap();
        let old = Sign::from_header("v=1, seq=9, state=offline").unwrap();
        assert!(cell.observe(new.clone(), at));
        assert!(!cell.observe(old, at + Duration::minutes(1)), "a cached, older sign");
        assert_eq!(cell.latest().unwrap().sign.state, SignState::Online);
        assert_eq!(cell.latest().unwrap().seen_at, at);
        // The same sign read in full replaces the header copy.
        let mut full = Sign::from_json(&api_2_2_sign_object()).unwrap();
        full.seq = 10;
        assert!(cell.observe(full, at + Duration::minutes(2)));
        assert!(cell.latest().unwrap().sign.queue.is_some());
        // …and a header copy of it does not drop the queue numbers.
        assert!(!cell.observe(new, at + Duration::minutes(3)));
        assert!(cell.latest().unwrap().sign.queue.is_some());
        cell.clear();
        assert_eq!(cell.latest(), None);
    }

    #[test]
    fn freshness_is_wall_clock_age_and_a_future_sign_is_stale() {
        let at = Utc.with_ymd_and_hms(2026, 11, 11, 9, 0, 0).unwrap();
        let seen = SeenSign { sign: Sign::from_header("v=1, seq=1").unwrap(), seen_at: at };
        let policy = QueuePolicy::default();
        assert!(seen.is_fresh(at, &policy));
        assert!(seen.is_fresh(at + Duration::seconds(599), &policy));
        assert!(!seen.is_fresh(at + Duration::seconds(600), &policy));
        assert!(!seen.is_fresh(at - Duration::seconds(1), &policy));
    }
}
