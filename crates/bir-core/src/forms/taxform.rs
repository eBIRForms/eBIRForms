//! A return named the way people type it: `2551Q Q3 2026 file`,
//! `1601-C jan 2026`, `2551QQ32026`, or the canonical token value
//! `000000000/2551Q-2026-Q3` (with or without `@bir:taxform:` before it).
//!
//! [`parse`] reads such text leniently, words in any order.
//! [`TaxPeriod::canonical`] and [`canonical_value`] write it back the one
//! way: `FORM-YYYY-Qn`, `FORM-YYYY-MM`, `FORM-YYYY` or `FORM-YYYY-MM-DD`,
//! the taxpayer as `<tin>/` before it. [`period_to_file`] is the period a
//! form is filed for now when none is named, from the form's own calendar
//! rule ([`DeadlineResolver::last_closed_period`]).
//!
//! Nothing here reads the database: the stored drafts a name may reuse are
//! the caller's.

use chrono::{Datelike, NaiveDate};

use super::agent::AgentFormKind;
use super::registry::{FilingFrequency, find_form};
use crate::calendar_rules::{DeadlinePeriod, DeadlineResolver, Frequency};

/// What a return token starts with in a chat message.
pub const TOKEN_PREFIX: &str = "@bir:taxform:";

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTH_NAMES: [&str; 12] = [
    "JANUARY",
    "FEBRUARY",
    "MARCH",
    "APRIL",
    "MAY",
    "JUNE",
    "JULY",
    "AUGUST",
    "SEPTEMBER",
    "OCTOBER",
    "NOVEMBER",
    "DECEMBER",
];

/// Today's local calendar date, what a period without its year and a
/// missing period are read against (as `dues.list` reads its dues).
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// How often a form is filed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    Monthly,
    Quarterly,
    Annual,
    AsNeeded,
}

impl Cadence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Monthly => "monthly",
            Self::Quarterly => "quarterly",
            Self::Annual => "annual",
            Self::AsNeeded => "as needed",
        }
    }
}

/// How often `form_code` is filed: its official calendar rule, else the
/// form registry (a form the calendar no longer carries, such as 2550M).
pub fn cadence(form_code: &str) -> Option<Cadence> {
    let code = form_code_of(form_code)?;
    Some(match DeadlineResolver::frequency_of(code) {
        Some(Frequency::Monthly) => Cadence::Monthly,
        Some(Frequency::Quarterly) => Cadence::Quarterly,
        Some(Frequency::Annual) => Cadence::Annual,
        Some(Frequency::AsNeeded) => Cadence::AsNeeded,
        None => match find_form(code)?.frequency {
            FilingFrequency::Monthly => Cadence::Monthly,
            FilingFrequency::Quarterly => Cadence::Quarterly,
            FilingFrequency::Annual => Cadence::Annual,
            FilingFrequency::OpenEnded => Cadence::AsNeeded,
        },
    })
}

/// The canonical code of one of the forms BIR can open (`1601-C` -> `1601C`).
pub fn form_code_of(raw: &str) -> Option<&'static str> {
    AgentFormKind::from_code(raw).map(AgentFormKind::form_code)
}

/// One return's period, as a token names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaxPeriod {
    Month {
        year: u16,
        month: u8,
    },
    Quarter {
        year: u16,
        quarter: u8,
    },
    Year(u16),
    /// An as-needed return: the day of its transaction (or today).
    Day(NaiveDate),
}

impl TaxPeriod {
    pub fn year(self) -> u16 {
        match self {
            Self::Month { year, .. } | Self::Quarter { year, .. } | Self::Year(year) => year,
            Self::Day(date) => date.year() as u16,
        }
    }

    /// `2026-01`, `2026-Q3`, `2025`, `2026-10-26`.
    pub fn canonical(self) -> String {
        match self {
            Self::Month { year, month } => format!("{year}-{month:02}"),
            Self::Quarter { year, quarter } => format!("{year}-Q{quarter}"),
            Self::Year(year) => year.to_string(),
            Self::Day(date) => date.format("%Y-%m-%d").to_string(),
        }
    }

    /// `Jan 2026`, `Q3 2026`, `2025`, `Oct 26, 2026`.
    pub fn label(self) -> String {
        match self {
            Self::Month { year, month } => format!("{} {year}", MONTHS[usize::from(month) - 1]),
            Self::Quarter { year, quarter } => format!("Q{quarter} {year}"),
            Self::Year(year) => year.to_string(),
            Self::Day(date) => format!(
                "{} {}, {}",
                MONTHS[date.month0() as usize],
                date.day(),
                date.year()
            ),
        }
    }

    /// The `(year, period)` the form page opens a return by: the month, the
    /// quarter, 0 for a year, a day's month.
    pub fn open_args(self) -> (u16, u8) {
        match self {
            Self::Month { year, month } => (year, month),
            Self::Quarter { year, quarter } => (year, quarter),
            Self::Year(year) => (year, 0),
            Self::Day(date) => (date.year() as u16, date.month() as u8),
        }
    }

    fn start(self) -> NaiveDate {
        let ymd = |year: u16, month: u32| {
            NaiveDate::from_ymd_opt(i32::from(year), month, 1).expect("a valid first day")
        };
        match self {
            Self::Month { year, month } => ymd(year, u32::from(month)),
            Self::Quarter { year, quarter } => ymd(year, u32::from(quarter) * 3 - 2),
            Self::Year(year) => ymd(year, 1),
            Self::Day(date) => date,
        }
    }

    fn contains(self, date: NaiveDate) -> bool {
        match self {
            Self::Month { year, month } => {
                date.year() == i32::from(year) && date.month() == u32::from(month)
            }
            Self::Quarter { year, quarter } => {
                date.year() == i32::from(year) && (date.month0() / 3 + 1) == u32::from(quarter)
            }
            Self::Year(year) => date.year() == i32::from(year),
            Self::Day(day) => day == date,
        }
    }

    /// This period as `form_code` is filed. A month or a day of a quarterly
    /// form is its quarter, any period of an annual form its year; a year
    /// of a monthly or quarterly form is its latest closed period that year;
    /// a month, quarter or year of an as-needed form is today when today is
    /// in it, else its first day. A quarter of a monthly form is refused.
    pub fn for_form(self, form_code: &str, today: NaiveDate) -> Result<Self, String> {
        let cadence = cadence(form_code).ok_or_else(|| format!("BIR has no form `{form_code}`"))?;
        Ok(match (cadence, self) {
            (Cadence::Monthly, Self::Month { .. })
            | (Cadence::Quarterly, Self::Quarter { .. })
            | (Cadence::Annual, Self::Year(_))
            | (Cadence::AsNeeded, Self::Day(_)) => self,
            (Cadence::Monthly, Self::Day(date)) => Self::Month {
                year: date.year() as u16,
                month: date.month() as u8,
            },
            (Cadence::Monthly, Self::Quarter { year, quarter }) => {
                return Err(format!(
                    "{form_code} is filed monthly: name a month of Q{quarter} {year}, like {} {year}",
                    MONTHS[usize::from(quarter) * 3 - 3]
                ));
            }
            (Cadence::Quarterly, Self::Month { year, month }) => Self::Quarter {
                year,
                quarter: (month - 1) / 3 + 1,
            },
            (Cadence::Quarterly, Self::Day(date)) => Self::Quarter {
                year: date.year() as u16,
                quarter: (date.month0() / 3 + 1) as u8,
            },
            (Cadence::Monthly | Cadence::Quarterly, Self::Year(year)) => {
                latest_in_year(form_code, year, today)?
            }
            (Cadence::Annual, other) => Self::Year(other.year()),
            (Cadence::AsNeeded, other) => Self::Day(if other.contains(today) {
                today
            } else {
                other.start()
            }),
        })
    }
}

/// The latest closed period of a monthly or quarterly form in `year`, or
/// its first period when none of that year has closed yet.
fn latest_in_year(form_code: &str, year: u16, today: NaiveDate) -> Result<TaxPeriod, String> {
    let next_new_year = NaiveDate::from_ymd_opt(i32::from(year) + 1, 1, 1)
        .ok_or_else(|| format!("{year} is not a tax year"))?;
    let as_of = today.min(next_new_year);
    match period_to_file(form_code, as_of) {
        Some(period) if period.year() == year => Ok(period),
        _ => Ok(match cadence(form_code) {
            Some(Cadence::Quarterly) => TaxPeriod::Quarter { year, quarter: 1 },
            _ => TaxPeriod::Month { year, month: 1 },
        }),
    }
}

/// The return of `form_code` to file as of `today` when no period is
/// named: monthly, the last closed month; quarterly, the last closed
/// quarter; annual, the previous year; as needed, today. Read off the
/// form's calendar rule ([`DeadlineResolver::last_closed_period`]), so a
/// form that skips a quarter skips it here too. The caller reuses an
/// unsubmitted draft first. `None` for a code BIR cannot open.
pub fn period_to_file(form_code: &str, today: NaiveDate) -> Option<TaxPeriod> {
    let code = form_code_of(form_code)?;
    let cadence = cadence(code)?;
    if cadence == Cadence::AsNeeded {
        return Some(TaxPeriod::Day(today));
    }
    if let Some(period) = DeadlineResolver::last_closed_period(code, today) {
        match period {
            DeadlinePeriod::Monthly {
                taxable_year,
                month,
            } => {
                return Some(TaxPeriod::Month {
                    year: taxable_year as u16,
                    month,
                });
            }
            DeadlinePeriod::Quarterly {
                taxable_year,
                quarter,
            } => {
                return Some(TaxPeriod::Quarter {
                    year: taxable_year as u16,
                    quarter,
                });
            }
            DeadlinePeriod::Annual { taxable_year } => {
                return Some(TaxPeriod::Year(taxable_year as u16));
            }
            DeadlinePeriod::EventBased => {}
        }
    }
    let year = today.year() as u16;
    let month = today.month() as u8;
    Some(match cadence {
        Cadence::Monthly if month == 1 => TaxPeriod::Month {
            year: year - 1,
            month: 12,
        },
        Cadence::Monthly => TaxPeriod::Month {
            year,
            month: month - 1,
        },
        Cadence::Quarterly if month <= 3 => TaxPeriod::Quarter {
            year: year - 1,
            quarter: 4,
        },
        Cadence::Quarterly => TaxPeriod::Quarter {
            year,
            quarter: (month - 1) / 3,
        },
        Cadence::Annual | Cadence::AsNeeded => TaxPeriod::Year(year - 1),
    })
}

/// `view`, `file` or `submit`: what opening the return is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaxformAction {
    View,
    File,
    Submit,
}

impl TaxformAction {
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "view" => Some(Self::View),
            "file" => Some(Self::File),
            "submit" => Some(Self::Submit),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::File => "file",
            Self::Submit => "submit",
        }
    }
}

/// What a taxform text names. Every part is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaxformArgs {
    /// TIN digits: a 9-digit root (the head office) or 14 digits.
    pub tin: Option<String>,
    /// Canonical form code, e.g. `2551Q`.
    pub form: Option<&'static str>,
    pub period: Option<TaxPeriod>,
    pub action: Option<TaxformAction>,
    /// Words, or the rest of a word, that name none of these, as typed.
    pub unknown: Vec<String>,
}

/// The TIN digits a word is: 9 (a root) or 14, dashes and spaces allowed.
/// A 12-digit TIN (3-digit branch) becomes 14.
pub fn tin_digits(word: &str) -> Option<String> {
    let word = word.trim();
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return None;
    }
    let digits: String = word.chars().filter(char::is_ascii_digit).collect();
    match digits.len() {
        9 | 14 => Some(digits),
        12 => Some(format!("{}00{}", &digits[..9], &digits[9..])),
        _ => None,
    }
}

/// The TIN a token names: the 9-digit root for a head office, else all 14.
pub fn token_tin(tin: &str) -> String {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    if digits.len() == 14 && digits[9..].chars().all(|c| c == '0') {
        digits[..9].to_string()
    } else {
        digits
    }
}

/// `000000000/2551Q-2026-Q3`: the value after `@bir:taxform:`.
pub fn canonical_value(tin: &str, form_code: &str, period: TaxPeriod) -> String {
    format!("{}/{form_code}-{}", token_tin(tin), period.canonical())
}

/// Read `text` (a command's arguments or a token's value). A period named
/// without its year is the latest one that has started by `today`: `Q4` in
/// October 2026 is Q4 2026, `dec` is December 2025.
pub fn parse(text: &str, today: NaiveDate) -> TaxformArgs {
    let mut out = TaxformArgs::default();
    let text = text.trim();
    let text = match text.get(..TOKEN_PREFIX.len()) {
        Some(head) if head.eq_ignore_ascii_case(TOKEN_PREFIX) => &text[TOKEN_PREFIX.len()..],
        _ => text,
    };
    let mut rest = text;
    if let Some((head, tail)) = text.split_once('/')
        && let Some(tin) = tin_digits(head)
    {
        out.tin = Some(tin);
        rest = tail;
    }
    let mut pieces = Pieces::default();
    for word in rest
        .split(|c: char| c.is_whitespace() || c == ',' || c == '/')
        .filter(|word| !word.is_empty())
    {
        if let Some(action) = TaxformAction::parse(word) {
            out.action = Some(action);
            continue;
        }
        if out.tin.is_none()
            && let Some(tin) = tin_digits(word)
        {
            out.tin = Some(tin);
            continue;
        }
        for part in joined_codes(word) {
            scan(&part, &mut out, &mut pieces);
        }
    }
    out.period = pieces.finish(today, &mut out.unknown);
    out
}

/// A word split at `-` and `.`, with a code split by its dash put back
/// together (`1601-C` -> `1601C`, but `2026-Q3` stays two parts).
fn joined_codes(word: &str) -> Vec<String> {
    let parts: Vec<String> = word
        .split(['-', '.'])
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_uppercase)
        .collect();
    let mut out: Vec<String> = Vec::new();
    for part in parts {
        if let Some(last) = out.last_mut()
            && last.len() == 4
            && last.chars().all(|c| c.is_ascii_digit())
            && part.chars().all(|c| c.is_ascii_alphabetic())
            && form_code_of(&format!("{last}{part}")).is_some()
        {
            last.push_str(&part);
            continue;
        }
        out.push(part);
    }
    out
}

/// The period pieces named so far, and what the last one was.
#[derive(Default)]
struct Pieces {
    year: Option<u16>,
    quarter: Option<u8>,
    month: Option<u8>,
    day: Option<u8>,
    last: Last,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Last {
    #[default]
    Nothing,
    Year,
    Month,
    Other,
}

impl Pieces {
    fn finish(self, today: NaiveDate, unknown: &mut Vec<String>) -> Option<TaxPeriod> {
        match (self.year, self.quarter, self.month, self.day) {
            (_, Some(quarter), Some(month), _) => {
                unknown.push(format!("Q{quarter} and {}", MONTHS[usize::from(month) - 1]));
                None
            }
            (year, None, Some(month), Some(day)) => {
                let year = year.unwrap_or_else(|| started(today, |year| ymd(year, month, day)));
                match ymd(year, month, day) {
                    Some(date) => Some(TaxPeriod::Day(date)),
                    None => {
                        unknown.push(format!("{year}-{month:02}-{day:02}"));
                        None
                    }
                }
            }
            (year, None, Some(month), None) => Some(TaxPeriod::Month {
                year: year.unwrap_or_else(|| started(today, |year| ymd(year, month, 1))),
                month,
            }),
            (year, Some(quarter), None, _) => Some(TaxPeriod::Quarter {
                year: year.unwrap_or_else(|| started(today, |year| ymd(year, quarter * 3 - 2, 1))),
                quarter,
            }),
            (Some(year), None, None, _) => Some(TaxPeriod::Year(year)),
            (None, None, None, _) => None,
        }
    }
}

fn ymd(year: u16, month: u8, day: u8) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(i32::from(year), u32::from(month), u32::from(day))
}

/// The latest year whose `start(year)` is not after `today`.
fn started(today: NaiveDate, start: impl Fn(u16) -> Option<NaiveDate>) -> u16 {
    let this_year = today.year() as u16;
    match start(this_year) {
        Some(date) if date > today => this_year - 1,
        _ => this_year,
    }
}

/// The codes BIR can open, without dashes, longest first.
fn known_codes() -> &'static [&'static str] {
    static CODES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    CODES.get_or_init(|| {
        let mut codes: Vec<&'static str> = AgentFormKind::all()
            .into_iter()
            .map(AgentFormKind::form_code)
            .collect();
        codes.sort_by_key(|code| std::cmp::Reverse(code.len()));
        codes
    })
}

/// Read one part (upper case, no dashes): a form code, `Qn`, a month name,
/// a year, `YYYYMM`, `YYYYMMDD`, or a month or day number after a year or
/// a month. Pieces run together (`2551QQ32026`).
fn scan(part: &str, out: &mut TaxformArgs, pieces: &mut Pieces) {
    let bytes = part.as_bytes();
    let mut i = 0;
    while i < part.len() {
        let rest = &part[i..];
        if out.form.is_none()
            && let Some(code) = known_codes().iter().find(|code| {
                rest.starts_with(**code)
                    // An all-digit code followed by more digits is a number.
                    && !(code.chars().all(|c| c.is_ascii_digit())
                        && rest[code.len()..].starts_with(|c: char| c.is_ascii_digit()))
            })
        {
            out.form = Some(*code);
            pieces.last = Last::Other;
            i += code.len();
            continue;
        }
        if bytes[i] == b'Q'
            && let Some(quarter) = bytes
                .get(i + 1)
                .copied()
                .filter(|b| (b'1'..=b'4').contains(b))
        {
            pieces.quarter = Some(quarter - b'0');
            pieces.last = Last::Other;
            i += 2;
            continue;
        }
        if let Some((month, len)) = month_name(rest) {
            pieces.month = Some(month);
            pieces.last = Last::Month;
            i += len;
            continue;
        }
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            let number = &rest[..digits];
            if !number_piece(number, pieces) {
                out.unknown.push(number.to_string());
            }
            i += digits;
            continue;
        }
        out.unknown.push(rest.to_string());
        return;
    }
}

/// A month name or its first three letters (`SEPT` too), and its length.
fn month_name(rest: &str) -> Option<(u8, usize)> {
    for (index, name) in MONTH_NAMES.iter().enumerate() {
        if rest.starts_with(name) {
            return Some((index as u8 + 1, name.len()));
        }
    }
    if rest.starts_with("SEPT") {
        return Some((9, 4));
    }
    MONTH_NAMES
        .iter()
        .position(|name| rest.starts_with(&name[..3]))
        .map(|index| (index as u8 + 1, 3))
}

/// One run of digits. False when it is none of the period's pieces.
fn number_piece(number: &str, pieces: &mut Pieces) -> bool {
    let value = |range: std::ops::Range<usize>| number[range].parse::<u16>().ok();
    let year = |year: u16| (1900..=2100).contains(&year).then_some(year);
    let month = |month: u16| (1..=12).contains(&month).then_some(month as u8);
    let day = |day: u16| (1..=31).contains(&day).then_some(day as u8);
    match number.len() {
        4 => match value(0..4).and_then(year) {
            Some(found) => {
                pieces.year = Some(found);
                pieces.last = Last::Year;
                true
            }
            None => false,
        },
        6 => match (value(0..4).and_then(year), value(4..6).and_then(month)) {
            (Some(found), Some(m)) => {
                pieces.year = Some(found);
                pieces.month = Some(m);
                pieces.last = Last::Month;
                true
            }
            _ => false,
        },
        8 => match (
            value(0..4).and_then(year),
            value(4..6).and_then(month),
            value(6..8).and_then(day),
        ) {
            (Some(found), Some(m), Some(d)) => {
                pieces.year = Some(found);
                pieces.month = Some(m);
                pieces.day = Some(d);
                pieces.last = Last::Other;
                true
            }
            _ => false,
        },
        1 | 2 => {
            let Some(n) = value(0..number.len()) else {
                return false;
            };
            match pieces.last {
                // `2026-01`, then `2026-01-15`.
                Last::Year | Last::Nothing if pieces.month.is_none() => match month(n) {
                    Some(m) => {
                        pieces.month = Some(m);
                        pieces.last = Last::Month;
                        true
                    }
                    None => false,
                },
                // `jan 15`, `01/15`.
                Last::Month if pieces.day.is_none() => match day(n) {
                    Some(d) => {
                        pieces.day = Some(d);
                        pieces.last = Last::Other;
                        true
                    }
                    None => false,
                },
                _ => false,
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    const TODAY: (i32, u32, u32) = (2026, 10, 26);

    fn today() -> NaiveDate {
        ymd(TODAY.0, TODAY.1, TODAY.2)
    }

    fn read(text: &str) -> TaxformArgs {
        parse(text, today())
    }

    fn q(year: u16, quarter: u8) -> TaxPeriod {
        TaxPeriod::Quarter { year, quarter }
    }

    fn m(year: u16, month: u8) -> TaxPeriod {
        TaxPeriod::Month { year, month }
    }

    #[test]
    fn every_spelling_of_a_quarterly_return_reads_the_same() {
        for text in [
            "2551Q Q3 2026 file",
            "file 2551Q 2026-Q3",
            "2026-Q3 2551q",
            "2551QQ32026",
            "2551Q-2026-Q3",
            "@bir:taxform:000000000/2551Q-2026-Q3",
            "000000000/2551Q-2026-Q3 file",
            "2551Q q3, 2026",
        ] {
            let args = read(text);
            assert_eq!(args.form, Some("2551Q"), "{text}");
            assert_eq!(args.period, Some(q(2026, 3)), "{text}");
            assert!(args.unknown.is_empty(), "{text}: {:?}", args.unknown);
        }
        assert_eq!(read("2551Q Q3 2026 file").action, Some(TaxformAction::File));
        assert_eq!(
            read("2551Q Q3 2026 submit").action,
            Some(TaxformAction::Submit)
        );
        assert_eq!(read("2551Q Q3 2026").action, None);
        assert_eq!(
            read("@bir:taxform:000000000/2551Q-2026-Q3").tin.as_deref(),
            Some("000000000")
        );
        assert_eq!(
            read("123-456-789-00001 2551Q").tin.as_deref(),
            Some("12345678900001")
        );
    }

    #[test]
    fn months_years_and_days_in_every_spelling() {
        for text in [
            "1601C jan 2026",
            "1601-C 2026-01",
            "1601c January 2026",
            "1601C-2026-01",
            "1601C 202601",
        ] {
            let args = read(text);
            assert_eq!(args.form, Some("1601C"), "{text}");
            assert_eq!(args.period, Some(m(2026, 1)), "{text}");
            assert!(args.unknown.is_empty(), "{text}: {:?}", args.unknown);
        }
        assert_eq!(read("1701 2025").period, Some(TaxPeriod::Year(2025)));
        assert_eq!(read("1601-EQ Q1 2026").form, Some("1601EQ"));
        assert_eq!(read("1702-RT 2025").form, Some("1702RT"));
        assert_eq!(
            read("2000OT 2026-10-26").period,
            Some(TaxPeriod::Day(ymd(2026, 10, 26)))
        );
        assert_eq!(
            read("2000OT oct 26 2026").period,
            Some(TaxPeriod::Day(ymd(2026, 10, 26)))
        );
        assert_eq!(read("2000 2025").form, Some("2000"));
        assert_eq!(read("2000 2025").period, Some(TaxPeriod::Year(2025)));
    }

    #[test]
    fn a_period_without_its_year_is_the_latest_started() {
        // Today is October 26, 2026.
        assert_eq!(read("2551Q Q4").period, Some(q(2026, 4)));
        assert_eq!(read("2551Q Q1").period, Some(q(2026, 1)));
        assert_eq!(read("1601C oct").period, Some(m(2026, 10)));
        assert_eq!(read("1601C dec").period, Some(m(2025, 12)));
        assert_eq!(
            parse("1601C dec", ymd(2026, 12, 1)).period,
            Some(m(2026, 12))
        );
    }

    #[test]
    fn junk_is_reported_not_guessed() {
        assert_eq!(read("2551Q Q9 2026").unknown, vec!["Q9".to_string()]);
        assert_eq!(read("hello").unknown, vec!["HELLO".to_string()]);
        assert_eq!(read("9999 Q3").unknown, vec!["9999".to_string()]);
        assert!(!read("2551Q Q3 jan 2026").unknown.is_empty());
        assert_eq!(read("").form, None);
        assert_eq!(read("").period, None);
    }

    #[test]
    fn canonical_values_round_trip() {
        assert_eq!(
            canonical_value("00000000000000", "2551Q", q(2026, 3)),
            "000000000/2551Q-2026-Q3"
        );
        assert_eq!(
            canonical_value("123456789-00001", "1601C", m(2026, 1)),
            "12345678900001/1601C-2026-01"
        );
        assert_eq!(
            canonical_value("123456789", "1701", TaxPeriod::Year(2025)),
            "123456789/1701-2025"
        );
        assert_eq!(
            canonical_value("123456789", "2000OT", TaxPeriod::Day(ymd(2026, 10, 2))),
            "123456789/2000OT-2026-10-02"
        );
        for value in [
            "000000000/2551Q-2026-Q3",
            "12345678900001/1601C-2026-01",
            "123456789/1701-2025",
            "123456789/2000OT-2026-10-02",
        ] {
            let args = read(value);
            let again = canonical_value(
                args.tin.as_deref().unwrap(),
                args.form.unwrap(),
                args.period.unwrap(),
            );
            assert_eq!(again, value);
        }
        assert_eq!(q(2026, 3).label(), "Q3 2026");
        assert_eq!(m(2026, 1).label(), "Jan 2026");
        assert_eq!(TaxPeriod::Day(ymd(2026, 10, 26)).label(), "Oct 26, 2026");
    }

    #[test]
    fn default_periods_cross_month_quarter_and_year_ends() {
        let file = |code: &str, today: NaiveDate| period_to_file(code, today).unwrap();
        assert_eq!(file("1601C", ymd(2026, 10, 26)), m(2026, 9));
        assert_eq!(file("1601C", ymd(2026, 1, 15)), m(2025, 12));
        assert_eq!(file("1601-C", ymd(2026, 3, 1)), m(2026, 2));
        assert_eq!(file("2551Q", ymd(2026, 10, 26)), q(2026, 3));
        assert_eq!(file("2551Q", ymd(2026, 9, 30)), q(2026, 2));
        assert_eq!(file("2551Q", ymd(2026, 10, 1)), q(2026, 3));
        assert_eq!(file("2551Q", ymd(2027, 1, 2)), q(2026, 4));
        assert_eq!(file("1701Q", ymd(2027, 1, 2)), q(2026, 3));
        assert_eq!(file("1701", ymd(2026, 10, 26)), TaxPeriod::Year(2025));
        assert_eq!(file("1701", ymd(2027, 1, 1)), TaxPeriod::Year(2026));
        assert_eq!(
            file("2000OT", ymd(2026, 10, 26)),
            TaxPeriod::Day(ymd(2026, 10, 26))
        );
        // No calendar rule: the registry's monthly cadence.
        assert_eq!(file("2550M", ymd(2026, 1, 3)), m(2025, 12));
        assert!(period_to_file("9999", today()).is_none());
    }

    #[test]
    fn a_named_period_is_read_as_the_form_files_it() {
        let t = today();
        assert_eq!(m(2026, 8).for_form("2551Q", t), Ok(q(2026, 3)));
        assert_eq!(q(2026, 3).for_form("1701", t), Ok(TaxPeriod::Year(2026)));
        assert!(q(2026, 3).for_form("1601C", t).is_err());
        assert_eq!(TaxPeriod::Year(2025).for_form("1601C", t), Ok(m(2025, 12)));
        assert_eq!(TaxPeriod::Year(2026).for_form("1601C", t), Ok(m(2026, 9)));
        assert_eq!(TaxPeriod::Year(2025).for_form("2551Q", t), Ok(q(2025, 4)));
        assert_eq!(
            TaxPeriod::Year(2026).for_form("1701Q", ymd(2027, 1, 2)),
            Ok(q(2026, 3))
        );
        assert_eq!(TaxPeriod::Year(2027).for_form("2551Q", t), Ok(q(2027, 1)));
        assert_eq!(
            m(2026, 10).for_form("2000OT", t),
            Ok(TaxPeriod::Day(ymd(2026, 10, 26)))
        );
        assert_eq!(
            m(2026, 3).for_form("2000OT", t),
            Ok(TaxPeriod::Day(ymd(2026, 3, 1)))
        );
        assert_eq!(
            TaxPeriod::Day(ymd(2026, 5, 9)).for_form("1601C", t),
            Ok(m(2026, 5))
        );
    }

    #[test]
    fn tins_read_as_roots_or_full_branches() {
        assert_eq!(tin_digits("000-000-000").as_deref(), Some("000000000"));
        assert_eq!(
            tin_digits("123456789000").as_deref(),
            Some("12345678900000")
        );
        assert_eq!(tin_digits("2026"), None);
        assert_eq!(tin_digits("2026-10-26"), None);
        assert_eq!(token_tin("123-456-789-00000"), "123456789");
        assert_eq!(token_tin("12345678900002"), "12345678900002");
    }
}
