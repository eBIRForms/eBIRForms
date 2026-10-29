//! What the print sheets say about a box (rule 5, and rule 4's dates):
//! which boxes the printed sheet gives a fixed number of character slots,
//! which it prints as a whole date, and which stacked lines of one printed
//! address the page shows as one box.
//!
//! The facts come from the print sheets (`html-frozen/<slug>/`: a
//! writer-cells join puts a key in a comb cell, whose `data-comb-slots` is
//! its capacity; a date join prints a key over MM, DD and YYYY combs),
//! copied into `app/forms/print-cells.json` by the ignored test
//! `write_print_cells`; `print_cells_match_the_print_sheets` keeps the copy
//! in step. The page reads them from the schema:
//!
//! - `slots: N` on a text, phone or number box: boxes until the text no
//!   longer fits, then a plain field. Email, amounts and dates have none.
//! - `pool: {keys, slots, write}` on the first line of a printed address
//!   that runs over stacked lines (2552's 40 + 31 slots): one box for the
//!   whole address, laid over the lines' slots while it fits. The later
//!   lines are hidden: they are part of the pool. The host reads and writes
//!   the pool from the draft (`pool_texts`, `write_pool`), never from the
//!   field map, whose lines are cut at a fixed width:
//!   - `write` is the first line alone where only it has a draft field: the
//!     whole text goes there; where the draft keeps a second line the form
//!     joins to it (0619E and 0619F's `registered_address_2`), the text is
//!     split at the first line's boxes so both lines fill.
//!   - `write` is every line where each has its own entry of one draft list
//!     (1702RT's `registered_address_lines`), or where two lines are two
//!     fields of their own (1601C's `registered_address` and
//!     `registered_address_2`, which its print fills through `derived:`
//!     keys): the text is split over them (provisional: rules page question
//!     5).
//!   - A split ([`split_lines`]) ends a line at the last space run that
//!     starts within its boxes, the run kept at its end, or cuts a word
//!     longer than the line at its last box; the lines put end to end are
//!     what was typed. The print (rules page question 4, decided
//!     2026-10-17, in bir-print's fill): within the boxes the lines fill
//!     them; past them the whole text prints as plain type across the
//!     lines, shrunk until it fits. The print maps are not changed.
//!   - Any other mix (1702MX: its first line is the whole address, the
//!     others list entries, and its print reads the list) is not pooled.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use bir_core::forms::agent::{AgentFormDef, AgentFormKind, Binding};
use serde_json::{Map, Value, json};

/// `app/forms/print-cells.json` as written from the frozen writer-cells.json
/// files: form code -> key -> `{slots?, date?, printed}`.
fn file_cells() -> &'static Value {
    static CELLS: OnceLock<Value> = OnceLock::new();
    CELLS.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/forms/print-cells.json"))
            .expect("app/forms/print-cells.json is a JSON object")
    })
}

/// What the page uses: [`file_cells`] plus the comb joins bir-print adds on
/// the Rust side (`frozen_html::sheet_join_combs`, 2550M's item 4 TIN),
/// from the one table the print fills them with; html-frozen and the
/// generated file stay as synced.
fn print_cells() -> &'static Value {
    static CELLS: OnceLock<Value> = OnceLock::new();
    CELLS.get_or_init(|| {
        let mut cells = file_cells().clone();
        for kind in AgentFormKind::all() {
            let code = kind.form_code();
            let Some(slug) = crate::bir_module::print_slug_for(code) else {
                continue;
            };
            for (key, slots, printed) in bir_print::frozen_html::sheet_join_combs(slug) {
                if cells.get(code).is_none() {
                    cells[code] = json!({});
                }
                if cells[code].get(&key).is_none() {
                    cells[code][key] = json!({"slots": slots, "printed": printed});
                }
            }
        }
        cells
    })
}

/// Whether the sheet prints `key` as one whole date (MM, DD and YYYY combs).
pub(super) fn printed_as_date(code: &str, key: &str) -> bool {
    print_cells()[code][key]["date"].as_bool() == Some(true)
}

/// A printed caption without its line marker: "9 Registered Address (second
/// line)" and "9 Registered Address, line 2" both read "9 registered address".
fn caption_stem(caption: &str) -> String {
    let lower = caption.trim().to_ascii_lowercase();
    const MARKERS: [&str; 12] = [
        " (first line)",
        " (second line)",
        " (third line)",
        ", first line",
        ", second line",
        ", third line",
        " (line 1)",
        " (line 2)",
        " (line 3)",
        ", line 1",
        ", line 2",
        ", line 3",
    ];
    MARKERS
        .iter()
        .find_map(|marker| lower.strip_suffix(marker))
        .unwrap_or(&lower)
        .trim()
        .to_string()
}

/// The draft path a text box writes, if it writes one whole text.
fn text_path<'a>(def: &'a AgentFormDef, key: &str) -> Option<&'a str> {
    match def.bindings.get(key).map(|bound| &bound.binding) {
        Some(Binding::Text {
            path,
            date: None,
            part: None,
            ..
        }) => Some(path),
        _ => None,
    }
}

/// An email box, by its page type, its key or the draft field it writes.
fn is_email(def: Option<&AgentFormDef>, field: &Value) -> bool {
    let key = field["k"].as_str().unwrap_or_default();
    field["t"] == "email"
        || key.to_ascii_lowercase().contains("email")
        || def
            .and_then(|def| text_path(def, key))
            .is_some_and(|path| path.to_ascii_lowercase().contains("email"))
}

/// Boxes that keep character slots: text, phone and number boxes. An email
/// is always a plain field, and amounts and dates have their own look.
fn takes_slots(def: Option<&AgentFormDef>, field: &Value) -> bool {
    matches!(field["t"].as_str(), Some("text" | "phone" | "int")) && !is_email(def, field)
}

/// `/a/b/2` -> `("/a/b", 2)`.
fn list_entry(path: &str) -> Option<(&str, usize)> {
    let (list, index) = path.rsplit_once('/')?;
    Some((list, index.parse().ok()?))
}

/// Which lines of a run the page writes, or `None` when the run cannot be
/// one pool (see the module docs).
fn pool_write(def: &AgentFormDef, lines: &[String]) -> Option<Vec<String>> {
    let paths: Vec<Option<&str>> = lines.iter().map(|key| text_path(def, key)).collect();
    let first = paths[0]?;
    if paths[1..].iter().all(Option::is_none) {
        return Some(vec![lines[0].clone()]);
    }
    // Two lines the draft keeps as two fields of their own (1601C's
    // `registered_address` and `registered_address_2`, each its own
    // official control): the text is split over both.
    if let [_, Some(second)] = paths.as_slice()
        && *second == format!("{first}_2")
    {
        return Some(lines.to_vec());
    }
    let entries: Option<Vec<(&str, usize)>> =
        paths.iter().map(|path| path.and_then(list_entry)).collect();
    let entries = entries?;
    let list = entries[0].0;
    entries
        .iter()
        .enumerate()
        .all(|(i, (l, index))| *l == list && *index == i)
        .then(|| lines.to_vec())
}

/// Give each box its printed slots and gather each stacked address into one
/// pool (see the module docs).
pub(super) fn apply_combs(form: &mut Value, code: &str) {
    let Some(cells) = print_cells()[code].as_object() else {
        return;
    };
    let def = AgentFormKind::from_code(code).map(|kind| kind.definition());
    let items = form["parts"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten());
    for item in items {
        let Some(fields) = item["f"].as_array_mut() else {
            continue;
        };
        let slot = |field: &Value| -> Option<(u64, String)> {
            let cell = cells.get(field["k"].as_str()?)?;
            Some((
                cell["slots"].as_u64()?,
                cell["printed"].as_str()?.to_string(),
            ))
        };
        for field in fields.iter_mut() {
            if let (true, Some((n, _))) = (takes_slots(def, field), slot(field)) {
                field["slots"] = json!(n);
            }
        }
        // Runs of text lines printed under one address caption.
        let mut start = 0;
        while start < fields.len() {
            let head = &fields[start];
            let stem = slot(head)
                .filter(|_| head["t"] == "text")
                .map(|(_, caption)| caption_stem(&caption))
                .filter(|stem| stem.contains("address"));
            let Some(stem) = stem else {
                start += 1;
                continue;
            };
            let mut end = start + 1;
            while end < fields.len()
                && fields[end]["t"] == "text"
                && slot(&fields[end]).is_some_and(|(_, caption)| caption_stem(&caption) == stem)
            {
                end += 1;
            }
            let lines: Vec<(String, u64)> = fields[start..end]
                .iter()
                .filter_map(|field| Some((field["k"].as_str()?.to_string(), slot(field)?.0)))
                .collect();
            let keys: Vec<String> = lines.iter().map(|line| line.0.clone()).collect();
            let write = def
                .filter(|_| keys.len() >= 2)
                .and_then(|def| pool_write(def, &keys));
            if let Some(write) = write {
                let head_key = keys[0].clone();
                let mut pool = Map::new();
                pool.insert("keys".into(), json!(keys));
                pool.insert(
                    "slots".into(),
                    json!(lines.iter().map(|l| l.1).collect::<Vec<_>>()),
                );
                pool.insert("write".into(), json!(write));
                fields[start]["pool"] = Value::Object(pool);
                for field in &mut fields[start + 1..end] {
                    field["t"] = Value::from("hidden");
                    field["pooled"] = Value::from(head_key.as_str());
                }
            }
            start = end;
        }
    }
}

/// One pooled address: its lines, their slots, and the lines it writes.
#[derive(Debug, Clone)]
pub(super) struct Pool {
    pub keys: Vec<String>,
    pub slots: Vec<usize>,
    pub write: Vec<String>,
}

/// The form's pools, by their first line.
pub(super) fn pools(code: &str) -> BTreeMap<String, Pool> {
    let strings = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    };
    super::schema_entry(code)
        .map(|form| {
            form["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|part| part["items"].as_array().into_iter().flatten())
                .flat_map(|item| item["f"].as_array().into_iter().flatten())
                .filter(|field| field["pool"].is_object())
                .map(|field| {
                    let pool = &field["pool"];
                    let slots = pool["slots"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|n| n.as_u64().and_then(|n| usize::try_from(n).ok()))
                        .collect();
                    (
                        field["k"].as_str().unwrap_or_default().to_string(),
                        Pool {
                            keys: strings(&pool["keys"]),
                            slots,
                            write: strings(&pool["write"]),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A second draft line the form joins to the first (`<path>_2`: 0619E and
/// 0619F's `registered_address_2`), when the draft has one.
fn second_line(draft: &Value, path: &str) -> Option<String> {
    let path = format!("{path}_2");
    draft.pointer(&path).filter(|v| v.is_string()).map(|_| path)
}

fn read(draft: &Value, path: &str) -> String {
    draft
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The slots of the lines a pool writes, in order.
fn write_slots(pool: &Pool) -> Vec<usize> {
    pool.write
        .iter()
        .map(|key| {
            pool.keys
                .iter()
                .position(|k| k == key)
                .and_then(|i| pool.slots.get(i).copied())
                .unwrap_or(0)
        })
        .collect()
}

/// The pool's address as the draft holds it: the first line and its second
/// draft line joined as the form joins them (0619E's `full_address`), or
/// the list's lines joined by [`join_lines`].
pub(super) fn pool_text(kind: AgentFormKind, pool: &Pool, draft: &Value) -> String {
    let def = kind.definition();
    let paths: Vec<&str> = pool
        .write
        .iter()
        .filter_map(|k| text_path(def, k))
        .collect();
    if let [path] = paths.as_slice() {
        let second = second_line(draft, path)
            .map(|p| read(draft, &p))
            .unwrap_or_default();
        return format!("{}{second}", read(draft, path)).trim().to_string();
    }
    let lines: Vec<String> = paths.iter().map(|p| read(draft, p)).collect();
    if two_fields(def, pool).is_some() {
        // Two lines of their own, joined as the print joins them.
        return lines
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
    }
    join_lines(&lines, &write_slots(pool)).trim().to_string()
}

/// 1601C's address: two lines that are two fields of their own
/// (`<path>` and `<path>_2`, each an official control), not one text the
/// form cuts. `None` for every other pool.
fn two_fields<'a>(def: &'a AgentFormDef, pool: &Pool) -> Option<(&'a str, &'a str)> {
    let [first, second] = pool.write.as_slice() else {
        return None;
    };
    let (first, second) = (text_path(def, first)?, text_path(def, second)?);
    (second == format!("{first}_2")).then_some((first, second))
}

/// The official lengths of 1601C's two address fields (form_1601c.rs
/// validate: "Address exceeds the official 100-character field", "Address
/// (line 2) exceeds the official 50-character field").
pub(super) const TWO_FIELD_MAX: [usize; 2] = [100, 50];

/// What the page refuses past what the two fields can file.
pub(super) const TWO_FIELD_TOO_LONG: &str = "This address is longer than the two official address fields take (100 characters on line 1, 50 on line 2); shorten it.";

/// Split an address typed in one box over two fields of their own. At the
/// last space before the first line's `first`th letter (rules page question
/// 5) when both parts fit the boxes' line and the second field's official
/// length; otherwise the whole text stays in the first field, as a filer
/// typing it there could; past that field's official length, at its last
/// space with the rest in the second field. Both lines are stored trimmed.
/// An error past what the two fields can file.
pub(super) fn split_two_fields(text: &str, first: usize) -> Result<(String, String), String> {
    let chars: Vec<char> = text.trim().chars().collect();
    let [max1, max2] = TWO_FIELD_MAX;
    let at_space = |limit: usize| -> Option<(String, String)> {
        (1..limit.min(chars.len()))
            .rev()
            .find(|&p| chars[p].is_whitespace() && !chars[p - 1].is_whitespace())
            .map(|p| {
                let one: String = chars[..p].iter().collect();
                let two: String = chars[p..].iter().collect();
                (one.trim().to_string(), two.trim().to_string())
            })
    };
    let fits = |(one, two): &(String, String), limit: usize| {
        one.chars().count() <= limit && two.chars().count() <= max2
    };
    if chars.len() <= first {
        return Ok((chars.iter().collect(), String::new()));
    }
    if let Some(lines) = at_space(first).filter(|lines| fits(lines, first)) {
        return Ok(lines);
    }
    if chars.len() <= max1 {
        return Ok((chars.iter().collect(), String::new()));
    }
    at_space(max1 + 1)
        .filter(|lines| fits(lines, max1))
        .ok_or_else(|| TWO_FIELD_TOO_LONG.to_string())
}

/// Each pool's address, by its first line (what the page shows).
pub(super) fn pool_texts(kind: AgentFormKind, draft: &Value) -> Value {
    Value::Object(
        pools(kind.form_code())
            .into_iter()
            .map(|(head, pool)| (head, json!(pool_text(kind, &pool, draft))))
            .collect(),
    )
}

/// Lay `text` over lines of `slots` characters, each line but the last
/// ending at the last space run that starts within its slots (the run stays
/// at the end of that line, so nothing is lost), or, for a word longer than
/// the line, at the line's last box. The last line takes the rest. The lines
/// put end to end are `text`.
pub(super) fn split_lines(text: &str, slots: &[usize]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest: Vec<char> = text.trim().chars().collect();
    for (i, &n) in slots.iter().enumerate() {
        if i + 1 == slots.len() || rest.len() <= n {
            lines.push(rest.drain(..).collect());
            continue;
        }
        // A space right after the `n`th letter ends the line too: the line
        // then holds `n` letters and keeps the space at its end.
        let space = (1..=n)
            .rev()
            .find(|&p| rest[p].is_whitespace() && !rest[p - 1].is_whitespace());
        let cut = match space {
            Some(p) => {
                let mut end = p;
                while end < rest.len() && rest[end].is_whitespace() {
                    end += 1;
                }
                end
            }
            None => n,
        };
        lines.push(rest.drain(..cut).collect());
    }
    lines
}

/// Put lines back together: end to end where a line ends in a space or
/// fills its boxes (a word cut at the last box), else with one space (lines
/// saved before the split kept its space).
pub(super) fn join_lines(lines: &[String], slots: &[usize]) -> String {
    let mut text = String::new();
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let glued = text.is_empty()
            || text.ends_with(char::is_whitespace)
            || lines[..i]
                .iter()
                .rposition(|l| !l.is_empty())
                .is_some_and(|j| lines[j].chars().count() >= slots.get(j).copied().unwrap_or(0));
        if !glued {
            text.push(' ');
        }
        text.push_str(line);
    }
    text
}

/// Write the pool's address from the page into the draft (see the module
/// docs). `lift`: which lock the page has opened for a line. Each line goes
/// through the form's own fill first (its checks), then keeps its exact
/// text, so the lines put end to end are what was typed.
pub(super) fn write_pool(
    kind: AgentFormKind,
    pool: &Pool,
    draft: &mut Value,
    text: &Value,
    lift: impl Fn(&str) -> super::catalogs::Lift,
) -> Result<(), String> {
    let text = match text {
        Value::String(text) => text.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let def = kind.definition();
    let write = |draft: &mut Value, key: &str, line: &str| -> Result<(), String> {
        super::catalogs::fill(kind, draft, key, &json!(line.trim()), lift(key))?;
        if let Some(path) = text_path(def, key) {
            super::set_draft_path(draft, path, json!(line));
        }
        Ok(())
    };
    if let [head] = pool.write.as_slice() {
        let path = text_path(def, head).unwrap_or_default();
        return match second_line(draft, path) {
            // The form keeps two lines: split at the first line's boxes.
            Some(second) => {
                let first = pool.slots.first().copied().unwrap_or(0);
                let lines = split_lines(&text, &[first, 0]);
                write(draft, head, &lines[0])?;
                super::set_draft_path(draft, &second, json!(lines[1]));
                Ok(())
            }
            None => write(draft, head, &text),
        };
    }
    if two_fields(def, pool).is_some() {
        let first = pool.slots.first().copied().unwrap_or(0);
        let (one, two) = split_two_fields(&text, first)?;
        write(draft, &pool.write[0], &one)?;
        write(draft, &pool.write[1], &two)?;
        return Ok(());
    }
    for (key, line) in pool
        .write
        .iter()
        .zip(split_lines(&text, &write_slots(pool)))
    {
        write(draft, key, &line)?;
    }
    Ok(())
}
