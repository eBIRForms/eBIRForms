//! The smart send queue's rules, pure: no I/O, no clock, no global
//! randomness. The worker (`background_cron`) passes the time and a random
//! source in, so every rule here is deterministic under a seeded
//! [`SeededRng`] and an injected clock.
//!
//! Every connection toward the BIR is opened by the person's own app, from
//! their own internet address. So every rule here is about sending less:
//!
//! - **Waits.** Every wait follows one ladder: 5 min, 30 min, 1 h, 2 h
//!   (`waits_s`), then `repeat_s` (2 h) every time after, each multiplied
//!   by a random factor uniform in `[1 - spread, 1 + spread]` (0.5 to 1.5
//!   by default), so queues that failed together do not come back
//!   together ([`next_try_at`]).
//! - **Clamps.** The server may tune the numbers ([`QueuePolicy::from_json`])
//!   only toward sending less: a wait below its default, a repeat below
//!   2 h, a spread outside 0.2 to 0.5, any value missing or out of range,
//!   reads as the default. One return at a time ([`PARALLEL`]) is fixed.
//! - **The way in waits as one** ([`Blame::Path`], [`PathBackoff`]). A try
//!   that never got past the BIR's front desk says nothing about the
//!   return: it moves a queue-wide back-off up the ladder, no return starts
//!   before it, and when it passes one return (due soonest, a different one
//!   each time) goes. So an outage costs the same number of tries with 1
//!   or 20 returns queued. A landing anywhere clears it.
//! - **A return's own failure moves only it** ([`Blame::Upload`],
//!   [`Blame::Return`], [`Blame::NoService`]): its own ladder, and it steps
//!   aside so the next return is the head. Three in a row of the same kind
//!   about the return itself (a refused login, no upload service for the
//!   TIN, the same validation refusal) stop its automatic tries until the
//!   person presses Retry ([`STUCK_AFTER`]).
//!   The same stop follows a session that Buwiz keeps refusing with the same
//!   4xx answer (a revoked device key): it is not a try ([`TryClass::Answered`]),
//!   so it counts for nothing else.
//! - **One at a time.** [`line_up_after`] picks the one return that may go
//!   now (due soonest first, across every TIN) and gives every other a real
//!   next-try time at least a minute apart and never before the queue-wide
//!   back-off, so a build that knows none of this, sharing the database,
//!   does not send them all at once.
//! - **Device guard.** When the sign says returns get in and this device's
//!   last three tries that reached for the BIR all failed, the next wait is
//!   at least `repeat_s` and the person is told it may be their own
//!   connection ([`guard_trips`]).
//! - **What counts** ([`TryClass`]): a hold, being signed out, Buwiz being
//!   unreachable or answering 5xx or 429, and a failure of class `ours`
//!   before any stream opened are never tries; they climb a ladder of their
//!   own (`not_tried`), so nothing loops at a short fixed interval. An
//!   `unknown` outcome is never resent by itself; a TIN refusal goes back
//!   to Draft. Neither gets a retry ([`plan`]).
//! - **A person's Retry** is at most once per 30 minutes and three times a
//!   day per return ([`QueueState::retry_allowed_at`]), and a cancelled
//!   return queued again within 6 hours keeps its place on the ladder
//!   ([`QueueState::requeued`]).
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
/// The longest any time the server names (`wait_until`, `Retry-After`) is
/// believed: anything further reads as this far.
pub const MAX_SERVER_WAIT_S: i64 = 7 * 86_400;
/// After this many failures in a row of the same kind about the return
/// itself, its automatic tries stop until the person presses Retry.
pub const STUCK_AFTER: u32 = 3;
/// A person's Retry on one return: at most once per this many seconds…
pub const RETRY_GAP_S: i64 = 1800;
/// …and this many times in any 24 hours.
pub const RETRIES_PER_DAY: usize = 3;
/// A return cancelled and queued again within this long keeps its place on
/// the ladder (failures and next try).
pub const REQUEUE_KEEP_S: i64 = 6 * 3600;

/// The queue numbers (public contract, `docs/bir-uptime.md` in `buwiz-com/buwiz-com`), always within their clamps.
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

    /// The longest any one wait of the ladder can be, spread included.
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
    /// The `n`-th not-a-try in a row, never shorter than the wait the
    /// return's own `failures` already earned: nothing class-driven
    /// shortens a wait.
    NotATryAgain { n: u32, failures: u32 },
}

/// `until`, read as at most [`MAX_SERVER_WAIT_S`] after `now`.
pub fn capped_wait_until(until: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    until.min(now + Duration::seconds(MAX_SERVER_WAIT_S))
}

/// When a return may try again. The wait starts when the try ended
/// (`ended_at`); it is the base wait for `wait` times the spread factor.
/// When the server's `wait_until` is later than that, the try goes at
/// `wait_until` plus a random share (0 to `spread`) of the time that was
/// left until it (`wait_until - now`): never before `wait_until`. A
/// `wait_until` more than 7 days ahead reads as 7 days.
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
        Wait::NotATryAgain { n, failures } => {
            let own = if failures > 0 { policy.wait_s(failures) } else { 0 };
            policy.wait_s(n).max(own)
        }
    };
    let at = ended_at + seconds_f64(f64::from(base) * spread_factor(rng, policy.spread));
    match wait_until.map(|until| capped_wait_until(until, now)) {
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
    /// Buwiz ANSWERED a session open with a refusal that is not a hold, a
    /// 429, being signed out or a TIN refusal (a 4xx, say a device key the
    /// server revoked). Not a try, like [`Self::NotATry`]: no count, no
    /// path back-off, no device guard. But the same refusal three times in
    /// a row stops the return until the person presses Retry
    /// ([`STUCK_AFTER`]).
    Answered,
}

impl TryClass {
    /// Whether the failure moves the return along its schedule.
    pub fn counts(self) -> bool {
        matches!(self, Self::Counted | Self::NoService)
    }
}

/// The server's `class` on a failure: whose side it failed on. An unknown
/// word reads as `bir` (public contract).
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

/// The `429` codes that say "not now" about Buwiz's own limits: nothing
/// reached the BIR, so never a try; the answer's `Retry-After` is the wait.
pub const LIMIT_CODES: [&str; 3] = ["rate_limited", "quota_exceeded", "try_anyway_limited"];

pub fn is_limit_code(code: &str) -> bool {
    LIMIT_CODES.contains(&code)
}

/// The code with which Buwiz says the sealed return does not match its
/// ticket. Final, like a TIN refusal: the return goes back to Draft.
pub const RETURN_MISMATCH: &str = "return_mismatch";

/// A reason that ends in a code no retry can change: a TIN refusal
/// (`tin_*`, `account_banned`) or [`RETURN_MISMATCH`]. Such a reason sends
/// the return back to Draft whatever class the server gave it.
pub fn is_final_reason(reason: &str) -> bool {
    crate::tin_refusal::TinRefusal::from_reason(reason).is_some() || is_mismatch_reason(reason)
}

/// The reason ends in `(return_mismatch)`.
pub fn is_mismatch_reason(reason: &str) -> bool {
    reason
        .trim()
        .strip_suffix(')')
        .and_then(|inner| inner.rsplit_once('('))
        .is_some_and(|(message, code)| {
            code == RETURN_MISMATCH && (message.is_empty() || message.ends_with(char::is_whitespace))
        })
}

/// Where a counted try failed, which decides who waits for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blame {
    /// The try never got past the BIR's front desk (every connection it
    /// opened toward the BIR was refused or timed out, no upload stream
    /// opened, or the front desk failed). It says nothing about this
    /// return: the whole queue backs off ([`PathBackoff`]), and the device
    /// guard hears it.
    Path,
    /// An upload stream opened and the send broke after it (a write, the
    /// session). This return's own ladder; the device guard hears it.
    Upload,
    /// About this return: a refused login, a validation refusal, Buwiz's
    /// answer to its ticket. Its own ladder; not the device guard. Three
    /// of the same kind in a row stop its automatic tries.
    Return,
    /// The BIR has no upload service for this TIN: like `Return`.
    NoService,
}

impl Blame {
    pub fn class(self) -> TryClass {
        match self {
            Self::NoService => TryClass::NoService,
            _ => TryClass::Counted,
        }
    }

    /// Whether the device guard hears this failure: only those that say
    /// something about this device's way to the BIR.
    pub fn heard_by_guard(self) -> bool {
        matches!(self, Self::Path | Self::Upload)
    }
}

/// When a return that did not land goes again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retry {
    pub at: DateTime<Utc>,
    /// The failure counts against the return: its failure count is now
    /// `failures` and its stored `submission_attempts` goes up by one. Not
    /// so for a try that was not one, nor for a [`Blame::Path`] failure,
    /// which says nothing about the return (see [`Retry::was_a_try`]).
    pub counted: bool,
    pub failures: u32,
    /// The device guard set this wait.
    pub slowed: bool,
    /// When the try ended (a clock that later reads earlier went back).
    pub ended_at: DateTime<Utc>,
    /// The server's time, never tried before (already capped to 7 days).
    pub wait_until: Option<DateTime<Utc>>,
    /// Not-a-try in a row, after this one (0 after a counted try).
    pub not_tried: u32,
    /// Where it failed, for a counted try.
    pub blame: Option<Blame>,
    /// For `Return` and `NoService`, and for an [`Ending::Answered`]: what
    /// kind of failure (its reason), to tell three of the same kind in a
    /// row.
    pub kind: Option<String>,
}

impl Retry {
    /// Whether this was a try that reached for the BIR, counted against
    /// the return or not: a path failure is one (the log says so, and the
    /// way in is blamed), but it does not count against the return.
    pub fn was_a_try(&self) -> bool {
        self.counted || self.blame == Some(Blame::Path)
    }
}

/// How a try that did not land ended, for [`plan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending<'a> {
    /// Not a try: nothing reached for the BIR. `wait_until` is the
    /// server's time, if it gave one.
    NotATry { wait_until: Option<DateTime<Utc>> },
    /// A counted try that failed, with where and why.
    Failed { blame: Blame, reason: &'a str },
    /// Not a try, but Buwiz answered with a refusal ([`TryClass::Answered`]):
    /// the same `reason` three times in a row stops the return.
    Answered { reason: &'a str },
}

/// The retry of a return in `state` whose try ended at `ended_at` as
/// `ending`. A `Path` failure moves `path`, the queue-wide back-off, and
/// the return waits for it; every other failure moves only the return's
/// own ladder. `slowed_down` is the device guard's answer
/// ([`guard_trips`]); it lengthens only failures the guard hears.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    state: &QueueState,
    ending: Ending<'_>,
    slowed_down: bool,
    ended_at: DateTime<Utc>,
    now: DateTime<Utc>,
    path: &mut PathBackoff,
    policy: &QueuePolicy,
    rng: &mut dyn QueueRng,
) -> Retry {
    match ending {
        Ending::NotATry { wait_until } => {
            let n = state.not_tried.saturating_add(1);
            let wait = Wait::NotATryAgain {
                n,
                failures: state.failures,
            };
            let wait_until = wait_until.map(|until| capped_wait_until(until, now));
            Retry {
                at: next_try_at(wait, ended_at, wait_until, now, policy, rng),
                counted: false,
                failures: state.failures,
                slowed: false,
                ended_at,
                wait_until,
                not_tried: n,
                blame: None,
                kind: None,
            }
        }
        Ending::Answered { reason } => {
            let mut retry = plan(
                state,
                Ending::NotATry { wait_until: None },
                slowed_down,
                ended_at,
                now,
                path,
                policy,
                rng,
            );
            retry.kind = Some(failure_kind(reason));
            retry
        }
        Ending::Failed {
            blame: Blame::Path,
            ..
        } => {
            let at = path.record(ended_at, slowed_down, policy, rng);
            Retry {
                at,
                counted: false,
                failures: state.failures,
                slowed: slowed_down,
                ended_at,
                wait_until: None,
                not_tried: 0,
                blame: Some(Blame::Path),
                kind: None,
            }
        }
        Ending::Failed { blame, reason } => {
            let failures = state.failures.saturating_add(1);
            let slowed = slowed_down && blame.heard_by_guard();
            Retry {
                at: next_try_at(
                    Wait::Failed { failures, slowed },
                    ended_at,
                    None,
                    now,
                    policy,
                    rng,
                ),
                counted: true,
                failures,
                slowed,
                ended_at,
                wait_until: None,
                not_tried: 0,
                blame: Some(blame),
                kind: matches!(blame, Blame::Return | Blame::NoService)
                    .then(|| failure_kind(reason)),
            }
        }
    }
}

/// The kind of a failure about the return, from its reason: the text
/// without the queue's own prefix, so the same refusal reads the same.
pub fn failure_kind(reason: &str) -> String {
    let mut rest = reason.trim();
    for prefix in [
        "Needs attention: ",
        "Not sent (nothing reached BIR): ",
        "Submission session failed before upload (no PUT attempted): ",
    ] {
        if let Some(after) = rest.strip_prefix(prefix) {
            rest = after.trim();
        }
    }
    rest.chars().take(300).collect()
}

/// The retry for a try of `class` that ended at `ended_at`, for a return
/// that had `failures_before` counted failures, as before the queue knew
/// where a try failed: a counted failure is the return's own (`Upload`).
/// `None` for a refusal or an unknown outcome: neither is ever retried by
/// the queue.
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
    let ending = match class {
        TryClass::Refused | TryClass::MaybeSent => return None,
        TryClass::NotATry | TryClass::Answered => Ending::NotATry { wait_until },
        TryClass::Counted => Ending::Failed {
            blame: Blame::Upload,
            reason: "",
        },
        TryClass::NoService => Ending::Failed {
            blame: Blame::NoService,
            reason: "",
        },
    };
    let state = QueueState {
        failures: failures_before,
        ..QueueState::default()
    };
    Some(plan(
        &state,
        ending,
        slowed_down,
        ended_at,
        now,
        &mut PathBackoff::default(),
        policy,
        rng,
    ))
}

/// A counted failure at `now` on the default numbers and the OS random
/// source: what a caller without the worker's policy (a test, a tool)
/// records.
pub fn default_retry(failures_before: u32, now: DateTime<Utc>) -> Retry {
    let state = QueueState {
        failures: failures_before,
        ..QueueState::default()
    };
    plan(
        &state,
        Ending::Failed {
            blame: Blame::Upload,
            reason: "",
        },
        false,
        now,
        now,
        &mut PathBackoff::default(),
        &QueuePolicy::default(),
        &mut OsRng,
    )
}

// ------------------------------------------------- the way in, queue-wide

/// The settings key of the queue-wide [`PathBackoff`].
pub const PATH_SETTING: &str = "smart_queue_path";

/// The queue-wide back-off after tries that never got past the BIR's front
/// desk. Stored in the settings table, so a restart keeps it; every queued
/// row's `next_retry_at` is also stamped at or after it, so an older build
/// sharing the database waits too.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathBackoff {
    /// Path failures in a row.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failures: u32,
    /// No return starts before this (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_try_at: Option<String>,
    /// When the last path failure ended (a clock that reads earlier went
    /// back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// When this run of path failures began: returns moved to the back of
    /// the line since then go after the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

impl PathBackoff {
    /// The stored back-off; anything unreadable is none.
    pub fn from_setting(stored: Option<&str>) -> Self {
        stored
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default()
    }

    pub fn to_setting(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned())
    }

    pub fn is_clear(&self) -> bool {
        *self == Self::default()
    }

    /// A path failure that ended at `ended`: one more on the ladder, and no
    /// return before `ended + wait(failures) × spread`. The answer is that
    /// time.
    pub fn record(
        &mut self,
        ended: DateTime<Utc>,
        slowed: bool,
        policy: &QueuePolicy,
        rng: &mut dyn QueueRng,
    ) -> DateTime<Utc> {
        self.failures = self.failures.saturating_add(1);
        let at = next_try_at(
            Wait::Failed {
                failures: self.failures,
                slowed,
            },
            ended,
            None,
            ended,
            policy,
            rng,
        );
        self.next_try_at = Some(at.to_rfc3339());
        self.ended_at = Some(ended.to_rfc3339());
        if self.since.is_none() {
            self.since = Some(ended.to_rfc3339());
        }
        at
    }

    /// No return starts before this, as of `now`; `None` when it has passed
    /// or there is none. A clock that went back (it reads earlier than the
    /// failure ended) keeps the same wait from now, never more.
    pub fn floor(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let next = self.next_try_at.as_deref().and_then(parse_time)?;
        let next = match self.ended_at.as_deref().and_then(parse_time) {
            Some(ended) if now < ended => now + (next - ended).max(Duration::zero()),
            _ => next,
        };
        (next > now).then_some(next)
    }

    /// A clock that reads earlier than when the last path failure ended
    /// went back: move every stored time back by the same amount, once, so
    /// the wait that was owed is owed from now and no more. `true` when it
    /// changed.
    pub fn reanchor(&mut self, now: DateTime<Utc>) -> bool {
        let Some(ended) = self.ended_at.as_deref().and_then(parse_time) else {
            return false;
        };
        if now >= ended {
            return false;
        }
        let shift = |stored: &mut Option<String>| {
            if let Some(at) = stored.as_deref().and_then(parse_time) {
                *stored = Some((at - (ended - now)).to_rfc3339());
            }
        };
        shift(&mut self.next_try_at);
        shift(&mut self.since);
        self.ended_at = Some(now.to_rfc3339());
        true
    }

    /// When this run of path failures began.
    pub fn since(&self) -> Option<DateTime<Utc>> {
        self.since.as_deref().and_then(parse_time)
    }

    /// A landing anywhere: no back-off.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
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
    /// Claimed (being sent, or with an unknown outcome), or stopped until
    /// the person presses Retry: never the head, never restamped.
    pub blocked: bool,
    /// Moved to the back of the line by a path failure in the current run
    /// of them, at this time: it goes after every return not moved.
    pub rotated: Option<DateTime<Utc>>,
    /// A person confirmed "try anyway": it may go before the queue-wide
    /// back-off, once, and before the rest.
    pub try_anyway: bool,
}

/// The queue's order at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineUp {
    /// The one return that may go now.
    pub head: Option<usize>,
    /// Each row's own next-try time, never before the queue-wide back-off
    /// (but for a row with "try anyway"). Blocked rows keep theirs.
    pub ready: Vec<DateTime<Utc>>,
    /// The next-try time to store on each row (`None` for a blocked row):
    /// now for the head, and for every other row its own time but at least
    /// [`BEHIND_GAP_S`] after the row before it in line. Never null.
    pub stamps: Vec<Option<DateTime<Utc>>>,
    /// Waiting its turn: its own time is passed, or it was pushed back to
    /// keep the gap, and it is not the head.
    pub behind: Vec<bool>,
}

/// [`line_up_after`] with no queue-wide back-off.
pub fn line_up(rows: &[Waiting], now: DateTime<Utc>, policy: &QueuePolicy) -> LineUp {
    line_up_after(rows, now, policy, None)
}

/// Order the queue: the head is the eligible row (not blocked, its own
/// time passed, and the queue-wide back-off `not_before` passed) due
/// soonest (BIR due date, then the time it was queued), a row moved to the
/// back by a path failure after every row not moved; the rest follow, the
/// eligible ones in the same order, then the others by their own time.
/// Every row's stamp keeps a gap after the one before, and none is before
/// `not_before`. Stored times are never cut: a clock that went back is
/// read from when the try ended ([`QueueState::ready_at`]).
pub fn line_up_after(
    rows: &[Waiting],
    now: DateTime<Utc>,
    _policy: &QueuePolicy,
    not_before: Option<DateTime<Utc>>,
) -> LineUp {
    let not_before = not_before.filter(|floor| *floor > now);
    let ready: Vec<DateTime<Utc>> = rows
        .iter()
        .map(|row| {
            let own = row.ready_at.unwrap_or(now);
            match not_before {
                Some(floor) if !row.blocked && !row.try_anyway => own.max(floor),
                _ => own,
            }
        })
        .collect();
    let eligible = |i: usize| !rows[i].blocked && ready[i] <= now;
    let by_due = |i: &usize| {
        (
            !rows[*i].try_anyway,
            rows[*i].rotated.unwrap_or(DateTime::<Utc>::MIN_UTC),
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
    /// When its last try ended. A clock that reads earlier went back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// The server's time it waits for (never tried before it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_until: Option<String>,
    /// Not-a-try in a row: its own ladder, so nothing loops at a short
    /// fixed interval. A counted try or a landing resets it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub not_tried: u32,
    /// Times in a row it was the head and was not tried (the worker could
    /// not take it): its own ladder, so one bad row never starves the rest.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skips: u32,
    /// Moved to the back of the line by a path failure (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotated_at: Option<String>,
    /// The kind of its last failure about the return itself, and how many
    /// of that kind came in a row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub same_kind: u32,
    /// Stopped: it keeps being refused for this reason, and only the
    /// person's Retry tries it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stuck: Option<String>,
    /// The person's Retry presses in the last 24 hours (RFC 3339).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retries: Vec<String>,
    /// The person confirmed "try anyway": the next try may go before the
    /// queue-wide back-off, once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub try_anyway: bool,
    /// When it went back to Draft (cancelled), for a requeue within 6 hours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_at: Option<String>,
    /// The fingerprint it was queued with, so a requeue of the same content
    /// keeps a stop and one of changed content clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// When the queue first saw it waiting, for a row with no queue
    /// authorization time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_at: Option<String>,
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

    /// When it may go, as of `now`: its own next try, never before its
    /// stored `wait_until`. A clock that went back (it reads earlier than
    /// when the try ended) keeps the same waits from now; no stored time is
    /// cut. A time stored without when the try ended (an older build) is
    /// read as at most 7 days from now. `None` when it has no own time.
    pub fn ready_at(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let next = self.next_try()?;
        let until = self.wait_until.as_deref().and_then(parse_time);
        Some(match self.ended_at.as_deref().and_then(parse_time) {
            Some(ended) if now < ended => {
                let from_now = |at: DateTime<Utc>| now + (at - ended).max(Duration::zero());
                let own = from_now(next);
                until.map_or(own, |until| own.max(from_now(until)))
            }
            Some(_) => until.map_or(next, |until| next.max(until)),
            None => next
                .max(until.unwrap_or(next))
                .min(now + Duration::seconds(MAX_SERVER_WAIT_S)),
        })
    }

    /// A clock that reads earlier than when its last try ended went back:
    /// move the times that try set back by the same amount, once, so the wait
    /// it owed is owed from now and no more (and it is not stranded until
    /// the clock catches up). `true` when it changed; `next_retry_at` is
    /// the row's stored time, moved the same way.
    pub fn reanchor(&mut self, now: DateTime<Utc>, next_retry_at: &mut Option<String>) -> bool {
        let Some(ended) = self.ended_at.as_deref().and_then(parse_time) else {
            return false;
        };
        if now >= ended {
            return false;
        }
        let back = ended - now;
        let shift = |stored: &mut Option<String>| {
            if let Some(at) = stored.as_deref().and_then(parse_time) {
                *stored = Some((at - back).to_rfc3339());
            }
        };
        shift(&mut self.next_try_at);
        shift(&mut self.wait_until);
        shift(&mut self.rotated_at);
        shift(next_retry_at);
        self.ended_at = Some(now.to_rfc3339());
        true
    }

    /// Stopped until the person presses Retry.
    pub fn is_stuck(&self) -> bool {
        self.stuck.is_some()
    }

    /// When it was moved to the back of the line.
    pub fn rotated(&self) -> Option<DateTime<Utc>> {
        self.rotated_at.as_deref().and_then(parse_time)
    }

    /// After a try that did not land.
    pub fn after(&mut self, retry: &Retry) {
        self.failures = retry.failures;
        self.next_try_at = Some(retry.at.to_rfc3339());
        self.slowed = retry.slowed;
        self.ended_at = Some(retry.ended_at.to_rfc3339());
        self.wait_until = retry.wait_until.map(|until| until.to_rfc3339());
        self.not_tried = retry.not_tried;
        self.skips = 0;
        self.try_anyway = false;
        match retry.blame {
            Some(Blame::Path) => {
                self.rotated_at = Some(retry.ended_at.to_rfc3339());
                self.kind = None;
                self.same_kind = 0;
            }
            Some(Blame::Upload) => {
                self.kind = None;
                self.same_kind = 0;
            }
            Some(Blame::Return | Blame::NoService) => self.count_kind(retry),
            // Not a try: a plain one leaves the run alone; a refusal Buwiz
            // answered (it carries its kind) counts in it.
            None if retry.kind.is_some() => self.count_kind(retry),
            None => {}
        }
    }

    /// One more failure of `retry.kind`: three of the same kind in a row
    /// stop the return.
    fn count_kind(&mut self, retry: &Retry) {
        if retry.kind.is_some() && retry.kind == self.kind {
            self.same_kind = self.same_kind.saturating_add(1);
        } else {
            self.kind = retry.kind.clone();
            self.same_kind = 1;
        }
        if self.same_kind >= STUCK_AFTER {
            self.stuck = self.kind.clone();
        }
    }

    /// The worker could not take it as the head: the next skip waits
    /// longer (the same ladder), from `now`.
    pub fn skipped(&mut self, now: DateTime<Utc>, policy: &QueuePolicy, rng: &mut dyn QueueRng) {
        self.skips = self.skips.saturating_add(1);
        let at = next_try_at(
            Wait::Failed {
                failures: self.skips,
                slowed: false,
            },
            now,
            None,
            now,
            policy,
            rng,
        );
        self.next_try_at = Some(at.to_rfc3339());
        self.ended_at = Some(now.to_rfc3339());
    }

    /// The person's Retry presses still counted at `now` (the last 24 h). A
    /// press stored "in the future" (the clock went back) reads as now.
    fn presses(&self, now: DateTime<Utc>) -> Vec<DateTime<Utc>> {
        let mut presses: Vec<DateTime<Utc>> = self
            .retries
            .iter()
            .filter_map(|at| parse_time(at))
            .filter(|at| *at > now - Duration::days(1))
            .map(|at| at.min(now))
            .collect();
        presses.sort();
        presses
    }

    /// When the person may press Retry on this return again: `None` now.
    /// At most once per [`RETRY_GAP_S`] and [`RETRIES_PER_DAY`] times in any
    /// 24 hours.
    pub fn retry_allowed_at(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let presses = self.presses(now);
        let after_last = presses
            .last()
            .map(|last| *last + Duration::seconds(RETRY_GAP_S));
        let after_day = (presses.len() >= RETRIES_PER_DAY)
            .then(|| presses[presses.len() - RETRIES_PER_DAY] + Duration::days(1));
        after_last
            .into_iter()
            .chain(after_day)
            .max()
            .filter(|at| *at > now)
    }

    /// The person pressed Retry at `now` (allowed: see
    /// [`Self::retry_allowed_at`]): it goes as soon as the line lets it; a
    /// stop ends. `try_anyway` lets it go before the queue-wide back-off,
    /// once. Its failure count stays.
    pub fn press_retry(&mut self, now: DateTime<Utc>, try_anyway: bool) {
        let mut kept: Vec<String> = self
            .presses(now)
            .into_iter()
            .map(|at| at.to_rfc3339())
            .collect();
        kept.push(now.to_rfc3339());
        self.retries = kept;
        self.next_try_at = Some(now.to_rfc3339());
        self.ended_at = Some(now.to_rfc3339());
        self.wait_until = None;
        self.stuck = None;
        self.kind = None;
        self.same_kind = 0;
        self.skips = 0;
        self.try_anyway = try_anyway;
    }

    /// It went back to Draft at `now` (cancelled, or rejected before a
    /// try): what the queue knows stays, so a requeue within 6 hours keeps
    /// its place on the ladder.
    pub fn left(&mut self, now: DateTime<Utc>, fingerprint: Option<&str>) {
        if self.is_empty() {
            return;
        }
        self.left_at = Some(now.to_rfc3339());
        self.fingerprint = fingerprint.map(str::to_owned);
        self.notices = 0;
        self.try_anyway = false;
        self.rotated_at = None;
        self.queued_at = None;
    }

    /// Queued again at `now` with `fingerprint`. Within
    /// [`REQUEUE_KEEP_S`] of leaving, it keeps its failures, its next try
    /// and its Retry presses; a stop stays when the content is the same.
    /// Later (or never left), it starts afresh but keeps its Retry presses.
    pub fn requeued(&self, now: DateTime<Utc>, fingerprint: Option<&str>) -> Self {
        let fresh = Self {
            retries: self
                .presses(now)
                .into_iter()
                .map(|at| at.to_rfc3339())
                .collect(),
            ..Self::default()
        };
        let Some(left) = self.left_at.as_deref().and_then(parse_time) else {
            return fresh;
        };
        if now < left || now - left > Duration::seconds(REQUEUE_KEEP_S) {
            return fresh;
        }
        let same = fingerprint.is_some() && fingerprint == self.fingerprint.as_deref();
        Self {
            failures: self.failures,
            next_try_at: self.next_try_at.clone(),
            slowed: self.slowed,
            ended_at: self.ended_at.clone(),
            wait_until: self.wait_until.clone(),
            not_tried: self.not_tried,
            kind: if same { self.kind.clone() } else { None },
            same_kind: if same { self.same_kind } else { 0 },
            stuck: if same { self.stuck.clone() } else { None },
            ..fresh
        }
    }

    /// The `next_retry_at` a requeue at `now` stores: now, or its kept own
    /// time when that is later.
    pub fn requeue_next_retry_at(&self, now: DateTime<Utc>) -> String {
        self.ready_at(now).unwrap_or(now).max(now).to_rfc3339()
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

/// The gate for the newest sign (the public contract's fixed rules). No sign, or one
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

    /// The numbers reach the waits: a policy the server lengthened is what
    /// a failed try waits, and one it tried to shorten is not.
    #[test]
    fn the_policy_in_use_decides_the_waits() {
        let longer = QueuePolicy::from_json(&json!({ "waits_s": [600, 3600, 7200, 21600], "repeat_s": 21600, "spread": 0.2 }));
        let shorter = QueuePolicy::from_json(&json!({ "waits_s": [60, 60, 60, 60], "repeat_s": 60, "spread": 0.5 }));
        let state = QueueState::default();
        let ending = Ending::Failed { blame: Blame::Upload, reason: "x" };
        let mut rng = SeededRng::new(5);
        for _ in 0..100 {
            let mut path = PathBackoff::default();
            let at = plan(&state, ending, false, t0(), t0(), &mut path, &longer, &mut rng).at;
            let wait = (at - t0()).num_seconds();
            assert!((480..=720).contains(&wait), "{wait}: 600 s at 0.8 to 1.2");
            let at = plan(&state, ending, false, t0(), t0(), &mut path, &shorter, &mut rng).at;
            assert!((at - t0()).num_seconds() >= 150, "never below the default 300 s x 0.5");
        }
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
            not_a_try.at >= t0() + Duration::seconds(1800),
            "never shorter than the wait the return's own failures earned (the fourth)"
        );
        assert!(not_a_try.not_tried == 1);
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
            rotated: None,
            try_anyway: false,
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
                    rotated: None,
                    try_anyway: false,
                })
                .collect();
            let floor = match rng.next_u64() % 3 {
                0 => None,
                1 => Some(t0() + Duration::minutes((rng.next_u64() % 300) as i64 + 1)),
                _ => Some(t0() - Duration::minutes(5)),
            };
            let line = line_up_after(&rows, t0(), &policy, floor);
            let mut stamps: Vec<(DateTime<Utc>, usize)> = Vec::new();
            for (i, row) in rows.iter().enumerate() {
                match line.stamps[i] {
                    None => assert!(row.blocked),
                    Some(stamp) => {
                        assert!(!row.blocked);
                        let own = row.ready_at.unwrap_or(t0());
                        if Some(i) != line.head {
                            assert!(stamp >= own, "never before its own time");
                        }
                        if let Some(floor) = floor.filter(|f| *f > t0()) {
                            assert!(stamp >= floor, "never before the back-off");
                            assert_eq!(line.head, None, "nobody starts before it");
                        }
                        stamps.push((stamp, i));
                    }
                }
            }
            // An older build sharing the database ticks once a minute and
            // sends every row whose stored time has passed. Replay that
            // over the next day against the stamps: never two in one tick,
            // and never a row before its own time.
            for minute in 0..(24 * 60) {
                let tick = t0() + Duration::minutes(minute);
                let due: Vec<usize> = stamps
                    .iter()
                    .filter(|(at, _)| *at <= tick && *at > tick - Duration::minutes(1))
                    .map(|(_, i)| *i)
                    .collect();
                assert!(due.len() <= 1, "{due:?} at minute {minute}");
            }
            if let Some(head) = line.head {
                assert!(!rows[head].blocked && line.ready[head] <= t0());
                assert_eq!(line.stamps[head], Some(t0()));
            }
        }
    }

    #[test]
    fn no_stored_time_is_cut_and_a_clock_that_went_back_keeps_the_same_waits() {
        let policy = QueuePolicy::default();
        // A server repeat of 6 hours, stored with when the try ended: read
        // as stored, whatever the policy of this process says.
        let ended = t0();
        let state = QueueState {
            next_try_at: Some((ended + Duration::hours(6)).to_rfc3339()),
            ended_at: Some(ended.to_rfc3339()),
            ..QueueState::default()
        };
        assert_eq!(
            state.ready_at(t0() + Duration::minutes(1)),
            Some(ended + Duration::hours(6))
        );
        let far = row(10, 0, Some(60 * 6));
        let line = line_up(&[far], t0(), &policy);
        assert_eq!(line.ready[0], t0() + Duration::hours(6), "not cut");
        // The clock went back a day: the same 6 hours, counted from now.
        let back = t0() - Duration::days(1);
        assert_eq!(state.ready_at(back), Some(back + Duration::hours(6)));
        // A stored wait_until is never undercut, and goes back with it.
        let held = QueueState {
            wait_until: Some((ended + Duration::hours(9)).to_rfc3339()),
            ..state.clone()
        };
        assert_eq!(held.ready_at(t0()), Some(ended + Duration::hours(9)));
        assert_eq!(held.ready_at(back), Some(back + Duration::hours(9)));
        // An older build's time, without when it ended: at most 7 days.
        let old = QueueState {
            next_try_at: Some((t0() + Duration::days(400)).to_rfc3339()),
            ..QueueState::default()
        };
        assert_eq!(old.ready_at(t0()), Some(t0() + Duration::days(7)));
    }

    #[test]
    fn a_wait_until_beyond_seven_days_reads_as_seven_days() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(31);
        let far = t0() + Duration::days(365);
        for _ in 0..200 {
            let at = next_try_at(Wait::NotATry, t0(), Some(far), t0(), &policy, &mut rng);
            let week = t0() + Duration::seconds(MAX_SERVER_WAIT_S);
            assert!(at >= week && at <= week + Duration::seconds(MAX_SERVER_WAIT_S / 2 + 1));
        }
        assert_eq!(capped_wait_until(far, t0()), t0() + Duration::days(7));
    }

    // -------------------------------------------- the way in, queue-wide

    fn failed(blame: Blame) -> Ending<'static> {
        Ending::Failed {
            blame,
            reason: "Not sent (nothing reached BIR): the login was refused",
        }
    }

    #[test]
    fn a_path_failure_backs_off_the_whole_queue_on_the_ladder() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(40);
        let mut path = PathBackoff::default();
        let state = QueueState::default();
        let mut now = t0();
        let mut waits = Vec::new();
        for _ in 0..6 {
            let retry = plan(&state, failed(Blame::Path), false, now, now, &mut path, &policy, &mut rng);
            assert_eq!(retry.failures, 0, "the return's own count does not move");
            assert!(!retry.counted, "it does not count against the return");
            assert!(retry.was_a_try(), "but it was a try that reached for the BIR");
            assert_eq!(path.floor(now), Some(retry.at));
            waits.push((retry.at - now).num_seconds());
            now = retry.at;
        }
        for (wait, base) in waits.iter().zip([300, 1800, 3600, 7200, 7200, 7200]) {
            assert!(
                (*wait as f64) >= base as f64 * 0.5 && (*wait as f64) <= base as f64 * 1.5,
                "{waits:?}"
            );
        }
        // It survives a restart through its setting.
        let stored = path.to_setting();
        assert_eq!(PathBackoff::from_setting(Some(&stored)), path);
        assert_eq!(PathBackoff::from_setting(Some("not json")), PathBackoff::default());
        // A landing clears it.
        path.clear();
        assert!(path.is_clear() && path.floor(now).is_none());
    }

    #[test]
    fn nobody_starts_before_the_back_off_and_then_one_other_return_goes() {
        let policy = QueuePolicy::default();
        let floor = t0() + Duration::minutes(5);
        let mut rows = vec![row(10, 0, None), row(20, 0, None), row(30, 0, None)];
        // The first was the probe that failed: back of the line.
        rows[0].rotated = Some(t0());
        let before = line_up_after(&rows, t0(), &policy, Some(floor));
        assert_eq!(before.head, None);
        assert!(before.stamps.iter().all(|s| s.unwrap() >= floor));
        let at = line_up_after(&rows, floor, &policy, Some(floor));
        assert_eq!(at.head, Some(1), "due soonest among those not moved back");
        // "Try anyway" may go before it, and first.
        rows[2].try_anyway = true;
        assert_eq!(line_up_after(&rows, t0(), &policy, Some(floor)).head, Some(2));
    }

    #[test]
    fn a_not_a_try_climbs_its_own_ladder_and_never_undercuts_the_returns_own_wait() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(41);
        let mut path = PathBackoff::default();
        let mut state = QueueState::default();
        let mut now = t0();
        let mut waits = Vec::new();
        for _ in 0..6 {
            let retry = plan(
                &state,
                Ending::NotATry { wait_until: None },
                false,
                now,
                now,
                &mut path,
                &policy,
                &mut rng,
            );
            assert!(!retry.counted && path.is_clear());
            waits.push((retry.at - now).num_seconds());
            state.after(&retry);
            now = retry.at;
        }
        assert_eq!(state.not_tried, 6);
        for (wait, base) in waits.iter().zip([300, 1800, 3600, 7200, 7200, 7200]) {
            assert!((*wait as f64) >= base as f64 * 0.5, "{waits:?}");
        }
        // A return three failures in (its own wait 1 h) is not shortened
        // by a first not-a-try.
        let three = QueueState {
            failures: 3,
            ..QueueState::default()
        };
        for _ in 0..200 {
            let retry = plan(&three, Ending::NotATry { wait_until: None }, false, t0(), t0(), &mut path, &policy, &mut rng);
            assert!(retry.at >= t0() + Duration::seconds(1800), "{}", retry.at);
            assert_eq!(retry.failures, 3);
        }
        // A counted try resets it.
        let retry = plan(&state, failed(Blame::Upload), false, now, now, &mut path, &policy, &mut rng);
        state.after(&retry);
        assert_eq!((state.not_tried, state.failures), (0, 1));
    }

    /// A refusal Buwiz answered to the session is not a try (no count, no
    /// path back-off, nothing for the guard), but three of the same kind in
    /// a row stop the return, and Retry lifts it.
    #[test]
    fn three_answered_session_refusals_stop_a_return_without_counting() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(41);
        let mut path = PathBackoff::default();
        let mut state = QueueState::default();
        let reason = "Submission session failed before upload (no PUT attempted): This device may not send returns. (device_not_allowed)";
        let mut now = t0();
        for n in 1..=3u32 {
            let retry = plan(&state, Ending::Answered { reason }, true, now, now, &mut path, &policy, &mut rng);
            assert!(!retry.counted && !retry.was_a_try() && retry.blame.is_none(), "{retry:?}");
            assert!(!retry.slowed, "the guard is not heard");
            assert_eq!((retry.failures, retry.not_tried), (0, n));
            assert!(path.is_clear(), "the way in is not blamed");
            state.after(&retry);
            assert_eq!(state.is_stuck(), n == 3, "after {n}");
            now = retry.at;
        }
        assert_eq!(
            state.stuck.as_deref(),
            Some("This device may not send returns. (device_not_allowed)")
        );
        assert_eq!(state.failures, 0);
        // A different refusal starts the run again; a plain not-a-try in
        // between leaves it alone.
        let mut other = QueueState::default();
        for reason in ["a (x)", "b (y)", "a (x)", "a (x)"] {
            let retry = plan(&other, Ending::Answered { reason }, false, now, now, &mut path, &policy, &mut rng);
            other.after(&retry);
        }
        assert!(!other.is_stuck() && other.same_kind == 2);
        state.press_retry(now, false);
        assert!(!state.is_stuck() && state.same_kind == 0);
        // The plan_retry of the class is a plain not-a-try.
        let plain = plan_retry(0, TryClass::Answered, false, now, None, now, &policy, &mut rng).unwrap();
        assert!(!plain.counted && plain.kind.is_none());
    }

    #[test]
    fn three_refusals_of_the_same_kind_stop_a_return_until_retry() {
        let policy = QueuePolicy::default();
        let mut rng = SeededRng::new(42);
        let mut path = PathBackoff::default();
        let mut state = QueueState::default();
        let other = Ending::Failed {
            blame: Blame::Return,
            reason: "Not sent (nothing reached BIR): the period is not valid",
        };
        for (ending, stuck) in [
            (failed(Blame::Return), false),
            (failed(Blame::Return), false),
            (other, false),
            (failed(Blame::Return), false),
            (failed(Blame::Upload), false),
            (failed(Blame::Return), false),
            (failed(Blame::Return), false),
            (failed(Blame::Return), true),
        ] {
            let retry = plan(&state, ending, false, t0(), t0(), &mut path, &policy, &mut rng);
            state.after(&retry);
            assert_eq!(state.is_stuck(), stuck, "{state:?}");
        }
        assert_eq!(state.stuck.as_deref(), Some("the login was refused"));
        assert!(path.is_clear(), "a return's own failures never touch the back-off");
        state.press_retry(t0(), false);
        assert!(!state.is_stuck() && state.same_kind == 0);
        // No service counts as the return's own kind too.
        let mut no_service = QueueState::default();
        for _ in 0..3 {
            let retry = plan(&no_service, failed(Blame::NoService), true, t0(), t0(), &mut path, &policy, &mut rng);
            assert!(!retry.slowed, "the device guard does not hear it");
            no_service.after(&retry);
        }
        assert!(no_service.is_stuck());
    }

    #[test]
    fn retry_is_once_per_30_minutes_and_three_times_a_day() {
        let mut state = QueueState::default();
        let mut now = t0();
        assert_eq!(state.retry_allowed_at(now), None);
        state.press_retry(now, false);
        assert_eq!(state.retry_allowed_at(now), Some(now + Duration::minutes(30)));
        assert_eq!(
            state.retry_allowed_at(now + Duration::minutes(29)),
            Some(now + Duration::minutes(30))
        );
        now += Duration::minutes(30);
        assert_eq!(state.retry_allowed_at(now), None);
        state.press_retry(now, false);
        now += Duration::minutes(45);
        state.press_retry(now, true);
        assert!(state.try_anyway);
        // Three today: the next is a day after the first.
        assert_eq!(state.retry_allowed_at(now + Duration::hours(1)), Some(t0() + Duration::days(1)));
        assert_eq!(state.retry_allowed_at(t0() + Duration::days(1)), None);
        // A clock that went back does not reset the count.
        assert!(state.retry_allowed_at(t0() - Duration::days(3)).is_some());
    }

    #[test]
    fn a_requeue_within_six_hours_keeps_its_place_on_the_ladder() {
        let next = t0() + Duration::hours(1);
        let mut state = QueueState {
            failures: 3,
            next_try_at: Some(next.to_rfc3339()),
            ended_at: Some(t0().to_rfc3339()),
            stuck: Some("the login was refused".into()),
            same_kind: 3,
            kind: Some("the login was refused".into()),
            fingerprint: Some("f1".into()),
            notices: 2,
            ..QueueState::default()
        };
        state.left(t0() + Duration::minutes(1), Some("f1"));
        let again = state.requeued(t0() + Duration::minutes(2), Some("f1"));
        assert_eq!((again.failures, again.next_try(), again.notices), (3, Some(next), 0));
        assert!(again.is_stuck(), "the same content stays stopped");
        assert_eq!(again.requeue_next_retry_at(t0() + Duration::minutes(2)), next.to_rfc3339());
        let edited = state.requeued(t0() + Duration::minutes(2), Some("f2"));
        assert!(!edited.is_stuck() && edited.failures == 3);
        let later = state.requeued(t0() + Duration::hours(7), Some("f1"));
        assert_eq!(later.failures, 0);
        assert!(!later.is_stuck());
        // Never left (a fresh queueing): fresh.
        assert_eq!(QueueState::default().requeued(t0(), None).failures, 0);
    }

    #[test]
    fn final_codes_are_refusals_and_return_mismatch() {
        assert!(is_final_reason("Not sent: x (tin_paused)"));
        assert!(is_final_reason("Not sent (nothing reached BIR): did not match (return_mismatch)"));
        assert!(is_final_reason("(return_mismatch)"));
        assert!(!is_final_reason("x(return_mismatch)"));
        assert!(!is_final_reason("the relay closed early (relay_closed)"));
        for code in ["rate_limited", "quota_exceeded", "try_anyway_limited"] {
            assert!(is_limit_code(code) && !is_hold_code(code));
        }
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
            ended_at: t0(),
            wait_until: None,
            not_tried: 0,
            blame: Some(Blame::Upload),
            kind: None,
        });
        assert_eq!(state.next_try(), Some(t0()));
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            json!({"failures": 2, "next_try_at": "2026-11-11T07:00:00+00:00", "slowed": true,
                   "ended_at": "2026-11-11T07:00:00+00:00"})
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
