//! The smart send queue's rules, pure: no I/O, no clock, no global
//! randomness. The worker (`background_cron`) passes the time and a random
//! source in, so every rule here is deterministic under a seeded
//! [`SeededRng`] and an injected clock.
//!
//! Every connection toward the BIR is opened by the person's own app, from
//! their own internet address. So every rule here is about sending less:
//!
//! - **Waits.** After a return's n-th counted failure it waits
//!   `waits_s[n-1]` (5 min, 30 min, 1 h, 2 h), then `repeat_s` (2 h) every
//!   time after, each multiplied by a random factor uniform in
//!   `[1 - spread, 1 + spread]` (0.5 to 1.5 by default), so queues that
//!   failed together do not come back together ([`next_try_at`]).
//! - **Clamps.** The server may tune the numbers ([`QueuePolicy::from_json`])
//!   only toward sending less: a wait below its default, a repeat below
//!   2 h, a spread outside 0.2 to 0.5, any value missing or out of range,
//!   reads as the default. One return at a time ([`PARALLEL`]) is fixed.
//! - **One at a time.** [`line_up`] picks the one return that may go now
//!   (due soonest first, across every TIN) and gives every other a real
//!   next-try time at least a minute apart, so a build that knows none of
//!   this, sharing the database, does not send them all at once.
//! - **Step aside.** A counted failure moves only that return's own time
//!   forward, so the next due return is the head: one stuck return never
//!   blocks the others.
//! - **Device guard.** When the sign says returns get in and this device's
//!   last three counted tries all failed, the next wait is at least
//!   `repeat_s` and the person is told it may be their own connection
//!   ([`guard_trips`]).
//! - **What counts** ([`TryClass`]): a hold, being signed out, Buwiz being
//!   unreachable and a failure of class `ours` are never tries. An
//!   `unknown` outcome is never resent by itself; a TIN refusal goes back
//!   to Draft. Neither gets a retry ([`plan_retry`]).
//! - **The gate** ([`gate`]) is what the sign-aware queue will drive
//!   (sleep while Offline or while Buwiz pauses, turns while Busy). Nothing
//!   calls it yet.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bir_sign::{Relay, SeenSign, SignState};

/// Returns in flight at once, per database. Fixed; no server number
/// changes it.
pub const PARALLEL: usize = 1;

pub const DEFAULT_WAITS_S: [u32; 4] = [300, 1800, 3600, 7200];
pub const DEFAULT_REPEAT_S: u32 = 7200;
pub const DEFAULT_SPREAD: f64 = 0.5;
pub const DEFAULT_NOTICE_AFTER_S: u32 = 86_400;
pub const DEFAULT_SIGN_READ_S: u32 = 60;
pub const DEFAULT_REPORT_S: u32 = 300;
pub const DEFAULT_STALE_AFTER_S: u32 = 600;
/// No wait the server tunes may be longer than 6 hours.
pub const MAX_WAIT_S: u32 = 21_600;
/// The spread's half-width may be tuned only within this range.
pub const SPREAD_RANGE: (f64, f64) = (0.2, 0.5);
/// Rows behind the head are at least this far apart in their stored
/// next-try time.
pub const BEHIND_GAP_S: i64 = 60;

/// The queue numbers (API §9), always within their clamps.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuePolicy {
    /// The wait after a return's 1st, 2nd, 3rd and 4th counted failure.
    pub waits_s: [u32; 4],
    /// Every wait after the 4th.
    pub repeat_s: u32,
    /// Half-width of the random factor each wait is multiplied by.
    pub spread: f64,
    /// Tell the person a return is still waiting after this long, and again
    /// each time this long passes.
    pub notice_after_s: u32,
    pub sign_read_s: u32,
    pub report_s: u32,
    pub stale_after_s: u32,
}

impl Default for QueuePolicy {
    fn default() -> Self {
        Self {
            waits_s: DEFAULT_WAITS_S,
            repeat_s: DEFAULT_REPEAT_S,
            spread: DEFAULT_SPREAD,
            notice_after_s: DEFAULT_NOTICE_AFTER_S,
            sign_read_s: DEFAULT_SIGN_READ_S,
            report_s: DEFAULT_REPORT_S,
            stale_after_s: DEFAULT_STALE_AFTER_S,
        }
    }
}

impl QueuePolicy {
    /// The sign's `queue` object, every key clamped: a key that is missing,
    /// not a whole number (or, for `spread`, not a finite number), or
    /// outside its range reads as its default. Anything but an object is
    /// the defaults. Unknown keys (a `parallel` among them) are ignored.
    pub fn from_json(value: &Value) -> Self {
        let defaults = Self::default();
        let Some(object) = value.as_object() else {
            return defaults;
        };
        let whole = |key: &str, low: u32, high: u32, default: u32| {
            object
                .get(key)
                .and_then(Value::as_u64)
                .filter(|n| (u64::from(low)..=u64::from(high)).contains(n))
                .map_or(default, |n| n as u32)
        };
        let waits_s = match object.get("waits_s").and_then(Value::as_array) {
            Some(waits) if waits.len() == DEFAULT_WAITS_S.len() => {
                let mut out = DEFAULT_WAITS_S;
                for (slot, (value, default)) in
                    out.iter_mut().zip(waits.iter().zip(DEFAULT_WAITS_S))
                {
                    *slot = value
                        .as_u64()
                        .filter(|n| (u64::from(default)..=u64::from(MAX_WAIT_S)).contains(n))
                        .map_or(default, |n| n as u32);
                }
                out
            }
            _ => DEFAULT_WAITS_S,
        };
        let spread = object
            .get("spread")
            .and_then(Value::as_f64)
            .filter(|s| s.is_finite() && (SPREAD_RANGE.0..=SPREAD_RANGE.1).contains(s))
            .unwrap_or(DEFAULT_SPREAD);
        let sign_read_s = whole("sign_read_s", 30, 600, DEFAULT_SIGN_READ_S);
        let stale_floor = 2 * sign_read_s;
        let stale_after_s = whole(
            "stale_after_s",
            stale_floor,
            3600,
            DEFAULT_STALE_AFTER_S.max(stale_floor),
        );
        Self {
            waits_s,
            repeat_s: whole("repeat_s", DEFAULT_REPEAT_S, MAX_WAIT_S, DEFAULT_REPEAT_S),
            spread,
            notice_after_s: whole("notice_after_s", 3600, 604_800, defaults.notice_after_s),
            sign_read_s,
            report_s: whole("report_s", 60, 300, DEFAULT_REPORT_S),
            stale_after_s,
        }
    }

    /// The base wait, in seconds, after a return's `failures`-th counted
    /// failure (0 reads as the first).
    pub fn wait_s(&self, failures: u32) -> u32 {
        match failures {
            0 | 1 => self.waits_s[0],
            2..=4 => self.waits_s[failures as usize - 1],
            _ => self.repeat_s,
        }
    }

    /// The longest any one wait can be, spread included. A stored next try
    /// further away than this (the clock went back) is taken as this far.
    pub fn longest_wait(&self) -> Duration {
        let longest = self
            .waits_s
            .iter()
            .copied()
            .chain([self.repeat_s])
            .max()
            .unwrap_or(MAX_WAIT_S);
        seconds_f64(f64::from(longest) * (1.0 + self.spread))
    }
}

fn seconds_f64(seconds: f64) -> Duration {
    Duration::milliseconds((seconds * 1000.0).round() as i64)
}

// ------------------------------------------------------------ randomness

/// The randomness the spread draws from. It only spreads load; nothing
/// about it hides or shapes traffic.
pub trait QueueRng {
    fn next_u64(&mut self) -> u64;
}

/// A uniform draw in `[0, 1)`.
pub fn unit(rng: &mut dyn QueueRng) -> f64 {
    (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// SplitMix64: a small, seeded source for tests and simulations.
#[derive(Clone, Debug)]
pub struct SeededRng(u64);

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
}

impl QueueRng for SeededRng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The operating system's random source, for the app.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsRng;

impl QueueRng for OsRng {
    fn next_u64(&mut self) -> u64 {
        rand::random()
    }
}

/// A factor uniform in `[1 - spread, 1 + spread)`.
pub fn spread_factor(rng: &mut dyn QueueRng, spread: f64) -> f64 {
    1.0 - spread + 2.0 * spread * unit(rng)
}

// ----------------------------------------------------------------- waits

/// Which wait a try that did not land earns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    /// The return's `failures`-th counted failure; `slowed` when the device
    /// guard tripped (the wait is then at least `repeat_s`).
    Failed { failures: u32, slowed: bool },
    /// Not a try: the first wait, and no count.
    NotATry,
}

/// When a return may try again. The wait starts when the try ended
/// (`ended_at`); it is the base wait for `wait` times the spread factor.
/// When the server's `wait_until` is later than that, the try goes at
/// `wait_until` plus a random share (0 to `spread`) of the time that was
/// left until it (`wait_until - now`): never before `wait_until`.
pub fn next_try_at(
    wait: Wait,
    ended_at: DateTime<Utc>,
    wait_until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    policy: &QueuePolicy,
    rng: &mut dyn QueueRng,
) -> DateTime<Utc> {
    let base = match wait {
        Wait::Failed { failures, slowed } => {
            let wait = policy.wait_s(failures);
            if slowed {
                wait.max(policy.repeat_s)
            } else {
                wait
            }
        }
        Wait::NotATry => policy.waits_s[0],
    };
    let at = ended_at + seconds_f64(f64::from(base) * spread_factor(rng, policy.spread));
    match wait_until {
        Some(until) if until > at => {
            let left = (until - now).max(Duration::zero());
            let share = policy.spread * unit(rng);
            until + Duration::milliseconds((left.num_milliseconds() as f64 * share).round() as i64)
        }
        _ => at,
    }
}

// ------------------------------------------------------- what counts

/// What one try that did not land means for the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TryClass {
    /// Not a try: a hold (`503 bir_offline`, `bir_busy`, `relay_paused`,
    /// `restarting`), being signed out, Buwiz unreachable, a failure of
    /// class `ours`. No count; the first wait, or the server's
    /// `wait_until`.
    NotATry,
    /// A real try that failed (class `bir` or `user_network`, or no class
    /// from an older server), or a ticket or submission error that is not a
    /// refusal or a hold: the return's own schedule.
    Counted,
    /// The BIR has no upload service for this TIN: counted on the return's
    /// schedule, but not a failure of the BIR's windows, so the device
    /// guard does not hear it.
    NoService,
    /// A TIN refusal: back to Draft at once, as before.
    Refused,
    /// The outcome is unknown: BIR may have the return. Never sent again
    /// by itself.
    MaybeSent,
}

impl TryClass {
    /// Whether the failure moves the return along its schedule.
    pub fn counts(self) -> bool {
        matches!(self, Self::Counted | Self::NoService)
    }
}

/// The server's `class` on a failure: whose side it failed on. An unknown
/// word reads as `bir` (API §12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureClass {
    Bir,
    UserNetwork,
    Ours,
}

impl FailureClass {
    pub fn from_wire(word: &str) -> Self {
        match word {
            "ours" => Self::Ours,
            "user_network" => Self::UserNetwork,
            _ => Self::Bir,
        }
    }

    /// A failed try of this class: `ours` is not a try; anything else
    /// counts.
    pub fn try_class(self) -> TryClass {
        match self {
            Self::Ours => TryClass::NotATry,
            Self::Bir | Self::UserNetwork => TryClass::Counted,
        }
    }
}

/// The `503` codes with which Buwiz holds a ticket. Nothing reached the
/// BIR; a hold is never a try.
pub const HOLD_CODES: [&str; 4] = ["bir_offline", "bir_busy", "relay_paused", "restarting"];

pub fn is_hold_code(code: &str) -> bool {
    HOLD_CODES.contains(&code)
}

/// When a return that did not land goes again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retry {
    pub at: DateTime<Utc>,
    /// The failure counted: the return's failure count is now `failures`.
    pub counted: bool,
    pub failures: u32,
    /// The device guard set this wait.
    pub slowed: bool,
}

/// The retry for a try of `class` that ended at `ended_at`, for a return
/// that had `failures_before` counted failures. `None` for a refusal or an
/// unknown outcome: neither is ever retried by the queue. `slowed_down` is
/// the device guard's answer ([`guard_trips`]); it only lengthens a
/// counted failure's wait.
#[allow(clippy::too_many_arguments)]
pub fn plan_retry(
    failures_before: u32,
    class: TryClass,
    slowed_down: bool,
    ended_at: DateTime<Utc>,
    wait_until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    policy: &QueuePolicy,
    rng: &mut dyn QueueRng,
) -> Option<Retry> {
    let (wait, failures, slowed) = match class {
        TryClass::Refused | TryClass::MaybeSent => return None,
        TryClass::NotATry => (Wait::NotATry, failures_before, false),
        TryClass::Counted | TryClass::NoService => {
            let failures = failures_before.saturating_add(1);
            let slowed = slowed_down && class == TryClass::Counted;
            (Wait::Failed { failures, slowed }, failures, slowed)
        }
    };
    Some(Retry {
        at: next_try_at(wait, ended_at, wait_until, now, policy, rng),
        counted: class.counts(),
        failures,
        slowed,
    })
}

/// A counted failure at `now` on the default numbers and the OS random
/// source: what a caller without the worker's policy (a test, a tool)
/// records.
pub fn default_retry(failures_before: u32, now: DateTime<Utc>) -> Retry {
    plan_retry(
        failures_before,
        TryClass::Counted,
        false,
        now,
        None,
        now,
        &QueuePolicy::default(),
        &mut OsRng,
    )
    .expect("a counted failure has a retry")
}

// ---------------------------------------------------------- device guard

/// How many of this device's last counted tries the guard looks at.
pub const GUARD_TRIES: usize = 3;

/// This device's last counted tries, newest last (`true`: it got in).
/// Stored as a short string of `o` (got in) and `x` (failed).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceTries(Vec<bool>);

impl DeviceTries {
    pub fn from_setting(stored: &str) -> Self {
        let mut tries = Self::default();
        for c in stored.chars() {
            match c {
                'o' => tries.record(true),
                'x' => tries.record(false),
                _ => {}
            }
        }
        tries
    }

    pub fn to_setting(&self) -> String {
        self.0
            .iter()
            .map(|&got_in| if got_in { 'o' } else { 'x' })
            .collect()
    }

    pub fn record(&mut self, got_in: bool) {
        self.0.push(got_in);
        if self.0.len() > GUARD_TRIES {
            self.0.remove(0);
        }
    }

    /// The last [`GUARD_TRIES`] counted tries all failed.
    pub fn all_failed(&self) -> bool {
        self.0.len() == GUARD_TRIES && self.0.iter().all(|&got_in| !got_in)
    }
}

/// The guard is on: the sign says returns get in for others, and this
/// device's last tries all failed. It is off again after a landing, or
/// once the sign is anything but Online (or there is no sign).
pub fn guard_on(sign: Option<SignState>, tries: &DeviceTries) -> bool {
    sign == Some(SignState::Online) && tries.all_failed()
}

/// Whether a counted failure now, added to `tries`, turns the guard on.
pub fn guard_trips(sign: Option<SignState>, tries: &DeviceTries) -> bool {
    let mut after = tries.clone();
    after.record(false);
    guard_on(sign, &after)
}

// -------------------------------------------------------------- the line

/// One queued return, as [`line_up`] sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    /// Its BIR due date (Manila), when the calendar knows one.
    pub due: Option<NaiveDate>,
    /// When the person queued it.
    pub queued_at: DateTime<Utc>,
    /// When it may next try on its own schedule; `None` is now.
    pub ready_at: Option<DateTime<Utc>>,
    /// Claimed (being sent, or with an unknown outcome): never the head,
    /// never restamped.
    pub blocked: bool,
}

/// The queue's order at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineUp {
    /// The one return that may go now.
    pub head: Option<usize>,
    /// Each row's own next-try time, a clock that went back clamped to
    /// [`QueuePolicy::longest_wait`] from now. Blocked rows keep theirs.
    pub ready: Vec<DateTime<Utc>>,
    /// The next-try time to store on each row (`None` for a blocked row):
    /// now for the head, and for every other row its own time but at least
    /// [`BEHIND_GAP_S`] after the row before it in line. Never null.
    pub stamps: Vec<Option<DateTime<Utc>>>,
    /// Waiting its turn: its own time is passed, or it was pushed back to
    /// keep the gap, and it is not the head.
    pub behind: Vec<bool>,
}

/// Order the queue: the head is the eligible row (not blocked, its own
/// time passed) due soonest (BIR due date, then the time it was queued);
/// the rest follow, the eligible ones in the same order, then the others
/// by their own time. Every row's stamp keeps a gap after the one before.
pub fn line_up(rows: &[Waiting], now: DateTime<Utc>, policy: &QueuePolicy) -> LineUp {
    let cap = now + policy.longest_wait();
    let ready: Vec<DateTime<Utc>> = rows
        .iter()
        .map(|row| {
            let own = row.ready_at.unwrap_or(now);
            if row.blocked { own } else { own.min(cap) }
        })
        .collect();
    let eligible = |i: usize| !rows[i].blocked && ready[i] <= now;
    let by_due = |i: &usize| {
        (
            rows[*i].due.unwrap_or(NaiveDate::MAX),
            rows[*i].queued_at,
            *i,
        )
    };
    let mut first: Vec<usize> = (0..rows.len()).filter(|&i| eligible(i)).collect();
    first.sort_by_key(by_due);
    let mut later: Vec<usize> = (0..rows.len())
        .filter(|&i| !rows[i].blocked && !eligible(i))
        .collect();
    later.sort_by_key(|i| (ready[*i], by_due(i)));
    let head = first.first().copied();

    let mut stamps = vec![None; rows.len()];
    let mut behind = vec![false; rows.len()];
    let mut previous: Option<DateTime<Utc>> = None;
    for i in first.into_iter().chain(later) {
        let stamp = if Some(i) == head {
            now
        } else {
            match previous {
                Some(before) => ready[i].max(before + Duration::seconds(BEHIND_GAP_S)),
                None => ready[i],
            }
        };
        behind[i] = Some(i) != head && (ready[i] <= now || stamp > ready[i]);
        stamps[i] = Some(stamp);
        previous = Some(stamp);
    }
    LineUp {
        head,
        ready,
        stamps,
        behind,
    }
}

/// The one return that may go now ([`line_up`]'s head).
pub fn pick_head(rows: &[Waiting], now: DateTime<Utc>, policy: &QueuePolicy) -> Option<usize> {
    line_up(rows, now, policy).head
}

// --------------------------------------------------- still waiting, pause

/// How many "still waiting" notices a return queued at `queued_at` has
/// earned by `now`: one per full `notice_after_s` (24 h) of wall time,
/// sleep included.
pub fn notices_due(queued_at: DateTime<Utc>, now: DateTime<Utc>, policy: &QueuePolicy) -> u32 {
    if now <= queued_at {
        return 0;
    }
    let notice = i64::from(policy.notice_after_s.max(1));
    u32::try_from((now - queued_at).num_seconds() / notice).unwrap_or(u32::MAX)
}

/// The person's switch: the queue sends by itself, or not at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueueMode {
    #[default]
    Auto,
    Paused,
}

/// The settings key of [`QueueMode`].
pub const QUEUE_MODE_SETTING: &str = "smart_queue_mode";
/// The settings key of this device's [`DeviceTries`].
pub const DEVICE_TRIES_SETTING: &str = "smart_queue_recent";

impl QueueMode {
    /// Only `paused` pauses; anything else (or nothing) is Auto.
    pub fn from_setting(stored: Option<&str>) -> Self {
        if stored.map(str::trim) == Some("paused") {
            Self::Paused
        } else {
            Self::Auto
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Paused => "paused",
        }
    }
}

/// The queue's own facts about one return, kept in its stored JSON under
/// `smart_queue`. Absent (the default) on every row queued before.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueState {
    /// Counted failures under these rules. A row queued under the old rules
    /// starts at 0: its `submission_attempts` is history.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failures: u32,
    /// Its own next try (RFC 3339). The stored `next_retry_at` may be later
    /// while it waits behind another return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_try_at: Option<String>,
    /// The device guard set the last wait.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub slowed: bool,
    /// "Still waiting" notices already raised for this queueing.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub notices: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl QueueState {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Its own next try, when stored and readable.
    pub fn next_try(&self) -> Option<DateTime<Utc>> {
        self.next_try_at.as_deref().and_then(parse_time)
    }

    /// After a try that did not land.
    pub fn after(&mut self, retry: &Retry) {
        self.failures = retry.failures;
        self.next_try_at = Some(retry.at.to_rfc3339());
        self.slowed = retry.slowed;
    }
}

pub fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

// --------------------------------------------------------------- the gate

/// A turn the server gave one queued return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// Try now while Offline.
    Scout,
    /// Take a place while Busy or waking.
    Admit,
}

/// What the sign lets the queue do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// The head goes when its own time passes.
    Open,
    /// Offline, or Buwiz paused sending: no try, none counted.
    Sleep,
    /// Busy or waking: the head goes only with an admit turn.
    NeedTurn,
    /// Nothing before this time.
    WaitUntil(DateTime<Utc>),
}

/// The gate for the newest sign (API §9's fixed rules). No sign, or one
/// older than `stale_after_s`, is Open: an app that cannot hear the sign
/// sends as it would without one. A turn wins over `wait_until` and over
/// admission, and a scout turn over Offline; nothing lifts `relay: paused`.
pub fn gate(
    sign: Option<&SeenSign>,
    now: DateTime<Utc>,
    policy: &QueuePolicy,
    turn: Option<Turn>,
) -> Gate {
    let Some(seen) = sign.filter(|seen| seen.is_fresh(now, policy)) else {
        return Gate::Open;
    };
    let sign = &seen.sign;
    if sign.relay == Relay::Paused {
        return Gate::Sleep;
    }
    if sign.state == SignState::Offline {
        return if turn == Some(Turn::Scout) {
            Gate::Open
        } else {
            Gate::Sleep
        };
    }
    if turn.is_some() {
        return Gate::Open;
    }
    if sign.state == SignState::Busy || sign.admission {
        return Gate::NeedTurn;
    }
    match sign.wait_until {
        Some(until) if until > now => Gate::WaitUntil(until),
        _ => Gate::Open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bir_sign::Sign;
    use chrono::TimeZone;
    use serde_json::json;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 11, 7, 0, 0).unwrap()
    }

    fn secs(d: Duration) -> f64 {
        d.num_milliseconds() as f64 / 1000.0
    }

    // ------------------------------------------------------------- policy

    #[test]
    fn the_default_waits_are_5_min_30_min_1_h_2_h_then_every_2_h() {
        let policy = QueuePolicy::default();
        let waits: Vec<u32> = (1..=8).map(|n| policy.wait_s(n)).collect();
        assert_eq!(waits, [300, 1800, 3600, 7200, 7200, 7200, 7200, 7200]);
        assert_eq!(policy.wait_s(0), 300);
        assert_eq!(policy.wait_s(u32::MAX), 7200);
        assert_eq!(policy.spread, 0.5);
        assert_eq!(PARALLEL, 1);
    }

    #[test]
    fn the_api_9_queue_object_reads_as_the_defaults() {
        let api_9 = json!({ "waits_s": [300, 1800, 3600, 7200], "repeat_s": 7200, "spread": 0.5,
                            "notice_after_s": 86400, "sign_read_s": 60, "report_s": 300, "stale_after_s": 600 });
        assert_eq!(QueuePolicy::from_json(&api_9), QueuePolicy::default());
    }

    #[test]
    fn the_server_can_lengthen_waits_within_their_clamps() {
        let longer = json!({ "waits_s": [600, 3600, 7200, 21600], "repeat_s": 21600, "spread": 0.2,
                             "notice_after_s": 3600, "sign_read_s": 600, "report_s": 60, "stale_after_s": 1200 });
        let policy = QueuePolicy::from_json(&longer);
        assert_eq!(policy.waits_s, [600, 3600, 7200, 21600]);
        assert_eq!(policy.repeat_s, 21600);
        assert_eq!(policy.spread, 0.2);
        assert_eq!(policy.notice_after_s, 3600);
        assert_eq!(
            (policy.sign_read_s, policy.report_s, policy.stale_after_s),
            (600, 60, 1200)
        );
    }

    #[test]
    fn hostile_numbers_fall_back_to_the_defaults() {
        let defaults = QueuePolicy::default();
        let hostile = [
            json!({ "waits_s": [0, 0, 0, 0] }),
            json!({ "waits_s": [299, 1799, 3599, 7199] }),
            json!({ "waits_s": [300, 1800, 3600] }),
            json!({ "waits_s": [300, 1800, 3600, 7200, 60] }),
            json!({ "waits_s": [300.5, -1, "x", null] }),
            json!({ "waits_s": [21601, 99999999999u64, 1800, 7200] }),
            json!({ "repeat_s": 60 }),
            json!({ "repeat_s": 7199 }),
            json!({ "repeat_s": 21601 }),
            json!({ "spread": 0.9 }),
            json!({ "spread": 0.1 }),
            json!({ "spread": -0.5 }),
            json!({ "spread": "0.5" }),
            json!({ "report_s": 3600 }),
            json!({ "report_s": 10 }),
            json!({ "notice_after_s": 60 }),
            json!({ "notice_after_s": 9_999_999 }),
            json!({ "sign_read_s": 1 }),
            json!({ "stale_after_s": 5 }),
            json!({ "stale_after_s": 100_000 }),
            json!({ "parallel": 50 }),
            json!([1, 2, 3]),
            json!("waits"),
            json!(null),
        ];
        for value in hostile {
            let policy = QueuePolicy::from_json(&value);
            assert!(
                policy
                    .waits_s
                    .iter()
                    .zip(DEFAULT_WAITS_S)
                    .all(|(w, d)| *w >= d),
                "{value}"
            );
            assert_eq!(policy, defaults, "{value}");
        }
        // One bad entry falls back alone.
        let mixed = QueuePolicy::from_json(&json!({ "waits_s": [600, 5, 3600, 9000] }));
        assert_eq!(mixed.waits_s, [600, 1800, 3600, 9000]);
    }

    #[test]
    fn stale_after_is_at_least_twice_the_sign_read() {
        let policy = QueuePolicy::from_json(&json!({ "sign_read_s": 400, "stale_after_s": 700 }));
        assert_eq!(policy.sign_read_s, 400);
        assert_eq!(
            policy.stale_after_s, 800,
            "700 < 2 x 400, so the default, raised to 800"
        );
        let policy = QueuePolicy::from_json(&json!({ "sign_read_s": 200, "stale_after_s": 500 }));
        assert_eq!(policy.stale_after_s, 500);
    }

    /// Property: whatever numbers arrive, the clamped policy never sends
    /// more than the defaults.
    #[test]
    fn property_no_policy_sends_more_than_the_defaults() {
        let mut rng = SeededRng::new(0x5eed);
        let pick = |rng: &mut SeededRng| -> Value {
            match rng.next_u64() % 6 {
                0 => json!(rng.next_u64() % 40_000),
                1 => json!((unit(rng) * 2.0) - 0.5),
                2 => json!(-((rng.next_u64() % 1000) as i64)),
                3 => json!(null),
                4 => json!("300"),
                _ => json!(rng.next_u64()),
            }
        };
        for _ in 0..5_000 {
            let mut object = serde_json::Map::new();
            for key in [
                "repeat_s",
                "spread",
                "notice_after_s",
                "sign_read_s",
                "report_s",
                "stale_after_s",
                "parallel",
            ] {
                if !rng.next_u64().is_multiple_of(3) {
                    object.insert(key.into(), pick(&mut rng));
                }
            }
            let n = (rng.next_u64() % 6) as usize;
            object.insert(
                "waits_s".into(),
                Value::Array((0..n).map(|_| pick(&mut rng)).collect()),
            );
            let policy = QueuePolicy::from_json(&Value::Object(object.clone()));
            for (wait, default) in policy.waits_s.iter().zip(DEFAULT_WAITS_S) {
                assert!((default..=MAX_WAIT_S).contains(wait), "{object:?}");
            }
            assert!(
                (DEFAULT_REPEAT_S..=MAX_WAIT_S).contains(&policy.repeat_s),
                "{object:?}"
            );
            assert!(
                (SPREAD_RANGE.0..=SPREAD_RANGE.1).contains(&policy.spread),
                "{object:?}"
            );
            assert!((60..=300).contains(&policy.report_s));
            assert!((3600..=604_800).contains(&policy.notice_after_s));
            assert!(policy.stale_after_s >= 2 * policy.sign_read_s && policy.stale_after_s <= 3600);
            // Every wait, at its shortest, is no shorter than the default's.
            for failures in 1..=8 {
                let shortest = f64::from(policy.wait_s(failures)) * (1.0 - policy.spread);
                let default_shortest =
                    f64::from(QueuePolicy::default().wait_s(failures)) * (1.0 - DEFAULT_SPREAD);
                assert!(shortest >= default_shortest, "{object:?}");
            }
        }
    }

    // ------------------------------------------------------------- spread

    #[test]
    fn the_seeded_source_repeats_and_the_os_source_varies() {
        let a: Vec<u64> = {
            let mut r = SeededRng::new(7);
            (0..4).map(|_| r.next_u64()).collect()
        };
        let b: Vec<u64> = {
            let mut r = SeededRng::new(7);
            (0..4).map(|_| r.next_u64()).collect()
        };
        assert_eq!(a, b);
        let mut os = OsRng;
        assert_ne!((os.next_u64(), os.next_u64(), os.next_u64()), (0, 0, 0));
    }

    /// Property: 10 000 seeded draws of every wait stay in `[0.5, 1.5)` of
    /// the base, and average 1 within tolerance.
    #[test]
    fn property_the_spread_is_uniform_between_half_and_one_and_a_half() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(42);
        for failures in 1..=6 {
            let base = f64::from(policy.wait_s(failures));
            let mut sum = 0.0;
            let (mut low, mut high) = (0usize, 0usize);
            let draws = 10_000;
            for _ in 0..draws {
                let at = next_try_at(
                    Wait::Failed {
                        failures,
                        slowed: false,
                    },
                    t0(),
                    None,
                    t0(),
                    &policy,
                    &mut rng,
                );
                let factor = secs(at - t0()) / base;
                assert!(
                    (0.5..=1.5).contains(&factor),
                    "factor {factor} for failure {failures}"
                );
                sum += factor;
                if factor < 0.75 {
                    low += 1
                }
                if factor > 1.25 {
                    high += 1
                }
            }
            let mean = sum / f64::from(draws);
            assert!((mean - 1.0).abs() < 0.02, "mean {mean}");
            // Uniform: each outer quarter of the range holds about a quarter.
            for side in [low, high] {
                assert!((2_200..2_800).contains(&side), "{side} of {draws}");
            }
        }
        // 5 minutes becomes anywhere from 2.5 to 7.5.
        let mut rng = SeededRng::new(1);
        let draws: Vec<f64> = (0..10_000)
            .map(|_| {
                secs(
                    next_try_at(
                        Wait::Failed {
                            failures: 1,
                            slowed: false,
                        },
                        t0(),
                        None,
                        t0(),
                        &policy,
                        &mut rng,
                    ) - t0(),
                )
            })
            .collect();
        let (min, max) = draws
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &d| (lo.min(d), hi.max(d)));
        assert!((150.0..160.0).contains(&min), "{min}");
        assert!(max <= 450.0 && max > 440.0, "{max}");
    }

    #[test]
    fn a_narrower_spread_from_the_server_narrows_the_factor() {
        let policy = QueuePolicy::from_json(&json!({ "spread": 0.2 }));
        let mut rng = SeededRng::new(9);
        for _ in 0..2_000 {
            let at = next_try_at(
                Wait::Failed {
                    failures: 2,
                    slowed: false,
                },
                t0(),
                None,
                t0(),
                &policy,
                &mut rng,
            );
            let factor = secs(at - t0()) / 1800.0;
            assert!((0.8..=1.2).contains(&factor), "{factor}");
        }
    }

    /// Property: with a `wait_until`, the try is never before it, and at
    /// most `spread` of the time that was left after it.
    #[test]
    fn property_never_before_wait_until() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(77);
        for i in 0..5_000 {
            let now = t0() + Duration::seconds(i);
            let ahead = Duration::seconds((rng.next_u64() % 20_000) as i64);
            let until = now + ahead;
            let failures = (rng.next_u64() % 7) as u32 + 1;
            let at = next_try_at(
                Wait::Failed {
                    failures,
                    slowed: false,
                },
                now,
                Some(until),
                now,
                &policy,
                &mut rng,
            );
            assert!(at >= until, "{at} before {until}");
            let own = f64::from(policy.wait_s(failures)) * 1.5;
            let latest = secs(ahead) * 1.5;
            assert!(secs(at - now) <= own.max(latest) + 0.001, "{at}");
            let not_a_try = next_try_at(Wait::NotATry, now, Some(until), now, &policy, &mut rng);
            assert!(not_a_try >= until);
        }
        // A wait_until already passed changes nothing.
        let mut a = SeededRng::new(3);
        let mut b = SeededRng::new(3);
        let past = Some(t0() - Duration::hours(1));
        assert_eq!(
            next_try_at(Wait::NotATry, t0(), past, t0(), &policy, &mut a),
            next_try_at(Wait::NotATry, t0(), None, t0(), &policy, &mut b)
        );
    }

    #[test]
    fn the_wait_starts_when_the_try_ended() {
        let policy = QueuePolicy::default();
        let ended = t0() + Duration::minutes(3);
        let mut rng = SeededRng::new(5);
        for _ in 0..1000 {
            let at = next_try_at(
                Wait::Failed {
                    failures: 1,
                    slowed: false,
                },
                ended,
                None,
                t0(),
                &policy,
                &mut rng,
            );
            assert!(at >= ended + Duration::seconds(150) && at <= ended + Duration::seconds(450));
        }
    }

    // --------------------------------------------------------- what counts

    #[test]
    fn holds_sign_outs_and_ours_are_not_tries() {
        for code in ["bir_offline", "bir_busy", "relay_paused", "restarting"] {
            assert!(is_hold_code(code));
        }
        for code in [
            "tin_not_active",
            "rate_limited",
            "internal",
            "try_anyway_limited",
            "",
        ] {
            assert!(!is_hold_code(code), "{code}");
        }
        assert_eq!(
            FailureClass::from_wire("ours").try_class(),
            TryClass::NotATry
        );
        assert_eq!(
            FailureClass::from_wire("bir").try_class(),
            TryClass::Counted
        );
        assert_eq!(
            FailureClass::from_wire("user_network").try_class(),
            TryClass::Counted
        );
        assert_eq!(
            FailureClass::from_wire("something_new"),
            FailureClass::Bir,
            "an unknown class is bir"
        );
        assert!(TryClass::Counted.counts() && TryClass::NoService.counts());
        assert!(
            !TryClass::NotATry.counts()
                && !TryClass::Refused.counts()
                && !TryClass::MaybeSent.counts()
        );
    }

    #[test]
    fn a_refusal_or_an_unknown_outcome_is_never_retried() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(1);
        for class in [TryClass::Refused, TryClass::MaybeSent] {
            assert_eq!(
                plan_retry(2, class, true, t0(), None, t0(), &policy, &mut rng),
                None
            );
        }
    }

    #[test]
    fn only_a_counted_failure_moves_the_schedule() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(11);
        let not_a_try = plan_retry(
            3,
            TryClass::NotATry,
            true,
            t0(),
            None,
            t0(),
            &policy,
            &mut rng,
        )
        .unwrap();
        assert_eq!(
            (not_a_try.counted, not_a_try.failures, not_a_try.slowed),
            (false, 3, false)
        );
        assert!(
            not_a_try.at <= t0() + Duration::seconds(450),
            "the first wait, not the fourth"
        );
        let counted = plan_retry(
            3,
            TryClass::Counted,
            false,
            t0(),
            None,
            t0(),
            &policy,
            &mut rng,
        )
        .unwrap();
        assert_eq!((counted.counted, counted.failures), (true, 4));
        assert!(counted.at >= t0() + Duration::seconds(3600));
        let no_service = plan_retry(
            0,
            TryClass::NoService,
            true,
            t0(),
            None,
            t0(),
            &policy,
            &mut rng,
        )
        .unwrap();
        assert_eq!(
            (no_service.counted, no_service.failures, no_service.slowed),
            (true, 1, false)
        );
        let hold = t0() + Duration::hours(3);
        let held = plan_retry(
            0,
            TryClass::NotATry,
            false,
            t0(),
            Some(hold),
            t0(),
            &policy,
            &mut rng,
        )
        .unwrap();
        assert!(held.at >= hold && held.at <= hold + Duration::minutes(90));
        assert_eq!(held.failures, 0);
    }

    // -------------------------------------------------------- device guard

    #[test]
    fn the_guard_needs_online_and_three_failed_tries() {
        let mut tries = DeviceTries::default();
        assert!(!guard_trips(Some(SignState::Online), &tries));
        tries.record(false);
        assert!(!guard_trips(Some(SignState::Online), &tries));
        tries.record(false);
        assert!(
            guard_trips(Some(SignState::Online), &tries),
            "a third failure trips it"
        );
        for sign in [
            None,
            Some(SignState::Busy),
            Some(SignState::Offline),
            Some(SignState::NoRecentNews),
        ] {
            assert!(!guard_trips(sign, &tries), "{sign:?}");
        }
        tries.record(false);
        assert!(guard_on(Some(SignState::Online), &tries));
        assert_eq!(tries.to_setting(), "xxx");
        tries.record(true);
        assert!(
            !guard_on(Some(SignState::Online), &tries),
            "a landing clears it"
        );
        assert_eq!(DeviceTries::from_setting(&tries.to_setting()), tries);
        assert_eq!(DeviceTries::from_setting("oooxxx?").to_setting(), "xxx");
    }

    #[test]
    fn a_tripped_guard_waits_at_least_the_repeat() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(12);
        for failures in 0..3 {
            let retry = plan_retry(
                failures,
                TryClass::Counted,
                true,
                t0(),
                None,
                t0(),
                &policy,
                &mut rng,
            )
            .unwrap();
            assert!(retry.slowed);
            assert!(
                retry.at >= t0() + Duration::seconds(3600),
                "2 h x 0.5 at least"
            );
            assert!(retry.at <= t0() + Duration::seconds(10_800));
        }
    }

    // ------------------------------------------------------------ the line

    fn day(d: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(2026, 11, d)
    }

    fn row(due: u32, queued_min: i64, ready_min: Option<i64>) -> Waiting {
        Waiting {
            due: day(due),
            queued_at: t0() - Duration::days(2) + Duration::minutes(queued_min),
            ready_at: ready_min.map(|m| t0() + Duration::minutes(m)),
            blocked: false,
        }
    }

    #[test]
    fn the_head_is_due_soonest_then_queued_first() {
        let policy = QueuePolicy::default();
        let rows = [
            row(25, 0, None),
            row(10, 30, None),
            row(10, 5, None),
            row(5, 0, Some(10)),
        ];
        let line = line_up(&rows, t0(), &policy);
        assert_eq!(line.head, Some(2), "due the 10th, queued before the other");
        assert_eq!(line.stamps[2], Some(t0()));
        assert_eq!(line.stamps[1], Some(t0() + Duration::minutes(1)));
        assert_eq!(line.stamps[0], Some(t0() + Duration::minutes(2)));
        assert_eq!(
            line.stamps[3],
            Some(t0() + Duration::minutes(10)),
            "its own time is later still"
        );
        assert_eq!(line.behind, [true, true, false, false]);
        let undated = [
            Waiting {
                due: None,
                ..row(1, 0, None)
            },
            row(30, 10, None),
        ];
        assert_eq!(
            pick_head(&undated, t0(), &policy),
            Some(1),
            "a known due date goes first"
        );
    }

    #[test]
    fn a_return_that_failed_steps_aside() {
        let policy = QueuePolicy::default();
        let mut rows = vec![row(10, 0, None), row(20, 0, None), row(30, 0, None)];
        let mut rng = SeededRng::new(4);
        let mut sent = Vec::new();
        for minute in 0..3 {
            let now = t0() + Duration::minutes(minute);
            let head = pick_head(&rows, now, &policy).expect("someone is due");
            sent.push(head);
            let retry = plan_retry(
                0,
                TryClass::Counted,
                false,
                now,
                None,
                now,
                &policy,
                &mut rng,
            )
            .unwrap();
            rows[head].ready_at = Some(retry.at);
        }
        assert_eq!(sent, [0, 1, 2], "each failure lets the next one go");
        assert_eq!(pick_head(&rows, t0() + Duration::minutes(3), &policy), None);
    }

    #[test]
    fn blocked_rows_are_never_the_head_and_keep_no_stamp() {
        let policy = QueuePolicy::default();
        let mut claimed = row(1, 0, None);
        claimed.blocked = true;
        let rows = [claimed, row(9, 0, Some(-5))];
        let line = line_up(&rows, t0(), &policy);
        assert_eq!(line.head, Some(1));
        assert_eq!(line.stamps[0], None);
        assert!(!line.behind[0]);
        assert_eq!(pick_head(&rows[..1], t0(), &policy), None);
    }

    /// Property: whatever the rows, there is at most one head; every
    /// unblocked row has a stamp; no two stamps are closer than the gap; and
    /// no stamp is before the row's own (clamped) time.
    #[test]
    fn property_stamps_are_real_and_a_minute_apart() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(2024);
        for _ in 0..2_000 {
            let n = (rng.next_u64() % 8) as usize;
            let rows: Vec<Waiting> = (0..n)
                .map(|_| Waiting {
                    due: day((rng.next_u64() % 28) as u32 + 1)
                        .filter(|_| !rng.next_u64().is_multiple_of(5)),
                    queued_at: t0() - Duration::minutes((rng.next_u64() % 10_000) as i64),
                    ready_at: match rng.next_u64() % 4 {
                        0 => None,
                        1 => Some(t0() - Duration::minutes((rng.next_u64() % 100) as i64)),
                        2 => Some(t0() + Duration::seconds((rng.next_u64() % 200) as i64)),
                        _ => Some(t0() + Duration::days((rng.next_u64() % 400) as i64)),
                    },
                    blocked: rng.next_u64().is_multiple_of(6),
                })
                .collect();
            let line = line_up(&rows, t0(), &policy);
            let mut stamps: Vec<DateTime<Utc>> = Vec::new();
            for (i, row) in rows.iter().enumerate() {
                match line.stamps[i] {
                    None => assert!(row.blocked),
                    Some(stamp) => {
                        assert!(!row.blocked);
                        assert!(
                            stamp >= line.ready[i].min(t0()),
                            "never before its own time"
                        );
                        assert!(line.ready[i] <= t0() + policy.longest_wait());
                        stamps.push(stamp);
                    }
                }
            }
            stamps.sort();
            for pair in stamps.windows(2) {
                assert!(
                    pair[1] - pair[0] >= Duration::seconds(BEHIND_GAP_S),
                    "{pair:?}"
                );
            }
            let due_now = stamps.iter().filter(|stamp| **stamp <= t0()).count();
            assert!(
                due_now <= 1,
                "an older build would find at most one row due"
            );
            if let Some(head) = line.head {
                assert!(!rows[head].blocked && line.ready[head] <= t0());
                assert_eq!(line.stamps[head], Some(t0()));
            }
        }
    }

    #[test]
    fn a_clock_that_went_back_cannot_strand_a_return() {
        let policy = QueuePolicy::default();
        let far = row(10, 0, Some(60 * 24 * 30));
        let line = line_up(&[far], t0(), &policy);
        assert_eq!(line.ready[0], t0() + policy.longest_wait());
        assert_eq!(policy.longest_wait(), Duration::seconds(10_800));
    }

    // ---------------------------------------------------- notices, mode

    #[test]
    fn a_notice_is_due_every_24_hours_of_wall_time() {
        let policy = QueuePolicy::default();
        assert_eq!(notices_due(t0(), t0() - Duration::hours(1), &policy), 0);
        assert_eq!(notices_due(t0(), t0() + Duration::hours(23), &policy), 0);
        assert_eq!(notices_due(t0(), t0() + Duration::hours(24), &policy), 1);
        assert_eq!(notices_due(t0(), t0() + Duration::hours(49), &policy), 2);
    }

    #[test]
    fn only_paused_pauses() {
        assert_eq!(QueueMode::from_setting(Some("paused")), QueueMode::Paused);
        assert_eq!(
            QueueMode::from_setting(Some(" paused\n")),
            QueueMode::Paused
        );
        for other in [None, Some("auto"), Some(""), Some("PAUSED"), Some("off")] {
            assert_eq!(QueueMode::from_setting(other), QueueMode::Auto, "{other:?}");
        }
        assert_eq!(QueueMode::Paused.as_setting(), "paused");
        assert_eq!(QueueMode::Auto.as_setting(), "auto");
    }

    #[test]
    fn the_queue_state_is_absent_until_it_holds_something() {
        let state = QueueState::default();
        assert!(state.is_empty());
        assert_eq!(serde_json::to_value(&state).unwrap(), json!({}));
        let read: QueueState = serde_json::from_value(json!({})).unwrap();
        assert_eq!(read, state);
        let mut state = QueueState::default();
        state.after(&Retry {
            at: t0(),
            counted: true,
            failures: 2,
            slowed: true,
        });
        assert_eq!(state.next_try(), Some(t0()));
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            json!({"failures": 2, "next_try_at": "2026-11-11T07:00:00+00:00", "slowed": true})
        );
    }

    // ------------------------------------------------------------ the gate

    fn seen(header: &str) -> SeenSign {
        SeenSign {
            sign: Sign::from_header(header).unwrap(),
            seen_at: t0(),
        }
    }

    #[test]
    fn the_gate_follows_the_sign_and_turns() {
        let policy = QueuePolicy::default();
        let now = t0() + Duration::seconds(30);
        assert_eq!(
            gate(None, now, &policy, None),
            Gate::Open,
            "no sign: as without one"
        );
        let cases = [
            ("v=1, seq=1, state=online", None, Gate::Open),
            ("v=1, seq=1, state=no_recent_news", None, Gate::Open),
            ("v=1, seq=1, state=offline", None, Gate::Sleep),
            ("v=1, seq=1, state=offline", Some(Turn::Scout), Gate::Open),
            ("v=1, seq=1, state=offline", Some(Turn::Admit), Gate::Sleep),
            (
                "v=1, seq=1, state=online, relay=paused",
                Some(Turn::Scout),
                Gate::Sleep,
            ),
            ("v=1, seq=1, state=busy", None, Gate::NeedTurn),
            ("v=1, seq=1, state=online, admit=?1", None, Gate::NeedTurn),
            ("v=1, seq=1, state=busy", Some(Turn::Admit), Gate::Open),
        ];
        for (header, turn, want) in cases {
            assert_eq!(
                gate(Some(&seen(header)), now, &policy, turn),
                want,
                "{header} {turn:?}"
            );
        }
        let until = t0() + Duration::minutes(10);
        let waiting = seen(&format!(
            "v=1, seq=2, state=online, until={}",
            until.timestamp()
        ));
        assert_eq!(
            gate(Some(&waiting), now, &policy, None),
            Gate::WaitUntil(until)
        );
        assert_eq!(
            gate(Some(&waiting), now, &policy, Some(Turn::Admit)),
            Gate::Open,
            "a turn wins"
        );
        assert_eq!(gate(Some(&waiting), until, &policy, None), Gate::Open);
        // A stale sign is no sign.
        let stale = t0() + Duration::seconds(i64::from(policy.stale_after_s));
        assert_eq!(
            gate(
                Some(&seen("v=1, seq=1, state=offline")),
                stale,
                &policy,
                None
            ),
            Gate::Open
        );
    }
}
