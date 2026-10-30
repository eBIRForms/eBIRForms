//! Frozen HTML fill/print: set `input[name]` from the writer map.
//!
//! Layout lives in `html-frozen/<slug>/`. `name=` is a fail-closed catalog
//! join; `id=` stays the cell id. Catalog `official_field_key` stamps are
//! TIN/branch only. Unstamped leftover keys keep cell-id `name=`;
//! `html-frozen/<slug>/writer-cells.json` may copy a writer value onto that
//! cell when the freeze sheet has a 1:1 printed-caption join (`joins`),
//! split a leftover `{:.2}` money key onto a catalog peso comb plus 2-slot
//! cents comb (`money_joins`), or mark a catalog xbox when a leftover
//! boolean/radio writer is `"true"` (`xbox_joins`, glyph `X`). Text `joins`
//! ASCII-uppercase letters into comb slots and `data-writer-value` so BIR
//! CAPITAL LETTERS print matches the form instruction; digits, punctuation,
//! money, and xbox are unchanged. Profile/DB values stay as stored. Those are
//! fill-paths, not `official_field_key` harvests. Do not stamp `name=` on
//! peso/cent/xbox boxes, do not left-align a dotted money string into the
//! peso comb, and do not write `"true"` into an xbox.
//!
//! One writer key may print in several cells (`html_ids` beside `html_id`;
//! `peso_html_ids`/`cent_html_ids` and `decimal_html_ids` for money), so a
//! name, TIN or total repeats in the later pages' headers and schedules.
//! A money comb whose last two slots are the centavos is a
//! `decimal_html_id` (`point_slot` when the slot before them carries the
//! printed point). `date_joins` split one writer date (`MM/DD/YYYY`,
//! `MM/YYYY`, ISO) over separate MM, DD and YYYY combs. An xbox join may
//! name one box of a two-box cell by its input id (`<cell>-i<n>`).
//! `not_joined` lists, with a reason, each writer key the sheet does not
//! print, and `not_joined_cells` each cell no key fills: [`unprinted_keys`]
//! is the coverage gate over both.

use bir_core::forms::form_2551q::Form2551QDraft;
use std::collections::{BTreeMap, BTreeSet};

include!(concat!(env!("OUT_DIR"), "/frozen_bundles.rs"));

const BASE_CSS: &str = include_str!("../assets/html-frozen/base.css");
const FONT_ARIMO_NORMAL: &[u8] =
    include_bytes!("../assets/html-frozen/fonts/arimo-latin-wght-normal.woff2");
const FONT_ARIMO_ITALIC: &[u8] =
    include_bytes!("../assets/html-frozen/fonts/arimo-latin-wght-italic.woff2");

const STAMPED_TIN_NAMES: [&str; 4] = [
    "frm2551Qv2018:txtTIN1",
    "frm2551Qv2018:txtTIN2",
    "frm2551Qv2018:txtTIN3",
    "frm2551Qv2018:txtBranchCode",
];

struct InputTag<'a> {
    start: usize,
    end: usize,
    tag: &'a str,
    name: &'a str,
    slot: Option<usize>,
}

pub fn html_2551q() -> &'static str {
    bundle("2551q-2018").expect("2551q-2018 freeze bundle").html
}

/// Rewrite matching `<input name>` tags. Comb slots (`data-slot-index`) receive
/// one character each. Unknown keys are ignored. `id=` is never changed.
pub fn fill_by_name(html: &str, fields: &BTreeMap<String, String>) -> String {
    fill_by_name_with_cells(html, fields, &BTreeMap::new(), &BTreeMap::new())
}

/// `plan`: the stacked-line cells whose item's text overflows its boxes
/// ([`line_group_plan`]), printed as planned instead of from their key.
/// Outside a bundle: Arimo, no spacing ([`SheetType::plain`]).
fn fill_by_name_with_cells(
    html: &str,
    fields: &BTreeMap<String, String>,
    cells: &BTreeMap<String, Vec<String>>,
    plan: &BTreeMap<String, CellFill>,
) -> String {
    fill_by_name_set(html, fields, cells, plan, &SheetType::plain())
}

/// [`fill_by_name_with_cells`] with a value longer than its boxes measured
/// in `sheet`'s type.
fn fill_by_name_set(
    html: &str,
    fields: &BTreeMap<String, String>,
    cells: &BTreeMap<String, Vec<String>>,
    plan: &BTreeMap<String, CellFill>,
    sheet: &SheetType,
) -> String {
    let tags = input_tags(html);
    if tags.is_empty() {
        return html.to_string();
    }

    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    let grouped = grouped_input_indices(&tags);

    for (name, value) in fields {
        let targets: Vec<(&str, bool)> = if grouped.contains_key(name.as_str()) {
            vec![(name.as_str(), false)]
        } else if let Some(ids) = cells.get(name) {
            ids.iter().map(|id| (id.as_str(), true)).collect()
        } else {
            continue;
        };
        for (input_name, via_cell) in targets {
            let Some(indices) = grouped.get(input_name) else {
                continue;
            };
            // A stacked line whose item overflows prints as planned below.
            if via_cell && plan.contains_key(input_name) {
                continue;
            }
            let target = TextTarget {
                name,
                input_name,
                value,
                via_cell,
                sheet,
            };
            fill_text_input(html, &tags, indices, &target, &mut replacements);
        }
    }
    // An overflowing stacked item: re-split over its boxes, or, past them,
    // every slot left empty and each line's part laid over its cell as
    // plain type.
    for (cell, fill) in plan {
        let Some(indices) = grouped.get(cell.as_str()) else {
            continue;
        };
        match fill {
            CellFill::Boxes(key, line) => {
                let target = TextTarget {
                    name: key,
                    input_name: cell,
                    value: line,
                    via_cell: true,
                    sheet,
                };
                fill_text_input(html, &tags, indices, &target, &mut replacements);
            }
            CellFill::Plain(text, size) => {
                for &index in indices {
                    replacements.push((
                        tags[index].start,
                        tags[index].end,
                        set_value(tags[index].tag, "", None),
                    ));
                }
                if let Some((at, line)) =
                    overflow_input(html, cell, text, *size).filter(|_| !text.is_empty())
                {
                    replacements.push((at, at, line));
                }
            }
        }
    }

    apply_replacements(html, replacements)
}

/// How one stacked-line cell prints when its item's text overflows the
/// boxes as the print map laid it out.
#[derive(Debug, Clone, PartialEq)]
enum CellFill {
    /// The item re-split over its boxes (it fits them): this line,
    /// printed as the cell's key (`.0`) prints.
    Boxes(String, String),
    /// Past the boxes: the slots stay empty and this part of the text
    /// prints as plain type of `size` pt over the cell.
    Plain(String, f64),
}

/// The type a comb's boxes print in: the largest an overflow line uses.
const PLAIN_MAX_PT: f64 = 9.0;
/// What the per-character estimate assumed before every sheet's overflow
/// type was measured (P4-7): about 0.58 em a character. Kept for the tests
/// that show how it fell short.
#[cfg(test)]
const EM_PER_CHAR: f64 = 0.58;

/// Units per em of both bundled print faces
/// (html-frozen/fonts/arimo-latin-wght-normal.woff2 and
/// tinos-latin-400-normal.woff2).
const UNITS_PER_EM: f64 = 2048.0;

/// arimo-latin-wght-normal.woff2: units per em 2048; every character its
/// cmap maps from U+0020 up, with its advance width (`hmtx`), by code point.
const ARIMO_ADVANCES: &[(u32, u16)] = &[
    (0x0020, 569),
    (0x0021, 569),
    (0x0022, 727),
    (0x0023, 1139),
    (0x0024, 1139),
    (0x0025, 1821),
    (0x0026, 1366),
    (0x0027, 391),
    (0x0028, 682),
    (0x0029, 682),
    (0x002A, 797),
    (0x002B, 1196),
    (0x002C, 569),
    (0x002D, 682),
    (0x002E, 569),
    (0x002F, 569),
    (0x0030, 1139),
    (0x0031, 1139),
    (0x0032, 1139),
    (0x0033, 1139),
    (0x0034, 1139),
    (0x0035, 1139),
    (0x0036, 1139),
    (0x0037, 1139),
    (0x0038, 1139),
    (0x0039, 1139),
    (0x003A, 569),
    (0x003B, 569),
    (0x003C, 1196),
    (0x003D, 1196),
    (0x003E, 1196),
    (0x003F, 1139),
    (0x0040, 2079),
    (0x0041, 1366),
    (0x0042, 1366),
    (0x0043, 1479),
    (0x0044, 1479),
    (0x0045, 1366),
    (0x0046, 1251),
    (0x0047, 1593),
    (0x0048, 1479),
    (0x0049, 569),
    (0x004A, 1024),
    (0x004B, 1366),
    (0x004C, 1139),
    (0x004D, 1706),
    (0x004E, 1479),
    (0x004F, 1593),
    (0x0050, 1366),
    (0x0051, 1593),
    (0x0052, 1479),
    (0x0053, 1366),
    (0x0054, 1251),
    (0x0055, 1479),
    (0x0056, 1366),
    (0x0057, 1933),
    (0x0058, 1366),
    (0x0059, 1366),
    (0x005A, 1251),
    (0x005B, 569),
    (0x005C, 569),
    (0x005D, 569),
    (0x005E, 961),
    (0x005F, 1139),
    (0x0060, 682),
    (0x0061, 1139),
    (0x0062, 1139),
    (0x0063, 1024),
    (0x0064, 1139),
    (0x0065, 1139),
    (0x0066, 569),
    (0x0067, 1139),
    (0x0068, 1139),
    (0x0069, 455),
    (0x006A, 455),
    (0x006B, 1024),
    (0x006C, 455),
    (0x006D, 1706),
    (0x006E, 1139),
    (0x006F, 1139),
    (0x0070, 1139),
    (0x0071, 1139),
    (0x0072, 682),
    (0x0073, 1024),
    (0x0074, 569),
    (0x0075, 1139),
    (0x0076, 1024),
    (0x0077, 1479),
    (0x0078, 1024),
    (0x0079, 1024),
    (0x007A, 1024),
    (0x007B, 684),
    (0x007C, 532),
    (0x007D, 684),
    (0x007E, 1196),
    (0x00A0, 569),
    (0x00A1, 682),
    (0x00A2, 1139),
    (0x00A3, 1139),
    (0x00A4, 1139),
    (0x00A5, 1139),
    (0x00A6, 532),
    (0x00A7, 1139),
    (0x00A8, 682),
    (0x00A9, 1509),
    (0x00AA, 758),
    (0x00AB, 1139),
    (0x00AC, 1196),
    (0x00AD, 682),
    (0x00AE, 1509),
    (0x00AF, 1131),
    (0x00B0, 819),
    (0x00B1, 1124),
    (0x00B2, 682),
    (0x00B3, 682),
    (0x00B4, 682),
    (0x00B5, 1180),
    (0x00B6, 1100),
    (0x00B7, 682),
    (0x00B8, 682),
    (0x00B9, 682),
    (0x00BA, 748),
    (0x00BB, 1139),
    (0x00BC, 1708),
    (0x00BD, 1708),
    (0x00BE, 1708),
    (0x00BF, 1251),
    (0x00C0, 1366),
    (0x00C1, 1366),
    (0x00C2, 1366),
    (0x00C3, 1366),
    (0x00C4, 1366),
    (0x00C5, 1366),
    (0x00C6, 2048),
    (0x00C7, 1479),
    (0x00C8, 1366),
    (0x00C9, 1366),
    (0x00CA, 1366),
    (0x00CB, 1366),
    (0x00CC, 569),
    (0x00CD, 569),
    (0x00CE, 569),
    (0x00CF, 569),
    (0x00D0, 1479),
    (0x00D1, 1479),
    (0x00D2, 1593),
    (0x00D3, 1593),
    (0x00D4, 1593),
    (0x00D5, 1593),
    (0x00D6, 1593),
    (0x00D7, 1196),
    (0x00D8, 1593),
    (0x00D9, 1479),
    (0x00DA, 1479),
    (0x00DB, 1479),
    (0x00DC, 1479),
    (0x00DD, 1366),
    (0x00DE, 1366),
    (0x00DF, 1251),
    (0x00E0, 1139),
    (0x00E1, 1139),
    (0x00E2, 1139),
    (0x00E3, 1139),
    (0x00E4, 1139),
    (0x00E5, 1139),
    (0x00E6, 1821),
    (0x00E7, 1024),
    (0x00E8, 1139),
    (0x00E9, 1139),
    (0x00EA, 1139),
    (0x00EB, 1139),
    (0x00EC, 569),
    (0x00ED, 569),
    (0x00EE, 569),
    (0x00EF, 569),
    (0x00F0, 1139),
    (0x00F1, 1139),
    (0x00F2, 1139),
    (0x00F3, 1139),
    (0x00F4, 1139),
    (0x00F5, 1139),
    (0x00F6, 1139),
    (0x00F7, 1124),
    (0x00F8, 1251),
    (0x00F9, 1139),
    (0x00FA, 1139),
    (0x00FB, 1139),
    (0x00FC, 1139),
    (0x00FD, 1024),
    (0x00FE, 1139),
    (0x00FF, 1024),
    (0x0131, 569),
    (0x0152, 2048),
    (0x0153, 1933),
    (0x02BB, 455),
    (0x02BC, 455),
    (0x02C6, 683),
    (0x02DA, 682),
    (0x02DC, 682),
    (0x0300, 0),
    (0x0301, 0),
    (0x0303, 0),
    (0x0304, 0),
    (0x0308, 0),
    (0x0309, 0),
    (0x0323, 0),
    (0x0329, 0),
    (0x2002, 1024),
    (0x2009, 410),
    (0x200B, 0),
    (0x2013, 1139),
    (0x2014, 2048),
    (0x2018, 455),
    (0x2019, 455),
    (0x201A, 455),
    (0x201C, 682),
    (0x201D, 682),
    (0x201E, 682),
    (0x2022, 717),
    (0x2026, 2048),
    (0x2032, 384),
    (0x2033, 725),
    (0x2039, 682),
    (0x203A, 682),
    (0x2044, 342),
    (0x20AC, 1139),
    (0x2122, 2048),
    (0x2191, 1024),
    (0x2193, 1024),
    (0x2212, 1196),
    (0x2215, 342),
    (0xFEFF, 0),
    (0xFFFD, 1721),
];
/// tinos-latin-400-normal.woff2: units per em 2048; every character its
/// cmap maps from U+0020 up, with its advance width (`hmtx`), by code point.
const TINOS_ADVANCES: &[(u32, u16)] = &[
    (0x0020, 512),
    (0x0021, 682),
    (0x0022, 836),
    (0x0023, 1024),
    (0x0024, 1024),
    (0x0025, 1706),
    (0x0026, 1593),
    (0x0027, 369),
    (0x0028, 682),
    (0x0029, 682),
    (0x002A, 1024),
    (0x002B, 1155),
    (0x002C, 512),
    (0x002D, 682),
    (0x002E, 512),
    (0x002F, 569),
    (0x0030, 1024),
    (0x0031, 1024),
    (0x0032, 1024),
    (0x0033, 1024),
    (0x0034, 1024),
    (0x0035, 1024),
    (0x0036, 1024),
    (0x0037, 1024),
    (0x0038, 1024),
    (0x0039, 1024),
    (0x003A, 569),
    (0x003B, 569),
    (0x003C, 1155),
    (0x003D, 1155),
    (0x003E, 1155),
    (0x003F, 909),
    (0x0040, 1886),
    (0x0041, 1479),
    (0x0042, 1366),
    (0x0043, 1366),
    (0x0044, 1479),
    (0x0045, 1251),
    (0x0046, 1139),
    (0x0047, 1479),
    (0x0048, 1479),
    (0x0049, 682),
    (0x004A, 797),
    (0x004B, 1479),
    (0x004C, 1251),
    (0x004D, 1821),
    (0x004E, 1479),
    (0x004F, 1479),
    (0x0050, 1139),
    (0x0051, 1479),
    (0x0052, 1366),
    (0x0053, 1139),
    (0x0054, 1251),
    (0x0055, 1479),
    (0x0056, 1479),
    (0x0057, 1933),
    (0x0058, 1479),
    (0x0059, 1479),
    (0x005A, 1251),
    (0x005B, 682),
    (0x005C, 569),
    (0x005D, 682),
    (0x005E, 961),
    (0x005F, 1024),
    (0x0060, 682),
    (0x0061, 909),
    (0x0062, 1024),
    (0x0063, 909),
    (0x0064, 1024),
    (0x0065, 909),
    (0x0066, 682),
    (0x0067, 1024),
    (0x0068, 1024),
    (0x0069, 569),
    (0x006A, 569),
    (0x006B, 1024),
    (0x006C, 569),
    (0x006D, 1593),
    (0x006E, 1024),
    (0x006F, 1024),
    (0x0070, 1024),
    (0x0071, 1024),
    (0x0072, 682),
    (0x0073, 797),
    (0x0074, 569),
    (0x0075, 1024),
    (0x0076, 1024),
    (0x0077, 1479),
    (0x0078, 1024),
    (0x0079, 1024),
    (0x007A, 909),
    (0x007B, 983),
    (0x007C, 410),
    (0x007D, 983),
    (0x007E, 1108),
    (0x00A0, 512),
    (0x00A1, 682),
    (0x00A2, 1024),
    (0x00A3, 1024),
    (0x00A4, 1024),
    (0x00A5, 1024),
    (0x00A6, 410),
    (0x00A7, 1024),
    (0x00A8, 682),
    (0x00A9, 1556),
    (0x00AA, 565),
    (0x00AB, 1024),
    (0x00AC, 1155),
    (0x00AD, 682),
    (0x00AE, 1556),
    (0x00AF, 1024),
    (0x00B0, 819),
    (0x00B1, 1124),
    (0x00B2, 614),
    (0x00B3, 614),
    (0x00B4, 682),
    (0x00B5, 1180),
    (0x00B6, 928),
    (0x00B7, 682),
    (0x00B8, 682),
    (0x00B9, 614),
    (0x00BA, 635),
    (0x00BB, 1024),
    (0x00BC, 1536),
    (0x00BD, 1536),
    (0x00BE, 1536),
    (0x00BF, 909),
    (0x00C0, 1479),
    (0x00C1, 1479),
    (0x00C2, 1479),
    (0x00C3, 1479),
    (0x00C4, 1479),
    (0x00C5, 1479),
    (0x00C6, 1821),
    (0x00C7, 1366),
    (0x00C8, 1251),
    (0x00C9, 1251),
    (0x00CA, 1251),
    (0x00CB, 1251),
    (0x00CC, 682),
    (0x00CD, 682),
    (0x00CE, 682),
    (0x00CF, 682),
    (0x00D0, 1479),
    (0x00D1, 1479),
    (0x00D2, 1479),
    (0x00D3, 1479),
    (0x00D4, 1479),
    (0x00D5, 1479),
    (0x00D6, 1479),
    (0x00D7, 1155),
    (0x00D8, 1479),
    (0x00D9, 1479),
    (0x00DA, 1479),
    (0x00DB, 1479),
    (0x00DC, 1479),
    (0x00DD, 1479),
    (0x00DE, 1139),
    (0x00DF, 1024),
    (0x00E0, 909),
    (0x00E1, 909),
    (0x00E2, 909),
    (0x00E3, 909),
    (0x00E4, 909),
    (0x00E5, 909),
    (0x00E6, 1366),
    (0x00E7, 909),
    (0x00E8, 909),
    (0x00E9, 909),
    (0x00EA, 909),
    (0x00EB, 909),
    (0x00EC, 569),
    (0x00ED, 569),
    (0x00EE, 569),
    (0x00EF, 569),
    (0x00F0, 1024),
    (0x00F1, 1024),
    (0x00F2, 1024),
    (0x00F3, 1024),
    (0x00F4, 1024),
    (0x00F5, 1024),
    (0x00F6, 1024),
    (0x00F7, 1124),
    (0x00F8, 1024),
    (0x00F9, 1024),
    (0x00FA, 1024),
    (0x00FB, 1024),
    (0x00FC, 1024),
    (0x00FD, 1024),
    (0x00FE, 1024),
    (0x00FF, 1024),
    (0x0131, 569),
    (0x0152, 1821),
    (0x0153, 1479),
    (0x02BB, 682),
    (0x02BC, 682),
    (0x02C6, 682),
    (0x02DA, 682),
    (0x02DC, 682),
    (0x0300, 0),
    (0x0301, 0),
    (0x0303, 0),
    (0x0304, 0),
    (0x0308, 0),
    (0x0309, 0),
    (0x0323, 0),
    (0x0329, 0),
    (0x2002, 1024),
    (0x2009, 410),
    (0x200B, 0),
    (0x2013, 1024),
    (0x2014, 2048),
    (0x2018, 682),
    (0x2019, 682),
    (0x201A, 682),
    (0x201C, 909),
    (0x201D, 909),
    (0x201E, 909),
    (0x2022, 717),
    (0x2026, 2048),
    (0x2032, 448),
    (0x2033, 853),
    (0x2039, 682),
    (0x203A, 682),
    (0x2044, 342),
    (0x20AC, 1024),
    (0x2122, 2007),
    (0x2191, 1024),
    (0x2193, 1024),
    (0x2212, 1155),
    (0x2215, 342),
    (0xFEFF, 1638),
    (0xFFFD, 1721),
];

/// What a character a face's cmap does not map counts for, in ems: the
/// browser draws it from another font (Cyrillic, CJK, emoji, Latin
/// Extended), whose advance the print cannot know. 1.3 em is above the
/// about 1.2 em CJK ideographs and emoji take, so the type can only come
/// out smaller.
const FALLBACK_EMS: f64 = 1.3;

/// How many space advances apart Chromium sets tab stops (CSS `tab-size`,
/// 8 by default). A plain input's value keeps a typed tab, which Chromium
/// draws to the next stop. Measured in headless Chrome in an `<input>` with
/// the eBIRForms faces (2048 px type): the stops are 8 × (the face's space
/// advance + word-spacing + letter-spacing) apart from the start of the
/// line, Arimo 8 × 569 and Tinos 8 × 512 font units; the tab takes no
/// letter- or word-spacing of its own; and a tab that would end less than
/// half a space advance short of its stop runs on to the next one. The
/// stacked-line overflow path ([`plain_lines`]) splits a value into words,
/// so there a tab or a no-break space prints as a single space.
const TAB_SPACES: f64 = 8.0;

/// Whether word-spacing applies after `c`: a space or a no-break space
/// (U+00A0); headless Chrome adds an `<input>`'s word-spacing to both.
fn is_word_space(c: char) -> bool {
    c == ' ' || c == '\u{a0}'
}

/// The face a sheet sets its `.fi` inputs in (form.css `font-family`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Face {
    Arimo,
    Tinos,
}

impl Face {
    fn advances(self) -> &'static [(u32, u16)] {
        match self {
            Self::Arimo => ARIMO_ADVANCES,
            Self::Tinos => TINOS_ADVANCES,
        }
    }

    /// `text`'s advance width in ems of this face; a tab counts a whole
    /// tab stop, [`TAB_SPACES`] space advances (the most it takes; where it
    /// ends is [`SheetType::width`]'s), another character the face does not
    /// map [`FALLBACK_EMS`].
    fn ems(self, text: &str) -> f64 {
        text.chars().map(|c| self.char_ems(c)).sum()
    }

    /// [`Face::ems`] of one character.
    fn char_ems(self, c: char) -> f64 {
        if c == '\t' {
            return TAB_SPACES * self.char_ems(' ');
        }
        let table = self.advances();
        table
            .binary_search_by_key(&u32::from(c), |(cp, _)| *cp)
            .map_or(FALLBACK_EMS, |at| f64::from(table[at].1) / UNITS_PER_EM)
    }
}

/// A CSS length the print adds per character or per space: its `pt` part,
/// fixed, and its `em` part, which scales with the type size.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Spacing {
    pt: f64,
    em: f64,
}

impl Spacing {
    /// `word-spacing:0.3238pt`, `letter-spacing:-0.01em` in a CSS rule;
    /// none (normal, 0, absent) is zero.
    fn of(rule: &str, property: &str) -> Self {
        let Some(at) = rule.find(&format!("{property}:")) else {
            return Self::default();
        };
        let value = rule[at + property.len() + 1..]
            .split([';', '}'])
            .next()
            .unwrap_or("")
            .trim();
        let number = |unit: &str| value.strip_suffix(unit).and_then(|n| n.trim().parse().ok());
        if let Some(pt) = number("pt") {
            Self { pt, em: 0.0 }
        } else if let Some(px) = number("px") {
            Self {
                pt: px * 0.75,
                em: 0.0,
            }
        } else if let Some(em) = number("em") {
            Self { pt: 0.0, em }
        } else {
            Self::default()
        }
    }

    fn at(self, size: f64) -> f64 {
        self.pt + self.em * size
    }
}

/// The `{ ... }` body of CSS rule `selector` in `css` (`.fi`, `.fh4`).
fn css_rule<'a>(css: &'a str, selector: &str) -> Option<&'a str> {
    let open = format!("{selector} {{");
    let mut from = 0;
    while let Some(at) = css[from..].find(&open) {
        let at = from + at;
        // A whole selector: at a line start, not `.fi:hover` or `.fh40`.
        if at == 0 || css[..at].ends_with('\n') {
            let body = &css[at + open.len()..];
            return Some(&body[..body.find('}').unwrap_or(body.len())]);
        }
        from = at + open.len();
    }
    None
}

/// How a sheet sets the type of its inputs, from its form.css: the `.fi`
/// rule's face, word-spacing and letter-spacing (P4-7; the sheets set them
/// per sheet: word-spacing is 0.3238pt on 1604F, 0.4092pt on 2200T), and
/// the stylesheet, for each input class's size and letter-spacing.
#[derive(Debug, Clone, Copy)]
struct SheetType {
    face: Face,
    word: Spacing,
    letter: Spacing,
    css: &'static str,
    /// The sheet's [`SINGLE_CHARACTER_CELLS`].
    split: &'static [(&'static str, bool)],
}

impl SheetType {
    /// A sheet's type from its stylesheet.
    fn of(css: &'static str) -> Self {
        let rule = css_rule(css, ".fi").unwrap_or("");
        let family = rule
            .find("font-family:")
            .map(|at| &rule[at..])
            .and_then(|tail| tail.split([';', ',']).next())
            .unwrap_or("");
        Self {
            face: if family.contains("Tinos") {
                Face::Tinos
            } else {
                Face::Arimo
            },
            word: Spacing::of(rule, "word-spacing"),
            letter: Spacing::of(rule, "letter-spacing"),
            css,
            split: &[],
        }
    }

    /// The type of sheet `slug`, with its split one-character cells.
    fn of_sheet(slug: &str, css: &'static str) -> Self {
        Self {
            split: SINGLE_CHARACTER_CELLS
                .iter()
                .find(|(sheet, _)| *sheet == slug)
                .map_or(&[], |(_, cells)| *cells),
            ..Self::of(css)
        }
    }

    /// No stylesheet: Arimo with no spacing (a fill outside any bundle).
    fn plain() -> Self {
        Self {
            face: Face::Arimo,
            word: Spacing::default(),
            letter: Spacing::default(),
            css: "",
            split: &[],
        }
    }

    /// The letter-spacing an input of class list `classes` gets: its own
    /// size class's (`.fh4`, which follows `.fi` in the stylesheet), else
    /// `.fi`'s.
    fn letter_for(&self, classes: &str) -> Spacing {
        classes
            .split_whitespace()
            .filter(|class| class.starts_with("fh"))
            .filter_map(|class| css_rule(self.css, &format!(".{class}")))
            .find(|rule| rule.contains("letter-spacing:"))
            .map_or(self.letter, |rule| Spacing::of(rule, "letter-spacing"))
    }

    /// The type size (pt) class list `classes` sets (`.fh4 { font-size:9pt }`).
    fn size_for(&self, classes: &str) -> Option<f64> {
        classes
            .split_whitespace()
            .filter(|class| class.starts_with("fh"))
            .filter_map(|class| css_rule(self.css, &format!(".{class}")))
            .find_map(|rule| {
                let at = rule.find("font-size:")?;
                rule[at + "font-size:".len()..]
                    .split(';')
                    .next()?
                    .trim()
                    .strip_suffix("pt")?
                    .parse()
                    .ok()
            })
    }

    /// The width (pt) `text` sets at `size` pt with `letter` spacing: its
    /// advances in this face, the word-spacing on every space and the
    /// letter-spacing on every character; a tab runs to the next tab stop
    /// ([`TAB_SPACES`]).
    fn width(&self, letter: Spacing, text: &str, size: f64) -> f64 {
        if text.contains('\t') {
            return self.width_with_tabs(letter, text, size);
        }
        let spaces = text.chars().filter(|c| is_word_space(*c)).count() as f64;
        let chars = text.chars().count() as f64;
        self.face.ems(text) * size + spaces * self.word.at(size) + chars * letter.at(size)
    }

    /// [`SheetType::width`] of a text with tabs, set left to right: each
    /// tab ends at the next multiple of the stop, 8 × (space advance +
    /// word-spacing + letter-spacing), from the line's start, or at the one
    /// after it when it would end less than half a space advance short.
    fn width_with_tabs(&self, letter: Spacing, text: &str, size: f64) -> f64 {
        let (word, letter) = (self.word.at(size), letter.at(size));
        let space = self.face.char_ems(' ') * size;
        let stop = TAB_SPACES * (space + word + letter);
        text.chars().fold(0.0, |at, c| {
            if c == '\t' {
                if stop <= 0.0 {
                    return at;
                }
                let mut advance = stop - at.rem_euclid(stop);
                if advance < space / 2.0 {
                    advance += stop;
                }
                at + advance
            } else if is_word_space(c) {
                at + self.face.char_ems(c) * size + word + letter
            } else {
                at + self.face.char_ems(c) * size + letter
            }
        })
    }

    /// The largest type size (pt), at most `max` and rounded down to 0.01
    /// pt, at which `text` sets within `room` pt: the width is linear in the
    /// size, `ems × size + spaces × word + characters × letter`. A text with
    /// tabs, where a tab ends is neither linear nor monotone in the size
    /// (with spacing in points, a smaller size can push a tab past a
    /// stop), is scanned down from `max` a hundredth of a point at a time
    /// to the first size that sets, returned as that exact hundredth (0.01
    /// pt when none sets).
    fn fit(&self, letter: Spacing, room: f64, text: &str, max: f64) -> f64 {
        if text.contains('\t') {
            let top = ((max * 100.0).floor() as i64).max(1);
            let size = (1..=top)
                .rev()
                .find(|hundredths| self.width(letter, text, *hundredths as f64 / 100.0) <= room)
                .unwrap_or(1);
            return size as f64 / 100.0;
        }
        let spaces = text.chars().filter(|c| is_word_space(*c)).count() as f64;
        let chars = text.chars().count() as f64;
        let per_pt = self.face.ems(text) + spaces * self.word.em + chars * letter.em;
        let fixed = spaces * self.word.pt + chars * letter.pt;
        let size = if per_pt <= 0.0 {
            max
        } else {
            ((room - fixed) / per_pt).min(max)
        };
        floor_size(size)
    }
}

/// A type size rounded down to 0.01 pt (what the print writes), never
/// below 0.01 pt: rounding never adds width.
fn floor_size(size: f64) -> f64 {
    ((size * 100.0).floor() / 100.0).max(0.01)
}

/// `text`'s width in ems of Arimo (the tests that show the old estimate).
#[cfg(test)]
fn arimo_ems(text: &str) -> f64 {
    Face::Arimo.ems(text)
}

/// The type size (pt) a line of `text` past a comb of `width` pt prints
/// at: measured in the sheet's own face with its word- and letter-spacing
/// ([`SheetType::fit`]), within the cell less the line's 1 pt left padding
/// and 1 pt to spare (a cell under 3 pt wide keeps 1 pt, or what is past
/// the padding when that is less), at most [`PLAIN_MAX_PT`], rounded down
/// to 0.01 pt. Every sheet sizes its overflow type this way (maintainer decision P4-7,
/// 2026-10-19): a value that fits its boxes prints byte for byte as
/// before; a value longer than its boxes is measured.
fn fit_line(sheet: &SheetType, width: f64, text: &str) -> f64 {
    sheet.fit(
        sheet.letter,
        (width - 2.0).max((width - 1.0).min(1.0)),
        text,
        PLAIN_MAX_PT,
    )
}

/// How much wider than its room a value may measure and still fit: Chromium
/// lays text out in 1/64 px, so a value the model puts a hair over the room
/// fits on the page.
const FIT_EPSILON_PT: f64 = 0.02;

/// The width (pt) of plain (`data-field-kind="text"`) box `cell` of `html`.
fn plain_cell_width(html: &str, cell: &str) -> Option<f64> {
    let open = html.find(&format!("<div id=\"{cell}\""))?;
    let tag = &html[open..open + html[open..].find('>')?];
    if attr(tag, "data-field-kind") != Some("text") {
        return None;
    }
    let width_at = tag.find(";width:")? + ";width:".len();
    tag[width_at..].split("pt").next()?.parse().ok()
}

/// The room (pt) input `tag` leaves for text in a cell `width` pt wide:
/// the width less the input's left and right inset.
fn input_room(width: f64, tag: &str) -> f64 {
    let (left, right) = inset(attr(tag, "style").unwrap_or(""));
    width - left - right
}

/// The left and right of an input's `inset:` (1 to 4 lengths, pt).
fn inset(style: &str) -> (f64, f64) {
    let Some(at) = style.find("inset:") else {
        return (0.0, 0.0);
    };
    let values: Vec<f64> = style[at + "inset:".len()..]
        .split(';')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|v| v.strip_suffix("pt").and_then(|n| n.parse().ok()))
        .collect();
    match values.as_slice() {
        [all] => (*all, *all),
        [_, sides] | [_, sides, _] => (*sides, *sides),
        [_, right, _, left] => (*left, *right),
        _ => (0.0, 0.0),
    }
}

/// The type size (pt) `text` prints at in plain input `tag` of a cell
/// `width` pt wide when it is longer than the input: wider, at its class's
/// size, than the input's whole room (and [`FIT_EPSILON_PT`]). It is then
/// measured in the sheet's type ([`SheetType::fit`]) to set within
/// [`shrunk_room`], the room less 1 pt to spare, at a whole hundredth of a
/// point. `None` while it fits: it prints byte for byte as before.
fn plain_input_size(sheet: &SheetType, width: f64, tag: &str, text: &str) -> Option<f64> {
    let classes = attr(tag, "class")?;
    let size = sheet.size_for(classes)?;
    let letter = sheet.letter_for(classes);
    let room = input_room(width, tag);
    (sheet.width(letter, text, size) > room + FIT_EPSILON_PT)
        .then(|| sheet.fit(letter, shrunk_room(room), text, size))
}

/// The width (pt) a shrunk value sets within in an input of `room` pt: the
/// room less 1 pt to spare; an input under 2 pt wide gets 1 pt, or its
/// whole room when that is under 1 pt, never more than the room.
fn shrunk_room(room: f64) -> f64 {
    (room - 1.0).max(room.min(1.0))
}

/// `tag` with `font-size:{size}pt` last in its inline style.
fn with_font_size(tag: &str, size: f64) -> String {
    let declaration = format!("font-size:{size:.2}pt");
    if let Some(at) = tag.find(" style=\"") {
        let value_at = at + " style=\"".len();
        if let Some(end) = tag[value_at..].find('"') {
            let end = value_at + end;
            let style = tag[value_at..end].trim_end_matches(';');
            return format!("{}{style};{declaration}{}", &tag[..value_at], &tag[end..]);
        }
    }
    let close = tag.rfind('>').unwrap_or(tag.len());
    let close = if tag[..close].ends_with('/') {
        close - 1
    } else {
        close
    };
    format!("{} style=\"{declaration}\"{}", &tag[..close], &tag[close..])
}

/// One stacked line of a printed item: its cell, the key that fills it,
/// its boxes and its width (pt).
#[derive(Debug, Clone)]
struct LineCell {
    cell: String,
    key: String,
    slots: usize,
    width: f64,
}

/// A printed caption's stem and line number: "9 Registered Address" and
/// "9 Registered Address (second line)" are lines 0 and 1 of one item.
fn caption_line(caption: &str) -> (String, usize) {
    let lower = caption.trim().to_ascii_lowercase();
    const MARKERS: [(&str, usize); 12] = [
        (" (first line)", 0),
        (" (second line)", 1),
        (" (third line)", 2),
        (", first line", 0),
        (", second line", 1),
        (", third line", 2),
        (" (line 1)", 0),
        (" (line 2)", 1),
        (" (line 3)", 2),
        (", line 1", 0),
        (", line 2", 1),
        (", line 3", 2),
    ];
    for (marker, line) in MARKERS {
        if let Some(stem) = lower.strip_suffix(marker) {
            return (stem.trim().to_string(), line);
        }
    }
    (lower, 0)
}

/// A comb cell's boxes and width (pt), read from its `<div id>`.
fn comb_cell(html: &str, cell: &str) -> Option<(usize, f64)> {
    let open = html.find(&format!("<div id=\"{cell}\""))?;
    let tag = &html[open..open + html[open..].find('>')?];
    if attr(tag, "data-field-kind") != Some("comb") {
        return None;
    }
    let slots = attr(tag, "data-comb-slots")?.parse().ok()?;
    let width_at = tag.find(";width:")? + ";width:".len();
    let width = tag[width_at..].split("pt").next()?.parse().ok()?;
    Some((slots, width))
}

/// The sheet's items printed over stacked comb lines (a two-line address,
/// a three-line registered name), each its lines in order: text joins
/// whose captions name lines 1, 2 (and 3) of one item.
fn line_groups(html: &str, cells: &WriterCells) -> Vec<Vec<LineCell>> {
    let mut items: BTreeMap<String, BTreeMap<usize, LineCell>> = BTreeMap::new();
    for (key, ids) in &cells.joins {
        let Some(first) = ids.first() else {
            continue;
        };
        let (Some(caption), Some((slots, width))) =
            (cells.captions.get(first), comb_cell(html, first))
        else {
            continue;
        };
        let (stem, line) = caption_line(caption);
        items.entry(stem).or_default().insert(
            line,
            LineCell {
                cell: first.clone(),
                key: key.clone(),
                slots,
                width,
            },
        );
    }
    items
        .into_values()
        .filter(|lines| lines.len() >= 2 && lines.keys().copied().eq(0..lines.len()))
        .map(|lines| lines.into_values().collect())
        .collect()
}

/// Lay `text` over lines of `slots` boxes, each line but the last ending at
/// the last space run that starts within its boxes (kept at its end), or
/// at its last box for a word longer than the line; the last line takes
/// the rest (the page's own split, `combs::split_lines` in bir-shell).
fn split_lines(text: &str, slots: &[usize]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest: Vec<char> = text.trim().chars().collect();
    for (i, &n) in slots.iter().enumerate() {
        if i + 1 == slots.len() || rest.len() <= n {
            lines.push(rest.drain(..).collect());
            continue;
        }
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

/// The item's lines put back together: end to end where a line ends or
/// the next starts with a space, or the line runs past its boxes (a field
/// map's fixed-width cut, 1600PT's and 2552's 80 characters), else with
/// one space (a map that broke at a space and dropped it). A word longer
/// than a whole line, cut at its last box, reads as two words.
fn join_lines(lines: &[String], slots: &[usize]) -> String {
    let mut text = String::new();
    let mut previous: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let glued = previous.is_none_or(|j| {
            text.ends_with(char::is_whitespace)
                || line.starts_with(char::is_whitespace)
                || lines[j].chars().count() > slots[j]
        });
        if !glued {
            text.push(' ');
        }
        text.push_str(line);
        previous = Some(i);
    }
    text
}

/// The size the old per-character estimate gave `chars` characters in
/// `width` pt (before P4-7), for the tests that show how it fell short.
#[cfg(test)]
fn fit_size(width: f64, chars: usize) -> f64 {
    if chars == 0 {
        return PLAIN_MAX_PT;
    }
    ((width - 2.0).max(1.0) / (chars as f64 * EM_PER_CHAR)).min(PLAIN_MAX_PT)
}

/// `text` as plain type over lines of `widths` pt, broken only between
/// words: the breaks that let the largest common type size fit, and among
/// those the nearest each line's share of the width, so the text runs
/// across every line. Each line's part, and the size.
fn plain_lines(text: &str, widths: &[f64], sheet: &SheetType) -> (Vec<String>, f64) {
    fn walk(level: usize, words: usize, cuts: &mut Vec<usize>, visit: &mut dyn FnMut(&[usize])) {
        if level == cuts.len() - 1 {
            visit(cuts);
            return;
        }
        for cut in cuts[level - 1]..=words {
            cuts[level] = cut;
            walk(level + 1, words, cuts, visit);
        }
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let total: f64 = widths.iter().sum();
    let length =
        words.iter().map(|w| w.chars().count()).sum::<usize>() + words.len().saturating_sub(1);
    let line_text = |cuts: &[usize], k: usize| words[cuts[k]..cuts[k + 1]].join(" ");
    let mut cuts = vec![0usize; widths.len() + 1];
    cuts[widths.len()] = words.len();
    let mut best: Option<(i64, f64, Vec<usize>)> = None;
    let mut visit = |cuts: &[usize]| {
        let (mut size, mut deviation, mut so_far, mut share) = (PLAIN_MAX_PT, 0.0, 0usize, 0.0);
        for (k, width) in widths.iter().enumerate() {
            let line = line_text(cuts, k);
            let chars = line.chars().count();
            size = size.min(fit_line(sheet, *width, &line));
            so_far += chars;
            share += width / total;
            deviation += (so_far as f64 - share * length as f64).abs();
        }
        // Sizes within a hundredth of a point are the same size.
        let rank = (size * 100.0).floor() as i64;
        if best
            .as_ref()
            .is_none_or(|(r, d, _)| rank > *r || (rank == *r && deviation < *d))
        {
            best = Some((rank, deviation, cuts.to_vec()));
        }
    };
    walk(1, words.len(), &mut cuts, &mut visit);
    let cuts = best.map(|(_, _, cuts)| cuts).unwrap_or(cuts);
    let lines: Vec<String> = (0..widths.len()).map(|k| line_text(&cuts, k)).collect();
    let size = lines
        .iter()
        .zip(widths)
        .map(|(line, width)| fit_line(sheet, *width, line))
        .fold(PLAIN_MAX_PT, f64::min);
    (lines, size)
}

/// For each stacked-line item whose text overflows a line's boxes as the
/// print map laid it out, how each of its cells prints: re-split over the
/// boxes while the whole text fits them, else plain type across its lines,
/// shrunk until it fits. An item whose lines all fit their boxes prints
/// exactly as its keys say (no entry).
fn line_group_plan(
    html: &str,
    fields: &BTreeMap<String, String>,
    cells: &WriterCells,
    sheet: &SheetType,
) -> BTreeMap<String, CellFill> {
    let mut plan = BTreeMap::new();
    for group in line_groups(html, cells) {
        // As the fill prints a text join: in BIR capital letters.
        let values: Vec<String> = group
            .iter()
            .map(|line| {
                let value = fields.get(&line.key).map(String::as_str).unwrap_or("");
                if is_email_key(&line.key) {
                    value.to_string()
                } else {
                    value.to_ascii_uppercase()
                }
            })
            .collect();
        let slots: Vec<usize> = group.iter().map(|line| line.slots).collect();
        if values
            .iter()
            .zip(&slots)
            .all(|(value, n)| value.chars().count() <= *n)
        {
            continue;
        }
        let text = join_lines(&values, &slots).trim().to_string();
        let boxed = split_lines(&text, &slots);
        let fits = boxed
            .iter()
            .zip(&slots)
            .all(|(line, n)| line.trim_end().chars().count() <= *n);
        if fits {
            for (line, part) in group.iter().zip(boxed) {
                plan.insert(
                    line.cell.clone(),
                    CellFill::Boxes(line.key.clone(), part.trim_end().to_string()),
                );
            }
            continue;
        }
        let widths: Vec<f64> = group.iter().map(|line| line.width).collect();
        let (parts, size) = plain_lines(&text, &widths, sheet);
        for (line, part) in group.iter().zip(parts) {
            plan.insert(line.cell.clone(), CellFill::Plain(part, size));
        }
    }
    plan
}

/// Writer key `name` printed into input `input_name`: a stamped `name=`
/// (`via_cell` false) or a writer-cells join target.
struct TextTarget<'a> {
    name: &'a str,
    input_name: &'a str,
    value: &'a str,
    via_cell: bool,
    /// The sheet's type, which a value longer than its boxes is measured in.
    sheet: &'a SheetType,
}

/// One text value into one cell: a comb gets a character per slot, a plain
/// box the whole value.
fn fill_text_input(
    html: &str,
    tags: &[InputTag<'_>],
    indices: &[usize],
    target: &TextTarget<'_>,
    replacements: &mut Vec<(usize, usize, String)>,
) {
    let TextTarget {
        name,
        input_name,
        value,
        via_cell,
        sheet,
    } = *target;
    // Writer-cell text joins print as BIR CAPITAL LETTERS. Stamped
    // TIN/branch (`via_cell` false) keep the writer map as-is.
    // Like the official capital(), email inputs keep their typed casing.
    let print_value = (via_cell && !is_email_key(name)).then(|| value.to_ascii_uppercase());
    let print = print_value.as_deref().unwrap_or(value);
    let comb = indices.iter().any(|&index| tags[index].slot.is_some());
    if comb {
        let ordered = ordered_comb_indices(tags, indices);
        // Attachment counts occupy the sheet's existing digit boxes.
        // Pad the displayed count only; the upload value stays as written.
        if is_attachment_count_key(name)
            && !print.is_empty()
            && print.bytes().all(|b| b.is_ascii_digit())
            && print.len() < ordered.len()
        {
            let padded = format!("{print:0>width$}", width = ordered.len());
            for (offset, (index, ch)) in ordered.into_iter().zip(padded.chars()).enumerate() {
                let writer = (via_cell && offset == 0).then_some(print);
                replacements.push((
                    tags[index].start,
                    tags[index].end,
                    set_value(tags[index].tag, &ch.to_string(), writer),
                ));
            }
            return;
        }
        // An amount into a comb drawn with a decimal-point column (one gap in
        // the slot indices, exactly two slots after it): right-align the peso
        // digits into the slots before the point and put the two centavo
        // digits after it. Spreading the text left-to-right put `0.00` as
        // `0 . 0 0` from the first box (#print-audit decimal mapping).
        if let Some(writes) = money_comb_values(tags, &ordered, print) {
            for (offset, (index, ch)) in writes.into_iter().enumerate() {
                let writer = (via_cell && offset == 0).then_some(print);
                replacements.push((
                    tags[index].start,
                    tags[index].end,
                    set_value(tags[index].tag, &ch, writer),
                ));
            }
            return;
        }
        let chars: Vec<char> = print.chars().collect();
        // Text longer than its comb (a long name, an email, a
        // `derived:line:` print value) prints whole as plain type across
        // the comb, shrunk to fit, instead of losing the characters past
        // the last box.
        let whole_line = via_cell || is_email_key(name) || name.starts_with("derived:line:");
        let overflow = (whole_line && chars.len() > ordered.len())
            .then(|| comb_overflow_line(html, input_name, print, sheet))
            .flatten();
        let spread = overflow.is_none();
        if let Some((at, line)) = overflow {
            replacements.push((at, at, line));
        }
        for (offset, index) in ordered.into_iter().enumerate() {
            let ch = chars
                .get(offset)
                .filter(|_| spread)
                .copied()
                .map(|c| c.to_string())
                .unwrap_or_default();
            let writer = (via_cell && offset == 0).then_some(print);
            replacements.push((
                tags[index].start,
                tags[index].end,
                set_value(tags[index].tag, &ch, writer),
            ));
        }
    } else {
        // A split cell ([`SINGLE_CHARACTER_CELLS`]) holding no more
        // characters than boxes prints one per box at its class's size
        // (`split_single_character_cells`); a longer value prints once, as
        // a plain line measured across the whole cell, its boxes empty.
        let split =
            sheet.split.iter().any(|(cell, _)| *cell == input_name) && splits(tags, indices);
        let overflow = (split && !fits_one_per_box(print, indices.len()))
            .then(|| comb_overflow_line(html, input_name, print, sheet))
            .flatten();
        if let Some((at, line)) = overflow {
            replacements.push((at, at, line));
            for (offset, &index) in indices.iter().enumerate() {
                let writer = (via_cell && offset == 0).then_some(print);
                replacements.push((
                    tags[index].start,
                    tags[index].end,
                    set_value(tags[index].tag, "", writer),
                ));
            }
            return;
        }
        // A value longer than its plain input prints smaller, measured, so
        // it stays inside the input; one that fits keeps its bytes.
        let width = via_cell
            .then(|| plain_cell_width(html, input_name))
            .flatten();
        for &index in indices {
            let writer = via_cell.then_some(print);
            let tag = set_value(tags[index].tag, print, writer);
            let size = width
                .filter(|_| !split)
                .and_then(|width| plain_input_size(sheet, width, tags[index].tag, print));
            let tag = size.map_or_else(|| tag.clone(), |size| with_font_size(&tag, size));
            replacements.push((tags[index].start, tags[index].end, tag));
        }
    }
}

/// Whether a cell's inputs `indices` are split one character per box: two
/// or more one-character inputs that are not a comb.
fn splits(tags: &[InputTag<'_>], indices: &[usize]) -> bool {
    indices.len() >= 2
        && indices.iter().all(|&index| {
            tags[index].slot.is_none() && attr(tags[index].tag, "maxlength") == Some("1")
        })
}

/// Whether `value` goes one character per box into a split cell of
/// `boxes` boxes: no more characters than boxes, letters and digits only.
fn fits_one_per_box(value: &str, boxes: usize) -> bool {
    value.chars().count() <= boxes && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Attachment-count writer keys used by the reviewed frozen-sheet joins.
/// In particular, 1702RT calls its count PagesFilled. Split tens/units joins
/// already supply one digit each and do not need a different layout.
fn is_attachment_count_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(
        key.rsplit(':').next().unwrap_or(&key),
        "txtsheets"
            | "txtnosheets"
            | "txtnoofsheets"
            | "txtpg1i3nosheets"
            | "txtpg1i3noofsheets"
            | "txtpg1i4nosheets"
            | "txtpg1i4noofsheets"
            | "txti3sheets"
            | "txtpg1p2i23numofattachments"
            | "txtpg1pt2pagesfilled"
            | "attachments"
            | "attachments_tens"
            | "attachments_units"
    )
}

/// `(slot tag index, char)` for an amount printed into a point-column comb,
/// or `None` when the comb has no point column or the value is not an amount.
fn money_comb_values(
    tags: &[InputTag<'_>],
    ordered: &[usize],
    value: &str,
) -> Option<Vec<(usize, String)>> {
    let slots: Vec<usize> = ordered
        .iter()
        .map(|&i| tags[i].slot)
        .collect::<Option<_>>()?;
    let gaps: Vec<usize> = (1..slots.len())
        .filter(|&k| slots[k] != slots[k - 1] + 1)
        .collect();
    let [gap] = gaps.as_slice() else {
        return None;
    };
    if slots.len() - gap != 2 || *gap < 1 {
        return None;
    }
    let trimmed = value.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | '-' | ' '))
        || !trimmed.chars().any(|c| c.is_ascii_digit())
    {
        return None;
    }
    let (negative, peso, cents) = split_writer_money(trimmed)?;
    let peso = signed_digits(negative, &peso, &cents);
    let peso_values = right_aligned_slot_values(*gap, &peso)?;
    let mut out: Vec<(usize, String)> = ordered[..*gap].iter().copied().zip(peso_values).collect();
    out.extend(
        ordered[*gap..]
            .iter()
            .copied()
            .zip(cents.chars().map(|c| c.to_string())),
    );
    Some(out)
}

fn apply_replacements(html: &str, mut replacements: Vec<(usize, usize, String)>) -> String {
    replacements.sort_by_key(|(start, _, _)| *start);
    let mut out = String::with_capacity(html.len() + replacements.len() * 8);
    let mut last = 0;
    for (start, end, tag) in replacements {
        out.push_str(&html[last..start]);
        out.push_str(&tag);
        last = end;
    }
    out.push_str(&html[last..]);
    out
}

fn writer_cells_json(slug: &str) -> Option<&'static str> {
    match slug {
        "0605-1999" => Some(bir_core::packed_str!(
            "assets/html-frozen/0605-1999/writer-cells.json"
        )),
        "0619e-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/0619e-2018/writer-cells.json"
        )),
        "0619f-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/0619f-2018/writer-cells.json"
        )),
        "1600-pt-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1600-pt-2018/writer-cells.json"
        )),
        "1600-vt-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1600-vt-2018/writer-cells.json"
        )),
        "1600wp-2010" => Some(bir_core::packed_str!(
            "assets/html-frozen/1600wp-2010/writer-cells.json"
        )),
        "1601-fq-2020" => Some(bir_core::packed_str!(
            "assets/html-frozen/1601-fq-2020/writer-cells.json"
        )),
        "1601c-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1601c-2018/writer-cells.json"
        )),
        "1601eq-2019" => Some(bir_core::packed_str!(
            "assets/html-frozen/1601eq-2019/writer-cells.json"
        )),
        "1602q-2019" => Some(bir_core::packed_str!(
            "assets/html-frozen/1602q-2019/writer-cells.json"
        )),
        "1603q-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1603q-2018/writer-cells.json"
        )),
        "1604c-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1604c-2018/writer-cells.json"
        )),
        "1604e-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1604e-2018/writer-cells.json"
        )),
        "1604f-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1604f-2018/writer-cells.json"
        )),
        "1606-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1606-2018/writer-cells.json"
        )),
        "1700-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1700-2018/writer-cells.json"
        )),
        "1701-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1701-2018/writer-cells.json"
        )),
        "1701a-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1701a-2018/writer-cells.json"
        )),
        "1701ms-2024" => Some(bir_core::packed_str!(
            "assets/html-frozen/1701ms-2024/writer-cells.json"
        )),
        "1701q-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1701q-2018/writer-cells.json"
        )),
        "1702ex-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1702ex-2018/writer-cells.json"
        )),
        "1702mx-2018c" => Some(bir_core::packed_str!(
            "assets/html-frozen/1702mx-2018c/writer-cells.json"
        )),
        "1702q-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1702q-2018/writer-cells.json"
        )),
        "1702rt-2018c" => Some(bir_core::packed_str!(
            "assets/html-frozen/1702rt-2018c/writer-cells.json"
        )),
        "1706-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1706-2018/writer-cells.json"
        )),
        "1707-2021" => Some(bir_core::packed_str!(
            "assets/html-frozen/1707-2021/writer-cells.json"
        )),
        "1707a-2021" => Some(bir_core::packed_str!(
            "assets/html-frozen/1707a-2021/writer-cells.json"
        )),
        "1800-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1800-2018/writer-cells.json"
        )),
        "1801-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/1801-2018/writer-cells.json"
        )),
        "2000-dst-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/2000-dst-2018/writer-cells.json"
        )),
        "2000-ot-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/2000-ot-2018/writer-cells.json"
        )),
        "2200a-2020" => Some(bir_core::packed_str!(
            "assets/html-frozen/2200a-2020/writer-cells.json"
        )),
        "2200an-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/2200an-2018/writer-cells.json"
        )),
        "2200c-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/2200c-2018/writer-cells.json"
        )),
        "2200m-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/2200m-2018/writer-cells.json"
        )),
        "2200p-2020" => Some(bir_core::packed_str!(
            "assets/html-frozen/2200p-2020/writer-cells.json"
        )),
        "2200s-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/2200s-2018/writer-cells.json"
        )),
        "2200t-2022" => Some(bir_core::packed_str!(
            "assets/html-frozen/2200t-2022/writer-cells.json"
        )),
        "2550m-2007" => Some(bir_core::packed_str!(
            "assets/html-frozen/2550m-2007/writer-cells.json"
        )),
        "2550q-2024" => Some(bir_core::packed_str!(
            "assets/html-frozen/2550q-2024/writer-cells.json"
        )),
        "2551q-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/2551q-2018/writer-cells.json"
        )),
        "2552-2018" => Some(bir_core::packed_str!(
            "assets/html-frozen/2552-2018/writer-cells.json"
        )),
        "2553-1999" => Some(bir_core::packed_str!(
            "assets/html-frozen/2553-1999/writer-cells.json"
        )),
        _ => None,
    }
}

const XBOX_CHECKED_GLYPH: &str = "X";

/// Top-level `writer-cells.json` sections; anything else is a format error
/// (the old catch-all `left_out` is now the reviewed `not_joined_cells`).
const WRITER_CELLS_SECTIONS: [&str; 10] = [
    "form_id",
    "html_slug",
    "rule",
    "joins",
    "money_joins",
    "xbox_joins",
    "peso_joins",
    "date_joins",
    "not_joined",
    "not_joined_cells",
];

/// One printed amount box: a peso comb plus its 2-slot centavo comb, or
/// (`cent_html_id` `None`, a row's `decimal_html_ids`) one comb whose last
/// two slots take the centavos. With `point_slot` the slot before them holds
/// the printed decimal point and stays blank; without it the sheet prints
/// the point on the line between the peso and centavo slots.
struct MoneyCells {
    peso_html_id: String,
    cent_html_id: Option<String>,
    point_slot: bool,
}

/// A `{:.2}` writer amount printed in every listed peso + cents pair.
struct MoneyJoin {
    writer_key: String,
    cells: Vec<MoneyCells>,
}

struct XboxJoin {
    writer_key: String,
    html_ids: Vec<String>,
}

/// A peso comb with no centavo comb beside it ("Do not enter centavos"):
/// the amount prints rounded to whole pesos, 50 centavos and up rounding up.
struct PesoJoin {
    writer_key: String,
    html_ids: Vec<String>,
}

/// One writer date printed across the sheet's separate MM / DD / YYYY combs
/// (`dd_html_id` absent for an MM / YYYY pair).
struct DateJoin {
    writer_key: String,
    mm_html_id: String,
    dd_html_id: Option<String>,
    yyyy_html_id: String,
}

struct WriterCells {
    /// Text joins: one writer key, every cell it prints in (page-1 box and
    /// the later pages' header repeats).
    joins: BTreeMap<String, Vec<String>>,
    /// A text join's first cell -> its printed caption (`printed`), which
    /// says when cells are the stacked lines of one item.
    captions: BTreeMap<String, String>,
    money_joins: Vec<MoneyJoin>,
    xbox_joins: Vec<XboxJoin>,
    peso_joins: Vec<PesoJoin>,
    date_joins: Vec<DateJoin>,
    /// Reviewed exceptions: writer keys that print nowhere, with the reason.
    not_joined: BTreeMap<String, String>,
    /// Reviewed sheet cells no writer key fills, with the reason.
    not_joined_cells: BTreeMap<String, String>,
}

impl WriterCells {
    fn empty() -> Self {
        Self {
            joins: BTreeMap::new(),
            captions: BTreeMap::new(),
            money_joins: Vec::new(),
            xbox_joins: Vec::new(),
            peso_joins: Vec::new(),
            date_joins: Vec::new(),
            not_joined: BTreeMap::new(),
            not_joined_cells: BTreeMap::new(),
        }
    }

    fn has_fill_join(&self) -> bool {
        !self.joins.is_empty()
            || !self.money_joins.is_empty()
            || !self.xbox_joins.is_empty()
            || !self.peso_joins.is_empty()
            || !self.date_joins.is_empty()
    }

    /// Every writer key any join prints.
    fn joined_keys(&self) -> BTreeSet<&str> {
        self.joins
            .keys()
            .map(String::as_str)
            .chain(self.money_joins.iter().map(|j| j.writer_key.as_str()))
            .chain(self.xbox_joins.iter().map(|j| j.writer_key.as_str()))
            .chain(self.peso_joins.iter().map(|j| j.writer_key.as_str()))
            .chain(self.date_joins.iter().map(|j| j.writer_key.as_str()))
            .collect()
    }

    /// `(writer key, cell)` for every fill target, in file order.
    fn targets(&self) -> Vec<(&str, &str)> {
        let mut out = Vec::new();
        for (key, ids) in &self.joins {
            out.extend(ids.iter().map(|id| (key.as_str(), id.as_str())));
        }
        for join in &self.money_joins {
            for cells in &join.cells {
                out.push((join.writer_key.as_str(), cells.peso_html_id.as_str()));
                if let Some(cent) = &cells.cent_html_id {
                    out.push((join.writer_key.as_str(), cent.as_str()));
                }
            }
        }
        for join in &self.xbox_joins {
            out.extend(
                join.html_ids
                    .iter()
                    .map(|id| (join.writer_key.as_str(), id.as_str())),
            );
        }
        for join in &self.peso_joins {
            out.extend(
                join.html_ids
                    .iter()
                    .map(|id| (join.writer_key.as_str(), id.as_str())),
            );
        }
        for join in &self.date_joins {
            out.push((join.writer_key.as_str(), join.mm_html_id.as_str()));
            if let Some(dd) = &join.dd_html_id {
                out.push((join.writer_key.as_str(), dd.as_str()));
            }
            out.push((join.writer_key.as_str(), join.yyyy_html_id.as_str()));
        }
        out
    }
}

fn required_str<'a>(row: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    row.get(field)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
}

/// A row's cell ids: `single` (`"html_id"`) and/or `plural`
/// (`"html_ids": [...]`), at least one, each a cell id and listed once.
fn cell_ids(
    row: &serde_json::Value,
    key: &str,
    single: &str,
    plural: &str,
) -> Result<Vec<String>, String> {
    let mut ids: Vec<String> = Vec::new();
    if let Some(value) = row.get(single) {
        let id = value
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("writer-cells.json {key} {single} must be a cell id"))?;
        ids.push(id.to_string());
    }
    if let Some(value) = row.get(plural) {
        let list = value
            .as_array()
            .filter(|list| !list.is_empty())
            .ok_or_else(|| format!("writer-cells.json {key} {plural} must list cell ids"))?;
        for id in list {
            let id = id
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("writer-cells.json {key} {plural} must list cell ids"))?;
            ids.push(id.to_string());
        }
    }
    if ids.is_empty() {
        return Err(format!("writer-cells.json {key} missing {single}"));
    }
    for (at, id) in ids.iter().enumerate() {
        if id.starts_with("frm") {
            return Err(format!(
                "writer-cells.json {key} must target a cell id, not {id}"
            ));
        }
        if ids[..at].contains(id) {
            return Err(format!("writer-cells.json {key} lists {id} twice"));
        }
    }
    Ok(ids)
}

/// [`cell_ids`] for a row where these cells are optional: empty when the
/// row names neither field.
fn optional_cell_ids(
    row: &serde_json::Value,
    key: &str,
    single: &str,
    plural: &str,
) -> Result<Vec<String>, String> {
    if row.get(single).is_none() && row.get(plural).is_none() {
        return Ok(Vec::new());
    }
    cell_ids(row, key, single, plural)
}

fn join_rows<'a>(
    payload: &'a serde_json::Value,
    section: &str,
) -> Result<&'a [serde_json::Value], String> {
    match payload.get(section) {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("writer-cells.json {section} must be a list")),
    }
}

fn row_key<'a>(row: &'a serde_json::Value, section: &str) -> Result<&'a str, String> {
    required_str(row, "writer_key")
        .ok_or_else(|| format!("writer-cells.json {section} row missing writer_key"))
}

fn parse_writer_cells(json: &str) -> Result<WriterCells, String> {
    let payload: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("writer-cells.json: {error}"))?;
    let object = payload
        .as_object()
        .ok_or_else(|| "writer-cells.json must be an object".to_string())?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !WRITER_CELLS_SECTIONS.contains(&key.as_str()))
    {
        return Err(format!("writer-cells.json unknown section {unknown}"));
    }
    if payload
        .get("joins")
        .and_then(|value| value.as_array())
        .is_none()
    {
        return Err("writer-cells.json missing joins[]".to_string());
    }
    let mut cells = WriterCells::empty();
    // One writer key is one row across every join kind; a key that prints
    // in several boxes lists them all on that row.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut claim = |key: &str| -> Result<(), String> {
        if seen.insert(key.to_string()) {
            Ok(())
        } else {
            Err(format!(
                "writer-cells.json duplicate writer_key {key} across joins"
            ))
        }
    };

    for row in join_rows(&payload, "joins")? {
        let key = row_key(row, "join")?;
        let ids = cell_ids(row, key, "html_id", "html_ids")?;
        claim(key)?;
        if let (Some(first), Some(caption)) = (ids.first(), required_str(row, "printed")) {
            cells.captions.insert(first.clone(), caption.to_string());
        }
        cells.joins.insert(key.to_string(), ids);
    }

    for row in join_rows(&payload, "money_joins")? {
        let key = row_key(row, "money_join")?;
        let pesos = optional_cell_ids(row, key, "peso_html_id", "peso_html_ids")?;
        let cents = optional_cell_ids(row, key, "cent_html_id", "cent_html_ids")?;
        let decimals = optional_cell_ids(row, key, "decimal_html_id", "decimal_html_ids")?;
        let point_slot = match row.get("point_slot") {
            None => false,
            Some(value) => value.as_bool().ok_or_else(|| {
                format!("writer-cells.json {key} point_slot must be true or false")
            })?,
        };
        if pesos.len() != cents.len() {
            return Err(format!(
                "writer-cells.json {key} pairs {} peso cells with {} cent cells",
                pesos.len(),
                cents.len()
            ));
        }
        if pesos.is_empty() && decimals.is_empty() {
            return Err(format!("writer-cells.json {key} missing peso_html_id"));
        }
        claim(key)?;
        let mut pairs = Vec::new();
        for (peso_html_id, cent_html_id) in pesos.into_iter().zip(cents) {
            if peso_html_id == cent_html_id {
                return Err(format!(
                    "writer-cells.json {key} peso and cent cells must differ"
                ));
            }
            pairs.push(MoneyCells {
                peso_html_id,
                cent_html_id: Some(cent_html_id),
                point_slot: false,
            });
        }
        pairs.extend(decimals.into_iter().map(|peso_html_id| MoneyCells {
            peso_html_id,
            cent_html_id: None,
            point_slot,
        }));
        cells.money_joins.push(MoneyJoin {
            writer_key: key.to_string(),
            cells: pairs,
        });
    }

    for row in join_rows(&payload, "xbox_joins")? {
        let key = row_key(row, "xbox_join")?;
        let html_ids = cell_ids(row, key, "html_id", "html_ids")?;
        claim(key)?;
        cells.xbox_joins.push(XboxJoin {
            writer_key: key.to_string(),
            html_ids,
        });
    }

    for row in join_rows(&payload, "peso_joins")? {
        let key = row_key(row, "peso_join")?;
        let html_ids = cell_ids(row, key, "html_id", "html_ids")?;
        claim(key)?;
        cells.peso_joins.push(PesoJoin {
            writer_key: key.to_string(),
            html_ids,
        });
    }

    for row in join_rows(&payload, "date_joins")? {
        let key = row_key(row, "date_join")?;
        let one = |field: &str| -> Result<Option<String>, String> {
            match row.get(field) {
                None => Ok(None),
                Some(value) => value
                    .as_str()
                    .filter(|id| !id.is_empty() && !id.starts_with("frm"))
                    .map(|id| Some(id.to_string()))
                    .ok_or_else(|| format!("writer-cells.json {key} {field} must be a cell id")),
            }
        };
        let mm_html_id = one("mm_html_id")?
            .ok_or_else(|| format!("writer-cells.json {key} missing mm_html_id"))?;
        let yyyy_html_id = one("yyyy_html_id")?
            .ok_or_else(|| format!("writer-cells.json {key} missing yyyy_html_id"))?;
        let dd_html_id = one("dd_html_id")?;
        claim(key)?;
        cells.date_joins.push(DateJoin {
            writer_key: key.to_string(),
            mm_html_id,
            dd_html_id,
            yyyy_html_id,
        });
    }

    // A not_joined row names one key (`writer_key`) or a family sharing one
    // reason (`writer_keys`, e.g. table rows past the sheet's last row).
    for row in join_rows(&payload, "not_joined")? {
        let keys: Vec<&str> = match (required_str(row, "writer_key"), row.get("writer_keys")) {
            (Some(key), None) => vec![key],
            (None, Some(list)) => list
                .as_array()
                .filter(|list| !list.is_empty())
                .and_then(|list| list.iter().map(|key| key.as_str()).collect())
                .ok_or_else(|| {
                    "writer-cells.json not_joined writer_keys must list keys".to_string()
                })?,
            _ => {
                return Err(
                    "writer-cells.json not_joined row needs writer_key or writer_keys".to_string(),
                );
            }
        };
        let reason = required_str(row, "reason")
            .ok_or_else(|| format!("writer-cells.json not_joined {} needs a reason", keys[0]))?;
        for key in keys {
            if key.is_empty() {
                return Err("writer-cells.json not_joined lists an empty key".to_string());
            }
            if cells.joined_keys().contains(key) {
                return Err(format!(
                    "writer-cells.json {key} is joined; remove it from not_joined"
                ));
            }
            if cells
                .not_joined
                .insert(key.to_string(), reason.to_string())
                .is_some()
            {
                return Err(format!("writer-cells.json duplicate not_joined {key}"));
            }
        }
    }

    let mut used_cells: BTreeSet<String> = BTreeSet::new();
    for (key, id) in cells.targets() {
        if !used_cells.insert(id.to_string()) {
            return Err(format!(
                "writer-cells.json {key} cell {id} is already a fill target"
            ));
        }
    }
    {
        for row in join_rows(&payload, "not_joined_cells")? {
            let id = required_str(row, "html_id").ok_or_else(|| {
                "writer-cells.json not_joined_cells row missing html_id".to_string()
            })?;
            let reason = required_str(row, "reason")
                .ok_or_else(|| format!("writer-cells.json not_joined_cells {id} needs a reason"))?;
            if used_cells.contains(id) {
                return Err(format!(
                    "writer-cells.json cell {id} is a fill target; remove it from not_joined_cells"
                ));
            }
            if cells
                .not_joined_cells
                .insert(id.to_string(), reason.to_string())
                .is_some()
            {
                return Err(format!("writer-cells.json duplicate not_joined_cells {id}"));
            }
        }
    }

    Ok(cells)
}

fn writer_cells(slug: &str) -> Result<WriterCells, String> {
    let mut cells = match writer_cells_json(slug) {
        Some(json) => parse_writer_cells(json)?,
        None => WriterCells::empty(),
    };
    merge_sheet_joins(&mut cells, sheet_joins(slug));
    add_donor_estate_joins(slug, &mut cells);
    Ok(cells)
}

/// `slug`'s [`SHEET_JOINS`].
fn sheet_joins(slug: &str) -> &'static [SheetJoin] {
    SHEET_JOINS
        .iter()
        .find(|(sheet, _)| *sheet == slug)
        .map_or(&[], |(_, joins)| *joins)
}

/// Add `joins` to `cells`, each key leaving `not_joined`. A join whose key
/// writer-cells.json already joins, or whose cell it already fills, is
/// skipped: a later re-sync that adds the same join never fills a cell
/// twice (two replacements of one input would overlap).
fn merge_sheet_joins(cells: &mut WriterCells, joins: &[SheetJoin]) {
    for (key, cell, _) in joins {
        let filled: BTreeSet<String> = cells
            .targets()
            .into_iter()
            .map(|(_, cell)| cell.to_string())
            .collect();
        if cells.joins.contains_key(*key) || filled.contains(*cell) {
            continue;
        }
        cells.not_joined.remove(*key);
        cells
            .joins
            .entry((*key).to_string())
            .or_default()
            .push((*cell).to_string());
    }
}

/// `slug`'s writer-cells.json as JSON with [`SHEET_JOINS`] merged in the
/// way the print merges them (the same skip rules): each join added to
/// `joins` with its printed caption, its key taken off `not_joined`. The one
/// source the screen (comb capacity, the audit's not-joined reasons) and
/// the print share. `None` for a sheet with no writer-cells.json.
pub fn merged_writer_cells_json(slug: &str) -> Option<serde_json::Value> {
    let mut cells: serde_json::Value = serde_json::from_str(writer_cells_json(slug)?).ok()?;
    let mut joins: Vec<serde_json::Value> = cells["joins"].as_array().cloned().unwrap_or_default();
    for (key, cell, printed) in sheet_joins(slug) {
        let taken = joins
            .iter()
            .any(|join| join["writer_key"] == *key || join["html_id"] == *cell);
        if taken {
            continue;
        }
        joins.push(serde_json::json!({
            "writer_key": key,
            "html_id": cell,
            "printed": printed,
            "reason": "frozen_html.rs SHEET_JOINS (the frozen writer-cells.json leaves it out)",
        }));
        if let Some(rows) = cells["not_joined"].as_array_mut() {
            for row in rows.iter_mut() {
                if let Some(list) = row["writer_keys"].as_array_mut() {
                    list.retain(|k| k != *key);
                }
            }
            rows.retain(|row| {
                row["writer_key"] != *key
                    && row["writer_keys"]
                        .as_array()
                        .is_none_or(|list| !list.is_empty())
            });
        }
    }
    cells["joins"] = serde_json::Value::Array(joins);
    Some(cells)
}

/// `slug`'s [`SHEET_JOINS`] that print in a comb: `(writer key, comb
/// slots, printed caption)`, read from the bundled sheet, for the screen's
/// character boxes.
pub fn sheet_join_combs(slug: &str) -> Vec<(String, usize, String)> {
    let Some(loaded) = bundle(slug) else {
        return Vec::new();
    };
    let merged = merged_writer_cells_json(slug).unwrap_or(serde_json::Value::Null);
    sheet_joins(slug)
        .iter()
        .filter(|(key, cell, _)| {
            merged["joins"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|join| {
                    join["writer_key"] == *key
                        && join["html_id"] == *cell
                        && join["reason"]
                            .as_str()
                            .is_some_and(|r| r.starts_with("frozen_html.rs SHEET_JOINS"))
                })
        })
        .filter_map(|(key, cell, printed)| {
            let open = loaded.html.find(&format!("<div id=\"{cell}\""))?;
            let tag = &loaded.html[open..open + loaded.html[open..].find('>')?];
            let slots = attr(tag, "data-comb-slots")?.parse().ok()?;
            Some((key.to_string(), slots, printed.to_string()))
        })
        .collect()
}

/// What a PR 10 Rust-side join prints in its cell.
#[derive(Clone, Copy)]
enum SheetValue {
    /// The writer key's own value.
    Key(&'static str),
    /// The writer key's value as a `derived:line:` value: spread over the
    /// comb, or printed whole on one line when it is longer.
    Line(&'static str),
    /// The writer keys' values one after another (a TIN kept in three
    /// boxes, printed in one comb).
    Concat(&'static [&'static str]),
    /// Month, day and year keys in one `MMDDYY` comb, each typed part in its
    /// own two slots (the year's last two digits); a part not chosen (`00`)
    /// leaves its slots blank.
    ShortDate(&'static str, &'static str, &'static str),
    /// A tick: `X` in the box while the writer key is `true`.
    Tick(&'static str),
}

/// PR 10: joins the frozen 1800 and 1801 sheets' writer-cells.json leaves
/// out although the sheet has the cell (html-frozen is synced from upstream
/// and never edited here), `(sheet, cell, value)`. Each join supersedes the
/// `not_joined` / `not_joined_cells` reason writer-cells.json gives for its
/// key and cell; the sheet's own geometry and printed text
/// (html-frozen/<slug>/index.html) place it:
/// - 1801 p1c4: item 1 "Date of Death (MM/DD/YY)", one 6-slot comb
///   (data-comb-slots 6). Supersedes "item 1 Date of Death: one 6-slot comb
///   (p1c4) for MMDDYY; three writer keys onto one cell is not a join": the
///   join is not a writer-cells text join but [`fill_donor_estate_dates`],
///   which puts each part in its own two slots.
/// - 1801 p1c31 / p1c32: item 11 "TIN of Executor/Administrator". p1c31
///   has 9 inputs at slot indices 0-2, 4-6 and 8-10 (the printed dashes
///   take slots 3 and 7) and its first slot starts 114.74pt in, past the
///   caption; p1c32 is the 5-slot branch comb, first slot 14.28pt in.
///   Supersedes "executor TIN (item 11): one mixed 11-slot comb (p1c31)
///   with printed dashes inside; not split": the nine digits fill the nine
///   inputs in order.
/// - 1801 p1c33: item 12 "Contact Number", 11 slots, the first 56.64pt in,
///   past the caption. Supersedes "item 12 Contact Number comb (p1c33) is
///   mixed: its printed caption sits inside the slots".
/// - 1801 p1c38: item 14A "If yes, specify", 15 slots. Supersedes "item 14A
///   'If yes, specify': the writer holds an option code, not printable
///   text": 14A is the filer's text on this form.
/// - 1801 item 15D's frequency boxes, each the box left of its printed
///   choice on the row at top 344.42pt: p1c50 Monthly (x 303.12pt, text at
///   315.41pt), p1c53 Quarterly (352.2pt; 364.63pt), p1c51 Semi-Annual
///   (402.48pt; 415.27pt), p1c52 Others (459.72pt; 471.7pt). Supersedes
///   "frequency-of-payment options (p1c50 to p1c53): no printed option
///   labels in the text layer, so not guessed": the labels are in the text
///   layer at those positions. p1c197 is the input after "(specify)" (its
///   input starts 46.58pt into the cell, past the text at 491.86pt).
///   Supersedes "frequency-of-payment 'other' specify: no matching labelled
///   cell".
/// - 1801 p1c201: the blank box at the end of item 21's caption "...to be
///   paid on or before" (its input starts 289.65pt into the caption cell).
///   Supersedes "no fillable cell on the frozen sheet for this key
///   (installment year blank)".
/// - 1800 p1c26: item 8A "ZIP Code", 4 slots after the caption (the first
///   slot starts 56pt into the cell). Supersedes "8A ZIP cell p1c26 is
///   mixed: the printed '8A ZIP Code' caption overlaps its slots".
///   p1c57: item 13A "If yes, specify", 19 slots. Supersedes "item 13A 'If
///   yes, specify': the writer holds an option code, not printable text".
const DONOR_ESTATE_JOINS: &[(&str, &str, SheetValue)] = &[
    (
        "1801-2018",
        "p1c4",
        SheetValue::ShortDate(
            "frm1801v2018:txtDateMonth",
            "frm1801v2018:txtDateDay",
            "frm1801v2018:txtDateYear",
        ),
    ),
    (
        "1801-2018",
        "p1c31",
        SheetValue::Concat(&[
            "frm1801v2018:txtTINE1",
            "frm1801v2018:txtTINE2",
            "frm1801v2018:txtTINE3",
        ]),
    ),
    (
        "1801-2018",
        "p1c32",
        SheetValue::Key("frm1801v2018:txtBranchCodeE"),
    ),
    (
        "1801-2018",
        "p1c33",
        SheetValue::Line("frm1801v2018:telephoneNumber"),
    ),
    (
        "1801-2018",
        "p1c38",
        SheetValue::Line("frm1801v2018:treatyY"),
    ),
    (
        "1801-2018",
        "p1c50",
        SheetValue::Tick("frm1801v2018:freq_Mon"),
    ),
    (
        "1801-2018",
        "p1c53",
        SheetValue::Tick("frm1801v2018:freq_Qtr"),
    ),
    (
        "1801-2018",
        "p1c51",
        SheetValue::Tick("frm1801v2018:freq_Semi"),
    ),
    (
        "1801-2018",
        "p1c52",
        SheetValue::Tick("frm1801v2018:freq_Other"),
    ),
    (
        "1801-2018",
        "p1c197",
        SheetValue::Key("frm1801v2018:freqOtherInput"),
    ),
    (
        "1801-2018",
        "p1c201",
        SheetValue::Key("frm1801v2018:txtInstallmentYear"),
    ),
    (
        "1800-2018",
        "p1c26",
        SheetValue::Key("frm1800v2018:txtZipCode"),
    ),
    (
        "1800-2018",
        "p1c57",
        SheetValue::Line("frm1800v2018:TaxTreaty"),
    ),
];

/// How a PR 10 sheet prints a writer value (print only; the upload is
/// unchanged).
#[derive(Clone, Copy)]
enum SheetRewrite {
    /// A date part: `00` (not chosen) prints nothing.
    ZeroBlank,
    /// A year in a two-slot `YY` comb: its last two digits; `0` prints
    /// nothing.
    TwoDigitYear,
    /// Without its surrounding spaces.
    Trim,
    /// Digits padded with leading zeros to the comb's slots.
    ZeroPad(usize),
}

/// PR 10's print rewrites, `(sheet, writer key, rewrite)`. 1800 item 1
/// "Date of Donation (MM/DD/YY)": writer-cells.json joins txtMonth to p1c5,
/// txtDate to p1c6 and txtYear to p1c7, each a 2-slot comb; form_1800.rs
/// writes the 4-digit year and `00`/`0` for parts not chosen, which the
/// page shows blank. 1800 8A's ZIP is trimmed; 1801 item 11's branch code
/// fills its 5-slot comb (p1c32).
const DONOR_ESTATE_REWRITES: &[(&str, &str, SheetRewrite)] = &[
    (
        "1800-2018",
        "frm1800v2018:txtMonth",
        SheetRewrite::ZeroBlank,
    ),
    ("1800-2018", "frm1800v2018:txtDate", SheetRewrite::ZeroBlank),
    (
        "1800-2018",
        "frm1800v2018:txtYear",
        SheetRewrite::TwoDigitYear,
    ),
    ("1800-2018", "frm1800v2018:txtZipCode", SheetRewrite::Trim),
    (
        "1801-2018",
        "frm1801v2018:txtBranchCodeE",
        SheetRewrite::ZeroPad(5),
    ),
];

/// A date part's digits, or `None` for a part not chosen (blank, all
/// zeros) or not a number.
fn date_part(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) && text.chars().any(|c| c != '0'))
        .then_some(text)
}

/// A date part as the two slots it prints in: a month or day padded, a
/// year's last two digits. `None` when it prints nothing.
fn two_slots(text: &str, year: bool) -> Option<String> {
    let digits = date_part(text)?;
    match (year, digits.len()) {
        (true, n) if n >= 2 => Some(digits[n - 2..].to_string()),
        (false, 1 | 2) => Some(format!("{digits:0>2}")),
        _ => None,
    }
}

/// The writer key a [`DONOR_ESTATE_JOINS`] value prints under: the key
/// itself, or a derived key named after its cell.
fn donor_estate_key(cell: &str, value: SheetValue) -> String {
    match value {
        SheetValue::Key(key) | SheetValue::Tick(key) => key.to_string(),
        SheetValue::Line(_) => format!("derived:line:pr10:{cell}"),
        SheetValue::Concat(_) | SheetValue::ShortDate(..) => format!("derived:pr10:{cell}"),
    }
}

/// The writer keys a derived [`DONOR_ESTATE_JOINS`] value reads.
fn donor_estate_sources(value: SheetValue) -> Vec<&'static str> {
    match value {
        SheetValue::Key(_) | SheetValue::Tick(_) => Vec::new(),
        SheetValue::Line(key) => vec![key],
        SheetValue::Concat(keys) => keys.to_vec(),
        SheetValue::ShortDate(month, day, year) => vec![month, day, year],
    }
}

/// `slug`'s [`DONOR_ESTATE_JOINS`] merged into its writer cells: a join
/// for each (a date is filled by [`fill_donor_estate_dates`] instead), its
/// keys out of `not_joined` and its cell out of `not_joined_cells`. A key
/// or cell writer-cells.json already joins is left as it is (a later
/// re-sync must not fill a cell twice).
fn add_donor_estate_joins(slug: &str, cells: &mut WriterCells) {
    for (sheet, cell, value) in DONOR_ESTATE_JOINS {
        if *sheet != slug {
            continue;
        }
        let key = donor_estate_key(cell, *value);
        let filled = cells.targets().iter().any(|(_, target)| target == cell);
        if filled || cells.joined_keys().contains(key.as_str()) {
            continue;
        }
        match value {
            SheetValue::Tick(_) => cells.xbox_joins.push(XboxJoin {
                writer_key: key.clone(),
                html_ids: vec![(*cell).to_string()],
            }),
            SheetValue::ShortDate(..) => {}
            _ => {
                cells.joins.insert(key.clone(), vec![(*cell).to_string()]);
            }
        }
        cells.not_joined.remove(&key);
        for source in donor_estate_sources(*value) {
            cells.not_joined.remove(source);
        }
        cells.not_joined_cells.remove(*cell);
    }
}

/// `fields` with `slug`'s print rewrites applied and its derived
/// [`DONOR_ESTATE_JOINS`] values added.
fn with_donor_estate_values(
    slug: &str,
    fields: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = fields.clone();
    for (sheet, key, rewrite) in DONOR_ESTATE_REWRITES {
        if *sheet != slug {
            continue;
        }
        let Some(value) = out.get_mut(*key) else {
            continue;
        };
        *value = match rewrite {
            SheetRewrite::ZeroBlank => two_slots(value, false).unwrap_or_default(),
            SheetRewrite::TwoDigitYear => two_slots(value, true).unwrap_or_default(),
            SheetRewrite::Trim => value.trim().to_string(),
            SheetRewrite::ZeroPad(width) => {
                let text = value.trim();
                if !text.is_empty()
                    && text.len() < *width
                    && text.chars().all(|c| c.is_ascii_digit())
                {
                    format!("{text:0>width$}", width = *width)
                } else {
                    text.to_string()
                }
            }
        };
    }
    let get = |key: &str| fields.get(key).map(|v| v.trim()).unwrap_or("");
    for (sheet, cell, value) in DONOR_ESTATE_JOINS {
        if *sheet != slug {
            continue;
        }
        let printed = match value {
            SheetValue::Key(_) | SheetValue::Tick(_) | SheetValue::ShortDate(..) => continue,
            SheetValue::Line(key) => get(key).to_string(),
            SheetValue::Concat(keys) => keys.iter().map(|key| get(key)).collect(),
        };
        if !printed.is_empty() {
            out.insert(donor_estate_key(cell, *value), printed);
        }
    }
    out
}

/// Each [`SheetValue::ShortDate`] of `slug`: every typed part in its own
/// two slots of the cell; a part not chosen leaves its slots as they are.
fn fill_donor_estate_dates(html: &str, fields: &BTreeMap<String, String>, slug: &str) -> String {
    let tags = input_tags(html);
    let grouped = grouped_input_indices(&tags);
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    for (sheet, cell, value) in DONOR_ESTATE_JOINS {
        let (true, SheetValue::ShortDate(month, day, year)) = (*sheet == slug, value) else {
            continue;
        };
        let ordered = grouped
            .get(*cell)
            .map(|indices| ordered_comb_indices(&tags, indices))
            .unwrap_or_default();
        if ordered.len() != 6 {
            continue;
        }
        for (at, (key, is_year)) in [(*month, false), (*day, false), (*year, true)]
            .into_iter()
            .enumerate()
        {
            let get = fields.get(key).map(String::as_str).unwrap_or("");
            let Some(digits) = two_slots(get, is_year) else {
                continue;
            };
            for (offset, ch) in digits.chars().enumerate() {
                let index = ordered[at * 2 + offset];
                replacements.push((
                    tags[index].start,
                    tags[index].end,
                    set_value(tags[index].tag, &ch.to_string(), None),
                ));
            }
        }
    }
    apply_replacements(html, replacements)
}

/// Whether `slug`'s PR 10 joins print the value `value` of writer key
/// `key` (`Some(true)`), hold nothing to print (`Some(false)`: a date part
/// not chosen, as the page shows it blank), or do not read the key
/// (`None`). A date part counts as printed only when it reaches the sheet.
fn donor_estate_prints(slug: &str, key: &str, value: &str) -> Option<bool> {
    DONOR_ESTATE_JOINS
        .iter()
        .filter(|(sheet, _, _)| *sheet == slug)
        .find_map(|(_, _, joined)| match joined {
            SheetValue::ShortDate(month, day, _) if key == *month || key == *day => {
                Some(two_slots(value, false).is_some())
            }
            SheetValue::ShortDate(_, _, year) if key == *year => {
                Some(two_slots(value, true).is_some())
            }
            other => donor_estate_sources(*other).contains(&key).then_some(true),
        })
}

/// Whether `value` is a PR 10 date part of `slug` not chosen (all zeros),
/// which the page and the print show blank.
fn donor_estate_unchosen(slug: &str, key: &str, value: &str) -> bool {
    let value = value.trim();
    DONOR_ESTATE_JOINS.iter().any(|(sheet, _, joined)| {
        *sheet == slug
            && matches!(joined, SheetValue::ShortDate(..))
            && donor_estate_sources(*joined).contains(&key)
    }) && !value.is_empty()
        && value.chars().all(|c| c == '0')
}

fn grouped_input_indices<'a>(tags: &[InputTag<'a>]) -> BTreeMap<&'a str, Vec<usize>> {
    let mut grouped: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, tag) in tags.iter().enumerate() {
        grouped.entry(tag.name).or_default().push(index);
    }
    grouped
}

fn ordered_comb_indices(tags: &[InputTag<'_>], indices: &[usize]) -> Vec<usize> {
    let mut ordered = indices.to_vec();
    ordered.sort_by_key(|&index| tags[index].slot.unwrap_or(usize::MAX));
    ordered
}

/// Splits a writer amount into `(negative, peso digits, two cent digits)`.
fn split_writer_money(value: &str) -> Option<(bool, String, String)> {
    let cleaned: String = value
        .chars()
        .filter(|ch| *ch != ',' && !ch.is_whitespace())
        .collect();
    let (negative, cleaned) = match cleaned.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, cleaned.as_str()),
    };
    if cleaned.is_empty() {
        return None;
    }
    let (peso, cents) = match cleaned.split_once('.') {
        Some((peso, cents)) => (peso, cents),
        None => (cleaned, "00"),
    };
    if peso.chars().any(|ch| !ch.is_ascii_digit()) {
        return None;
    }
    if cents.is_empty() || cents.len() > 2 || cents.chars().any(|ch| !ch.is_ascii_digit()) {
        return None;
    }
    Some((negative, peso.to_string(), format!("{cents:0<2}")))
}

/// Puts the minus sign immediately left of the peso digits; a zero amount
/// (`-0`, `-0.00`) prints unsigned.
fn signed_digits(negative: bool, digits: &str, cents: &str) -> String {
    let zero = digits.chars().chain(cents.chars()).all(|ch| ch == '0');
    if negative && !zero {
        format!("-{digits}")
    } else {
        digits.to_string()
    }
}

fn right_aligned_slot_values(slots: usize, digits: &str) -> Option<Vec<String>> {
    if digits.len() > slots {
        return None;
    }
    let pad = slots - digits.len();
    let mut values = vec![String::new(); slots];
    for (offset, ch) in digits.chars().enumerate() {
        values[pad + offset] = ch.to_string();
    }
    Some(values)
}

fn fill_money_joins(
    html: &str,
    fields: &BTreeMap<String, String>,
    money_joins: &[MoneyJoin],
) -> String {
    if money_joins.is_empty() {
        return html.to_string();
    }
    let tags = input_tags(html);
    let grouped = grouped_input_indices(&tags);
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    for join in money_joins {
        let Some(value) = fields.get(&join.writer_key) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        let Some((negative, peso, cents)) = split_writer_money(value) else {
            continue;
        };
        let peso = signed_digits(negative, &peso, &cents);
        for cells in &join.cells {
            let Some(peso_indices) = grouped.get(cells.peso_html_id.as_str()) else {
                continue;
            };
            let mut peso_ordered = ordered_comb_indices(&tags, peso_indices);
            let cent_ordered = match &cells.cent_html_id {
                Some(cent) => match grouped.get(cent.as_str()) {
                    Some(cent_indices) => ordered_comb_indices(&tags, cent_indices),
                    None => continue,
                },
                // The comb's own last two slots; a point slot before them is
                // left blank (the sheet prints the point there).
                None if peso_ordered.len() >= 5 => {
                    let cents = peso_ordered.split_off(peso_ordered.len() - 2);
                    // The point is a real slot only when the slot indices run
                    // straight into the cents; a sheet that already leaves the
                    // point column out of the comb (an index gap) must not
                    // lose its last peso box to it (2200-S shifted every
                    // amount one box left).
                    let point_is_gap = match (peso_ordered.last(), cents.first()) {
                        (Some(&last), Some(&first)) => matches!(
                            (tags[last].slot, tags[first].slot),
                            (Some(a), Some(b)) if b > a + 1
                        ),
                        _ => false,
                    };
                    if cells.point_slot && !point_is_gap {
                        let point = peso_ordered.pop().expect("comb has a decimal slot");
                        replacements.push((
                            tags[point].start,
                            tags[point].end,
                            set_value(tags[point].tag, "", None),
                        ));
                    }
                    cents
                }
                None => continue,
            };
            if cent_ordered.len() != 2 {
                continue;
            }
            let Some(peso_values) = right_aligned_slot_values(peso_ordered.len(), &peso) else {
                continue;
            };
            for (offset, index) in peso_ordered.into_iter().enumerate() {
                let writer = (offset == 0).then_some(value.as_str());
                replacements.push((
                    tags[index].start,
                    tags[index].end,
                    set_value(tags[index].tag, &peso_values[offset], writer),
                ));
            }
            for (offset, index) in cent_ordered.into_iter().enumerate() {
                let ch = cents
                    .chars()
                    .nth(offset)
                    .map(|ch| ch.to_string())
                    .unwrap_or_default();
                replacements.push((
                    tags[index].start,
                    tags[index].end,
                    set_value(tags[index].tag, &ch, None),
                ));
            }
        }
    }
    apply_replacements(html, replacements)
}

/// `1234.49` -> `1234`, `1234.50` -> `1235`: the sheets' "49 centavos or
/// less drop down; 50 or more round up". Negatives round the absolute value
/// and keep the sign (`-1234.50` -> `-1235`); `-0` prints as `0`.
fn whole_pesos(value: &str) -> Option<String> {
    let (negative, peso, cents) = split_writer_money(value)?;
    let peso = peso.trim_start_matches('0');
    let mut digits: Vec<u8> = if peso.is_empty() {
        b"0".to_vec()
    } else {
        peso.bytes().collect()
    };
    if cents.as_str() >= "50" {
        let mut index = digits.len();
        loop {
            if index == 0 {
                digits.insert(0, b'1');
                break;
            }
            index -= 1;
            if digits[index] == b'9' {
                digits[index] = b'0';
            } else {
                digits[index] += 1;
                break;
            }
        }
    }
    let digits = String::from_utf8(digits).ok()?;
    Some(signed_digits(negative, &digits, ""))
}

fn fill_peso_joins(
    html: &str,
    fields: &BTreeMap<String, String>,
    peso_joins: &[PesoJoin],
) -> String {
    if peso_joins.is_empty() {
        return html.to_string();
    }
    let tags = input_tags(html);
    let grouped = grouped_input_indices(&tags);
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    for join in peso_joins {
        let Some(value) = fields.get(&join.writer_key) else {
            continue;
        };
        let Some(pesos) = whole_pesos(value) else {
            continue;
        };
        for html_id in &join.html_ids {
            let Some(indices) = grouped.get(html_id.as_str()) else {
                continue;
            };
            let ordered = ordered_comb_indices(&tags, indices);
            let Some(values) = right_aligned_slot_values(ordered.len(), &pesos) else {
                continue;
            };
            for (offset, index) in ordered.into_iter().enumerate() {
                let writer = (offset == 0).then_some(value.as_str());
                replacements.push((
                    tags[index].start,
                    tags[index].end,
                    set_value(tags[index].tag, &values[offset], writer),
                ));
            }
        }
    }
    apply_replacements(html, replacements)
}

/// Splits a writer date into `(MM, DD, YYYY)`: `MM/DD/YYYY`, `MM/YYYY`
/// (no day) or ISO `YYYY-MM-DD`. Month and day are zero-padded to two
/// digits; anything else is not a date and prints nowhere.
fn split_writer_date(value: &str) -> Option<(String, Option<String>, String)> {
    let value = value.trim();
    let parts: Vec<&str> = if value.contains('-') {
        let iso: Vec<&str> = value.split('-').collect();
        match iso.as_slice() {
            [year, month, day] if year.len() == 4 => vec![month, day, year],
            _ => return None,
        }
    } else {
        value.split('/').collect()
    };
    if parts
        .iter()
        .any(|part| part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()))
    {
        return None;
    }
    let two = |part: &str| (part.len() <= 2).then(|| format!("{part:0>2}"));
    match parts.as_slice() {
        [month, day, year] if year.len() == 4 => {
            Some((two(month)?, Some(two(day)?), year.to_string()))
        }
        [month, year] if year.len() == 4 => Some((two(month)?, None, year.to_string())),
        _ => None,
    }
}

fn fill_date_joins(
    html: &str,
    fields: &BTreeMap<String, String>,
    date_joins: &[DateJoin],
) -> String {
    if date_joins.is_empty() {
        return html.to_string();
    }
    let tags = input_tags(html);
    let grouped = grouped_input_indices(&tags);
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    for join in date_joins {
        let Some(value) = fields.get(&join.writer_key) else {
            continue;
        };
        let Some((month, day, year)) = split_writer_date(value) else {
            continue;
        };
        // A day the sheet has no comb for (or a comb with no day to fill)
        // is not this date's shape: print nothing rather than part of it.
        let day_cells = match (&join.dd_html_id, day) {
            (Some(html_id), Some(day)) => Some((html_id.as_str(), day)),
            (None, None) => None,
            _ => continue,
        };
        let combs = std::iter::once((join.mm_html_id.as_str(), month))
            .chain(day_cells)
            .chain(std::iter::once((join.yyyy_html_id.as_str(), year)));
        let mut pending = Vec::new();
        let mut fits = true;
        for (at, (html_id, mut digits)) in combs.enumerate() {
            let ordered = grouped
                .get(html_id)
                .map(|indices| ordered_comb_indices(&tags, indices))
                .unwrap_or_default();
            // A two-slot year comb takes the year's last two digits.
            if ordered.len() == 2 && digits.len() == 4 {
                digits = digits[2..].to_string();
            }
            if ordered.len() != digits.len() {
                fits = false;
                break;
            }
            for (offset, (index, ch)) in ordered.into_iter().zip(digits.chars()).enumerate() {
                let writer = (at == 0 && offset == 0).then_some(value.as_str());
                pending.push((
                    tags[index].start,
                    tags[index].end,
                    set_value(tags[index].tag, &ch.to_string(), writer),
                ));
            }
        }
        if fits {
            replacements.extend(pending);
        }
    }
    apply_replacements(html, replacements)
}

fn writer_is_checked(value: &str) -> bool {
    matches!(value.trim(), "true" | "TRUE" | "1" | "Y" | "y" | "X" | "x")
}

fn fill_xbox_joins(
    html: &str,
    fields: &BTreeMap<String, String>,
    xbox_joins: &[XboxJoin],
) -> String {
    if xbox_joins.is_empty() {
        return html.to_string();
    }
    let tags = input_tags(html);
    let grouped = grouped_input_indices(&tags);
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    for join in xbox_joins {
        let Some(value) = fields.get(&join.writer_key) else {
            continue;
        };
        if !writer_is_checked(value) {
            continue;
        }
        for html_id in &join.html_ids {
            let Some(index) = xbox_input(&tags, &grouped, html_id) else {
                continue;
            };
            // Char boxes carry `fc` (text-align:center); the catalog xbox inputs
            // do not, so the X sat against the box's left wall.
            replacements.push((
                tags[index].start,
                tags[index].end,
                with_class(
                    &set_value(tags[index].tag, XBOX_CHECKED_GLYPH, Some(value.as_str())),
                    "fc",
                ),
            ));
        }
    }
    apply_replacements(html, replacements)
}

/// The one input an xbox join marks: the single unslotted input of cell
/// `html_id`, or, in a cell that holds several boxes under one name, the
/// box whose input id is `html_id` (`<cell>-i<n>`).
fn xbox_input(
    tags: &[InputTag<'_>],
    grouped: &BTreeMap<&str, Vec<usize>>,
    html_id: &str,
) -> Option<usize> {
    let index = match grouped.get(html_id) {
        Some(indices) if indices.len() == 1 => indices[0],
        Some(_) => return None,
        None => tags
            .iter()
            .position(|tag| attr(tag.tag, "id") == Some(html_id))?,
    };
    tags[index].slot.is_none().then_some(index)
}

/// The tag with `class` present in its class list (added if missing).
fn with_class(tag: &str, class: &str) -> String {
    let needle = "class=\"";
    let Some(at) = tag.find(needle) else {
        let insert_at = tag.rfind('>').unwrap_or(tag.len());
        return format!(
            "{} class=\"{class}\"{}",
            &tag[..insert_at],
            &tag[insert_at..]
        );
    };
    let start = at + needle.len();
    let Some(len) = tag[start..].find('"') else {
        return tag.to_string();
    };
    let classes = &tag[start..start + len];
    if classes.split_whitespace().any(|c| c == class) {
        return tag.to_string();
    }
    let joined = if classes.trim().is_empty() {
        class.to_string()
    } else {
        format!("{classes} {class}")
    };
    format!("{}{}{}", &tag[..start], joined, &tag[start + len..])
}

fn validate_writer_cells(html: &str, slug: &str, cells: &WriterCells) -> Result<(), String> {
    if !cells.has_fill_join() && cells.not_joined.is_empty() && cells.not_joined_cells.is_empty() {
        return Ok(());
    }
    let tags = input_tags(html);
    let present: BTreeSet<&str> = tags.iter().map(|tag| tag.name).collect();
    let grouped = grouped_input_indices(&tags);
    let slots = |html_id: &str| grouped.get(html_id).map_or(0, Vec::len);
    let xbox_ids: BTreeSet<&str> = cells
        .xbox_joins
        .iter()
        .flat_map(|join| join.html_ids.iter().map(String::as_str))
        .collect();
    for (key, html_id) in cells.targets() {
        if !present.contains(html_id) && !xbox_ids.contains(html_id) {
            return Err(format!(
                "{slug}: writer-cells html id {html_id} for {key} is not an input name="
            ));
        }
        if present.contains(key) {
            return Err(format!(
                "{slug}: writer-cells {key} is already a stamped name=; remove the fill join"
            ));
        }
    }
    for join in &cells.money_joins {
        for pair in &join.cells {
            // A decimal comb also holds the point and two centavos.
            let least = if pair.cent_html_id.is_some() { 3 } else { 6 };
            if slots(&pair.peso_html_id) < least {
                return Err(format!(
                    "{slug}: writer-cells {} peso comb {} must have at least {least} slots",
                    join.writer_key, pair.peso_html_id
                ));
            }
            if let Some(cent) = pair.cent_html_id.as_ref().filter(|cent| slots(cent) != 2) {
                return Err(format!(
                    "{slug}: writer-cells {} cent comb {cent} must be a 2-slot comb",
                    join.writer_key
                ));
            }
        }
    }
    for join in &cells.peso_joins {
        for html_id in &join.html_ids {
            if slots(html_id) < 3 {
                return Err(format!(
                    "{slug}: writer-cells {} peso comb {html_id} must have at least 3 slots",
                    join.writer_key
                ));
            }
        }
    }
    for join in &cells.date_joins {
        let parts = [
            (Some(&join.mm_html_id), &[2][..]),
            (join.dd_html_id.as_ref(), &[2][..]),
            (Some(&join.yyyy_html_id), &[2, 4][..]),
        ];
        for (html_id, allowed) in parts {
            let Some(html_id) = html_id else {
                continue;
            };
            let indices = grouped.get(html_id.as_str()).cloned().unwrap_or_default();
            let comb = indices.iter().all(|&index| tags[index].slot.is_some());
            if !comb || !allowed.contains(&indices.len()) {
                return Err(format!(
                    "{slug}: writer-cells {} date comb {html_id} must be a {allowed:?}-slot comb",
                    join.writer_key
                ));
            }
        }
    }
    for join in &cells.xbox_joins {
        for html_id in &join.html_ids {
            if xbox_input(&tags, &grouped, html_id).is_none() {
                return Err(format!(
                    "{slug}: writer-cells {} xbox {html_id} must be a single unslotted input",
                    join.writer_key
                ));
            }
        }
    }
    for key in cells.not_joined.keys() {
        if present.contains(key.as_str()) {
            return Err(format!(
                "{slug}: writer-cells not_joined {key} is a stamped name= and prints"
            ));
        }
    }
    for html_id in cells.not_joined_cells.keys() {
        if !html.contains(&format!("<div id=\"{html_id}\"")) {
            return Err(format!(
                "{slug}: writer-cells not_joined_cells {html_id} is not a cell on the sheet"
            ));
        }
    }
    Ok(())
}

fn fill_bundle(
    html: &str,
    fields: &BTreeMap<String, String>,
    slug: &str,
) -> Result<String, String> {
    let cells = writer_cells(slug)?;
    validate_writer_cells(html, slug, &cells)?;
    let fields = &with_donor_estate_values(slug, fields);
    let filled = fill_donor_estate_dates(html, fields, slug);
    let sheet = bundle(slug).map_or_else(SheetType::plain, |loaded| {
        SheetType::of_sheet(slug, loaded.css)
    });
    let plan = line_group_plan(&filled, fields, &cells, &sheet);
    let filled = fill_by_name_set(&filled, fields, &cells.joins, &plan, &sheet);
    let filled = fill_money_joins(&filled, fields, &cells.money_joins);
    let filled = fill_peso_joins(&filled, fields, &cells.peso_joins);
    let filled = fill_date_joins(&filled, fields, &cells.date_joins);
    Ok(split_single_character_cells(
        slug,
        &fill_xbox_joins(&filled, fields, &cells.xbox_joins),
    ))
}

/// One Rust-side join: `(writer key, cell, printed caption)`.
type SheetJoin = (&'static str, &'static str, &'static str);

/// Text joins a frozen sheet's writer-cells.json leaves out although the
/// sheet has the cell (html-frozen is synced from upstream and never edited
/// here), per sheet. 2550M (2007) item 4: the TIN's three 3-slot combs and
/// the 5-slot branch comb of the item 4 row (data-row 7: p1c127, p1c9,
/// p1c11, p1c13). The branch comb's inputs carry the sheet's 00000 as
/// values; the join writes the draft's branch over them.
///
/// For these keys this table supersedes 2550m-2007/writer-cells.json's
/// rule text ("The TIN boxes ... are not fillable cells on this sheet") and
/// its not_joined reason for the TIN row ("the 2007 sheet's TIN row has no
/// fillable cells in the frozen page ... reported on #113"): the frozen page
/// has those cells. writer_cells() and merged_writer_cells_json() merge it,
/// skipping any join writer-cells.json itself gains on a later re-sync.
const SHEET_JOINS: &[(&str, &[SheetJoin])] = &[(
    "2550m-2007",
    &[
        ("frm2550m:txtTIN1", "p1c127", "4 TIN (1-3)"),
        ("frm2550m:txtTIN2", "p1c9", "4 TIN (4-6)"),
        ("frm2550m:txtTIN3", "p1c11", "4 TIN (7-9)"),
        ("frm2550m:txtBranchCode", "p1c13", "4 TIN branch"),
    ],
)];

/// Text cells drawn as a row of one-character inputs that are not a comb
/// (no `data-slot-index`), per sheet, each with how a code shorter than its
/// boxes sits in them. A text join writes its whole value into every input
/// of its cell, so these are re-split one character per box. A value with
/// more characters than boxes prints once as a plain line across the cell
/// (`fill_text_input`). Every joined cell of this shape on the frozen
/// sheets is listed.
const SINGLE_CHARACTER_CELLS: &[(&str, &[(&str, bool)])] = &[
    (
        "1600wp-2010",
        // The attachment's "FOR THE PERIOD (MM/DD/YYYY)": from MM, DD and
        // YYYY, to MM, DD and YYYY, one-character boxes (maxlength 1).
        &[
            ("p1c62", false),
            ("p1c63", false),
            ("p1c64", false),
            ("p1c65", false),
            ("p1c66", false),
            ("p1c67", false),
        ],
    ),
    (
        "2550m-2007",
        // Item 3 sheets (a count, right-aligned), item 5 RDO, item 10 ZIP.
        &[("p1c3", true), ("p1c130", false), ("p1c21", false)],
    ),
];

/// The sheets of [`SINGLE_CHARACTER_CELLS`] whose split cells have been
/// rewritten since they were listed (PR 4) whether a value was written or
/// not. 1600WP's, listed in round 6, are rewritten only when filled, so a
/// document that leaves them empty prints them as before.
const SPLIT_REWRITES_UNFILLED: &[&str] = &["2550m-2007"];

/// One character per input of `slug`'s [`SINGLE_CHARACTER_CELLS`], in the
/// inputs' order. A value with more characters than boxes, or with
/// characters other than letters and digits, was printed by the fill as a
/// line across the cell, its boxes empty, and is left as it is.
fn split_single_character_cells(slug: &str, html: &str) -> String {
    let Some((_, cells)) = SINGLE_CHARACTER_CELLS.iter().find(|(s, _)| *s == slug) else {
        return html.to_string();
    };
    let tags = input_tags(html);
    let grouped = grouped_input_indices(&tags);
    let mut replacements = Vec::new();
    for (cell, right) in *cells {
        let Some(indices) = grouped.get(cell) else {
            continue;
        };
        if !splits(&tags, indices) {
            continue;
        }
        let first = tags[indices[0]].tag;
        let written = attr(first, "data-writer-value").or_else(|| attr(first, "value"));
        // An unfilled cell is left as the sheet draws it, except on the
        // sheets whose split cells every fill has rewritten, filled or not
        // (an empty box gains `value=""`; the goldens pin those bytes).
        if written.is_none() && !SPLIT_REWRITES_UNFILLED.contains(&slug) {
            continue;
        }
        let value = written.unwrap_or("");
        let chars: Vec<char> = value.chars().collect();
        let boxes = indices.len();
        if !fits_one_per_box(value, boxes) {
            continue;
        }
        let pad = if *right {
            boxes.saturating_sub(chars.len())
        } else {
            0
        };
        for (offset, &index) in indices.iter().enumerate() {
            let shown = offset
                .checked_sub(pad)
                .and_then(|at| chars.get(at))
                .map(char::to_string)
                .unwrap_or_default();
            // The writer's whole value stays on the first box only, as a
            // comb keeps it on its first slot.
            let tag = if offset == 0 {
                tags[index].tag.to_string()
            } else {
                without_writer_value(tags[index].tag)
            };
            replacements.push((
                tags[index].start,
                tags[index].end,
                set_value(&tag, &shown, None),
            ));
        }
    }
    apply_replacements(html, replacements)
}

/// `tag` without its `data-writer-value` attribute.
fn without_writer_value(tag: &str) -> String {
    let needle = " data-writer-value=\"";
    let Some(at) = tag.find(needle) else {
        return tag.to_string();
    };
    let rest = &tag[at + needle.len()..];
    match rest.find('"') {
        Some(close) => format!("{}{}", &tag[..at], &rest[close + 1..]),
        None => tag.to_string(),
    }
}

/// Writer keys in `fields` holding a value that `slug`'s sheet prints
/// nowhere: not a stamped `name=`, in no writer-cells join, and not listed
/// in `not_joined` with its reason. The print coverage gate: every value
/// the print map carries has a box, or a reviewed reason it has none.
pub fn unprinted_keys(
    slug: &str,
    fields: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let loaded = bundle(slug).ok_or_else(|| format!("no frozen HTML bundle for {slug}"))?;
    let cells = writer_cells(slug)?;
    validate_writer_cells(loaded.html, slug, &cells)?;
    let tags = input_tags(loaded.html);
    let stamped: BTreeSet<&str> = tags.iter().map(|tag| tag.name).collect();
    let joined = cells.joined_keys();
    Ok(fields
        .iter()
        .filter(|(_, value)| !value.trim().is_empty())
        // A PR 10 date part not chosen (`00`) holds nothing to print.
        .filter(|(key, value)| !donor_estate_unchosen(slug, key, value))
        .filter(|(key, value)| {
            !stamped.contains(key.as_str())
                && !joined.contains(key.as_str())
                && donor_estate_prints(slug, key, value) != Some(true)
                && !cells.not_joined.contains_key(key.as_str())
        })
        .map(|(key, _)| key)
        .cloned()
        .collect())
}

/// The characters a filled document prints in input `name` (a cell id or
/// a stamped key), slots in order: what a reader sees in that box.
pub fn printed_text(document: &str, name: &str) -> String {
    let tags = input_tags(document);
    let indices: Vec<usize> = tags
        .iter()
        .enumerate()
        .filter(|(_, tag)| tag.name == name)
        .map(|(index, _)| index)
        .collect();
    ordered_comb_indices(&tags, &indices)
        .into_iter()
        .map(|index| attr(tags[index].tag, "value").unwrap_or(""))
        .collect()
}

/// Frozen 2551Q HTML with writer values on stamped `name=` inputs,
/// writer-cell identity, money, and xbox joins.
pub fn fill_2551q(draft: &Form2551QDraft) -> String {
    fill_bundle(html_2551q(), &draft.to_print_field_map(), "2551q-2018")
        .expect("2551q-2018 writer-cells")
}

/// Self-contained document for a WebView `with_html` host (inline CSS, fonts, PNGs).
pub fn filled_document(slug: &str, fields: &BTreeMap<String, String>) -> Result<String, String> {
    filled_document_with_receipt(slug, fields, None)
}

/// BIR's receipt confirmation, printed as one more page after the form.
///
/// Before the HTML-only print pipeline the PDF preview appended this from
/// the receipt row linked by `draft.receipt_id`; the migration dropped it.
/// `body_html` must already be sanitised by the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiptPage {
    pub filename: String,
    pub subject: String,
    pub from: String,
    pub to: String,
    pub received_at: String,
    /// When the email arrived, already formatted for display; falls back to
    /// `received_at` (BIR's stamp) when the row predates the column.
    pub email_received_at: Option<String>,
    pub body_text: String,
    pub body_html: Option<String>,
}

/// `filled_document` plus, when the return is confirmed, the receipt page.
pub fn filled_document_with_receipt(
    slug: &str,
    fields: &BTreeMap<String, String>,
    receipt: Option<&ReceiptPage>,
) -> Result<String, String> {
    let Some(loaded) = bundle(slug) else {
        return Err(format!("no frozen HTML bundle for {slug}"));
    };
    let mut document = inline_local_assets(&fill_bundle(loaded.html, fields, slug)?, &loaded);
    if let Some(receipt) = receipt {
        let (width, height) = page_size_from_css(loaded.css);
        document = append_before_body_end(&document, &receipt_page_html(receipt, &width, &height));
    }
    Ok(document)
}

/// `@page { size:612pt 936pt; … }` from the bundle's form.css, so the receipt
/// sheet matches the form's sheet; letter-long fallback if a bundle omits it.
fn page_size_from_css(css: &str) -> (String, String) {
    let fallback = ("612pt".to_string(), "936pt".to_string());
    let Some(start) = css.find("@page") else {
        return fallback;
    };
    let Some(size_at) = css[start..].find("size:") else {
        return fallback;
    };
    let rest = &css[start + size_at + "size:".len()..];
    let end = rest.find([';', '}']).unwrap_or(rest.len());
    let mut parts = rest[..end].split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(w), Some(h)) => (w.to_string(), h.to_string()),
        _ => fallback,
    }
}

fn append_before_body_end(document: &str, fragment: &str) -> String {
    match document.rfind("</body>") {
        Some(at) => format!("{}{}{}", &document[..at], fragment, &document[at..]),
        None => format!("{document}{fragment}"),
    }
}

/// A `div` like the form pages on purpose: `.page:last-of-type` in base.css
/// is per element type, so a `<section>` here would leave page 2 counted as
/// "last" — no page break before the receipt when printing, and no gap on
/// screen.
/// The receipt sheet, laid out like a mail client's print of the message:
/// the mailbox top right, the subject as the title, sender and arrival time,
/// the recipient, then BIR's email as sent. No labels of our own — the file
/// name and BIR's stamp are in the email. `.page` in base.css already breaks
/// after each page and not after the last; `height:auto;overflow:visible`
/// let a long email flow onto a further sheet.
fn receipt_page_html(receipt: &ReceiptPage, width: &str, height: &str) -> String {
    let arrived = receipt
        .email_received_at
        .as_deref()
        .unwrap_or(receipt.received_at.as_str());
    let mut html = String::new();
    html.push_str(&format!(
        "<div class=\"page page-receipt\" id=\"page-receipt\" style=\"width:{width};min-height:{height};height:auto;overflow:visible;padding:28pt 36pt;font-family:'eBIRForms Arimo',Arial,Helvetica,sans-serif;font-size:10.5pt;line-height:1.35;color:#000;background:#fff\">"
    ));
    html.push_str(
        "<style>\
#page-receipt .receipt-html p{margin:0 0 1em 0}\
#page-receipt .receipt-html ul,#page-receipt .receipt-html ol{margin:0.5em 0 1em 0;padding-left:2.4em}\
#page-receipt .receipt-html li{margin:0.15em 0}\
#page-receipt .receipt-html b,#page-receipt .receipt-html strong{font-weight:700}\
#page-receipt .receipt-html table{border-collapse:collapse}\
#page-receipt .receipt-html img{max-width:100%}\
</style>",
    );
    html.push_str(&format!(
        "<div style=\"text-align:right;color:#666;font-weight:700;font-size:10pt;margin:0 0 10pt 0\">{}</div>",
        html_escape(&receipt.to)
    ));
    html.push_str("<hr style=\"border:0;border-top:1px solid #999;margin:0 0 10pt 0\">");
    html.push_str(&format!(
        "<h1 style=\"font-size:17pt;font-weight:700;margin:0 0 10pt 0\">{}</h1>",
        html_escape(&receipt.subject)
    ));
    html.push_str("<hr style=\"border:0;border-top:1px solid #999;margin:0 0 8pt 0\">");
    html.push_str(&format!(
        "<div style=\"display:flex;justify-content:space-between;gap:12pt\"><div><b>{from}</b> &lt;{from}&gt;</div><div style=\"white-space:nowrap\">{when}</div></div>",
        from = html_escape(&receipt.from),
        when = html_escape(arrived)
    ));
    html.push_str(&format!(
        "<div style=\"margin:0 0 14pt 0\">To: {}</div>",
        html_escape(&receipt.to)
    ));
    // The email as BIR sent it when its HTML survived ingest; the plain text
    // only as a fallback. Both said the same thing, and printing both read
    // as a duplicate.
    match &receipt.body_html {
        Some(body_html) if !body_html.trim().is_empty() => {
            html.push_str("<div class=\"receipt-html\" style=\"padding:0 12pt\">");
            html.push_str(body_html);
            html.push_str("</div>");
        }
        _ => html.push_str(&format!(
            "<pre class=\"receipt-text\" style=\"white-space:pre-wrap;word-break:break-word;margin:0;padding:0 12pt;font-family:'eBIRForms Arimo',Arial,Helvetica,sans-serif;font-size:10.5pt;line-height:1.35\">{}</pre>",
            html_escape(&receipt.body_text)
        )),
    }
    html.push_str("</div>");
    html
}

pub fn filled_2551q_document(draft: &Form2551QDraft) -> String {
    filled_document("2551q-2018", &draft.to_print_field_map()).expect("2551q-2018 freeze bundle")
}

pub fn stamped_tin_names() -> &'static [&'static str] {
    &STAMPED_TIN_NAMES
}

fn input_tags(html: &str) -> Vec<InputTag<'_>> {
    let mut tags = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = html[search_from..].find("<input") {
        let start = search_from + rel;
        let Some(gt) = html[start..].find('>') else {
            break;
        };
        let end = start + gt + 1;
        let tag = &html[start..end];
        if let Some(name) = attr(tag, "name") {
            tags.push(InputTag {
                start,
                end,
                tag,
                name,
                slot: attr(tag, "data-slot-index").and_then(|value| value.parse().ok()),
            });
        }
        search_from = end;
    }
    tags
}

fn attr<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = tag.get(start..)?;
    let end = rest.find('"')?;
    rest.get(..end)
}

fn set_value(tag: &str, value: &str, writer_value: Option<&str>) -> String {
    let escaped = html_escape(value);
    let needle = "value=\"";
    let mut out = if let Some(value_at) = tag.find(needle) {
        let content_at = value_at + needle.len();
        if let Some(close) = tag[content_at..].find('"') {
            let close_at = content_at + close;
            let mut rewritten = String::with_capacity(tag.len() + escaped.len());
            rewritten.push_str(&tag[..content_at]);
            rewritten.push_str(&escaped);
            rewritten.push_str(&tag[close_at..]);
            rewritten
        } else {
            tag.to_string()
        }
    } else {
        let insert_at = tag.rfind('>').unwrap_or(tag.len());
        format!(
            "{} value=\"{}\"{}",
            &tag[..insert_at],
            escaped,
            &tag[insert_at..]
        )
    };
    if let Some(writer) = writer_value.filter(|value| !value.is_empty()) {
        let attr = format!(" data-writer-value=\"{}\"", html_escape(writer));
        if !out.contains("data-writer-value=\"") {
            let insert_at = out.rfind('>').unwrap_or(out.len());
            out.insert_str(insert_at, &attr);
        }
    }
    out
}

fn html_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

fn inline_local_assets(html: &str, loaded: &FrozenBundle) -> String {
    let mut document = html.to_string();
    let base = BASE_CSS.replace(
        "url(\"fonts/arimo-latin-wght-normal.woff2\")",
        &format!("url(\"{}\")", data_uri("font/woff2", FONT_ARIMO_NORMAL)),
    );
    let form = loaded.css.replace(
        "url(\"../fonts/arimo-latin-wght-italic.woff2\")",
        &format!("url(\"{}\")", data_uri("font/woff2", FONT_ARIMO_ITALIC)),
    );
    document = document.replace(
        "<link rel=\"stylesheet\" href=\"../base.css\">",
        &format!("<style>{base}</style>"),
    );
    document = document.replace(
        "<link rel=\"stylesheet\" href=\"form.css\">",
        &format!("<style>{form}</style>"),
    );
    for (name, bytes) in loaded.assets {
        let uri = data_uri("image/png", bytes);
        document = document.replace(&format!("../assets/{name}"), &uri);
    }
    document
}

fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64_encode(bytes))
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// One read-only input laid over a whole comb cell (`<div id="{cell}">`),
/// holding `text` left-aligned in a font small enough to fit. Returns the
/// insert position just inside the cell and the input tag.
fn comb_overflow_line(
    html: &str,
    cell: &str,
    text: &str,
    sheet: &SheetType,
) -> Option<(usize, String)> {
    let open = html.find(&format!("<div id=\"{cell}\""))?;
    let tag = &html[open..open + html[open..].find('>')?];
    let width_at = tag.find(";width:")? + ";width:".len();
    let width: f64 = tag[width_at..].split("pt").next()?.parse().ok()?;
    overflow_input(html, cell, text, fit_line(sheet, width, text))
}

/// [`comb_overflow_line`] at type `size` (pt). The input names the cell
/// it lies over (`data-overflow-of`).
fn overflow_input(html: &str, cell: &str, text: &str, size: f64) -> Option<(usize, String)> {
    let open = html.find(&format!("<div id=\"{cell}\""))?;
    let close = open + html[open..].find('>')?;
    Some((
        close + 1,
        format!(
            "<input type=\"text\" class=\"fi\" readonly value=\"{}\" style=\"font-size:{size:.2}pt;text-align:left;padding-left:1pt;z-index:1\" data-overflow-of=\"{cell}\">",
            html_escape(text)
        ),
    ))
}

/// [`printed_cell`] for every comb cell a filled document has, read in one
/// pass: cell id -> what prints over it and the plain line's type size.
pub fn printed_cells(document: &str) -> BTreeMap<String, (String, Option<f64>)> {
    let tags = input_tags(document);
    let grouped = grouped_input_indices(&tags);
    let mut out: BTreeMap<String, (String, Option<f64>)> = BTreeMap::new();
    for (name, indices) in &grouped {
        if !indices.iter().any(|&index| tags[index].slot.is_some()) {
            continue;
        }
        let boxes: String = ordered_comb_indices(&tags, indices)
            .into_iter()
            .map(|index| attr(tags[index].tag, "value").unwrap_or(""))
            .collect();
        out.insert((*name).to_string(), (boxes, None));
    }
    let needle = " data-overflow-of=\"";
    let mut from = 0;
    while let Some(at) = document[from..].find(needle) {
        let at = from + at;
        let start = document[..at].rfind("<input").unwrap_or(0);
        let end = at + document[at..].find('>').unwrap_or(0);
        let tag = &document[start..=end];
        from = end + 1;
        let Some(cell) = attr(tag, "data-overflow-of") else {
            continue;
        };
        let entry = out.entry(cell.to_string()).or_default();
        if !entry.0.trim().is_empty() {
            continue;
        }
        let text = attr(tag, "value")
            .unwrap_or("")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        let size = tag
            .find("font-size:")
            .and_then(|at| tag[at + "font-size:".len()..].split("pt").next())
            .and_then(|size| size.parse().ok());
        *entry = (text, size);
    }
    out
}

/// The cells of each item `slug`'s sheet prints over stacked comb lines (a
/// two-line address, a three-line registered name), each item's lines in
/// order: what an overflowing item is laid across.
pub fn stacked_lines(slug: &str) -> Result<Vec<Vec<String>>, String> {
    let loaded = bundle(slug).ok_or_else(|| format!("no frozen HTML bundle for {slug}"))?;
    let cells = writer_cells(slug)?;
    Ok(line_groups(loaded.html, &cells)
        .into_iter()
        .map(|group| group.into_iter().map(|line| line.cell).collect())
        .collect())
}

/// What a filled document prints over comb cell `cell`: its boxes'
/// characters in slot order, or, where every box is empty, the plain line
/// laid over the cell (`data-overflow-of`) with its type size.
pub fn printed_cell(document: &str, cell: &str) -> (String, Option<f64>) {
    let boxes = printed_text(document, cell);
    if !boxes.trim().is_empty() {
        return (boxes, None);
    }
    let needle = format!(" data-overflow-of=\"{cell}\">");
    let Some(end) = document.find(&needle) else {
        return (boxes, None);
    };
    let start = document[..end].rfind("<input").unwrap_or(0);
    let tag = &document[start..end + needle.len()];
    let text = attr(tag, "value")
        .unwrap_or("")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let size = tag
        .find("font-size:")
        .and_then(|at| tag[at + "font-size:".len()..].split("pt").next())
        .and_then(|size| size.parse().ok());
    (text, size)
}

/// Email inputs (`txtEmail`, `txtI11Email`, `txtPg1Pt1I12Email`, ...) are
/// never uppercased on print; matched case-insensitively on "email".
fn is_email_key(name: &str) -> bool {
    name.to_ascii_lowercase().contains("email")
}

#[cfg(test)]
mod tests {
    #[test]
    fn attachment_counts_fill_both_digits_for_one_through_ninety_nine() {
        let html = r#"<input name="frm1601c:txtSheets" data-slot-index="0" value=""><input name="frm1601c:txtSheets" data-slot-index="1" value="">"#;
        for n in 0..=99 {
            let fields = BTreeMap::from([("frm1601c:txtSheets".to_string(), n.to_string())]);
            let filled = super::fill_by_name(html, &fields);
            assert_eq!(comb_text(&filled, "frm1601c:txtSheets"), format!("{n:02}"));
        }
        let fields = BTreeMap::from([("frm1601c:txtSheets".to_string(), String::new())]);
        assert_eq!(
            comb_text(&super::fill_by_name(html, &fields), "frm1601c:txtSheets"),
            ""
        );
    }

    #[test]
    fn every_frozen_attachment_count_join_pads_to_its_existing_capacity() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/html-frozen");
        let mut checked = 0;
        for entry in std::fs::read_dir(root).unwrap() {
            let dir = entry.unwrap().path();
            let path = dir.join("writer-cells.json");
            if !path.exists() {
                continue;
            }
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let slug = dir.file_name().unwrap().to_str().unwrap();
            for join in doc["joins"].as_array().unwrap() {
                let caption = join["printed"].as_str().unwrap_or("").to_ascii_lowercase();
                if !(caption.contains("number of sheet")
                    || caption.contains("no. of sheet")
                    || caption.contains("number of attachment"))
                {
                    continue;
                }
                let key = join["writer_key"].as_str().unwrap();
                let cell = join["html_id"].as_str().unwrap();
                assert!(
                    super::is_attachment_count_key(key),
                    "{slug}: missing count key {key}"
                );
                let empty = super::filled_document(slug, &BTreeMap::new()).unwrap();
                let capacity = super::input_tags(&empty)
                    .iter()
                    .filter(|t| t.name == cell && t.slot.is_some())
                    .count();
                if capacity < 2 {
                    continue;
                } // Separate tens/units or plain fields.
                for value in ["", "0", "1", "9", "10", "99"] {
                    let fields = BTreeMap::from([(key.to_string(), value.to_string())]);
                    let filled = super::filled_document(slug, &fields).unwrap();
                    let expected = if value.is_empty() {
                        String::new()
                    } else {
                        format!("{value:0>capacity$}")
                    };
                    assert_eq!(
                        comb_text(&filled, cell),
                        expected,
                        "{slug}: {key} = {value}"
                    );
                }
                checked += 1;
            }
        }
        assert!(
            checked >= 30,
            "attachment coverage unexpectedly fell to {checked}"
        );
    }

    /// A comb drawn with a decimal-point column: slots 0..=4 for pesos, an
    /// index gap where the point is printed, then two centavo slots.
    fn point_column_comb() -> String {
        [0, 1, 2, 3, 4, 6, 7]
            .iter()
            .map(|i| format!("<input name=\"amt\" data-slot-index=\"{i}\" value=\"\">"))
            .collect()
    }

    fn money_writes(value: &str) -> Option<Vec<String>> {
        let html = point_column_comb();
        let tags = super::input_tags(&html);
        let ordered: Vec<usize> = (0..tags.len()).collect();
        super::money_comb_values(&tags, &ordered, value)
            .map(|w| w.into_iter().map(|(_, ch)| ch).collect())
    }

    #[test]
    fn amounts_right_align_pesos_and_put_cents_after_the_point_gap() {
        // Before the fix `0.00` spread from the first box as `0 . 0 0`.
        assert_eq!(
            money_writes("0.00").unwrap(),
            ["", "", "", "", "0", "0", "0"]
        );
        assert_eq!(
            money_writes("1,234.56").unwrap(),
            ["", "1", "2", "3", "4", "5", "6"]
        );
        assert_eq!(
            money_writes("12").unwrap(),
            ["", "", "", "1", "2", "0", "0"]
        );
    }

    #[test]
    fn non_amounts_and_plain_combs_keep_the_left_to_right_spread() {
        assert!(money_writes("ABC").is_none());
        assert!(money_writes("").is_none());
        // No index gap: not a point-column comb.
        let html: String = (0..7)
            .map(|i| format!("<input name=\"amt\" data-slot-index=\"{i}\" value=\"\">"))
            .collect();
        let tags = super::input_tags(&html);
        let ordered: Vec<usize> = (0..tags.len()).collect();
        assert!(super::money_comb_values(&tags, &ordered, "0.00").is_none());
    }

    #[test]
    fn every_print_map_is_registered_and_fills_its_sheet() {
        use std::collections::BTreeMap;
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/html-frozen");
        let mut checked = 0;
        for entry in std::fs::read_dir(root).unwrap() {
            let dir = entry.unwrap().path();
            if !dir.join("writer-cells.json").exists() {
                continue;
            }
            let slug = dir.file_name().unwrap().to_str().unwrap();
            assert!(
                super::writer_cells_json(slug).is_some(),
                "{slug}: not registered"
            );
            let cells = super::writer_cells(slug).unwrap_or_else(|e| panic!("{slug}: {e}"));
            let mut fields = BTreeMap::new();
            for key in cells.joins.keys() {
                fields.insert(key.clone(), "1".to_string());
            }
            for join in &cells.money_joins {
                fields.insert(join.writer_key.clone(), "1234.56".to_string());
            }
            for join in &cells.xbox_joins {
                fields.insert(join.writer_key.clone(), "true".to_string());
            }
            for join in &cells.peso_joins {
                fields.insert(join.writer_key.clone(), "1234.56".to_string());
            }
            for join in &cells.date_joins {
                let date = if join.dd_html_id.is_some() {
                    "06/15/2025"
                } else {
                    "06/2025"
                };
                fields.insert(join.writer_key.clone(), date.to_string());
            }
            let filled =
                super::filled_document(slug, &fields).unwrap_or_else(|e| panic!("{slug}: {e}"));
            if cells.has_fill_join() {
                let empty = super::filled_document(slug, &BTreeMap::new())
                    .unwrap_or_else(|e| panic!("{slug}: {e}"));
                assert_ne!(filled, empty, "{slug}: fill joins changed nothing");
            }
            checked += 1;
        }
        assert!(checked >= 30, "only {checked} print maps found");
    }

    #[test]
    fn peso_only_combs_round_to_whole_pesos() {
        assert_eq!(super::whole_pesos("1,234.49").as_deref(), Some("1234"));
        assert_eq!(super::whole_pesos("1234.50").as_deref(), Some("1235"));
        assert_eq!(super::whole_pesos("999.5").as_deref(), Some("1000"));
        assert_eq!(super::whole_pesos("0.49").as_deref(), Some("0"));
        assert_eq!(super::whole_pesos("12").as_deref(), Some("12"));
        assert_eq!(super::whole_pesos("-5.00").as_deref(), Some("-5"));
        assert_eq!(super::whole_pesos("-1,234.50").as_deref(), Some("-1235"));
        assert_eq!(super::whole_pesos("-0.49").as_deref(), Some("0"));
    }

    fn comb(name: &str, slots: usize) -> String {
        (0..slots)
            .map(|i| format!("<input name=\"{name}\" data-slot-index=\"{i}\" value=\"\">"))
            .collect()
    }

    fn comb_values(doc: &str, name: &str) -> Vec<String> {
        super::input_tags(doc)
            .iter()
            .filter(|tag| tag.name == name)
            .map(|tag| super::attr(tag.tag, "value").unwrap_or("").to_string())
            .collect()
    }

    #[test]
    fn negative_money_join_puts_minus_left_of_pesos() {
        use std::collections::BTreeMap;
        let html = format!("{}{}", comb("p", 8), comb("c", 2));
        let mut fields = BTreeMap::new();
        fields.insert("k".to_string(), "-122,500.25".to_string());
        let joins = [super::MoneyJoin {
            writer_key: "k".into(),
            cells: vec![super::MoneyCells {
                peso_html_id: "p".into(),
                cent_html_id: Some("c".into()),
                point_slot: false,
            }],
        }];
        let doc = super::fill_money_joins(&html, &fields, &joins);
        assert_eq!(
            comb_values(&doc, "p"),
            ["", "-", "1", "2", "2", "5", "0", "0"]
        );
        assert_eq!(comb_values(&doc, "c"), ["2", "5"]);
        fields.insert("k".to_string(), "-12,345,678.00".to_string());
        let doc = super::fill_money_joins(&html, &fields, &joins);
        assert_eq!(
            comb_values(&doc, "p"),
            vec![String::new(); 8],
            "no room for '-'"
        );
    }

    #[test]
    fn decimal_comb_money_puts_cents_in_the_last_two_slots() {
        use std::collections::BTreeMap;
        let html = comb("p", 8);
        let mut fields = BTreeMap::new();
        fields.insert("k".to_string(), "1,234.50".to_string());
        let mut joins = [super::MoneyJoin {
            writer_key: "k".into(),
            cells: vec![super::MoneyCells {
                peso_html_id: "p".into(),
                cent_html_id: None,
                point_slot: true,
            }],
        }];
        let doc = super::fill_money_joins(&html, &fields, &joins);
        assert_eq!(
            comb_values(&doc, "p"),
            ["", "1", "2", "3", "4", "", "5", "0"]
        );
        joins[0].cells[0].point_slot = false;
        let doc = super::fill_money_joins(&html, &fields, &joins);
        assert_eq!(
            comb_values(&doc, "p"),
            ["", "", "1", "2", "3", "4", "5", "0"]
        );
    }

    #[test]
    fn one_writer_key_fills_every_listed_cell() {
        use std::collections::BTreeMap;
        let html = format!("{}{}", comb("a", 4), comb("b", 3));
        let mut fields = BTreeMap::new();
        fields.insert("k".to_string(), "Ab1".to_string());
        let mut cells = BTreeMap::new();
        cells.insert("k".to_string(), vec!["a".to_string(), "b".to_string()]);
        let doc = super::fill_by_name_with_cells(&html, &fields, &cells, &BTreeMap::new());
        assert_eq!(comb_values(&doc, "a"), ["A", "B", "1", ""]);
        assert_eq!(comb_values(&doc, "b"), ["A", "B", "1"]);
    }

    #[test]
    fn date_join_splits_one_writer_date_over_its_combs() {
        use std::collections::BTreeMap;
        assert_eq!(
            super::split_writer_date("6/9/2025"),
            Some(("06".into(), Some("09".into()), "2025".into()))
        );
        assert_eq!(
            super::split_writer_date("2025-06-09"),
            Some(("06".into(), Some("09".into()), "2025".into()))
        );
        assert_eq!(
            super::split_writer_date("04/2025"),
            Some(("04".into(), None, "2025".into()))
        );
        assert_eq!(super::split_writer_date("June 9"), None);
        assert_eq!(super::split_writer_date(""), None);
        let html = format!("{}{}{}", comb("m", 2), comb("d", 2), comb("y", 4));
        let join = super::DateJoin {
            writer_key: "k".into(),
            mm_html_id: "m".into(),
            dd_html_id: Some("d".into()),
            yyyy_html_id: "y".into(),
        };
        let mut fields = BTreeMap::new();
        fields.insert("k".to_string(), "05/09/2025".to_string());
        let doc = super::fill_date_joins(&html, &fields, std::slice::from_ref(&join));
        assert_eq!(comb_values(&doc, "m"), ["0", "5"]);
        assert_eq!(comb_values(&doc, "d"), ["0", "9"]);
        assert_eq!(comb_values(&doc, "y"), ["2", "0", "2", "5"]);
        // A month/year value has no day for the DD comb: nothing prints.
        fields.insert("k".to_string(), "05/2025".to_string());
        let doc = super::fill_date_joins(&html, &fields, &[join]);
        assert_eq!(comb_values(&doc, "y"), ["", "", "", ""]);
    }

    #[test]
    fn xbox_join_names_one_box_of_a_two_box_cell_by_input_id() {
        use std::collections::BTreeMap;
        let html = concat!(
            r#"<input type="text" id="c-i0" name="c" maxlength="1">"#,
            r#"<input type="text" id="c-i1" name="c" maxlength="1">"#,
        );
        let join = super::XboxJoin {
            writer_key: "k".into(),
            html_ids: vec!["c-i1".into()],
        };
        let mut fields = BTreeMap::new();
        fields.insert("k".to_string(), "true".to_string());
        let doc = super::fill_xbox_joins(html, &fields, &[join]);
        assert_eq!(comb_values(&doc, "c"), ["", "X"]);
    }

    #[test]
    fn writer_cells_reject_reused_cells_stale_exceptions_and_old_sections() {
        let parse = |json: &str| super::parse_writer_cells(json).err().unwrap_or_default();
        assert!(parse(r#"{"joins":[{"writer_key":"a","html_id":"p1c1"},{"writer_key":"b","html_ids":["p1c2","p1c1"]}]}"#)
            .contains("already a fill target"));
        assert!(parse(r#"{"joins":[{"writer_key":"a","html_id":"p1c1"}],"money_joins":[{"writer_key":"a","decimal_html_id":"p1c2"}]}"#)
            .contains("duplicate writer_key a"));
        assert!(parse(r#"{"joins":[{"writer_key":"a","html_id":"p1c1"}],"not_joined":[{"writer_key":"a","reason":"x"}]}"#)
            .contains("remove it from not_joined"));
        assert!(
            parse(r#"{"joins":[],"not_joined":[{"writer_keys":["a","b"]}]}"#)
                .contains("needs a reason")
        );
        assert!(parse(r#"{"joins":[{"writer_key":"a","html_id":"p1c1"}],"not_joined_cells":[{"html_id":"p1c1","reason":"x"}]}"#)
            .contains("remove it from not_joined_cells"));
        assert!(parse(r#"{"joins":[],"left_out":[]}"#).contains("unknown section left_out"));
        let ok = super::parse_writer_cells(
            r#"{"joins":[{"writer_key":"a","html_id":"p1c1","html_ids":["p2c1"]}],
                "money_joins":[{"writer_key":"m","peso_html_id":"p1c2","cent_html_id":"p1c3","decimal_html_ids":["p2c2"],"point_slot":true}],
                "date_joins":[{"writer_key":"d","mm_html_id":"p1c4","yyyy_html_id":"p1c5"}],
                "not_joined":[{"writer_keys":["x","y"],"reason":"not on the paper form"}]}"#,
        )
        .unwrap();
        assert_eq!(ok.joins["a"], ["p1c1", "p2c1"]);
        assert_eq!(ok.money_joins[0].cells.len(), 2);
        assert!(ok.money_joins[0].cells[1].point_slot);
        assert_eq!(ok.not_joined["y"], "not on the paper form");
    }

    #[test]
    fn unprinted_keys_lists_values_with_no_box_and_no_reason() {
        use std::collections::BTreeMap;
        let mut fields = BTreeMap::new();
        for (key, value) in [
            ("frm1601c:txtTIN1", "111"),
            ("frm1601c:txtTaxpayerName", "NAME"),
            ("frm1601c:txtCurrentPage", "1"),
            ("frm1601c:txtTax14", "1.00"),
            ("frm1601c:unknownBox", "5"),
            ("frm1601c:emptyBox", ""),
        ] {
            fields.insert(key.to_string(), value.to_string());
        }
        assert_eq!(
            super::unprinted_keys("1601c-2018", &fields).unwrap(),
            ["frm1601c:unknownBox"]
        );
    }

    #[test]
    fn negative_peso_join_puts_minus_left_of_pesos() {
        use std::collections::BTreeMap;
        let html = comb("p", 5);
        let mut fields = BTreeMap::new();
        fields.insert("k".to_string(), "-122.50".to_string());
        let joins = [super::PesoJoin {
            writer_key: "k".into(),
            html_ids: vec!["p".into()],
        }];
        let doc = super::fill_peso_joins(&html, &fields, &joins);
        assert_eq!(comb_values(&doc, "p"), ["", "-", "1", "2", "3"]);
    }

    #[test]
    fn checked_xbox_is_centred_like_a_char_box() {
        use std::collections::BTreeMap;
        let mut fields = BTreeMap::new();
        fields.insert("frm1601c:AmendedRtn_1".to_string(), "true".to_string());
        let doc = super::filled_document("1601c-2018", &fields).expect("form");
        let tag_at = doc.find("id=\"p1c21-i\"").expect("amended-yes xbox");
        let tag =
            &doc[doc[..tag_at].rfind('<').unwrap()..doc[tag_at..].find('>').unwrap() + tag_at + 1];
        assert!(tag.contains("value=\"X\""), "{tag}");
        assert!(tag.contains("class=\"fi fh3 fc\""), "{tag}");
        assert_eq!(
            super::with_class("<input class=\"fi fc\">", "fc"),
            "<input class=\"fi fc\">"
        );
        assert_eq!(super::with_class("<input>", "fc"), "<input class=\"fc\">");
    }

    #[test]
    fn receipt_page_follows_the_last_form_page() {
        use std::collections::BTreeMap;
        let receipt = super::ReceiptPage {
            filename: "00000000000000-1601Cv2018-112026.xml".into(),
            subject: "Tax Return Receipt Confirmation".into(),
            from: "ebirforms-noreply@bir.gov.ph".into(),
            to: "taxpayer@example.com".into(),
            received_at: "12 September 2026, 02:52 PM".into(),
            email_received_at: Some("12 September 2026, 02:59 PM".into()),
            body_text: "This confirms receipt <of> your submission".into(),
            body_html: Some("<p>This confirms <b>receipt</b></p>".into()),
        };
        let fields = BTreeMap::new();
        let plain = super::filled_document("1601c-2018", &fields).expect("form");
        assert!(!plain.contains("page-receipt"));

        let with = super::filled_document_with_receipt("1601c-2018", &fields, Some(&receipt))
            .expect("form + receipt");
        let page2 = with.find("id=\"page-2\"").expect("page 2");
        let receipt_at = with.find("id=\"page-receipt\"").expect("receipt page");
        let body_end = with.rfind("</body>").expect("body end");
        assert!(page2 < receipt_at && receipt_at < body_end);
        assert!(
            with.contains("<div class=\"page page-receipt\""),
            "same element type as the form pages so :last-of-type moves to the receipt"
        );
        assert!(
            with.contains("style=\"width:612pt;min-height:936pt"),
            "sheet matches @page"
        );
        assert!(
            with.contains("<p>This confirms <b>receipt</b></p>"),
            "html body embedded as given"
        );
        assert!(with.contains("<h1 style=\"font-size:17pt;font-weight:700;margin:0 0 10pt 0\">Tax Return Receipt Confirmation</h1>"));
        assert!(with
            .contains("<b>ebirforms-noreply@bir.gov.ph</b> &lt;ebirforms-noreply@bir.gov.ph&gt;"));
        assert!(
            with.contains("12 September 2026, 02:59 PM"),
            "email arrival time shown"
        );
        assert!(with.contains("To: taxpayer@example.com"));
        assert!(!with.contains("<th"), "no label table of our own");
        assert!(
            !with.contains("receipt &lt;of&gt;"),
            "text is not printed beside the html"
        );

        let text_only = super::ReceiptPage {
            body_html: None,
            ..receipt.clone()
        };
        let with_text =
            super::filled_document_with_receipt("1601c-2018", &fields, Some(&text_only))
                .expect("form + text receipt");
        assert!(
            with_text.contains("receipt &lt;of&gt; your submission"),
            "text fallback is escaped"
        );
        assert!(!with_text.contains("class=\"receipt-html\""));
        assert!(with_text.contains("class=\"receipt-text\""));
    }

    #[test]
    fn page_size_comes_from_the_bundle_css() {
        assert_eq!(
            super::page_size_from_css("@page { size:612pt 936pt;margin:0 }"),
            ("612pt".to_string(), "936pt".to_string())
        );
        assert_eq!(
            super::page_size_from_css("@page{size:595pt 842pt}"),
            ("595pt".to_string(), "842pt".to_string())
        );
        assert_eq!(
            super::page_size_from_css(".page{}"),
            ("612pt".to_string(), "936pt".to_string())
        );
    }

    use super::*;
    use serde_json::json;

    fn sample_draft() -> Form2551QDraft {
        let mut draft: Form2551QDraft = serde_json::from_value(json!({
            "id": null,
            "tin": "000123453000",
            "taxpayer_type": "Individual",
            "business_start_date": "2010-01-01",
            "taxable_year": 2026,
            "quarter": 1,
            "tax_period_basis": "calendar",
            "year_end_month": 12,
            "eopt_tier": null,
            "is_amended": false,
            "original_return_filed_and_paid_on_time": true,
            "number_of_attached_sheets": 0,
            "tax_relief": false,
            "tax_relief_specification": "",
            "item_13_election": "graduated",
            "rdo_code": "018",
            "taxpayer_name": "Frozen Html Fixture",
            "registered_address": "Sample Brgy",
            "zip_code": "2200",
            "contact_number": "09170000100",
            "email": "tax@example.com",
            "schedule_1": [],
            "total_tax_due": 0.0,
            "creditable_tax_withheld": 0.0,
            "tax_paid_previous": 0.0,
            "other_tax_credit": 0.0,
            "other_tax_credit_description": "",
            "total_tax_credits": 0.0,
            "tax_payable": 0.0,
            "auto_compute_penalties": false,
            "surcharge": 0.0,
            "interest": 0.0,
            "compromise": 0.0,
            "total_penalties": 0.0,
            "total_amount_payable": 0.0,
            "overpayment_disposition": "none",
            "status": "Draft",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "submitted_at": null,
            "confirmed_at": null,
            "submission_filename": null,
            "receipt_id": null,
            "submission_attempts": 0,
            "next_retry_at": null,
            "last_error": null,
            "carried_forward_from": null,
            "payment_receipt_path": null
        }))
        .expect("fixture draft");
        draft.recompute(None);
        draft
    }

    fn named_values(html: &str, name: &str) -> Vec<String> {
        input_tags(html)
            .into_iter()
            .filter(|tag| tag.name == name)
            .map(|tag| attr(tag.tag, "value").unwrap_or("").to_string())
            .collect()
    }

    #[test]
    fn frozen_2551q_stamps_only_catalog_tin_names_from_fields_json() {
        let fields: serde_json::Value = serde_json::from_str(include_str!(
            "../../bir-core/tests/fixtures/rules-forms/2551q-v2018/fields.json"
        ))
        .unwrap();
        let allowed: std::collections::BTreeSet<String> = fields["fields"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["serialized_key"].as_str().map(str::to_string))
            .collect();

        let present: std::collections::BTreeSet<String> = input_tags(html_2551q())
            .into_iter()
            .map(|tag| tag.name.to_string())
            .filter(|name| name.starts_with("frm2551Qv2018:"))
            .collect();

        assert_eq!(
            present,
            STAMPED_TIN_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect()
        );
        for name in &present {
            assert!(allowed.contains(name), "{name} is not in fields.json");
        }
        assert!(html_2551q().contains("id=\"p1c20-s0\""));
        assert!(input_tags(html_2551q())
            .into_iter()
            .all(|tag| tag.name != "p1c20"));
    }

    #[test]
    fn frozen_0619e_stamps_only_catalog_tin_names() {
        let html = bundle("0619e-2018").expect("0619e bundle").html;
        let present: std::collections::BTreeSet<String> = input_tags(html)
            .into_iter()
            .map(|tag| tag.name.to_string())
            .filter(|name| name.starts_with("frm0619E:"))
            .collect();
        assert_eq!(
            present,
            [
                "frm0619E:txtTIN1",
                "frm0619E:txtTIN2",
                "frm0619E:txtTIN3",
                "frm0619E:txtBranchCode",
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        );
        let document = filled_document("0619e-2018", &BTreeMap::new()).unwrap();
        assert!(document.contains("<style>"));
        assert!(!document.contains("href=\"../base.css\""));
    }

    #[test]
    fn fill_by_name_distributes_tin_digits_across_comb_slots() {
        let html = concat!(
            "<div data-field-name=\"p1c20\">",
            "<input id=\"p1c20-s0\" name=\"frm2551Qv2018:txtTIN1\" data-slot-index=\"0\" maxlength=\"1\">",
            "<input id=\"p1c20-s1\" name=\"frm2551Qv2018:txtTIN1\" data-slot-index=\"1\" maxlength=\"1\">",
            "<input id=\"p1c20-s2\" name=\"frm2551Qv2018:txtTIN1\" data-slot-index=\"2\" maxlength=\"1\">",
            "</div>"
        );
        let mut fields = BTreeMap::new();
        fields.insert("frm2551Qv2018:txtTIN1".to_string(), "123".to_string());
        let filled = fill_by_name(html, &fields);
        assert_eq!(
            named_values(&filled, "frm2551Qv2018:txtTIN1"),
            ["1", "2", "3"]
        );
        assert!(filled.contains("id=\"p1c20-s0\""));
        assert!(filled.contains("data-field-name=\"p1c20\""));
    }

    #[test]
    fn fill_by_name_with_cells_uppercases_letter_combs_not_stamped_digits() {
        let html = concat!(
            r#"<input name="p1c36" data-slot-index="0" maxlength="1">"#,
            r#"<input name="p1c36" data-slot-index="1" maxlength="1">"#,
            r#"<input name="p1c36" data-slot-index="2" maxlength="1">"#,
            r#"<input name="frm1601c:txtTIN1" data-slot-index="0" maxlength="1">"#,
        );
        let mut fields = BTreeMap::new();
        fields.insert("frm1601c:txtTaxpayerName".to_string(), "Ab".to_string());
        fields.insert("frm1601c:txtTIN1".to_string(), "9".to_string());
        let mut cells = BTreeMap::new();
        cells.insert(
            "frm1601c:txtTaxpayerName".to_string(),
            vec!["p1c36".to_string()],
        );
        let filled = fill_by_name_with_cells(html, &fields, &cells, &BTreeMap::new());
        assert_eq!(comb_text(&filled, "p1c36"), "AB");
        assert!(filled.contains("data-writer-value=\"AB\""));
        assert!(!filled.contains("data-writer-value=\"Ab\""));
        assert_eq!(named_values(&filled, "frm1601c:txtTIN1"), ["9"]);
    }

    #[test]
    fn fill_2551q_sets_stamped_tin_names_from_the_writer_map() {
        let filled = fill_2551q(&sample_draft());
        assert_eq!(
            named_values(&filled, "frm2551Qv2018:txtTIN1"),
            ["0", "0", "0"]
        );
        assert_eq!(
            named_values(&filled, "frm2551Qv2018:txtTIN2"),
            ["1", "2", "3"]
        );
        assert_eq!(
            named_values(&filled, "frm2551Qv2018:txtTIN3"),
            ["4", "5", "3"]
        );
        assert_eq!(
            named_values(&filled, "frm2551Qv2018:txtBranchCode"),
            ["0", "0", "0", "0", "0"]
        );
        let document = filled_2551q_document(&sample_draft());
        assert!(document.contains("data:image/png;base64,"));
        assert!(document.contains("<style>"));
        assert!(!document.contains("href=\"../base.css\""));
    }

    #[test]
    fn frozen_preview_bundles_stamp_only_catalog_frm_names() {
        let cases = [
            ("0619f-2018", "frm0619F:", 4usize),
            ("0605-1999", "frm0605:", 4),
            ("1601c-2018", "frm1601c:", 4),
            ("1701q-2018", "frm1701q:", 6),
            ("2550q-2024", "frm2550qv2024:", 3),
            ("1701-2018", "frm1701:", 6),
            ("1702rt-2018c", "frm1702RT:", 3),
            ("1702mx-2018c", "frm1702MX:", 3),
        ];
        for (slug, prefix, count) in cases {
            let html = bundle(slug).unwrap_or_else(|| panic!("{slug}")).html;
            let present: std::collections::BTreeSet<String> = input_tags(html)
                .into_iter()
                .map(|tag| tag.name.to_string())
                .filter(|name| name.starts_with(prefix))
                .collect();
            assert_eq!(present.len(), count, "{slug} stamped names");
            let document = filled_document(slug, &BTreeMap::new()).unwrap();
            assert!(document.contains("<style>"), "{slug}");
        }
    }

    fn comb_text(html: &str, name: &str) -> String {
        named_values(html, name).concat()
    }

    fn identity_map(
        name_key: &str,
        name: &str,
        address_key: &str,
        address: &str,
    ) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        fields.insert(name_key.to_string(), name.to_string());
        fields.insert(address_key.to_string(), address.to_string());
        fields
    }

    #[test]
    fn fill_by_name_without_writer_cells_leaves_unstamped_identity_blank() {
        let html = bundle("1601c-2018").expect("1601c").html;
        let mut fields = identity_map(
            "frm1601c:txtTaxpayerName",
            "NEXUS PAYROLL CORP",
            "frm1601c:txtAddress",
            "42 Banahaw Street",
        );
        fields.insert("txtEmail".to_string(), "juan@example.com".to_string());
        fields.insert("frm1601c:txtTax14".to_string(), "8888.88".to_string());
        let filled = fill_by_name(html, &fields);
        assert_eq!(comb_text(&filled, "p1c36"), "");
        assert_eq!(comb_text(&filled, "p1c38"), "");
        assert_eq!(comb_text(&filled, "p1c48"), "");
        assert_eq!(comb_text(&filled, "p1c56"), "");
        assert!(!filled.contains("NEXUS PAYROLL CORP"));
        assert!(!filled.contains("42 Banahaw Street"));
        assert!(!filled.contains("juan@example.com"));
        assert!(!filled.contains("8888.88"));
    }

    /// A 1601-C for the test TIN 111-111-114 (never a real TIN), every
    /// Part II item non-zero, Item 13 Yes and one Schedule I row.
    fn draft_1601c() -> bir_core::forms::form_1601c::Form1601CDraft {
        use bir_core::forms::form_1601c::{Form1601CDraft, Form1601CSchedule1Row};
        let profile: bir_core::profile::TaxpayerProfile =
            serde_json::from_value(serde_json::json!({
                "id": null, "full_name": "Marita Joy Santiago Torres",
                "tin": {"segment1": "111", "segment2": "111", "segment3": "114", "branch": "00000"},
                "rdo_code": "039", "line_of_business": "Payroll",
                "registered_address": "42 Banahaw Street, Barangay Example, Quezon City",
                "zip_code": "1100", "phone": "09170000000", "email": "juan@example.com",
                "default_form_type": "1601Cv2018", "taxpayer_type": "Corporation"
            }))
            .unwrap();
        let mut draft = Form1601CDraft::new_from_profile(&profile, 2025, 6);
        draft.auto_compute_penalties = false;
        draft.tax_relief = true;
        draft.tax_relief_specification = "International Tax Treaty".to_string();
        draft.tax_14_total_compensation = 250_000.0;
        draft.tax_15_statutory_minimum_wage = 1_500.0;
        draft.tax_16_holiday_pay = 1_600.0;
        draft.tax_17_13th_month_pay = 20_000.0;
        draft.tax_18_de_minimis = 5_000.0;
        draft.tax_19_sss_gsis = 12_500.0;
        draft.tax_20_other_name = "Rice subsidy".to_string();
        draft.tax_20_other_amount = 2_000.0;
        draft.tax_23_not_subject = 3_000.0;
        draft.tax_25_total_taxes_withheld = 18_750.0;
        draft.tax_29_other_remittances_name = "Prior credit".to_string();
        draft.tax_29_other_remittances_amount = 1_000.0;
        for (component, value) in [
            (bir_core::penalties::PenaltyComponent::Surcharge, 100.0),
            (bir_core::penalties::PenaltyComponent::Interest, 200.0),
            (bir_core::penalties::PenaltyComponent::Compromise, 300.0),
        ] {
            draft.set_penalty_override(component, Some(value));
        }
        draft.schedule_1 = vec![Form1601CSchedule1Row {
            previous_month: "04/2025".to_string(),
            date_paid: "05/09/2025".to_string(),
            drawee_bank_code_or_agency: "BANK1".to_string(),
            payment_number: "REF001".to_string(),
            tax_paid: 15_000.0,
            should_be_tax_due: 15_250.0,
            adjustment: 250.0,
        }];
        draft.compute();
        draft
    }

    /// The pesos and centavos a peso + 2-slot cents pair prints.
    fn printed_amount(html: &str, peso: &str, cent: &str) -> String {
        format!("{}.{}", comb_text(html, peso).trim(), comb_text(html, cent))
    }

    #[test]
    fn filled_document_1601c_fills_taxpayer_name_and_address() {
        let html = filled_document("1601c-2018", &draft_1601c().to_print_field_map()).unwrap();
        assert_eq!(
            comb_text(&html, "p1c36").trim(),
            "MARITA JOY SANTIAGO TORRES"
        );
        // Item 9 over both lines, broken at a space, not cut at 40 slots.
        assert_eq!(
            comb_text(&html, "p1c38").trim(),
            "42 BANAHAW STREET, BARANGAY EXAMPLE,"
        );
        assert_eq!(comb_text(&html, "p1c39").trim(), "QUEZON CITY");
        assert!(html.contains("data-writer-value=\"MARITA JOY SANTIAGO TORRES\""));
        assert!(!html.contains("Marita Joy Santiago Torres"));
        let stamped: std::collections::BTreeSet<String> = input_tags(&html)
            .into_iter()
            .map(|tag| tag.name.to_string())
            .filter(|name| name.starts_with("frm1601c:"))
            .collect();
        assert_eq!(stamped.len(), 4);
    }

    /// #113 acceptance: Part II items 14-36, Item 13/13A, Schedule I and the
    /// page 2 header (name, TIN and branch) print from a 1601-C draft; the
    /// ATC is the sheet's preprinted WW010.
    #[test]
    fn filled_document_1601c_prints_every_part_ii_item_and_the_page_two_header() {
        let draft = draft_1601c();
        let fields = draft.to_print_field_map();
        let html = filled_document("1601c-2018", &fields).unwrap();
        let money = bir_core::official_xml::official_amount;
        let items: [(u8, &str, &str, f64); 23] = [
            (14, "p1c56", "p1c58", draft.tax_14_total_compensation),
            (15, "p1c61", "p1c63", draft.tax_15_statutory_minimum_wage),
            (16, "p1c65", "p1c67", draft.tax_16_holiday_pay),
            (17, "p1c69", "p1c71", draft.tax_17_13th_month_pay),
            (18, "p1c73", "p1c75", draft.tax_18_de_minimis),
            (19, "p1c77", "p1c79", draft.tax_19_sss_gsis),
            (20, "p1c81", "p1c83", draft.tax_20_other_amount),
            (21, "p1c85", "p1c87", draft.tax_21_total_non_taxable),
            (22, "p1c89", "p1c91", draft.tax_22_total_taxable),
            (23, "p1c93", "p1c95", draft.tax_23_not_subject),
            (24, "p1c97", "p1c99", draft.tax_24_net_taxable),
            (25, "p1c101", "p1c103", draft.tax_25_total_taxes_withheld),
            (26, "p1c105", "p1c107", draft.tax_26_adjustment),
            (
                27,
                "p1c109",
                "p1c111",
                draft.tax_27_taxes_withheld_for_remittance,
            ),
            (28, "p1c113", "p1c115", 0.0),
            (
                29,
                "p1c117",
                "p1c119",
                draft.tax_29_other_remittances_amount,
            ),
            (30, "p1c121", "p1c123", draft.tax_30_total_tax_remittances),
            (31, "p1c125", "p1c127", draft.tax_31_tax_still_due),
            (32, "p1c129", "p1c131", draft.tax_32_surcharge),
            (33, "p1c133", "p1c135", draft.tax_33_interest),
            (34, "p1c137", "p1c139", draft.tax_34_compromise),
            (35, "p1c141", "p1c143", draft.tax_35_total_penalties),
            (36, "p1c145", "p1c147", draft.tax_36_total_amount_payable),
        ];
        for (item, peso, cent, value) in items {
            assert_eq!(
                printed_amount(&html, peso, cent),
                money(value).replace(',', ""),
                "item {item}"
            );
        }
        assert_ne!(draft.tax_36_total_amount_payable, 0.0);
        assert_eq!(named_values(&html, "p1c223"), ["RICE SUBSIDY"]);
        assert_eq!(named_values(&html, "p1c225"), ["PRIOR CREDIT"]);
        // Item 13 Yes, 13A printed whole (longer than its 17 slots).
        assert_eq!(named_values(&html, "p1c52"), ["X"]);
        assert_eq!(named_values(&html, "p1c53"), [""]);
        assert!(html.contains("readonly value=\"INTERNATIONAL TAX TREATY\""));
        // Item 5 ATC: preprinted on the sheet, so a reviewed not_joined key.
        assert_eq!(fields["frm1601c:txtATC"], "WW010");
        assert!(html.contains(">WW010</div>"));
        let cells = writer_cells("1601c-2018").unwrap();
        assert!(cells.not_joined["frm1601c:txtATC"].contains("preprinted WW010"));
        // Page 1 TIN (stamped) and the page 2 header: TIN, branch and name.
        assert_eq!(comb_text(&html, "frm1601c:txtTIN1"), "111");
        assert_eq!(comb_text(&html, "frm1601c:txtTIN3"), "114");
        assert_eq!(comb_text(&html, "frm1601c:txtBranchCode"), "00000");
        assert_eq!(comb_text(&html, "p2c5"), "11111111400000");
        assert_eq!(
            comb_text(&html, "p2c6").trim(),
            "MARITA JOY SANTIAGO TORRES"
        );
        // Schedule I row 1: both dates split over their combs; rows 2-3 blank.
        assert_eq!(comb_text(&html, "p2c15"), "04");
        assert_eq!(comb_text(&html, "p2c16"), "2025");
        assert_eq!(comb_text(&html, "p2c17"), "05");
        assert_eq!(comb_text(&html, "p2c18"), "09");
        assert_eq!(comb_text(&html, "p2c19"), "2025");
        assert_eq!(comb_text(&html, "p2c20"), "BANK1");
        assert_eq!(printed_amount(&html, "p2c22", "p2c24"), "15000.00");
        assert_eq!(printed_amount(&html, "p2c53", "p2c55"), "15250.00");
        assert_eq!(printed_amount(&html, "p2c57", "p2c59"), "250.00");
        assert_eq!(comb_text(&html, "p2c33").trim(), "");
        assert_eq!(printed_amount(&html, "p2c83", "p2c85"), "250.00");
        // Every value the map carries prints or has a reviewed reason.
        assert_eq!(
            unprinted_keys("1601c-2018", &fields).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn filled_document_keeps_mixed_case_email_but_uppercases_name() {
        let mut fields = identity_map(
            "frm1601c:txtTaxpayerName",
            "Marita Joy Torres",
            "frm1601c:txtAddress",
            "42 Banahaw Street",
        );
        fields.insert(
            "txtEmail".to_string(),
            "Marita.Torres@Example.com".to_string(),
        );
        let html = filled_document("1601c-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c48"), "Marita.Torres@Example.com");
        assert!(html.contains("data-writer-value=\"Marita.Torres@Example.com\""));
        assert!(!html.contains("MARITA.TORRES@EXAMPLE.COM"));
        assert_eq!(comb_text(&html, "p1c36"), "MARITA JOY TORRES");
        assert!(!html.contains("Marita Joy Torres"));
        assert!(is_email_key("txtEmail"));
        assert!(is_email_key("frm1700:txtI11Email"));
        assert!(!is_email_key("txtTaxpayerName"));
    }

    #[test]
    fn sheet_2000ot_prints_date_tin_and_whole_email() {
        let long = "Marita.Joy.Torres.Accounting@Example.ph";
        let mut fields = BTreeMap::new();
        fields.insert("txtEmail".to_string(), long.to_string());
        for (key, value) in [
            ("derived:transaction_mm", "06"),
            ("derived:transaction_dd", "15"),
            ("derived:transaction_yyyy", "2025"),
            ("derived:tin", "12345678800000"),
            ("derived:pg2_tin", "12345678800000"),
        ] {
            fields.insert(key.to_string(), value.to_string());
        }
        let html = filled_document("2000-ot-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c7"), "06");
        assert_eq!(comb_text(&html, "p1c8"), "15");
        assert_eq!(comb_text(&html, "p1c9"), "2025");
        assert_eq!(comb_text(&html, "p1c18"), "12345678800000");
        assert_eq!(comb_text(&html, "p2c5"), "12345678800000");
        // 39 characters over the 24-slot comb: printed whole, as typed.
        assert!(html.contains(&format!("readonly value=\"{long}\"")));
        assert_eq!(comb_text(&html, "p1c31"), "");
    }

    #[test]
    fn sheet_1707a_shares_the_contact_email_comb() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "derived:contact_email".to_string(),
            "09171234567 Me@Example.ph".to_string(),
        );
        let html = filled_document("1707a-2021", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c39"), "09171234567 Me@Example.ph");
    }

    #[test]
    fn shared_contact_email_comb_prints_long_values_whole_in_smaller_type() {
        for (slug, cell, slot) in [
            ("2200a-2020", "p1c34", 12),
            ("2200c-2018", "p1c36", 13),
            ("2200p-2020", "p1c34", 12),
            ("1707a-2021", "p1c39", 12),
        ] {
            let long = format!(
                "{:<slot$}{}",
                "09171234567", "marita.joy.torres.accounting@example.com.ph"
            );
            assert!(long.chars().count() > 40);
            let mut fields = BTreeMap::new();
            fields.insert("derived:contact_email".to_string(), long.clone());
            let html = filled_document(slug, &fields).unwrap();
            assert!(
                html.contains(&format!("readonly value=\"{long}\" style=\"font-size:")),
                "{slug}: long line printed whole, in smaller type"
            );
            assert_eq!(comb_text(&html, cell), "", "{slug}: boxes left empty");
        }
    }

    #[test]
    fn shared_contact_email_comb_prints_short_values_in_the_boxes() {
        for (slug, cell) in [
            ("2200a-2020", "p1c34"),
            ("2200c-2018", "p1c36"),
            ("2200p-2020", "p1c34"),
            ("1707a-2021", "p1c39"),
        ] {
            let short = "0917123      a@b.ph";
            let mut fields = BTreeMap::new();
            fields.insert("derived:contact_email".to_string(), short.to_string());
            let html = filled_document(slug, &fields).unwrap();
            assert!(!html.contains("readonly value="), "{slug}: no overlay");
            assert_eq!(
                comb_text(&html, cell).replace(' ', ""),
                "0917123a@b.ph",
                "{slug}"
            );
        }
    }

    #[test]
    fn email_longer_than_its_comb_prints_whole() {
        let long = "Marita.Joy.Torres.Accounting@Example-Company.com.ph";
        let mut fields = BTreeMap::new();
        fields.insert("txtEmail".to_string(), long.to_string());
        let html = filled_document("2200an-2018", &fields).unwrap();
        assert!(html.contains(&format!("readonly value=\"{long}\"")));
        assert_eq!(comb_text(&html, "p1c35"), "");
        fields.insert("txtEmail".to_string(), "juan@example.com".to_string());
        let html = filled_document("2200an-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c35"), "juan@example.com");
        assert!(!html.contains("readonly value="));
    }

    #[test]
    fn filled_document_1601c_fills_email_and_splits_tax_money() {
        let mut fields = BTreeMap::new();
        fields.insert("txtEmail".to_string(), "juan@example.com".to_string());
        fields.insert("frm1601c:txtTax14".to_string(), "8888.88".to_string());
        fields.insert("frm1601c:txtTax25".to_string(), "7777.77".to_string());
        let html = filled_document("1601c-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c48"), "juan@example.com");
        assert!(html.contains("juan@example.com"));
        assert!(!html.contains("JUAN@EXAMPLE.COM"));
        assert_eq!(comb_text(&html, "p1c56"), "8888");
        assert_eq!(comb_text(&html, "p1c58"), "88");
        assert_eq!(comb_text(&html, "p1c101"), "7777");
        assert_eq!(comb_text(&html, "p1c103"), "77");
        assert!(!comb_text(&html, "p1c56").contains('.'));
        assert!(!comb_text(&html, "p1c101").contains('.'));
        assert!(html.contains("8888.88"));
        assert!(html.contains("7777.77"));
        let stamped: std::collections::BTreeSet<String> = input_tags(&html)
            .into_iter()
            .map(|tag| tag.name.to_string())
            .filter(|name| name.starts_with("frm1601c:"))
            .collect();
        assert_eq!(stamped.len(), 4);
        assert!(!html.contains("name=\"frm1601c:txtTax14\""));
        assert!(!html.contains("name=\"txtEmail\""));
    }

    #[test]
    fn filled_document_1601c_fills_month_year_amended_and_withheld() {
        let mut fields = BTreeMap::new();
        fields.insert("frm1601c:txtMonth".to_string(), "08".to_string());
        fields.insert("frm1601c:txtYear".to_string(), "2026".to_string());
        fields.insert("frm1601c:AmendedRtn_1".to_string(), "false".to_string());
        fields.insert("frm1601c:AmendedRtn_2".to_string(), "true".to_string());
        fields.insert("frm1601c:TaxWithheld_1".to_string(), "false".to_string());
        fields.insert("frm1601c:TaxWithheld_2".to_string(), "true".to_string());
        let html = filled_document("1601c-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c9"), "08");
        assert_eq!(comb_text(&html, "p1c10"), "2026");
        assert!(html.contains("2026"));
        assert_eq!(named_values(&html, "p1c22"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c20"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c21"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c23"), vec!["".to_string()]);
        assert!(!comb_text(&html, "p1c9").contains("true"));
        assert_ne!(named_values(&html, "p1c22"), vec!["true".to_string()]);
        assert!(!html.contains("name=\"frm1601c:txtMonth\""));
        assert!(!html.contains("name=\"frm1601c:txtYear\""));
        assert!(!html.contains("name=\"frm1601c:AmendedRtn_2\""));
        assert!(!html.contains("name=\"frm1601c:TaxWithheld_2\""));
        let stamped: std::collections::BTreeSet<String> = input_tags(&html)
            .into_iter()
            .map(|tag| tag.name.to_string())
            .filter(|name| name.starts_with("frm1601c:"))
            .collect();
        assert_eq!(stamped.len(), 4);

        fields.insert("frm1601c:AmendedRtn_1".to_string(), "true".to_string());
        fields.insert("frm1601c:AmendedRtn_2".to_string(), "false".to_string());
        fields.insert("frm1601c:TaxWithheld_1".to_string(), "true".to_string());
        fields.insert("frm1601c:TaxWithheld_2".to_string(), "false".to_string());
        let yes = filled_document("1601c-2018", &fields).unwrap();
        assert_eq!(named_values(&yes, "p1c21"), vec!["X".to_string()]);
        assert_eq!(named_values(&yes, "p1c23"), vec!["X".to_string()]);
        assert_eq!(named_values(&yes, "p1c22"), vec!["".to_string()]);
        assert_eq!(named_values(&yes, "p1c20"), vec!["".to_string()]);
    }

    #[test]
    fn filled_document_1601c_marks_item_11_category_of_withholding_agent() {
        let mut fields = BTreeMap::new();
        fields.insert("frm1601c:CatAgent_P".to_string(), "true".to_string());
        fields.insert("frm1601c:CatAgent_G".to_string(), "false".to_string());
        let private = filled_document("1601c-2018", &fields).unwrap();
        assert_eq!(named_values(&private, "p1c45"), vec!["X".to_string()]);
        assert_eq!(named_values(&private, "p1c46"), vec!["".to_string()]);
        assert!(!private.contains("name=\"frm1601c:CatAgent_P\""));

        fields.insert("frm1601c:CatAgent_P".to_string(), "false".to_string());
        fields.insert("frm1601c:CatAgent_G".to_string(), "true".to_string());
        let government = filled_document("1601c-2018", &fields).unwrap();
        assert_eq!(named_values(&government, "p1c45"), vec!["".to_string()]);
        assert_eq!(named_values(&government, "p1c46"), vec!["X".to_string()]);
        assert!(!government.contains("name=\"frm1601c:CatAgent_G\""));
    }

    #[test]
    fn filled_document_2551q_fills_taxpayer_name_and_address() {
        let draft = sample_draft();
        assert_eq!(draft.taxpayer_name, "Frozen Html Fixture");
        assert_eq!(draft.registered_address, "Sample Brgy");
        assert_eq!(draft.email, "tax@example.com");
        let html = filled_2551q_document(&draft);
        assert_eq!(draft.taxpayer_name, "Frozen Html Fixture");
        assert_eq!(comb_text(&html, "p1c30"), "FROZEN HTML FIXTURE");
        assert_eq!(comb_text(&html, "p1c32"), "SAMPLE BRGY");
        assert_eq!(comb_text(&html, "p1c39"), "tax@example.com");
        assert_eq!(comb_text(&html, "p1c9"), "12");
        assert_eq!(comb_text(&html, "p1c10"), "2026");
        assert_eq!(named_values(&html, "p1c7"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c11"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c15"), vec!["".to_string()]);
        assert!(html.contains("FROZEN HTML FIXTURE"));
        assert!(html.contains("data-writer-value=\"FROZEN HTML FIXTURE\""));
        assert!(html.contains("SAMPLE BRGY"));
        assert!(html.contains("tax@example.com"));
        assert!(!html.contains("Frozen Html Fixture"));
        assert!(!html.contains("Sample Brgy"));
        assert!(!html.contains("TAX@EXAMPLE.COM"));
        assert!(html.contains("2026"));
        let stamped: std::collections::BTreeSet<String> = input_tags(&html)
            .into_iter()
            .map(|tag| tag.name.to_string())
            .filter(|name| name.starts_with("frm2551Qv2018:"))
            .collect();
        assert_eq!(
            stamped,
            STAMPED_TIN_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect()
        );
    }

    #[test]
    fn filled_document_2551q_fills_email_and_splits_tax_money() {
        let mut fields = BTreeMap::new();
        fields.insert("txtEmail".to_string(), "marita@example.com".to_string());
        fields.insert("frm2551Qv2018:txt14".to_string(), "1643.10".to_string());
        fields.insert("txtATCAmt1".to_string(), "54770.00".to_string());
        let html = filled_document("2551q-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c39"), "marita@example.com");
        assert!(html.contains("marita@example.com"));
        assert!(!html.contains("MARITA@EXAMPLE.COM"));
        assert_eq!(comb_text(&html, "p1c50"), "1643");
        assert_eq!(comb_text(&html, "p1c52"), "10");
        assert_eq!(comb_text(&html, "p2c15"), "54770");
        assert_eq!(comb_text(&html, "p2c17"), "00");
        assert!(!comb_text(&html, "p1c50").contains('.'));
        assert!(!comb_text(&html, "p2c15").contains('.'));
        assert!(html.contains("1643.10"));
        assert!(html.contains("54770.00"));
        assert!(!html.contains("name=\"frm2551Qv2018:txt14\""));
        assert!(!html.contains("name=\"txtATCAmt1\""));
        assert!(!html.contains("name=\"txtEmail\""));
    }

    #[test]
    fn filled_document_2551q_fills_year_ended_and_quarter() {
        let mut fields = BTreeMap::new();
        fields.insert("frm2551Qv2018:txtYear".to_string(), "2026".to_string());
        fields.insert("frm2551Qv2018:rtnMonth".to_string(), "12".to_string());
        fields.insert("frm2551Qv2018:qtr_1".to_string(), "false".to_string());
        fields.insert("frm2551Qv2018:qtr_2".to_string(), "true".to_string());
        fields.insert("frm2551Qv2018:qtr_3".to_string(), "false".to_string());
        fields.insert("frm2551Qv2018:qtr_4".to_string(), "false".to_string());
        fields.insert("frm2551Qv2018:forThe_1".to_string(), "true".to_string());
        fields.insert("frm2551Qv2018:forThe_2".to_string(), "false".to_string());
        let html = filled_document("2551q-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c10"), "2026");
        assert_eq!(comb_text(&html, "p1c9"), "12");
        assert!(html.contains("2026"));
        assert_eq!(named_values(&html, "p1c15"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c11"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c12"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c13"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c7"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c8"), vec!["".to_string()]);
        assert!(!comb_text(&html, "p1c10").contains("true"));
        assert_ne!(named_values(&html, "p1c15"), vec!["true".to_string()]);
        assert!(!html.contains("name=\"frm2551Qv2018:txtYear\""));
        assert!(!html.contains("name=\"frm2551Qv2018:qtr_2\""));
        let stamped: std::collections::BTreeSet<String> = input_tags(&html)
            .into_iter()
            .map(|tag| tag.name.to_string())
            .filter(|name| name.starts_with("frm2551Qv2018:"))
            .collect();
        assert_eq!(stamped.len(), 4);
    }

    #[test]
    fn filled_document_1700_fills_identity_whole_peso_combs_and_boxes() {
        use bir_core::forms::form_1700::{
            Form1700CivilStatus, Form1700Column, Form1700Draft, Form1700Employer,
            Form1700TaxpayerType,
        };
        let profile: bir_core::profile::TaxpayerProfile =
            serde_json::from_value(serde_json::json!({
                "id": null, "full_name": "Fixture, Sample Employee", "tin": {"segment1": "123",
                "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
                "line_of_business": "Employee", "registered_address":
                "123 Sample Street, Barangay Example, Quezon City Metro Manila",
                "zip_code": "1100", "phone": "09170000000", "email": "fixture@example.com",
                "default_form_type": "1700", "taxpayer_type": "Individual"
            }))
            .unwrap();
        let mut draft = Form1700Draft::new_from_profile(&profile, 2025);
        draft.taxpayer_type = Form1700TaxpayerType::Employee;
        draft.birth_date = "01/15/1980".into();
        draft.citizenship = "Filipino".into();
        draft.foreign_tax_credits = Some(false);
        draft.civil_status = Form1700CivilStatus::Single;
        draft.number_of_attachments = 2;
        draft.taxpayer = Form1700Column {
            non_taxable: 90_000.0,
            surcharge: 1_000.0,
            ..Form1700Column::default()
        };
        // The fixture enters its penalties by hand.
        bir_core::forms::queueable::QueueableForm::keep_entered_penalties(&mut draft);
        let employer = |name: &str, regular: f64, withheld: f64| Form1700Employer {
            for_spouse: false,
            name: name.into(),
            name2: String::new(),
            tin: "12345678800000".into(),
            regular,
            flat: 0.0,
            withheld,
        };
        draft.employers = vec![employer("Sample Employer Inc", 600_000.5, 40_000.0)];
        draft.recompute();
        let html = filled_document("1700-2018", &draft.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c4"), "2025");
        assert_eq!(comb_text(&html, "p1c14"), "123");
        assert_eq!(comb_text(&html, "p1c21"), "039");
        assert!(html.contains("FIXTURE, SAMPLE EMPLOYEE"));
        assert_eq!(
            comb_text(&html, "p1c25"),
            "123 SAMPLE STREET, BARANGAY EXAMPLE,"
        );
        assert_eq!(comb_text(&html, "p1c26"), "QUEZON CITY METRO MANILA");
        assert_eq!(comb_text(&html, "p1c31"), "01");
        assert_eq!(comb_text(&html, "p1c33"), "1980");
        assert_eq!(comb_text(&html, "p1c118"), "02");
        // Whole pesos, right-aligned: Item 42A 600,001 in the 8-slot comb.
        let item_42a = named_values(&html, "p2c13");
        assert_eq!(item_42a.len(), 8);
        assert_eq!(item_42a.concat().trim(), "600001");
        assert_eq!(item_42a[7], "1");
        assert_eq!(comb_text(&html, "p2c16").trim(), "90000");
        // Item 31 Surcharge on the sheet; Item 32 Interest stays blank.
        assert_eq!(comb_text(&html, "p1c98").trim(), "1000");
        assert_eq!(comb_text(&html, "p1c101").trim(), "");
        // Column B of a non-joint return stays blank.
        assert_eq!(comb_text(&html, "p1c84").trim(), "");
        // Schedule 1 Item 1 and the 7-slot tax withheld comb.
        assert!(comb_text(&html, "p2c73").starts_with("SAMPLE EMPLOYER INC"));
        assert_eq!(comb_text(&html, "p2c76"), "12345678800000");
        assert_eq!(comb_text(&html, "p2c99").trim(), "40000");
        assert_eq!(named_values(&html, "p2c99").len(), 7);
        assert_eq!(comb_text(&html, "p2c5"), "123456788");
        assert_eq!(comb_text(&html, "p2c6"), "FIXTURE");
        assert_eq!(named_values(&html, "p1c9"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c8"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c42"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c47"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c55"), vec!["".to_string()]);
        assert!(!html.contains("name=\"frm1700:"));
        assert!(!html.contains("name=\"derived:"));
    }

    fn annual_fixture_profile(form: &str) -> bir_core::profile::TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Annual Fixture Corp", "tin": {"segment1": "123",
            "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
            "line_of_business": "Consulting",
            "registered_address": "Unit 1 Fixture Tower, 123 Sample Street, Barangay Example, Quezon City",
            "zip_code": "1100", "phone": "5551234", "email": "fixture@example.com",
            "default_form_type": form, "taxpayer_type": "Corporation"
        }))
        .unwrap()
    }

    #[test]
    fn filled_document_1604f_fills_identity_schedules_and_boxes() {
        use bir_core::forms::form_1604f::{Form1604fAgentCategory, Form1604fDraft};
        let mut draft = Form1604fDraft::new_from_profile(&annual_fixture_profile("1604F"), 2025);
        draft.agent_category = Form1604fAgentCategory::Private;
        draft.schedules[1][2].date = "10/30/2025".into();
        draft.schedules[1][2].reference = "tra-9".into();
        draft.schedules[1][2].taxes_withheld = 1_234.5;
        draft.schedules[1][2].penalties = 10.0;
        draft.recompute();
        let html = filled_document("1604f-2018", &draft.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c4"), "2025");
        assert_eq!(comb_text(&html, "p1c12"), "123");
        assert_eq!(comb_text(&html, "p1c18"), "00000");
        assert_eq!(comb_text(&html, "p1c20"), "039");
        assert_eq!(comb_text(&html, "p1c22"), "ANNUAL FIXTURE CORP");
        assert_eq!(
            comb_text(&html, "p1c24"),
            "UNIT 1 FIXTURE TOWER, 123 SAMPLE STREET,"
        );
        assert!(comb_text(&html, "p1c25").starts_with(" BARANGAY EXAMPLE, QUEZON CITY"));
        assert!(comb_text(&html, "p1c25").ends_with("1100"));
        assert_eq!(comb_text(&html, "p2c5"), "12345678800000");
        assert_eq!(named_values(&html, "p1c96"), vec!["TRA-9".to_string()]);
        assert_eq!(named_values(&html, "p1c97"), vec!["1,234.50".to_string()]);
        assert_eq!(named_values(&html, "p1c99"), vec!["1,244.50".to_string()]);
        assert_eq!(named_values(&html, "p1c109"), vec!["1,244.50".to_string()]);
        assert_eq!(named_values(&html, "p1c9"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c8"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c27"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c38"), vec!["X".to_string()]);
        assert!(!html.contains("name=\"frm1604f:"));
        assert!(!html.contains("name=\"derived:"));
    }

    #[test]
    fn filled_document_1604c_fills_months_refund_items_and_boxes() {
        use bir_core::forms::form_1604c::{Form1604cAgentCategory, Form1604cDraft};
        let mut draft = Form1604cDraft::new_from_profile(&annual_fixture_profile("1604C"), 2025);
        draft.agent_category = Form1604cAgentCategory::Government;
        let row = &mut draft.months[11];
        row.date = "01/15/2026".into();
        row.bank = "Bank 12".into();
        row.reference = "TRA12".into();
        row.taxes_withheld = 5_000.0;
        row.adjustment = -250.25;
        draft.recompute();
        let html = filled_document("1604c-2018", &draft.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c27"), "1100");
        assert_eq!(named_values(&html, "p1c114"), vec!["BANK 12".to_string()]);
        assert_eq!(named_values(&html, "p1c116"), vec!["5,000.00".to_string()]);
        assert_eq!(named_values(&html, "p1c169"), vec!["-250.25".to_string()]);
        assert_eq!(named_values(&html, "p1c171"), vec!["4,749.75".to_string()]);
        assert_eq!(named_values(&html, "p1c175"), vec!["4,749.75".to_string()]);
        // Item 11 "No": Items 11A-13 stay blank on paper.
        assert_eq!(comb_text(&html, "p1c40"), "");
        assert_eq!(comb_text(&html, "p1c46"), "");
        assert_eq!(named_values(&html, "p1c44"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c31"), vec!["X".to_string()]);

        draft.refunds_released = true;
        draft.refund_date = "03/15/2026".into();
        draft.overremittance = 4_321.56;
        draft.first_crediting_month = 2;
        draft.recompute();
        let html = filled_document("1604c-2018", &draft.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c40"), "03");
        assert_eq!(comb_text(&html, "p1c41"), "15");
        assert_eq!(comb_text(&html, "p1c42"), "2026");
        assert!(comb_text(&html, "p1c46").ends_with("4321"));
        assert_eq!(comb_text(&html, "p1c48"), "56");
        assert_eq!(comb_text(&html, "p1c50"), "02");
        assert_eq!(named_values(&html, "p1c43"), vec!["X".to_string()]);
        assert!(!html.contains("name=\"derived:"));
    }

    #[test]
    fn filled_document_1604e_fills_both_schedules_and_page_two() {
        use bir_core::forms::form_1604e::{Form1604eAgentCategory, Form1604eDraft};
        let mut draft = Form1604eDraft::new_from_profile(&annual_fixture_profile("1604E"), 2025);
        draft.agent_category = Form1604eAgentCategory::Private;
        draft.top_withholding_agent = Some(true);
        draft.schedule1[3].date = "01/30/2026".into();
        draft.schedule1[3].bank = "Bank4".into();
        draft.schedule1[3].reference = "Tra4".into();
        draft.schedule1[3].taxes_withheld = 250_000.0;
        draft.schedule2[11].date = "01/15/2026".into();
        draft.schedule2[11].bank = "BankDec".into();
        draft.schedule2[11].reference = "TRA12".into();
        draft.schedule2[11].taxes_withheld = 333.33;
        draft.schedule2[11].penalties = 12.0;
        draft.recompute();
        let html = filled_document("1604e-2018", &draft.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c22"), "ANNUAL FIXTURE CORP");
        assert_eq!(comb_text(&html, "p1c25"), " BARANGAY EXAMPLE, QUEZON CITY");
        assert_eq!(named_values(&html, "p1c71"), vec!["TRA4".to_string()]);
        assert_eq!(named_values(&html, "p1c72"), vec!["250,000.00".to_string()]);
        assert_eq!(named_values(&html, "p1c78"), vec!["250,000.00".to_string()]);
        assert_eq!(named_values(&html, "p1c170"), vec!["345.33".to_string()]);
        assert_eq!(named_values(&html, "p1c174"), vec!["345.33".to_string()]);
        assert_eq!(comb_text(&html, "p2c5"), "12345678800000");
        assert_eq!(comb_text(&html, "p2c6"), "ANNUAL FIXTURE CORP");
        assert_eq!(named_values(&html, "p1c30"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c32"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c33"), vec!["".to_string()]);
        assert!(!html.contains("name=\"frm1604e:"));
        assert!(!html.contains("name=\"derived:"));
    }

    #[test]
    fn filled_document_2553_fills_identity_rows_totals_and_boxes() {
        use bir_core::forms::form_2553::{Form2553Draft, FORM_2553_ATC_OPTIONS};
        let profile: bir_core::profile::TaxpayerProfile =
            serde_json::from_value(serde_json::json!({
                "id": null, "full_name": "Special Law Fixture Inc", "tin": {"segment1": "123",
                "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
                "line_of_business": "Gaming", "registered_address": "1 Fixture St",
                "zip_code": "1100", "phone": "5551234", "email": "fixture@example.com",
                "default_form_type": "2553", "taxpayer_type": "Corporation"
            }))
            .unwrap();
        let mut draft = Form2553Draft::new_from_profile(&profile, 2025, 2);
        draft.set_atc(0, &FORM_2553_ATC_OPTIONS[0]).unwrap();
        draft.schedule[0].taxable_amount = 123_456.79;
        draft.creditable_tax_withheld = 50_000.0;
        draft.recompute();
        let html = filled_document("2553-1999", &draft.to_bir_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c18"), "123");
        assert_eq!(comb_text(&html, "p1c24"), "00000");
        assert_eq!(comb_text(&html, "p1c16"), "039");
        assert_eq!(comb_text(&html, "p1c4"), "12");
        assert_eq!(comb_text(&html, "p1c5"), "2025");
        assert!(html.contains("SPECIAL LAW FIXTURE INC"));
        assert!(html.contains("123,456.79"));
        assert!(html.contains("6,172.84"));
        assert_eq!(named_values(&html, "p1c2"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c3"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c8"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c12"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c34"), vec!["X".to_string()]);
        assert!(!html.contains("name=\"frm2553:"));
    }

    fn fixture_profile(form: &str) -> bir_core::profile::TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Print Fixture Corp", "tin": {"segment1": "123",
            "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
            "line_of_business": "Brokerage", "registered_address": "1 Fixture St",
            "zip_code": "1100", "phone": "5551234", "email": "fixture@example.com",
            "default_form_type": form, "taxpayer_type": "Corporation"
        }))
        .unwrap()
    }

    #[test]
    fn filled_document_2552_fills_identity_money_schedules_and_boxes() {
        use bir_core::forms::form_2552::{Form2552Draft, Form2552ShareRow, Form2552Transaction};
        let mut draft = Form2552Draft::new_from_profile(&fixture_profile("2552"), 2025, 3, 14);
        draft.set_transaction(Form2552Transaction::LocalStockExchange);
        draft.schedule.push(Form2552ShareRow {
            date: "03/14/2025".into(),
            seller: "Seller One".into(),
            buyer: "Buyer One".into(),
            issuing_corporation: "Listed Corp".into(),
            number_of_shares: 1_000.0,
            tax_base: 1_234_567.89,
            ..Default::default()
        });
        draft.recompute();
        let html = filled_document("2552-2018", &draft.to_bir_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c6"), "03");
        assert_eq!(comb_text(&html, "p1c8"), "2025");
        assert_eq!(comb_text(&html, "p1c14"), "123");
        assert_eq!(comb_text(&html, "p1c20"), "00000");
        assert_eq!(comb_text(&html, "p1c22"), "039");
        assert!(
            html.contains("PRINT FIXTURE CORP") || comb_text(&html, "p1c24").starts_with("PRINT")
        );
        // Item 13: 7,407.00 splits into the peso and centavo combs.
        assert_eq!(comb_text(&html, "p1c51").trim(), "7407");
        assert_eq!(comb_text(&html, "p1c53"), "00");
        assert_eq!(named_values(&html, "p1c41"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c11"), vec!["X".to_string()]);
        assert!(html.contains("1,234,567.89"));
        assert!(html.contains("SELLER ONE"));
        assert!(!html.contains("name=\"frm2552:"));
    }

    #[test]
    fn filled_document_2550m_fills_items_and_schedules() {
        use bir_core::forms::form_2550m::{Form2550MDraft, Form2550MWithholdingRow};
        let mut draft = Form2550MDraft::new_from_profile(&fixture_profile("2550M"), 2022, 6);
        draft.add_sales_atc("VB010").unwrap();
        draft.sales_schedule[0].amount = 100_000.0;
        draft.schedule_6.push(Form2550MWithholdingRow {
            period_covered: "06/30/2022".into(),
            withholding_agent: "Agent Corp".into(),
            income_payment: 1_000.0,
            total_withheld: 50.0,
            applied_current_month: 40.0,
        });
        draft.recompute();
        let html = filled_document("2550m-2007", &draft.to_bir_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c1"), "06");
        assert_eq!(comb_text(&html, "p1c2"), "2022");
        assert!(html.contains("PRINT FIXTURE CORP"));
        assert!(html.contains("100,000.00"));
        assert!(html.contains("12,000.00"));
        assert!(html.contains("VB010"));
        assert!(html.contains("AGENT CORP"));
        assert_eq!(named_values(&html, "p1c5"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c25"), vec!["X".to_string()]);
    }

    /// A Rust-side join that writer-cells.json already has (a later
    /// re-sync), by key or by cell, is skipped: the cell is filled once.
    #[test]
    fn a_sheet_join_the_writer_cells_already_have_is_not_added_twice() {
        let mut cells = WriterCells::empty();
        cells
            .joins
            .insert("frm2550m:txtTIN1".into(), vec!["p1c127".into()]);
        cells
            .joins
            .insert("frm2550m:txtOther".into(), vec!["p1c9".into()]);
        cells
            .not_joined
            .insert("frm2550m:txtTIN3".into(), "old reason".into());
        merge_sheet_joins(&mut cells, sheet_joins("2550m-2007"));
        assert_eq!(cells.joins["frm2550m:txtTIN1"], vec!["p1c127".to_string()]);
        assert!(
            !cells.joins.contains_key("frm2550m:txtTIN2"),
            "p1c9 is taken"
        );
        assert_eq!(cells.joins["frm2550m:txtTIN3"], vec!["p1c11".to_string()]);
        assert!(!cells.not_joined.contains_key("frm2550m:txtTIN3"));
        let targets: Vec<&str> = cells.targets().into_iter().map(|(_, c)| c).collect();
        let unique: BTreeSet<&str> = targets.iter().copied().collect();
        assert_eq!(targets.len(), unique.len(), "{targets:?}");
        // The fill of a sheet whose joins name a cell twice would overlap
        // its replacements; the merged print fills each once.
        let real = writer_cells("2550m-2007").unwrap();
        let targets: Vec<&str> = real.targets().into_iter().map(|(_, c)| c).collect();
        let unique: BTreeSet<&str> = targets.iter().copied().collect();
        assert_eq!(targets.len(), unique.len());
        let merged = merged_writer_cells_json("2550m-2007").unwrap();
        let joined: Vec<&str> = merged["joins"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|j| {
                j["writer_key"].as_str().unwrap().contains("txtTIN")
                    || j["writer_key"] == "frm2550m:txtBranchCode"
            })
            .map(|j| j["html_id"].as_str().unwrap())
            .collect();
        assert_eq!(joined, ["p1c127", "p1c9", "p1c11", "p1c13"]);
        assert!(!merged["not_joined"].to_string().contains("txtTIN1"));
        assert_eq!(
            sheet_join_combs("2550m-2007"),
            vec![
                ("frm2550m:txtTIN1".to_string(), 3, "4 TIN (1-3)".to_string()),
                ("frm2550m:txtTIN2".to_string(), 3, "4 TIN (4-6)".to_string()),
                ("frm2550m:txtTIN3".to_string(), 3, "4 TIN (7-9)".to_string()),
                (
                    "frm2550m:txtBranchCode".to_string(),
                    5,
                    "4 TIN branch".to_string()
                ),
            ]
        );
    }

    /// PR 4 (2550M manual-3): item 4 prints the TIN's three parts and the
    /// branch in the row's combs (p1c127, p1c9, p1c11, p1c13; frozen_html.rs
    /// SHEET_JOINS, since the frozen writer-cells.json leaves them out),
    /// and items 3, 5 and
    /// 10, rows of one-character boxes, print one character per box: never
    /// the whole value in each.
    #[test]
    fn filled_document_2550m_prints_the_tin_and_one_character_per_box() {
        use bir_core::forms::form_2550m::Form2550MDraft;
        let mut draft = Form2550MDraft::new_from_profile(&fixture_profile("2550M"), 2022, 6);
        draft.number_of_attached_sheets = 3;
        draft.recompute();
        let fields = draft.to_bir_field_map();
        let tin: String = draft.tin.chars().filter(char::is_ascii_digit).collect();
        let html = filled_document("2550m-2007", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c127"), tin[0..3]);
        assert_eq!(comb_text(&html, "p1c9"), tin[3..6]);
        assert_eq!(comb_text(&html, "p1c11"), tin[6..9]);
        assert_eq!(comb_text(&html, "p1c13"), "00000");
        // Each box of a split cell keeps only its own character; the
        // writer's whole value stays on the first box alone.
        let writer_values: Vec<Option<String>> = input_tags(&html)
            .into_iter()
            .filter(|tag| tag.name == "p1c130")
            .map(|tag| attr(tag.tag, "data-writer-value").map(str::to_string))
            .collect();
        assert_eq!(writer_values, vec![Some("039".to_string()), None, None]);
        let boxes = |name: &str| named_values(&html, name);
        let one_each = |value: &str| value.chars().map(String::from).collect::<Vec<_>>();
        assert_eq!(boxes("p1c130"), one_each(&fields["frm2550m:txtRDOCode"]));
        assert_eq!(fields["frm2550m:txtRDOCode"].len(), 3);
        assert_eq!(boxes("p1c21"), one_each(&fields["frm2550m:txtZipCode"]));
        assert_eq!(fields["frm2550m:txtZipCode"].len(), 4);
        // A one-digit count sits in the right-hand box.
        assert_eq!(fields["frm2550m:txtSheets"], "3");
        assert_eq!(boxes("p1c3"), vec![String::new(), "3".to_string()]);
        // A value longer than its boxes prints once, as a plain line across
        // the cell; its boxes stay empty, the writer's value on the first.
        // (Replaced in round 6, D4: it printed whole in the first box.)
        let mut long = fields.clone();
        long.insert("frm2550m:txtZipCode".into(), "123456".into());
        let html = filled_document("2550m-2007", &long).unwrap();
        assert_eq!(named_values(&html, "p1c21"), vec![String::new(); 4]);
        let writer_values: Vec<Option<&str>> = input_tags(&html)
            .into_iter()
            .filter(|tag| tag.name == "p1c21")
            .map(|tag| attr(tag.tag, "data-writer-value"))
            .collect();
        assert_eq!(writer_values, vec![Some("123456"), None, None, None]);
        let (text, size) = printed_cells(&html).remove("p1c21").unwrap();
        assert_eq!(text, "123456");
        // The line prints at 9 pt.
        assert_eq!(size, Some(9.0));
        // A branch other than 00000 prints over the sheet's 00000 (the
        // comb's inputs carry 0s as values; the html-frozen file is not
        // edited).
        let mut branch = draft.clone();
        branch.tin = "12345678800001".into();
        let html = filled_document("2550m-2007", &branch.to_bir_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c13"), "00001");
        assert_eq!(comb_text(&html, "p1c127"), "123");
    }

    #[test]
    fn filled_document_2200m_fills_identity_schedule_money_and_boxes() {
        use bir_core::forms::form_2200m::{Form2200MDraft, Form2200MPayment};
        let profile: bir_core::profile::TaxpayerProfile =
            serde_json::from_value(serde_json::json!({
                "id": null, "full_name": "Mineral Fixture Corp", "tin": {"segment1": "123",
                "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
                "line_of_business": "Mining", "registered_address": "1 Fixture St",
                "zip_code": "1100", "phone": "5551234", "email": "fixture@example.com",
                "default_form_type": "2200M", "taxpayer_type": "Corporation"
            }))
            .unwrap();
        let mut draft = Form2200MDraft::new_from_profile(&profile, 2025, 3);
        draft.day = 14;
        draft.manner_of_payment = Form2200MPayment::Prepayment;
        let row = draft.row_mut(3).unwrap();
        row.place_of_removal = "Rizal".into();
        row.local_value_taxable = Some(100_000.0);
        row.local_tax_due = Some(4_000.0);
        draft.surcharge = 25.5;
        // The fixture enters its penalties by hand.
        bir_core::forms::queueable::QueueableForm::keep_entered_penalties(&mut draft);
        draft.recompute();
        draft.tax_deposit = draft.tax_still_due;
        draft.recompute();
        let html = filled_document("2200m-2018", &draft.to_bir_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c4"), "03");
        assert_eq!(comb_text(&html, "p1c6"), "14");
        assert_eq!(comb_text(&html, "p1c8"), "2025");
        assert_eq!(comb_text(&html, "p1c16"), "123");
        assert_eq!(comb_text(&html, "p1c22"), "00000");
        assert_eq!(comb_text(&html, "p1c24"), "039");
        assert!(html.contains("MINERAL FIXTURE CORP"));
        assert!(html.contains("100,000.00"));
        assert!(html.contains("RIZAL"));
        // Item 16 4,000.00 and Item 22 4,025.50 in peso + centavo combs.
        assert_eq!(comb_text(&html, "p1c65"), "4000");
        assert_eq!(comb_text(&html, "p1c67"), "00");
        assert_eq!(comb_text(&html, "p1c109"), "4025");
        assert_eq!(comb_text(&html, "p1c111"), "50");
        assert_eq!(named_values(&html, "p1c13"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c12"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c59"), vec!["X".to_string()]);
        assert!(!html.contains("name=\"frm2200M:"));
    }

    #[test]
    fn filled_document_2200a_fills_shared_combs_and_others_rows() {
        use bir_core::forms::form_2200a::{Form2200ADraft, Form2200APlace};
        let mut draft = Form2200ADraft::new_from_profile(&fixture_profile("2200A"), 2025, 6);
        draft.contact_number = "09171234567".into();
        draft.email = "Me@Example.ph".into();
        draft.place_of_production = Form2200APlace {
            region: "010000000".into(),
            province: "012800000".into(),
            city: "012801000".into(),
        };
        let row = draft.other_mut(1).expect("Others row 2");
        row.atc_digits = "123".into();
        row.description = "Fruit wine".into();
        row.bracket = "per liter".into();
        row.rate = "52.00".into();
        let html = filled_document("2200a-2020", &draft.to_print_field_map()).unwrap();
        // Items 8-9 share comb p1c34: contact from slot 0, email (as typed) from slot 12.
        assert_eq!(comb_text(&html, "p1c34"), "09171234567 Me@Example.ph");
        // Item 10 comb p1c39: region from slot 0, province from 10, city from 24.
        assert_eq!(
            comb_text(&html, "p1c39"),
            "010000000 012800000     012801000"
        );
        assert_eq!(comb_text(&html, "p1c44"), "");
        // Others row 2: ATC p3c103, description p3c104, bracket p3c105, rate p3c106
        // (the sheet prints in capitals; only the email stays as typed).
        for (cell, value) in [
            ("p3c103", "123"),
            ("p3c104", "FRUIT WINE"),
            ("p3c105", "PER LITER"),
            ("p3c106", "52.00"),
        ] {
            assert_eq!(named_values(&html, cell), vec![value.to_string()], "{cell}");
        }
    }

    #[test]
    fn filled_document_2200c_fills_item_6_and_schedule_1_rows() {
        use bir_core::forms::form_2200c::{Form2200CDraft, Form2200CManner, Form2200CRow};
        let mut draft = Form2200CDraft::new_from_profile(&fixture_profile("2200C"), 2025);
        draft.month = 6;
        draft.day = 30;
        draft.rdo_code = "039".into();
        // Part V is cleared until a manner of payment is picked (resetPartFive).
        draft.manner = Form2200CManner::ActualRemoval;
        // Row 1 is populated, rows 2-9 stay blank, row 10 is populated.
        let mut schedule = vec![Form2200CRow {
            exempt_procedures: 1.0,
            excisable_procedures: 3.0,
            non_excisable_procedures: 2.0,
            net_of_vat: 10_000.0,
            excisable_net: 50_010.0,
            non_excisable: 5_000.0,
            ..Default::default()
        }];
        schedule.resize(9, Form2200CRow::default());
        schedule.push(Form2200CRow {
            exempt_procedures: 4.0,
            net_of_vat: 2_000.0,
            ..Default::default()
        });
        draft.schedule = schedule;
        draft.recompute();
        draft.contact_number = "09171234567".into();
        draft.email = "Me@Example.ph".into();
        draft.place = bir_core::forms::excise_places::ExcisePlace {
            region: "010000000".into(),
            province: "012800000".into(),
            city: "012801000".into(),
        };
        let fields = draft.to_print_field_map();
        let html = filled_document("2200c-2018", &fields).unwrap();
        // Item 6 RDO comb (p1c26), 4 slots.
        assert_eq!(comb_text(&html, "p1c26"), "039");
        // Items 9-10 share comb p1c36: contact from slot 0, email (as typed) from slot 13.
        assert_eq!(comb_text(&html, "p1c36"), "09171234567  Me@Example.ph");
        // Schedule 1 row 1 runs p2c25 (A) to p2c34 (J), top to bottom.
        let row_1 = [
            "1.00",
            "3.00",
            "2.00",
            "10,000.00",
            "50,010.00",
            "0.00",
            "5,000.00",
            "2,501.00",
            "8,101.00",
            "75,612.00",
        ];
        for (offset, value) in row_1.iter().enumerate() {
            assert_eq!(
                named_values(&html, &format!("p2c{}", 25 + offset)),
                vec![value.to_string()],
                "schedule row 1 column {offset}"
            );
        }
        // Schedule 1 row 10 runs p2c115 to p2c124; its values come from the field map.
        assert_eq!(named_values(&html, "p2c115"), vec!["4.00".to_string()]);
        let columns = [
            "CPPexmpt",
            "CPPexcise",
            "CPPnonexcise",
            "GRnetVat",
            "GRexciseNet",
            "GRexciseExmpt",
            "GRnonexcise",
            "ACexcise",
            "ACvat",
            "ACtotAmountBill",
        ];
        for (offset, column) in columns.iter().enumerate() {
            let key = format!("frm2200C:{column}10");
            assert_eq!(
                named_values(&html, &format!("p2c{}", 115 + offset)),
                vec![fields[&key].clone()],
                "{key}"
            );
        }
        // A blank row (row 2) still prints the map's zero amounts, as the official XML does.
        assert_eq!(named_values(&html, "p2c35"), vec!["0.00".to_string()]);
        // TOTAL EXCISE TAX DUE (p2c126) carries the ACexciseTotal value.
        assert_eq!(
            named_values(&html, "p2c126"),
            vec![fields["frm2200C:ACexciseTotal"].clone()]
        );
        // Item 11 shares comb p1c41: region from slot 0, province from 10, city from 24.
        assert_eq!(
            comb_text(&html, "p1c41"),
            "010000000 012800000     012801000"
        );
    }

    fn withholding_profile(code: &str) -> bir_core::profile::TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Withholding Fixture Inc", "tin": {"segment1": "123",
            "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
            "line_of_business": "Fixture", "registered_address": "1 Fixture St",
            "zip_code": "1100", "phone": "5551234", "email": "fixture@example.com",
            "default_form_type": code, "taxpayer_type": "Corporation"
        }))
        .unwrap()
    }

    #[test]
    fn filled_document_1606_fills_parties_atc_box_amounts_and_schedules() {
        use bir_core::forms::form_1606::{
            Form1606Draft, Form1606SellerType, Form1606TaxRate, Form1606Transaction,
        };
        let mut draft = Form1606Draft::new_from_profile(&withholding_profile("1606"), 2025, 1);
        draft.transaction_month = 6;
        draft.transaction_day = 15;
        draft.is_amended = true;
        draft.seller_tin = "98765432100000".into();
        draft.seller_name = "Seller Fixture".into();
        draft.seller_type = Some(Form1606SellerType::Corporation);
        draft.tct_number = "T-1".into();
        draft.set_transaction(Form1606Transaction::InstallmentSale);
        draft.gross_selling_price = 5_000_000.0;
        draft.installment_collected = 250_000.0;
        draft.tax_rate = Some(Form1606TaxRate::OnePointFive);
        draft.recompute();
        let html = filled_document("1606-2018", &draft.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c4"), "06");
        assert_eq!(comb_text(&html, "p1c6"), "2025");
        assert_eq!(named_values(&html, "p1c14"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c11"), vec!["".to_string()]);
        assert!(html.contains("WITHHOLDING FIXTURE INC"));
        assert_eq!(comb_text(&html, "p1c27"), "123-456-788-00000");
        assert_eq!(comb_text(&html, "p1c42"), "987-654-321-00000");
        // Corporate seller at 1.5%: the WC 555 box.
        assert_eq!(named_values(&html, "p1c57"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c53"), vec!["".to_string()]);
        assert_eq!(comb_text(&html, "p1c74"), "1");
        assert_eq!(comb_text(&html, "p1c76"), "5");
        assert_eq!(comb_text(&html, "p1c70"), "250000");
        assert_eq!(comb_text(&html, "p1c79"), "3750");
        assert_eq!(comb_text(&html, "p1c81"), "00");
        assert_eq!(named_values(&html, "p2c85"), vec!["X".to_string()]);
        assert_eq!(comb_text(&html, "p2c81"), "5000000");
        assert_eq!(comb_text(&html, "p2c5"), "12345678800000");
        assert_eq!(comb_text(&html, "p2c13"), "T-1");
        assert!(!html.contains("name=\"frm1606:"));
        assert!(!html.contains("name=\"derived:"));
    }

    #[test]
    fn print_carries_the_used_penalty_values() {
        // A late June 2025 1600-VT filed 13 October 2026: the print shows the
        // computed surcharge and interest and the compromise override, the
        // same values the XML and the queue fingerprint carry.
        use bir_core::forms::form_1600vt::{Form1600VtAgentCategory, Form1600VtDraft};
        use bir_core::forms::penalty_items::with_clock;
        use bir_core::forms::queueable::QueueableForm;
        use bir_core::penalties::PenaltyComponent;
        let mut vt = Form1600VtDraft::new_from_profile(&withholding_profile("1600VT"), 2025, 6);
        vt.set_agent_category(Form1600VtAgentCategory::Government);
        vt.set_taxes_withheld(true);
        vt.toggle_atc(2).unwrap();
        vt.atc_rows[0].tax_base = 100_000.0;
        vt.set_penalty_override(PenaltyComponent::Compromise, Some(500.0));
        let filed = serde_json::from_value(serde_json::json!("2026-10-13")).unwrap();
        with_clock(filed, || vt.recompute());
        let computed = vt.penalty.computed_penalties.clone().unwrap();
        assert!(computed.surcharge > 0.0 && computed.interest > 0.0);
        assert_eq!(vt.surcharge, computed.surcharge);
        assert_eq!(vt.interest, computed.interest);
        assert_eq!(vt.compromise, 500.0);

        let fields = vt.to_print_field_map();
        let amount = bir_core::official_xml::official_amount;
        assert_eq!(
            fields["frm1600VT:txtPg1P2I23Surcharge"],
            amount(computed.surcharge)
        );
        assert_eq!(
            fields["frm1600VT:txtPg1P2I24Interest"],
            amount(computed.interest)
        );
        assert_eq!(fields["frm1600VT:txtPg1P2I25Compromise"], amount(500.0));
        assert_eq!(fields, {
            let mut xml = vt.field_map();
            xml.extend(fields.iter().map(|(k, v)| (k.clone(), v.clone())));
            xml
        });
        let html = filled_document("1600-vt-2018", &fields).unwrap();
        let total_penalties = amount(vt.total_penalties);
        assert!(
            html.contains(&total_penalties) || html.contains(&total_penalties.replace(',', "")),
            "the print shows Item 26 = {total_penalties}"
        );
    }

    #[test]
    fn filled_document_1600vt_and_1600pt_fill_identity_part_ii_and_schedule() {
        use bir_core::forms::form_1600pt::{Form1600PtAgentCategory, Form1600PtDraft};
        use bir_core::forms::form_1600vt::{
            Form1600VtAgentCategory, Form1600VtDraft, Form1600VtScheduleRow,
        };
        let mut vt = Form1600VtDraft::new_from_profile(&withholding_profile("1600VT"), 2025, 6);
        // The fixture enters its penalties by hand (none), so the late
        // period's computed penalties do not move the printed totals.
        bir_core::forms::queueable::QueueableForm::keep_entered_penalties(&mut vt);
        vt.set_agent_category(Form1600VtAgentCategory::Government);
        vt.set_taxes_withheld(true);
        vt.toggle_atc(2).unwrap();
        vt.atc_rows[0].tax_base = 10_000.0;
        vt.schedule.push(Form1600VtScheduleRow {
            tin: "987654321000".into(),
            payee_name: "Payee Fixture".into(),
            atc: "WV110".into(),
            income_payment: 10_000.0,
            tax_rate: 12.0,
            tax_withheld: 0.0,
        });
        vt.recompute();
        let html = filled_document("1600-vt-2018", &vt.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c4"), "06");
        assert_eq!(comb_text(&html, "p1c5"), "25");
        assert_eq!(comb_text(&html, "p1c16"), "123");
        assert_eq!(comb_text(&html, "p1c22"), "00000");
        assert_eq!(named_values(&html, "p1c35"), vec!["X".to_string()]);
        assert_eq!(comb_text(&html, "p1c50"), "WV110");
        assert_eq!(comb_text(&html, "p1c51"), "10000");
        assert_eq!(comb_text(&html, "p1c54"), "12");
        assert_eq!(comb_text(&html, "p1c56"), "1200");
        assert_eq!(comb_text(&html, "p1c136"), "1200");
        assert_eq!(comb_text(&html, "p2c5"), "12345678800000");
        assert_eq!(comb_text(&html, "p2c17"), "PAYEE FIXTURE");
        assert_eq!(comb_text(&html, "p2c21"), "1,200.00");
        assert!(!html.contains("name=\"frm1600VT:"));

        let mut pt = Form1600PtDraft::new_from_profile(&withholding_profile("1600PT"), 2025, 6);
        bir_core::forms::queueable::QueueableForm::keep_entered_penalties(&mut pt);
        pt.set_agent_category(Form1600PtAgentCategory::Government);
        pt.set_taxes_withheld(true);
        let wb200 = bir_core::forms::form_1600pt::form_1600pt_atc_index("WB200", "0.6").unwrap();
        pt.toggle_atc(wb200).unwrap();
        pt.atc_rows[0].tax_base = 10_000.0;
        pt.recompute();
        let html = filled_document("1600-pt-2018", &pt.to_print_field_map()).unwrap();
        // 1600-PT prints Private left of Government (1600-VT the other way).
        assert_eq!(named_values(&html, "p1c36"), vec!["X".to_string()]);
        assert_eq!(comb_text(&html, "p1c50"), "WB200");
        assert_eq!(comb_text(&html, "p1c54"), ".6");
        assert_eq!(comb_text(&html, "p1c56"), "60");
    }

    #[test]
    fn filled_document_1600wp_fills_part_ii_by_atc_row_and_schedule() {
        use bir_core::forms::form_1600wp::{
            Form1600WpAgentCategory, Form1600WpDraft, Form1600WpScheduleRow,
        };
        let mut draft = Form1600WpDraft::new_from_profile(&withholding_profile("1600WP"), 2025, 6);
        draft.day = 15;
        draft.set_taxes_withheld(true);
        draft.set_agent_category(Form1600WpAgentCategory::Private);
        draft.toggle_atc("WB194").unwrap();
        draft.atc_rows[0].tax_base = 2_500.0;
        draft.schedule.push(Form1600WpScheduleRow {
            tin: "987654321000".into(),
            payee_name: "Winner Fixture".into(),
            atc_code: "WB194".into(),
            amount: 2_500.0,
            tax_withheld: 0.0,
        });
        draft.recompute();
        let html = filled_document("1600wp-2010", &draft.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c1"), "06152025");
        assert_eq!(comb_text(&html, "p1c4"), "2025");
        assert_eq!(named_values(&html, "p1c8"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c18"), vec!["X".to_string()]);
        // The only ticked ATC is WB194: its printed row, not WB191's.
        assert_eq!(comb_text(&html, "p1c47"), "2,500.00");
        assert_eq!(comb_text(&html, "p1c49"), "250.00");
        assert_eq!(comb_text(&html, "p1c32"), "");
        assert_eq!(comb_text(&html, "p1c51"), "250.00");
        assert_eq!(comb_text(&html, "p1c96"), "WINNER FIXTURE");
        assert_eq!(comb_text(&html, "p1c74"), "00000");
        assert!(!html.contains("name=\"frm1600WP:"));
    }

    #[test]
    fn filled_document_1702q_fills_identity_amounts_rates_and_boxes() {
        use bir_core::forms::form_1702q::{
            Form1702qDeduction, Form1702qDraft, Form1702qOtherCredit, Form1702qSchedule1Column,
        };
        let profile: bir_core::profile::TaxpayerProfile =
            serde_json::from_value(serde_json::json!({
                "id": null, "full_name": "Sample Dummy Corporation", "tin": {"segment1": "123",
                "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
                "line_of_business": "Consulting",
                "registered_address": "123 Sample Street, Barangay Example, Quezon City",
                "zip_code": "1100", "phone": "09170000000", "email": "sample@example.com",
                "default_form_type": "1702Q", "taxpayer_type": "Corporation"
            }))
            .unwrap();
        let mut draft = Form1702qDraft::new_from_profile(&profile, 2025, 2);
        draft.atc_mcit = true;
        draft.atc = "IC010_25%".to_string();
        draft.deduction = Form1702qDeduction::Itemized;
        draft.special = Form1702qSchedule1Column {
            sales: 800_000.5,
            cost_of_sales: 300_000.0,
            rate: 10.0,
            ..Form1702qSchedule1Column::default()
        };
        draft.sales = 5_000_000.5;
        draft.cost_of_sales = 1_200_000.0;
        draft.mcit_gross_income_q2 = 3_800_000.5;
        draft.other_credits = vec![Form1702qOtherCredit {
            description: "Sample credit".to_string(),
            amount: 1_234.0,
        }];
        draft.recompute();
        let html = filled_document("1702q-2018", &draft.to_print_field_map()).unwrap();
        assert_eq!(named_values(&html, "p1c8"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c7"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c13"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c16"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c11"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c19"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c49"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c54"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c20"), vec!["1".to_string()]);
        assert_eq!(named_values(&html, "p1c23"), vec!["5".to_string()]);
        assert_eq!(named_values(&html, "p1c17"), vec!["IC 010".to_string()]);
        assert_eq!(comb_text(&html, "p1c26"), "123");
        assert_eq!(comb_text(&html, "p1c34"), "039");
        assert_eq!(comb_text(&html, "p1c37"), "SAMPLE DUMMY CORPORATION");
        assert_eq!(
            comb_text(&html, "p1c40"),
            "123 SAMPLE STREET, BARANGAY EXAMPLE, Q"
        );
        assert_eq!(comb_text(&html, "p1c41"), "UEZON CITY");
        assert_eq!(comb_text(&html, "p1c47"), "sample@example.com");
        assert_eq!(comb_text(&html, "p2c5"), "123456788");
        assert_eq!(comb_text(&html, "p2c55"), "5,000,001.00");
        assert_eq!(comb_text(&html, "p2c57"), "1,200,000.00");
        assert_eq!(comb_text(&html, "p2c13"), "  800,001.00");
        assert_eq!(comb_text(&html, "p2c42"), "10");
        assert_eq!(comb_text(&html, "p2c44"), "0");
        assert_eq!(comb_text(&html, "p2c73"), "25");
        assert_eq!(comb_text(&html, "p2c110"), "SAMPLE CREDIT");
        assert_eq!(comb_text(&html, "p2c112"), "    1,234.00");
        assert_eq!(
            comb_text(&html, "p1c57"),
            format!(
                "{:>12}",
                bir_core::official_xml::official_amount(draft.income_tax_due)
            )
        );
        assert!(!html.contains("name=\"frm1702q:"));
    }

    #[test]
    fn filled_document_1701a_fills_identity_whole_peso_combs_and_boxes() {
        use bir_core::forms::form_1701a::{
            Form1701AAtc, Form1701ACivilStatus, Form1701AColumn, Form1701ADraft, Form1701AFilerType,
        };
        let profile: bir_core::profile::TaxpayerProfile =
            serde_json::from_value(serde_json::json!({
                "id": null, "full_name": "Dummy, Sample Taxpayer", "tin": {"segment1": "123",
                "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
                "line_of_business": "Consulting",
                "registered_address": "123 Sample Street, Barangay Example, Quezon City",
                "zip_code": "1100", "phone": "09170000000", "email": "sample@example.com",
                "default_form_type": "1701A", "taxpayer_type": "Individual"
            }))
            .unwrap();
        let mut draft = Form1701ADraft::new_from_profile(&profile, 2025);
        draft.filer_type = Form1701AFilerType::SingleProprietor;
        draft.atc = Form1701AAtc::II012;
        draft.civil_status = Form1701ACivilStatus::Single;
        draft.birth_date = "01/15/1980".to_string();
        draft.citizenship = "Filipino".to_string();
        draft.other_income_41_description = "Interest income".to_string();
        draft.taxpayer = Form1701AColumn {
            sales: 1_234_567.89,
            sales_returns: 10_000.5,
            other_income_41: 5_000.55,
            gpp_share: 20_000.0,
            quarterly_payments: 30_000.0,
            surcharge: 1_000.0,
            ..Form1701AColumn::default()
        };
        draft.recompute();
        let html = filled_document("1701a-2018", &draft.to_print_field_map()).unwrap();
        // Header and identity.
        assert_eq!(comb_text(&html, "p1c4"), "12");
        assert_eq!(comb_text(&html, "p1c5"), "2025");
        assert_eq!(comb_text(&html, "p1c16"), "123");
        assert_eq!(comb_text(&html, "p1c20"), "788");
        assert_eq!(comb_text(&html, "p1c23"), "039");
        assert_eq!(comb_text(&html, "p1c32"), "DUMMY, SAMPLE TAXPAYER");
        assert_eq!(
            comb_text(&html, "p1c34"),
            "123 SAMPLE STREET, BARANGAY EXAMPLE,"
        );
        assert_eq!(comb_text(&html, "p1c35"), "QUEZON CITY");
        assert_eq!(comb_text(&html, "p1c40"), "01");
        assert_eq!(comb_text(&html, "p1c42"), "1980");
        assert_eq!(comb_text(&html, "p1c43"), "sample@example.com");
        assert_eq!(comb_text(&html, "p1c47"), "FILIPINO");
        assert_eq!(comb_text(&html, "p2c5"), "123456788");
        assert_eq!(comb_text(&html, "p2c6"), "DUMMY");
        assert!(html.contains("INTEREST INCOME"));
        // Whole pesos, right-aligned in the 8-slot comb; no decimal glyph.
        assert_eq!(comb_text(&html, "p2c13"), " 1234568");
        assert_eq!(comb_text(&html, "p2c48"), "   94448");
        assert_eq!(comb_text(&html, "p1c74"), "   94448");
        assert_eq!(comb_text(&html, "p1c89"), "    1000");
        assert!(!comb_text(&html, "p1c74").contains('.'));
        // Zero and column B (not joint) stay blank.
        assert_eq!(comb_text(&html, "p2c52").trim(), "");
        assert_eq!(comb_text(&html, "p1c75").trim(), "");
        // X boxes.
        assert_eq!(named_values(&html, "p1c9"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c163"), vec!["".to_string()]);
        assert_eq!(named_values(&html, "p1c11"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c24"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c27"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c195"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c56"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p1c67"), vec!["X".to_string()]);
        assert_eq!(named_values(&html, "p2c126"), vec!["".to_string()]);
        assert!(!html.contains("name=\"frm1701A:"));
    }

    #[test]
    fn filled_document_1701ms_fills_both_columns_amounts_and_boxes() {
        use bir_core::forms::form_1701ms::{
            Form1701MsCivilStatus, Form1701MsColumn, Form1701MsDeduction, Form1701MsDraft,
            Form1701MsFiling, Form1701MsSource, Form1701MsSpouseInfo, Form1701MsTaxOption,
        };
        let profile: bir_core::profile::TaxpayerProfile =
            serde_json::from_value(serde_json::json!({
                "id": null, "full_name": "Sample Dummy Taxpayer", "tin": {"segment1": "123",
                "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
                "line_of_business": "Retail", "registered_address": "1 Fixture St",
                "zip_code": "1100", "phone": "09170000000", "email": "fixture@example.com",
                "default_form_type": "1701MS", "taxpayer_type": "Individual"
            }))
            .unwrap();
        let mut draft = Form1701MsDraft::new_from_profile(&profile, 2025);
        draft.civil_status = Form1701MsCivilStatus::Married;
        draft.filing = Form1701MsFiling::Jointly;
        draft.spouse_info = Form1701MsSpouseInfo {
            tin: "12345678800000".to_string(),
            rdo_code: "039".to_string(),
            name: "Sample Dummy Spouse".to_string(),
            email: "spouse@example.com".to_string(),
            contact_number: "09170000001".to_string(),
        };
        draft.taxpayer = Form1701MsColumn {
            source: Form1701MsSource::Mixed,
            tax_option: Form1701MsTaxOption::Graduated,
            deduction: Form1701MsDeduction::Osd,
            gross_compensation: 900_000.0,
            non_taxable_compensation: 90_000.0,
            sales: 1_500_000.0,
            sales_returns: 10_000.0,
            quarterly_payments: 50_000.0,
            cwt_2316: 80_000.0,
            ..Form1701MsColumn::default()
        };
        draft.spouse = Form1701MsColumn {
            source: Form1701MsSource::Compensation,
            gross_compensation: 600_000.0,
            non_taxable_compensation: 90_000.0,
            cwt_2316: 44_500.0,
            ..Form1701MsColumn::default()
        };
        draft.perjury_agreed = true;
        draft.recompute();
        let fields = draft.to_print_field_map();
        let html = filled_document("1701ms-2024", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c4"), "12");
        assert_eq!(comb_text(&html, "p1c5"), "2025");
        assert_eq!(comb_text(&html, "p1c23"), "123-456-788-00000");
        assert_eq!(comb_text(&html, "p1c29"), "SAMPLE DUMMY TAXPAYER");
        assert_eq!(comb_text(&html, "p1c30"), "SAMPLE DUMMY SPOUSE");
        assert_eq!(comb_text(&html, "p1c85"), "II013");
        assert_eq!(comb_text(&html, "p1c90"), "II011");
        // Whole-peso 9-slot combs, right-aligned digits without separators.
        assert_eq!(named_values(&html, "p2c13").len(), 9);
        assert_eq!(comb_text(&html, "p2c13"), "   900000");
        assert_eq!(comb_text(&html, "p2c14"), "   600000");
        assert_eq!(comb_text(&html, "p2c27"), "  1500000");
        assert_eq!(comb_text(&html, "p2c126"), "    80000");
        let aggregate = format!("{:>9}", draft.aggregate_amount_payable as i64);
        assert_eq!(comb_text(&html, "p1c141"), aggregate);
        assert_eq!(comb_text(&html, "p2c5"), "123456788");
        assert_eq!(comb_text(&html, "p2c6"), "SAMPLE DUMMY TAXPAYER");
        for x in [
            "p1c9", "p1c11", "p1c15", "p1c44", "p1c46", "p1c52", "p1c81", "p1c153",
        ] {
            assert_eq!(named_values(&html, x), vec!["X".to_string()], "{x}");
        }
        for blank in ["p1c8", "p1c10", "p1c43", "p1c79", "p1c144"] {
            assert_eq!(named_values(&html, blank), vec!["".to_string()], "{blank}");
        }
        assert!(!html.contains("name=\"frm1701MS:"));
        // A separate return prints no spouse column.
        draft.filing = Form1701MsFiling::Separately;
        draft.recompute();
        let html = filled_document("1701ms-2024", &draft.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c24"), "");
        assert_eq!(comb_text(&html, "p2c14"), "");
    }

    #[test]
    fn filled_document_1702ex_fills_identity_whole_pesos_schedules_and_boxes() {
        use bir_core::forms::form_1702ex::{
            Form1702ExAtc, Form1702ExDeduction, Form1702ExDraft, Form1702ExOverpayment,
            Form1702ExRow, Form1702ExSpecialRow,
        };
        let profile: bir_core::profile::TaxpayerProfile =
            serde_json::from_value(serde_json::json!({
                "id": null, "full_name": "Exempt Fixture Foundation Inc", "tin": {"segment1": "123",
                "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
                "line_of_business": "Charity", "registered_address": "1 Fixture St",
                "zip_code": "1100", "phone": "09170000000", "email": "fixture@example.com",
                "default_form_type": "1702EX", "taxpayer_type": "Corporation"
            }))
            .unwrap();
        let mut draft = Form1702ExDraft::new_from_profile(&profile, 2025);
        draft.atc = Form1702ExAtc::IC011;
        draft.deduction = Form1702ExDeduction::Itemized;
        draft.date_of_incorporation = "03/01/2010".to_string();
        draft.sales = 1_234_567.5;
        draft.ordinary_items[1] = 10_000.0;
        draft.other_deductions = vec![Form1702ExRow {
            description: "Sample expense".to_string(),
            amount: 5_000.0,
        }];
        draft.special_rows = vec![Form1702ExSpecialRow {
            description: "Sample deduction".to_string(),
            legal_basis: "RA 0000".to_string(),
            amount: 15_000.0,
        }];
        draft.cwt_q4 = 1_000.0;
        draft.overpayment = Form1702ExOverpayment::Refund;
        draft.recompute();
        let fields = draft.to_print_field_map();
        let cells = writer_cells("1702ex-2018").unwrap();
        for key in cells
            .joins
            .keys()
            .chain(cells.xbox_joins.iter().map(|join| &join.writer_key))
        {
            assert!(fields.contains_key(key), "print map lacks {key}");
        }
        let html = filled_document("1702ex-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c25"), "123");
        assert_eq!(comb_text(&html, "p1c33"), "039");
        assert_eq!(comb_text(&html, "p2c5"), "123456788");
        assert_eq!(comb_text(&html, "p3c5"), "123456788");
        assert!(comb_text(&html, "p1c36").starts_with("EXEMPT FIXTURE FOUNDATION INC"));
        // 29 characters past the page-2 header's 24 boxes: the whole name
        // as plain type over the boxes (was cut at 24); at the boxes' 9 pt
        // it fits, so it is not shrunk.
        assert_eq!(comb_text(&html, "p2c6"), "");
        let (header, size) = printed_cell(&html, "p2c6");
        assert_eq!(header, "EXEMPT FIXTURE FOUNDATION INC");
        assert_eq!(size, Some(9.0));
        assert_eq!(comb_text(&html, "p1c50"), "2010");
        assert_eq!(named_values(&html, "p1c19"), vec!["1".to_string()]);
        assert_eq!(named_values(&html, "p1c20"), vec!["2".to_string()]);
        assert_eq!(named_values(&html, "p1c22"), vec!["5".to_string()]);
        // Whole pesos, right-aligned in the 12-slot comb.
        assert_eq!(comb_text(&html, "p2c9"), "     1234568");
        assert_eq!(comb_text(&html, "p3c12"), "       10000");
        assert_eq!(comb_text(&html, "p3c52"), "SAMPLE EXPENSE");
        assert_eq!(comb_text(&html, "p3c54"), "        5000");
        assert_eq!(comb_text(&html, "p3c58"), "");
        // Past its 15 boxes the description prints whole (was cut at 15).
        assert_eq!(comb_text(&html, "p3c81"), "");
        assert_eq!(printed_cell(&html, "p3c81").0, "SAMPLE DEDUCTION");
        assert_eq!(comb_text(&html, "p3c87"), "");
        assert_eq!(comb_text(&html, "p2c47"), "        1000");
        assert_eq!(comb_text(&html, "p1c83"), "      (1000)");
        for checked in ["p1c7", "p1c13", "p1c15", "p1c11", "p1c56", "p1c86"] {
            assert_eq!(
                named_values(&html, checked),
                vec!["X".to_string()],
                "{checked}"
            );
        }
        for unchecked in ["p1c8", "p1c12", "p1c18", "p1c57", "p1c87", "p1c88"] {
            assert_eq!(
                named_values(&html, unchecked),
                vec!["".to_string()],
                "{unchecked}"
            );
        }
        // Items 18 and 41 (freeze preprints 000), signatory and Part III stay blank.
        assert_eq!(comb_text(&html, "p1c75"), "");
        assert_eq!(comb_text(&html, "p2c38"), "");
        assert_eq!(comb_text(&html, "p1c116"), "");
        assert!(!html.contains("name=\"frm1702EX:"));
    }

    #[test]
    fn writer_cells_target_catalog_cell_ids_not_stamps() {
        // Sheets that print amounts in one text cell, not peso + cent combs, say false.
        // 2553 (1999) prints each amount in one text cell and 1702Q (2018) in
        // one 12-slot comb, not peso + cent combs.
        for (slug, split_money) in [
            ("1601c-2018", true),
            ("2551q-2018", true),
            ("2553-1999", false),
            ("2552-2018", true),
            ("2550m-2007", false),
            ("2200m-2018", true),
            ("1606-2018", true),
            ("1600-vt-2018", true),
            ("1600-pt-2018", true),
            ("1600wp-2010", false),
            ("1702q-2018", false),
            // 1700 (2018) prints whole pesos in one comb per column.
            ("1700-2018", false),
            // 1701A (2018) prints whole pesos in one 8-slot comb per column.
            ("1701a-2018", false),
            ("1701ms-2024", false),
            ("1702ex-2018", false),
            // 1604F/1604E print amounts in single text cells; 1604C splits Item 12.
            ("1604f-2018", false),
            ("1604c-2018", true),
            ("1604e-2018", false),
        ] {
            let html = bundle(slug).unwrap().html;
            let cells = writer_cells(slug).unwrap();
            assert!(!cells.joins.is_empty(), "{slug}");
            assert_eq!(
                !cells.money_joins.is_empty(),
                split_money,
                "{slug} money_joins"
            );
            assert!(!cells.xbox_joins.is_empty(), "{slug} xbox_joins");
            let present: std::collections::BTreeSet<&str> =
                input_tags(html).into_iter().map(|tag| tag.name).collect();
            for (key, html_id) in cells.targets() {
                assert!(
                    present.contains(html_id) || html.contains(&format!("id=\"{html_id}\"")),
                    "{slug} {key} -> {html_id}"
                );
                assert!(
                    !key.starts_with('p'),
                    "{slug} writer_key looks like a cell id: {key}"
                );
                assert!(
                    html_id.starts_with('p'),
                    "{slug} html_id must be a cell id: {html_id}"
                );
                assert!(!present.contains(key), "{slug} {key} is stamped");
            }
            for join in &cells.money_joins {
                for pair in &join.cells {
                    assert_ne!(Some(&pair.peso_html_id), pair.cent_html_id.as_ref());
                }
            }
        }
    }

    /// Every input of plain (`data-field-kind="text"`) cell `cell` of
    /// `html`, in order: its whole room (pt), its class list and its tag.
    fn plain_inputs<'a>(html: &'a str, cell: &str) -> Vec<(f64, &'a str, &'a str)> {
        let Some(width) = plain_cell_width(html, cell) else {
            return Vec::new();
        };
        let open = html.find(&format!("<div id=\"{cell}\"")).unwrap();
        let inner = &html[open..open + html[open..].find("</div>").unwrap()];
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(at) = inner[from..].find("<input") {
            let at = from + at;
            let end = at + inner[at..].find('>').unwrap();
            let tag = &inner[at..end];
            from = end;
            if tag.contains(" data-overflow-of=\"") {
                continue;
            }
            if let Some(classes) = attr(tag, "class") {
                out.push((input_room(width, tag), classes, tag));
            }
        }
        out
    }

    /// Whether a plain overflow line `text` at `size` pt sets within a
    /// comb `width` pt wide on sheet `slug`: measured in the sheet's face
    /// with its word- and letter-spacing, after the line's 1 pt padding.
    fn line_fits(slug: &str, text: &str, size: f64, width: f64) -> bool {
        let sheet = SheetType::of(bundle(slug).unwrap().css);
        1.0 + sheet.width(sheet.letter, text, size) <= width
    }

    /// Every frozen sheet slug with a writer-cells file.
    fn sheet_slugs() -> Vec<String> {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/html-frozen");
        let mut slugs: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                entry
                    .path()
                    .join("writer-cells.json")
                    .exists()
                    .then(|| entry.file_name().to_string_lossy().into_owned())
            })
            .collect();
        slugs.sort();
        slugs
    }

    /// Upper-case text of exactly `len` characters: an address's words
    /// with single spaces, never ending in a space.
    fn words_of(len: usize) -> String {
        const WORDS: [&str; 12] = [
            "UNIT",
            "1203",
            "TWELFTH",
            "FLOOR",
            "CURVE",
            "BUILDING",
            "BONIFACIO",
            "GLOBAL",
            "CITY",
            "TAGUIG",
            "METRO",
            "MANILA",
        ];
        let mut text = String::new();
        for word in WORDS.iter().cycle() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(word);
            if text.len() >= len {
                break;
            }
        }
        text.truncate(len);
        if text.ends_with(' ') {
            text.pop();
            text.push('X');
        }
        assert_eq!(text.chars().count(), len);
        text
    }

    /// An independent reading of a printed cell, straight from the
    /// document's tags: the slot inputs' values, and the plain overlay
    /// (`data-overflow-of`) with its type size.
    fn read_cell(document: &str, cell: &str) -> (String, Option<(String, f64)>) {
        let boxes: String = named_values(document, cell).concat();
        let needle = format!("data-overflow-of=\"{cell}\"");
        let overlay = document.find(&needle).map(|at| {
            let start = document[..at].rfind("<input").unwrap();
            let tag = &document[start..at];
            let value = tag
                .split("value=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap();
            let size: f64 = tag
                .split("font-size:")
                .nth(1)
                .unwrap()
                .split("pt")
                .next()
                .unwrap()
                .parse()
                .unwrap();
            (value.replace("&amp;", "&"), size)
        });
        (boxes, overlay)
    }

    /// Rules page question 4 (decided 2026-10-17) on every sheet's item
    /// printed over stacked comb lines: text that fits the boxes prints
    /// exactly as before; past them, the whole text prints as plain type
    /// across every line, broken between words; too long for that at the
    /// boxes' 9 pt, it shrinks until it fits. Nothing is lost.
    #[test]
    fn a_stacked_item_past_its_boxes_prints_whole_as_plain_type_on_every_sheet() {
        let mut items = 0;
        for slug in sheet_slugs() {
            let loaded = bundle(&slug).unwrap();
            let cells = writer_cells(&slug).unwrap();
            for group in line_groups(loaded.html, &cells) {
                items += 1;
                let slots: Vec<usize> = group.iter().map(|l| l.slots).collect();
                let capacity: usize = slots.iter().sum();
                let fill = |values: &[String]| {
                    let mut fields = BTreeMap::new();
                    for (line, value) in group.iter().zip(values) {
                        fields.insert(line.key.clone(), value.clone());
                    }
                    let document = filled_document(&slug, &fields).unwrap();
                    let before = fill_by_name_with_cells(
                        loaded.html,
                        &fields,
                        &cells.joins,
                        &BTreeMap::new(),
                    );
                    (document, before)
                };
                // Fits: each line within its boxes prints exactly as the
                // fill without this rule prints it.
                let fitting: Vec<String> = slots.iter().map(|n| words_of((*n).min(12))).collect();
                let (document, before) = fill(&fitting);
                assert!(!document.contains("data-overflow-of"), "{slug}");
                for (line, value) in group.iter().zip(&fitting) {
                    assert_eq!(
                        read_cell(&document, &line.cell),
                        (value.clone(), None),
                        "{slug}"
                    );
                    assert_eq!(
                        read_cell(&before, &line.cell),
                        (value.clone(), None),
                        "{slug}"
                    );
                }
                // Past the boxes: plain type, every line used, nothing lost,
                // each line inside its cell. Fed whole into the first line,
                // and as a print map cuts it (the first line at its boxes).
                let plain_at_9: usize = group
                    .iter()
                    .map(|l| ((l.width - 2.0) / (9.0 * 0.58)) as usize)
                    .sum();
                for (len, shrunk) in [
                    (90.max(capacity + 10), false),
                    (200.max(plain_at_9 + 20), true),
                ] {
                    let text = words_of(len);
                    let cut = text[..=slots[0]].rfind(' ').unwrap();
                    let mut first_cut = vec![text[..cut].to_string(), text[cut + 1..].to_string()];
                    first_cut.resize(group.len(), String::new());
                    let mut whole = vec![text.clone()];
                    whole.resize(group.len(), String::new());
                    for values in [whole, first_cut] {
                        let (document, _) = fill(&values);
                        let mut parts = Vec::new();
                        let mut sizes = Vec::new();
                        for line in &group {
                            let (boxes, overlay) = read_cell(&document, &line.cell);
                            assert_eq!(boxes.trim(), "", "{slug} {}: boxes empty", line.cell);
                            let (part, size) = overlay
                                .unwrap_or_else(|| panic!("{slug} {}: a plain line", line.cell));
                            assert!(!part.is_empty(), "{slug} {}: every line used", line.cell);
                            assert!(
                                line_fits(&slug, &part, size, line.width),
                                "{slug} {}: {part:?} at {size}pt fits {}pt",
                                line.cell,
                                line.width
                            );
                            assert_eq!(
                                printed_cell(&document, &line.cell),
                                (part.clone(), Some(size))
                            );
                            parts.push(part);
                            sizes.push(size);
                        }
                        assert_eq!(parts.join(" "), text, "{slug}: the whole text, in order");
                        assert!(
                            sizes.iter().all(|s| (s - sizes[0]).abs() < 1e-9),
                            "{slug}: one size {sizes:?}"
                        );
                        if shrunk {
                            assert!(
                                sizes[0] < 9.0,
                                "{slug}: {len} characters shrink ({})",
                                sizes[0]
                            );
                        } else {
                            assert_eq!(
                                sizes[0], 9.0,
                                "{slug}: {len} characters at the boxes' size"
                            );
                        }
                    }
                }
            }
        }
        assert!(items >= 40, "{items} stacked items");
    }

    /// A single-line name past its boxes prints whole as plain type across
    /// them, shrunk to fit; within them it prints in the boxes as before.
    #[test]
    fn a_single_line_name_past_its_boxes_prints_whole_on_every_sheet() {
        let mut names = 0;
        for slug in sheet_slugs() {
            let loaded = bundle(&slug).unwrap();
            let cells = writer_cells(&slug).unwrap();
            let stacked: BTreeSet<String> = line_groups(loaded.html, &cells)
                .into_iter()
                .flatten()
                .map(|line| line.cell)
                .collect();
            for (key, ids) in &cells.joins {
                let cell = &ids[0];
                let caption = cells.captions.get(cell).map(|c| c.to_ascii_lowercase());
                if !caption.is_some_and(|c| c.contains("name") && !c.contains("tin"))
                    || stacked.contains(cell)
                {
                    continue;
                }
                let Some((slots, width)) = comb_cell(loaded.html, cell) else {
                    continue;
                };
                names += 1;
                let print = |value: &str| {
                    let fields = BTreeMap::from([(key.clone(), value.to_string())]);
                    filled_document(&slug, &fields).unwrap()
                };
                let short = words_of(slots.min(14));
                let document = print(&short);
                assert_eq!(
                    read_cell(&document, cell),
                    (short.clone(), None),
                    "{slug} {key}"
                );
                let plain_at_9 = ((width - 2.0) / (9.0 * 0.58)) as usize;
                for (len, shrunk) in [(slots + 5, false), (200.max(plain_at_9 + 20), true)] {
                    let text = words_of(len);
                    let document = print(&text);
                    let (boxes, overlay) = read_cell(&document, cell);
                    assert_eq!(boxes.trim(), "", "{slug} {key}");
                    let (part, size) = overlay.unwrap_or_else(|| panic!("{slug} {key}: plain"));
                    assert_eq!(part, text, "{slug} {key}: whole");
                    assert!(
                        line_fits(&slug, &part, size, width),
                        "{slug} {key}: {len} characters at {size}pt fit {width}pt"
                    );
                    if shrunk {
                        assert!(size < 9.0, "{slug} {key}: {size}");
                    }
                }
            }
        }
        assert!(names >= 40, "{names} single-line names");
    }

    /// A fitting two-line address prints byte for byte as before the
    /// overflow rule: 0619E's whole address item (its two comb lines, from
    /// `<div id="p1c41"` to the next cell) equals what the fill printed at
    /// `pr0b-push`, captured then into `fixtures/0619e-fitting-address.html`.
    #[test]
    fn a_fitting_address_prints_byte_for_byte_as_before() {
        let fields = BTreeMap::from([
            (
                "frm0619E:txtAddress".to_string(),
                "1 Example Street".to_string(),
            ),
            (
                "frm0619E:txtAddress2".to_string(),
                "Quezon City".to_string(),
            ),
        ]);
        let document = filled_document("0619e-2018", &fields).unwrap();
        let start = document.find("<div id=\"p1c41\"").unwrap();
        let end = document.find("<div id=\"p1c43\"").unwrap();
        assert_eq!(
            &document[start..end],
            include_str!("fixtures/0619e-fitting-address.html"),
            "the address item as it printed before"
        );
        assert!(!document.contains("data-overflow-of"));
    }

    /// One long address, pinned: 200 characters over 0619E's two address
    /// lines print as plain type in one size; far longer, the type keeps
    /// shrinking so the text still fits its cells (no floor). The size was
    /// 8.61 pt by the per-character estimate, 8.56 pt measured to the
    /// nearest 0.01 pt, and 8.55 pt now that the measured size is rounded
    /// down to 0.01 pt (rounding never adds width; [`floor_size`]).
    #[test]
    fn a_long_address_prints_at_a_pinned_size() {
        let text = words_of(200);
        let fields = BTreeMap::from([("frm0619E:txtAddress".to_string(), text.clone())]);
        let document = filled_document("0619e-2018", &fields).unwrap();
        let (first, size) = printed_cell(&document, "p1c41");
        let (second, again) = printed_cell(&document, "p1c42");
        assert_eq!(format!("{first} {second}"), text);
        assert_eq!(size, again);
        assert_eq!(size, Some(8.55));
        assert!(document.contains(
            "font-size:8.55pt;text-align:left;padding-left:1pt;z-index:1\" data-overflow-of=\"p1c41\""
        ));
        // 600 characters: under 4 pt, and still inside each cell.
        let text = words_of(600);
        let fields = BTreeMap::from([("frm0619E:txtAddress".to_string(), text.clone())]);
        let document = filled_document("0619e-2018", &fields).unwrap();
        let mut parts = Vec::new();
        for cell in ["p1c41", "p1c42"] {
            let (part, size) = printed_cell(&document, cell);
            let size = size.unwrap();
            let width = comb_cell(&document, cell).unwrap().1;
            assert!(size < 4.0, "{cell}: {size}");
            assert!(
                line_fits("0619e-2018", &part, size, width),
                "{cell}: {} characters at {size}pt fit {width}pt",
                part.chars().count()
            );
            parts.push(part);
        }
        assert_eq!(parts.join(" "), text, "nothing is lost");
        let sheet = SheetType::plain();
        assert_eq!(
            sheet.fit(sheet.letter, 998.0, "UNIT 1", PLAIN_MAX_PT),
            PLAIN_MAX_PT
        );
    }

    /// The bug as found: a 200-character 0619E registered address
    /// overflowing item 10. The inputs are synthetic: the address typed
    /// into the two writer keys as 111 characters (the trailing space
    /// included) and 89, the shape the review's screenshot showed; the print
    /// rejoins the lines either way. (BIR's character-127 cut of the escaped
    /// address does not apply: this address's cut lands inside a %20, so
    /// form_0619e_official refuses it.) By the old estimate, line 2 printed
    /// at 8.61 pt and measured 453.1 pt in Arimo, past its 446.5 pt cell,
    /// cut at the 10A ZIP Code caption. Measured, each line's text at its
    /// size fits its cell less the input's 1 pt left padding, so line 2 ends
    /// before the caption; the ZIP prints in its own boxes and the whole
    /// address prints.
    #[test]
    fn a_200_character_0619e_address_keeps_line_2_off_the_zip_box() {
        let first = "UNIT 4512 TOWER B THE RESIDENCES AT GREENBELT 123 ESPERANZA STREET COR. \
                     MAKATI AVENUE BRGY. SAN LORENZO MAKATI";
        let second = "CITY METRO MANILA PHILIPPINES NEAR AYALA TRIANGLE GARDENS ACROSS \
                      GREENBELT 5 MALL GATE 12";
        assert_eq!(first.chars().count() + 1 + second.chars().count(), 200);
        // The estimate's size, and what it measured: past the cell.
        assert!((fit_size(446.52, 89) - 8.61).abs() < 0.005);
        assert!(arimo_ems(second) * 8.61 > 446.52);
        let fields = BTreeMap::from([
            ("frm0619E:txtAddress".to_string(), format!("{first} ")),
            ("frm0619E:txtAddress2".to_string(), second.to_string()),
            ("frm0619E:txtZipCode".to_string(), "1226".to_string()),
        ]);
        let document = filled_document("0619e-2018", &fields).unwrap();
        let mut parts = Vec::new();
        for cell in ["p1c41", "p1c42"] {
            let (part, size) = printed_cell(&document, cell);
            let size = size.unwrap_or_else(|| panic!("{cell}: plain type"));
            let width = comb_cell(&document, cell).unwrap().1;
            assert!(
                1.0 + arimo_ems(&part) * size <= width,
                "{cell}: {part:?} at {size}pt measures {}pt in {width}pt",
                arimo_ems(&part) * size
            );
            parts.push(part);
        }
        assert_eq!(parts.join(" "), format!("{first} {second}"));
        // Line 2's cell ends where the 10A ZIP Code caption starts.
        assert!(document.contains("<div id=\"p1c42\" class=\"c f\""));
        assert!(document.contains("left:17.62pt;top:240.62pt;width:446.52pt"));
        assert!(document.contains("<div id=\"p1c43\" class=\"c\" data-cell-kind=\"label\" data-row=\"11\" data-col=\"34\" style=\"left:464.14pt"));
        assert_eq!(printed_cell(&document, "p1c44"), ("1226".to_string(), None));
    }

    /// Per cell, on every sheet (P4-7): every text join filled with a
    /// 200-character text, then with a 90-character one, then the review's
    /// sweep probe (the 200-character address below in each join whose
    /// printed caption contains "address", "name", "email", "employer" or
    /// "business", in any case: "6A Seller name", "Donee A name"). Each
    /// plain overflow line over a comb, measured in the sheet's face with
    /// its word- and letter-spacing at its printed size, sets within its
    /// cell after the line's 1 pt padding; each plain box's value, at the
    /// size it prints (its class's, or the smaller inline size a value
    /// longer than the box takes), sets within the box less its input's
    /// inset. The inputs are synthetic. On 0619E and 0619F the address
    /// typed across its two keys (111 + 89 characters) prints the very same
    /// document as typed whole, so it is checked once.
    #[test]
    fn every_sheet_fits_every_plain_line_in_its_cell() {
        let first = "UNIT 4512 TOWER B THE RESIDENCES AT GREENBELT 123 ESPERANZA STREET COR. \
                     MAKATI AVENUE BRGY. SAN LORENZO MAKATI";
        let second = "CITY METRO MANILA PHILIPPINES NEAR AYALA TRIANGLE GARDENS ACROSS \
                      GREENBELT 5 MALL GATE 12";
        let whole = format!("{first} {second}");
        for (slug, prefix) in [("0619e-2018", "frm0619E"), ("0619f-2018", "frm0619F")] {
            let address = |one: String, two: &str| {
                BTreeMap::from([
                    (format!("{prefix}:txtAddress"), one),
                    (format!("{prefix}:txtAddress2"), two.to_string()),
                    (format!("{prefix}:txtZipCode"), "1226".to_string()),
                ])
            };
            assert_eq!(
                filled_document(slug, &address(format!("{first} "), second)).unwrap(),
                filled_document(slug, &address(whole.clone(), "")).unwrap(),
                "{slug}: typed across the two lines, it prints as typed whole"
            );
        }
        let (mut lines, mut boxes, mut swept) = (0, 0, 0);
        for slug in sheet_slugs() {
            let loaded = bundle(&slug).unwrap();
            let sheet = SheetType::of(loaded.css);
            let cells = writer_cells(&slug).unwrap();
            let every = |text: &str| -> BTreeMap<String, String> {
                cells
                    .joins
                    .keys()
                    .map(|key| (key.clone(), text.to_string()))
                    .collect()
            };
            let sweep: BTreeMap<String, String> = cells
                .joins
                .iter()
                .filter(|(_, ids)| {
                    ids.first()
                        .and_then(|id| cells.captions.get(id))
                        .is_some_and(|caption| {
                            let caption = caption.to_lowercase();
                            ["address", "name", "email", "employer", "business"]
                                .iter()
                                .any(|word| caption.contains(word))
                        })
                })
                .map(|(key, _)| (key.clone(), whole.clone()))
                .collect();
            swept += sweep.len();
            for (case, fields) in [
                ("200", every(&words_of(200))),
                ("90", every(&words_of(90))),
                ("sweep", sweep),
            ] {
                let document = filled_document(&slug, &fields).unwrap();
                for (cell, (text, size)) in printed_cells(&document) {
                    let Some(size) = size else {
                        continue;
                    };
                    // Over a comb, or over a split cell's boxes.
                    let width = comb_cell(&document, &cell)
                        .map(|(_, width)| width)
                        .or_else(|| plain_cell_width(&document, &cell))
                        .unwrap();
                    let measured = sheet.width(sheet.letter, &text, size);
                    assert!(
                        1.0 + measured <= width,
                        "{slug} {case} {cell}: {text:?} at {size}pt measures {measured}pt in {width}pt"
                    );
                    lines += 1;
                }
                for ids in cells.joins.values() {
                    for cell in ids {
                        // Every input of the cell, each by its own room.
                        for (room, classes, input) in plain_inputs(&document, cell) {
                            let text = attr(input, "value").unwrap_or("").replace("&amp;", "&");
                            if text.is_empty() {
                                continue;
                            }
                            let style = attr(input, "style").unwrap_or("");
                            let shrunk: Option<f64> = style
                                .find("font-size:")
                                .and_then(|at| style[at + "font-size:".len()..].split("pt").next())
                                .and_then(|size| size.parse().ok());
                            let letter = sheet.letter_for(classes);
                            if let Some(size) = shrunk {
                                // Shrunk: it sets within the room less 1 pt;
                                // a room under 2 pt, within the room.
                                let measured = sheet.width(letter, &text, size);
                                let bound = if room >= 2.0 { room - 1.0 } else { room };
                                assert!(
                                    measured <= bound,
                                    "{slug} {case} {cell}: {text:?} shrunk to {size}pt measures {measured}pt in a {room}pt box"
                                );
                            } else {
                                // Unshrunk: it fit at its class size.
                                let size = sheet
                                    .size_for(classes)
                                    .unwrap_or_else(|| panic!("{slug} {cell}: a type size"));
                                let measured = sheet.width(letter, &text, size);
                                assert!(
                                    measured <= room + FIT_EPSILON_PT,
                                    "{slug} {case} {cell}: {text:?} unshrunk at {size}pt measures {measured}pt in a {room}pt box"
                                );
                            }
                            boxes += 1;
                        }
                    }
                }
            }
        }
        assert!(lines > 500, "{lines} plain lines checked");
        assert!(boxes > 500, "{boxes} plain boxes checked");
        assert!(swept > 100, "{swept} joins swept");
    }

    /// Every sheet filled three ways: each text join's key holding a
    /// 200-character text, a 90-character one and a 12-character one (past
    /// the boxes of most combs, past a line, and fitting), hashed together.
    fn overflow_print_hash(slug: &str) -> String {
        use sha2::{Digest, Sha256};
        let cells = writer_cells(slug).unwrap();
        let mut hasher = Sha256::new();
        for len in [200, 90, 12] {
            let fields: BTreeMap<String, String> = cells
                .joins
                .keys()
                .map(|key| (key.clone(), words_of(len)))
                .collect();
            hasher.update(filled_document(slug, &fields).unwrap().as_bytes());
            hasher.update([0u8]);
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// The sheets whose long-text print changed when every sheet's overflow
    /// type was measured (P4-7), against `fixtures/overflow-goldens-estimate.json`
    /// (what every sheet printed at f7b7f475, by the 0.58 em estimate, written
    /// once and never rewritten). 0619E and 0619F, measured first, are among
    /// them. Every sheet changed: the probe fills every text join with 200
    /// characters, so each sheet has some plain line to size.
    const CHANGED_BY_MEASURING: &[&str] = &[
        "0605-1999",
        "0619e-2018",
        "0619f-2018",
        "1600-pt-2018",
        "1600-vt-2018",
        "1600wp-2010",
        "1601-fq-2020",
        "1601c-2018",
        "1601eq-2019",
        "1602q-2019",
        "1603q-2018",
        "1604c-2018",
        "1604e-2018",
        "1604f-2018",
        "1606-2018",
        "1700-2018",
        "1701-2018",
        "1701a-2018",
        "1701ms-2024",
        "1701q-2018",
        "1702ex-2018",
        "1702mx-2018c",
        "1702q-2018",
        "1702rt-2018c",
        "1706-2018",
        "1707-2021",
        "1707a-2021",
        "1800-2018",
        "1801-2018",
        "2000-dst-2018",
        "2000-ot-2018",
        "2200a-2020",
        "2200an-2018",
        "2200c-2018",
        "2200m-2018",
        "2200p-2020",
        "2200s-2018",
        "2200t-2022",
        "2550m-2007",
        "2550q-2024",
        "2551q-2018",
        "2552-2018",
        "2553-1999",
    ];

    /// Goldens: every sheet's long-text print pinned exactly
    /// (`fixtures/overflow-goldens.json`, `BIR_PRINT_BLESS_OVERFLOW=1`), and
    /// the sheets whose print measuring changed against the estimate's
    /// print, listed exactly ([`CHANGED_BY_MEASURING`]); the rest print
    /// byte for byte as they did.
    #[test]
    fn long_text_prints_as_pinned_on_every_sheet() {
        let now: BTreeMap<String, String> = sheet_slugs()
            .iter()
            .map(|slug| (slug.clone(), overflow_print_hash(slug)))
            .collect();
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/fixtures/overflow-goldens.json"
        );
        if std::env::var_os("BIR_PRINT_BLESS_OVERFLOW").is_some() {
            std::fs::write(path, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
            return;
        }
        let golden: BTreeMap<String, String> =
            serde_json::from_str(include_str!("fixtures/overflow-goldens.json")).unwrap();
        assert_eq!(golden, now, "every sheet prints long text as pinned");
        let estimate: BTreeMap<String, String> =
            serde_json::from_str(include_str!("fixtures/overflow-goldens-estimate.json")).unwrap();
        assert_eq!(
            estimate.keys().collect::<Vec<_>>(),
            now.keys().collect::<Vec<_>>()
        );
        let changed: Vec<&str> = now
            .iter()
            .filter(|(slug, hash)| estimate[*slug] != **hash)
            .map(|(slug, _)| slug.as_str())
            .collect();
        assert_eq!(
            changed, CHANGED_BY_MEASURING,
            "the sheets measuring changed"
        );
    }

    /// Each sheet's type is read from its form.css: the 11 sheets that set
    /// `.fi` in Tinos measure in Tinos, the rest in Arimo, and the `.fi`
    /// word-spacing is each sheet's own (fixed points: it does not scale
    /// with the size); no `.fi` sets a letter-spacing, the size classes do.
    #[test]
    fn each_sheet_measures_in_its_own_face_and_spacing() {
        const TINOS: [&str; 11] = [
            "0605-1999",
            "1601c-2018",
            "1602q-2019",
            "1603q-2018",
            "1604e-2018",
            "1604f-2018",
            "2200c-2018",
            "2200p-2020",
            "2200t-2022",
            "2550m-2007",
            "2553-1999",
        ];
        for slug in sheet_slugs() {
            let sheet = SheetType::of(bundle(&slug).unwrap().css);
            let face = if TINOS.contains(&slug.as_str()) {
                Face::Tinos
            } else {
                Face::Arimo
            };
            assert_eq!(sheet.face, face, "{slug}");
            assert_eq!(sheet.word.em, 0.0, "{slug}");
            assert_eq!(sheet.letter, Spacing::default(), "{slug}");
        }
        for (slug, pt) in [
            ("1604f-2018", 0.3238),
            ("2200t-2022", 0.4092),
            ("1600-pt-2018", 0.2778),
            ("1604e-2018", 0.1518),
            ("1600wp-2010", 0.0546),
            ("0605-1999", -0.1071),
            ("0619e-2018", 0.0),
        ] {
            assert_eq!(
                SheetType::of(bundle(slug).unwrap().css).word.pt,
                pt,
                "{slug}"
            );
        }
        // A size class's letter-spacing (0605's .fh10) and size.
        let sheet = SheetType::of(bundle("0605-1999").unwrap().css);
        assert_eq!(
            sheet.letter_for("fi fh10"),
            Spacing {
                pt: 0.0102,
                em: 0.0
            }
        );
        assert_eq!(sheet.size_for("fi fh10"), Some(9.47));
        // The measure: advances, plus the word-spacing on each space, plus
        // the letter-spacing on each character.
        let tinos = SheetType::of(bundle("1604f-2018").unwrap().css);
        let text = "AB CD";
        let expected = Face::Tinos.ems(text) * 9.0 + 0.3238;
        assert!((tinos.width(tinos.letter, text, 9.0) - expected).abs() < 1e-9);
        let letter = Spacing { pt: 0.5, em: 0.1 };
        let expected = Face::Tinos.ems(text) * 9.0 + 0.3238 + 5.0 * (0.5 + 0.9);
        assert!((tinos.width(letter, text, 9.0) - expected).abs() < 1e-9);
        // The review's word-spacing regression texts (narrow letters, many
        // spaces), on every sheet at each width: measured with the sheet's
        // spacing, each line sets within its comb after its 1 pt padding.
        let texts = [
            "ILIGAN LILIO IMPERIAL LIMITLESS INDIVISIBLE LIMITING INITIAL ILLUMINATED ILIGAN CITY",
            "LILIAN ILIGAN LILI INI LIMIT INDIA LIMITED PILI LILIO INC",
            "IVIRIS IN ILIAN LIMIT LILIA SILI TIPIL INITIAL LIMITING INC",
            "UNIT 1011 TILL INTERNATIONAL ILLINOIS MILL ISLAND LIMITED ITI BILIT",
            "DELA CRUZ JUAN CARLO SANTOS ILAGAN MIRANDA VILLANUEVA IMPORT INC",
        ];
        for slug in sheet_slugs() {
            let sheet = SheetType::of(bundle(&slug).unwrap().css);
            for width in [100.0, 150.0, 200.0, 300.0, 446.52] {
                for text in texts {
                    let size = fit_line(&sheet, width, text);
                    let set = sheet.width(sheet.letter, text, size);
                    assert!(
                        1.0 + set <= width,
                        "{slug} {width}pt: {text} sets {set}pt at {size}pt"
                    );
                }
            }
        }
        // The fit is the largest size that sets within the room, rounded down.
        let size = tinos.fit(letter, 100.0, text, 50.0);
        assert!(tinos.width(letter, text, size) <= 100.0);
        assert!(tinos.width(letter, text, size + 0.01) > 100.0);
    }

    /// A character the face does not map is drawn from another font: it
    /// counts 1.3 em, more than CJK or an emoji takes (about 1.2 em), so the
    /// type comes out small enough; a mapped one counts its own advance.
    #[test]
    fn an_unmapped_character_counts_wide() {
        for face in [Face::Arimo, Face::Tinos] {
            for c in ['\u{0416}', '\u{4E2D}', '\u{1F600}', '\u{0150}'] {
                assert_eq!(face.ems(&c.to_string()), FALLBACK_EMS, "{face:?} {c}");
            }
            const { assert!(FALLBACK_EMS >= 1.3) };
            // Latin-1's Ñ is mapped, at its own width.
            let n = face.ems("\u{00D1}");
            assert!(n > 0.5 && n < 1.0, "{face:?} Ñ: {n}");
        }
        // A no-break space takes the word-spacing as a space does.
        let sheet = SheetType::of(bundle("1604f-2018").unwrap().css);
        let spacing =
            |text: &str| sheet.width(sheet.letter, text, 9.0) - Face::Tinos.ems(text) * 9.0;
        assert!((spacing("A\u{a0}B") - 0.3238).abs() < 1e-9);
        assert!((spacing("A B") - 0.3238).abs() < 1e-9);
        assert!(spacing("AB").abs() < 1e-9);
        assert_eq!(Face::Arimo.ems("A"), 1366.0 / 2048.0);
        assert_eq!(
            Face::Tinos.ems("A"),
            Face::Tinos.advances()[33].1 as f64 / 2048.0
        );
    }

    /// Round 6, D8: tabs and no-break spaces measure what headless Chrome
    /// draws. Each width is an `<input>`'s scrollWidth in Chrome
    /// (`--headless=new`) with the eBIRForms Arimo or Tinos face at 2048 px
    /// type, where one font unit is one pixel, with no spacing, 100 px
    /// word-spacing or 50 px letter-spacing: a tab runs to the next stop,
    /// 8 × (space + word-spacing + letter-spacing) from the line's start,
    /// or the one after when it would end less than half a space short
    /// (`AAA'`); a no-break space takes the word-spacing.
    #[test]
    fn tabs_and_no_break_spaces_measure_as_headless_chrome_draws_them() {
        let measured: [(Face, f64, f64, &str, f64); 33] = [
            (Face::Arimo, 0.0, 0.0, "A\tB", 5918.0),
            (Face::Arimo, 0.0, 0.0, "\tB", 5918.0),
            (Face::Arimo, 0.0, 0.0, "AAA\tB", 5918.0),
            (Face::Arimo, 0.0, 0.0, "AAA'\tB", 10470.0),
            (Face::Arimo, 0.0, 0.0, "AAAAAAAAAA\tB", 19574.0),
            (Face::Arimo, 0.0, 0.0, "A\t\tB", 10470.0),
            (Face::Arimo, 0.0, 0.0, "A\tB\tC", 10583.0),
            (Face::Arimo, 0.0, 0.0, "A\u{a0}B", 3301.0),
            (Face::Arimo, 0.0, 0.0, "A B", 3301.0),
            (Face::Arimo, 100.0, 0.0, "A\tB", 6718.0),
            (Face::Arimo, 100.0, 0.0, "AAA'\tB", 6718.0),
            (Face::Arimo, 100.0, 0.0, "AAAAAAAAAA\tB", 17422.0),
            (Face::Arimo, 100.0, 0.0, "A\t\tB", 12070.0),
            (Face::Arimo, 100.0, 0.0, "A\u{a0}B", 3401.0),
            (Face::Arimo, 0.0, 50.0, "A\tB", 6368.0),
            (Face::Arimo, 0.0, 50.0, "AAA'\tB", 11320.0),
            (Face::Arimo, 0.0, 50.0, "AAAAAAAAAA\tB", 16272.0),
            (Face::Arimo, 0.0, 50.0, "A\tB\tC", 11433.0),
            (Face::Arimo, 0.0, 50.0, "A\u{a0}B", 3451.0),
            (Face::Tinos, 0.0, 0.0, "A\tB", 5462.0),
            (Face::Tinos, 0.0, 0.0, "AAA\tB", 9558.0),
            (Face::Tinos, 0.0, 0.0, "AAA'\tB", 9558.0),
            (Face::Tinos, 0.0, 0.0, "A\t\tB", 9558.0),
            (Face::Tinos, 0.0, 0.0, "AAAAAAAAAA\tB", 17750.0),
            (Face::Tinos, 0.0, 0.0, "A\u{a0}B", 3357.0),
            (Face::Tinos, 100.0, 0.0, "AAA\tB", 6262.0),
            (Face::Tinos, 100.0, 0.0, "AAA'\tB", 11158.0),
            (Face::Tinos, 100.0, 0.0, "A\tB\tC", 11158.0),
            (Face::Tinos, 100.0, 0.0, "A\u{a0}B", 3457.0),
            (Face::Tinos, 0.0, 50.0, "AAA\tB", 10408.0),
            (Face::Tinos, 0.0, 50.0, "AAAAAAAAAA\tB", 19400.0),
            (Face::Tinos, 0.0, 50.0, "A\tB\tC", 10408.0),
            (Face::Tinos, 0.0, 50.0, "A\u{a0}B", 3507.0),
        ];
        for (face, word, letter, text, chrome) in measured {
            let sheet = SheetType {
                face,
                word: Spacing { pt: word, em: 0.0 },
                ..SheetType::plain()
            };
            let letter = Spacing {
                pt: letter,
                em: 0.0,
            };
            assert_eq!(
                sheet.width(letter, text, 2048.0),
                chrome,
                "{face:?} word {word} {text:?}"
            );
        }
        // A tab's most is a whole stop: 8 space advances.
        assert_eq!(Face::Arimo.ems("\t"), 8.0 * 569.0 / 2048.0);
        assert_eq!(Face::Tinos.ems("\t"), 8.0 * 512.0 / 2048.0);
    }

    /// Round 7, F1: where a tab ends is not monotone in the size, so the fit
    /// scans down from the largest size instead of bisecting. The
    /// reviewer's case on 0605's Tinos spacing (word -0.1071 pt, letter
    /// 0.0113 pt) sets at 7.83 pt in Chrome; bisecting returned 5.97 pt.
    #[test]
    fn a_tab_fit_finds_the_largest_size_that_sets() {
        let sheet = SheetType {
            face: Face::Tinos,
            word: Spacing {
                pt: -0.1071,
                em: 0.0,
            },
            ..SheetType::plain()
        };
        let letter = Spacing {
            pt: 0.0113,
            em: 0.0,
        };
        let text = "E  FFAEADF\t";
        let size = sheet.fit(letter, 44.724, text, 9.0);
        assert_eq!(size, 7.83);
        assert!(sheet.width(letter, text, size) <= 44.724);
        // No larger hundredth up to 9 pt sets.
        assert!((784..=900).all(|h| sheet.width(letter, text, h as f64 / 100.0) > 44.724));
    }

    /// Round 8: a tab fit returns the hundredth that sets as it is, not
    /// rounded down again (0.29 * 100 is 28.999..., which floor_size would
    /// print as 0.28). With no spacing the width scales with the size, so a
    /// room exactly as wide as the text at h/100 pt fits it at h/100 pt.
    #[test]
    fn a_tab_fit_returns_its_exact_hundredth() {
        let sheet = SheetType::plain();
        let letter = Spacing::default();
        let text = "\tA";
        for hundredths in 1..=900 {
            let size = hundredths as f64 / 100.0;
            let room = sheet.width(letter, text, size);
            assert_eq!(sheet.fit(letter, room, text, 9.0), size, "{hundredths}");
        }
        let room = sheet.width(letter, text, 0.29);
        assert_eq!(sheet.fit(letter, room, text, 9.0), 0.29);
    }

    /// A printed size is rounded down to 0.01 pt, never to the nearest:
    /// rounding never adds width.
    #[test]
    fn a_printed_size_rounds_down() {
        assert_eq!(floor_size(8.559), 8.55);
        assert_eq!(floor_size(8.5), 8.5);
        assert_eq!(floor_size(0.001), 0.01);
        let sheet = SheetType::plain();
        let text = words_of(200);
        let size = sheet.fit(sheet.letter, 444.52, &text, PLAIN_MAX_PT);
        assert_eq!(size, floor_size(size));
        assert!(sheet.width(sheet.letter, &text, size) <= 444.52);
    }

    /// A plain box's value longer than the box prints smaller, measured, so
    /// it stays inside (0605's 80-character registered address, p1c38,
    /// 365.51 pt, showed 61 characters); one that fits keeps its bytes.
    #[test]
    fn a_plain_box_value_longer_than_its_box_is_measured() {
        let slug = "0605-1999";
        let loaded = bundle(slug).unwrap();
        let sheet = SheetType::of(loaded.css);
        let cells = writer_cells(slug).unwrap();
        let (key, _) = cells
            .joins
            .iter()
            .find(|(_, ids)| ids.iter().any(|id| id == "p1c38"))
            .expect("a join prints in p1c38");
        let (room, classes, _) = plain_inputs(loaded.html, "p1c38")[0];
        let class_size = sheet.size_for(classes).unwrap();
        let read = |document: &str| -> (String, Option<f64>) {
            let at = document.find("<div id=\"p1c38\"").unwrap();
            let at = at + document[at..].find("<input").unwrap();
            let input = &document[at..at + document[at..].find('>').unwrap()];
            let style = attr(input, "style").unwrap_or("");
            let size = style
                .find("font-size:")
                .and_then(|at| style[at + "font-size:".len()..].split("pt").next())
                .and_then(|size| size.parse().ok());
            (attr(input, "value").unwrap_or("").to_string(), size)
        };
        let long = words_of(80);
        let document =
            filled_document(slug, &BTreeMap::from([(key.clone(), long.clone())])).unwrap();
        let (value, size) = read(&document);
        assert_eq!(value, long, "the whole value prints");
        let size = size.expect("a smaller size");
        assert!(size < class_size);
        assert!(room >= 2.0);
        assert!(sheet.width(sheet.letter_for(classes), &value, size) <= room - 1.0);
        let fitting = words_of(20);
        let document =
            filled_document(slug, &BTreeMap::from([(key.clone(), fitting.clone())])).unwrap();
        assert_eq!(
            read(&document),
            (fitting, None),
            "a value that fits keeps its size"
        );
        // The size goes last in the input's own style.
        assert_eq!(
            with_font_size(
                "<input class=\"fi fh15\" style=\"inset:0.75pt\" value=\"X\">",
                7.5
            ),
            "<input class=\"fi fh15\" style=\"inset:0.75pt;font-size:7.50pt\" value=\"X\">"
        );
        assert_eq!(
            with_font_size("<input class=\"fi\" value=\"X\">", 7.5),
            "<input class=\"fi\" value=\"X\" style=\"font-size:7.50pt\">"
        );
    }

    /// Every sheet filled with values that fit their combs: each text join
    /// whose cells are all combs holding as many characters as its smallest
    /// comb has boxes (at most 12), hashed. Nothing overflows, so the
    /// overflow sizing never runs.
    fn fitting_print_hash(slug: &str) -> String {
        use sha2::{Digest, Sha256};
        let loaded = bundle(slug).unwrap();
        let cells = writer_cells(slug).unwrap();
        let tags = input_tags(loaded.html);
        let grouped = grouped_input_indices(&tags);
        // Only the joins whose every cell is a comb (its boxes are its slot
        // inputs; a 20YY comb has two): a plain box's room is in points,
        // not boxes, and `fitting_plain_print_hash` fills those.
        let fields: BTreeMap<String, String> = cells
            .joins
            .iter()
            .filter_map(|(key, ids)| {
                let slots: Option<Vec<usize>> = ids
                    .iter()
                    .map(|cell| {
                        grouped
                            .get(cell.as_str())
                            .filter(|indices| indices.iter().all(|&i| tags[i].slot.is_some()))
                            .map(Vec::len)
                    })
                    .collect();
                let slots = slots?.into_iter().min()?.min(12);
                Some((key.clone(), words_of(slots.max(1))))
            })
            .collect();
        let document = filled_document(slug, &fields).unwrap();
        let overflowing: Vec<String> = printed_cells(&document)
            .into_iter()
            .filter(|(_, (_, size))| size.is_some())
            .map(|(cell, (text, _))| format!("{cell} {text:?}"))
            .collect();
        assert!(
            overflowing.is_empty(),
            "{slug}: nothing overflows: {overflowing:?}"
        );
        Sha256::digest(document.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Every sheet filled with values that fit its plain boxes: each text
    /// join whose cells are all plain boxes holding the longest address
    /// text that sets within each of them at its own class's size, in the
    /// sheet's face with its word- and letter-spacing, hashed, with the
    /// number of keys filled.
    fn fitting_plain_print_hash(slug: &str) -> (String, usize) {
        use sha2::{Digest, Sha256};
        let fields = fitting_plain_fields(slug);
        let document = filled_document(slug, &fields).unwrap();
        let hash = Sha256::digest(document.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        (hash, fields.len())
    }

    /// Whether `cell` of sheet `slug` is one whose layout round 6 fixed
    /// (1600WP's attachment-period dates, which every earlier fill printed
    /// whole into each one-character box): the goldens recorded with the
    /// earlier fills leave its joins out, and
    /// `split_cells_print_one_character_per_box_or_one_line` pins it.
    fn layout_fixed_in_round_6(slug: &str, cell: &str) -> bool {
        slug == "1600wp-2010"
            && SheetType::of_sheet(slug, "")
                .split
                .iter()
                .any(|(split, _)| *split == cell)
    }

    /// The fields [`fitting_plain_print_hash`] fills.
    fn fitting_plain_fields(slug: &str) -> BTreeMap<String, String> {
        let loaded = bundle(slug).unwrap();
        let cells = writer_cells(slug).unwrap();
        let sheet = SheetType::of_sheet(slug, loaded.css);
        let mut fields = BTreeMap::new();
        for (key, ids) in &cells.joins {
            // A split cell takes one character per box, not a plain line.
            if ids
                .iter()
                .any(|cell| sheet.split.iter().any(|(split, _)| split == cell))
            {
                continue;
            }
            // As recorded: each cell's first input, its room less 1 pt.
            let boxes: Option<Vec<(f64, &str)>> = ids
                .iter()
                .map(|cell| {
                    plain_inputs(loaded.html, cell)
                        .first()
                        .map(|(room, classes, _)| (room - 1.0, *classes))
                })
                .collect();
            let Some(boxes) = boxes.filter(|boxes| !boxes.is_empty()) else {
                continue;
            };
            let fits = |text: &str| {
                boxes.iter().all(|(room, classes)| {
                    sheet.size_for(classes).is_some_and(|size| {
                        sheet.width(sheet.letter_for(classes), text, size) <= *room
                    })
                })
            };
            if let Some(len) = (1..=120).rev().find(|len| fits(&words_of(*len))) {
                fields.insert(key.clone(), words_of(len));
            }
        }
        fields
    }

    /// A value that fits its boxes prints byte for byte as before; a value
    /// longer than its boxes is measured (P4-7). The goldens were written
    /// once by the fill before every sheet's type was measured
    /// (`fixtures/fitting-goldens.json` for the combs,
    /// `fixtures/fitting-plain-goldens.json` for the plain boxes) and are
    /// never rewritten: the test has no way to bless them. Round 6 left the
    /// split cells (2550M's and 1600WP's) out of the plain boxes, since
    /// they take one character per box, and recorded those two sheets'
    /// entries again with dcc6386a's fill, the way
    /// `fitting-band-goldens.json` was. The 12b merge into integration
    /// recorded the two 1800/1801 comb entries and 1801's plain-box entry
    /// again the same way: PR 10's donor-estate joins, which the branch's
    /// pins predate, are the only values that moved them.
    #[test]
    fn fitting_text_prints_as_before_on_every_sheet() {
        let combs: BTreeMap<String, String> = sheet_slugs()
            .iter()
            .map(|slug| (slug.clone(), fitting_print_hash(slug)))
            .collect();
        let golden: BTreeMap<String, String> =
            serde_json::from_str(include_str!("fixtures/fitting-goldens.json")).unwrap();
        assert_eq!(
            golden, combs,
            "every sheet prints fitting comb values as before"
        );
        let mut keys = 0;
        let plain: BTreeMap<String, String> = sheet_slugs()
            .iter()
            .map(|slug| {
                let (hash, filled) = fitting_plain_print_hash(slug);
                keys += filled;
                (slug.clone(), hash)
            })
            .collect();
        assert!(keys > 100, "{keys} plain-box keys filled to fit");
        let golden: BTreeMap<String, String> =
            serde_json::from_str(include_str!("fixtures/fitting-plain-goldens.json")).unwrap();
        assert_eq!(
            golden, plain,
            "every sheet prints fitting plain-box values as before"
        );
    }

    /// The values of every text join of sheet `slug` that fit their boxes
    /// to the edge: the longest start of an address (letters and digits for
    /// a split one-character cell) that, in every cell the join prints in,
    /// takes no more characters than a comb has boxes, one character per
    /// box in a split cell, and in each input of a plain cell sets within
    /// that input's whole room at its class's size, measured in the sheet's
    /// type. Some plain-box values reach the band where the round-4 fill
    /// shrank a value that fit (the 1 pt spare); [`band_box_fields`] puts
    /// one in every band box. The joins of [`layout_fixed_in_round_6`]
    /// cells are left out.
    fn band_fields(slug: &str) -> BTreeMap<String, String> {
        const ADDRESS: &str =
            "UNIT 1203 TWELFTH FLOOR CURVE BUILDING BONIFACIO GLOBAL CITY TAGUIG METRO MANILA";
        const CODE: &str = "1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let loaded = bundle(slug).unwrap();
        let sheet = SheetType::of_sheet(slug, loaded.css);
        let cells = writer_cells(slug).unwrap();
        let tags = input_tags(loaded.html);
        let grouped = grouped_input_indices(&tags);
        let mut fields = BTreeMap::new();
        for (key, ids) in &cells.joins {
            if ids.iter().any(|cell| layout_fixed_in_round_6(slug, cell)) {
                continue;
            }
            let split = ids.iter().any(|cell| {
                sheet.split.iter().any(|(split, _)| split == cell)
                    && grouped
                        .get(cell.as_str())
                        .is_some_and(|indices| splits(&tags, indices))
            });
            let pool = if split { CODE } else { ADDRESS };
            // Each cell's limit, read once: a comb's or a split cell's
            // boxes, a plain cell's inputs (room, size, letter-spacing).
            enum Limit {
                Boxes(usize),
                Split(usize),
                Plain(Vec<Option<(f64, f64, Spacing)>>),
            }
            let limits: Vec<Limit> = ids
                .iter()
                .filter_map(|cell| {
                    let indices = grouped.get(cell.as_str())?;
                    if indices.iter().all(|&i| tags[i].slot.is_some()) {
                        return Some(Limit::Boxes(indices.len()));
                    }
                    if sheet.split.iter().any(|(split, _)| split == cell) && splits(&tags, indices)
                    {
                        return Some(Limit::Split(indices.len()));
                    }
                    Some(Limit::Plain(
                        plain_inputs(loaded.html, cell)
                            .into_iter()
                            .map(|(room, classes, _)| {
                                sheet
                                    .size_for(classes)
                                    .map(|size| (room, size, sheet.letter_for(classes)))
                            })
                            .collect(),
                    ))
                })
                .collect();
            let fits = |text: &str| {
                limits.iter().all(|limit| match limit {
                    Limit::Boxes(boxes) => text.chars().count() <= *boxes,
                    Limit::Split(boxes) => fits_one_per_box(text, *boxes),
                    Limit::Plain(inputs) => {
                        !inputs.is_empty()
                            && inputs.iter().all(|input| {
                                input.is_some_and(|(room, size, letter)| {
                                    sheet.width(letter, text, size) <= room
                                })
                            })
                    }
                })
            };
            let best = (1..=pool.len())
                .rev()
                .map(|n| pool[..n].trim_end())
                .find(|text| !text.is_empty() && fits(text));
            if let Some(text) = best {
                fields.insert(key.clone(), text.to_string());
            }
        }
        fields
    }

    /// A value that fits its boxes to the edge prints byte for byte as at
    /// dcc6386a, the last commit before every sheet's type was measured
    /// (round 5: the 1 pt spare and the split cells shrank such values).
    /// `fixtures/fitting-band-goldens.json` holds, per sheet, the SHA-256 of
    /// the document dcc6386a's fill printed for [`band_fields`]: the fields
    /// were written out by this branch's code, dcc6386a was exported with
    /// `git archive dcc6386a` into a scratch directory, and a scratch binary
    /// depending on its bir-print filled and hashed each sheet. Round 6
    /// recorded the 1600wp-2010 entry again that way, without its
    /// attachment dates ([`layout_fixed_in_round_6`]). The test cannot
    /// bless it. The 12b merge into integration recorded the 1800/1801
    /// entries again too: PR 10's donor-estate joins are the only values
    /// that moved them, and the branch's pins predate those joins.
    #[test]
    fn fitting_band_prints_as_at_dcc6386a_on_every_sheet() {
        let golden: BTreeMap<String, String> =
            serde_json::from_str(include_str!("fixtures/fitting-band-goldens.json")).unwrap();
        let mut filled = 0;
        let now: BTreeMap<String, String> = sheet_slugs()
            .iter()
            .map(|slug| {
                let fields = band_fields(slug);
                filled += fields.len();
                (slug.clone(), document_hash(slug, &fields))
            })
            .collect();
        assert!(filled > 500, "{filled} joins filled to the edge");
        assert!(
            band_fields("1706-2018").contains_key("frm1706:txtBuyerName"),
            "1706's buyer name is filled"
        );
        assert_eq!(
            golden, now,
            "every sheet prints values that fit to the edge as before"
        );
    }

    /// The SHA-256 of the document sheet `slug` prints for `fields`.
    fn document_hash(slug: &str, fields: &BTreeMap<String, String>) -> String {
        use sha2::{Digest, Sha256};
        let document = filled_document(slug, fields).unwrap();
        Sha256::digest(document.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// One band box of a sheet: a text join printing in a single plain cell
    /// with one input, not a comb or a split cell, filled with a value that
    /// sets at the input's class size within its room but not within the
    /// room less 1 pt (the band the round-4 fill shrank). The value is the
    /// longest start of the address, else of "1234567890 ABC...", else of
    /// "1234567890ABC...", that lands in the band.
    struct BandBox {
        key: String,
        cell: String,
        value: String,
        room: f64,
        width: f64,
    }

    /// Every band box of sheet `slug` ([`BandBox`]).
    fn band_boxes(slug: &str) -> Vec<BandBox> {
        const POOLS: [&str; 3] = [
            "UNIT 1203 TWELFTH FLOOR CURVE BUILDING BONIFACIO GLOBAL CITY TAGUIG METRO MANILA",
            "1234567890 ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        ];
        let loaded = bundle(slug).unwrap();
        let sheet = SheetType::of_sheet(slug, loaded.css);
        let cells = writer_cells(slug).unwrap();
        let tags = input_tags(loaded.html);
        let grouped = grouped_input_indices(&tags);
        let mut boxes = Vec::new();
        for (key, ids) in &cells.joins {
            let [cell] = ids.as_slice() else {
                continue;
            };
            let Some(indices) = grouped.get(cell.as_str()) else {
                continue;
            };
            if indices.len() != 1
                || tags[indices[0]].slot.is_some()
                || sheet.split.iter().any(|(split, _)| split == cell)
            {
                continue;
            }
            let [(room, classes, _)] = plain_inputs(loaded.html, cell)[..] else {
                continue;
            };
            let Some(size) = sheet.size_for(classes) else {
                continue;
            };
            let letter = sheet.letter_for(classes);
            let found = POOLS.iter().find_map(|pool| {
                (1..=pool.len())
                    .rev()
                    .map(|n| pool[..n].trim_end())
                    .filter(|text| !text.is_empty())
                    .map(|text| (text, sheet.width(letter, text, size)))
                    .find(|(_, width)| room - 1.0 < *width && *width <= room)
            });
            if let Some((value, width)) = found {
                boxes.push(BandBox {
                    key: key.clone(),
                    cell: cell.clone(),
                    value: value.to_string(),
                    room,
                    width,
                });
            }
        }
        boxes
    }

    /// Every band box of every sheet ([`band_boxes`], 1,458 of them) holds
    /// a value in its 1 pt band and prints byte for byte as at dcc6386a:
    /// `fixtures/fitting-band-box-goldens.json` holds, per sheet, the
    /// SHA-256 of the document dcc6386a's fill printed for that sheet's
    /// band-box values (recorded as `fitting-band-goldens.json` was). A
    /// fill that kept the 1 pt spare shrinks every one of these values
    /// (round 6 checked it fails this test). The test cannot bless it.
    /// The 12b merge into integration added two band boxes on 1801 —
    /// PR 10's `freqOtherInput` and `txtInstallmentYear` joins, which the
    /// branch's pin predates — and re-recorded that sheet's golden; every
    /// other sheet stays byte-pinned at dcc6386a.
    #[test]
    fn every_band_box_prints_as_at_dcc6386a() {
        let golden: BTreeMap<String, String> =
            serde_json::from_str(include_str!("fixtures/fitting-band-box-goldens.json")).unwrap();
        let mut total = 0;
        let mut now = BTreeMap::new();
        for slug in sheet_slugs() {
            let boxes = band_boxes(&slug);
            if boxes.is_empty() {
                continue;
            }
            total += boxes.len();
            let fields: BTreeMap<String, String> = boxes
                .iter()
                .map(|band| (band.key.clone(), band.value.clone()))
                .collect();
            assert_eq!(fields.len(), boxes.len(), "{slug}: one key per box");
            let document = filled_document(&slug, &fields).unwrap();
            for band in &boxes {
                assert!(
                    band.room - 1.0 < band.width && band.width <= band.room,
                    "{slug} {}: {:?} measures {} in {}",
                    band.cell,
                    band.value,
                    band.width,
                    band.room
                );
                let inputs = plain_inputs(&document, &band.cell);
                let [(_, _, tag)] = inputs[..] else {
                    panic!("{slug} {}: one input", band.cell);
                };
                assert!(
                    !tag.contains("font-size"),
                    "{slug} {}: {:?} keeps its class size: {tag}",
                    band.cell,
                    band.value
                );
            }
            now.insert(slug.clone(), document_hash(&slug, &fields));
        }
        assert_eq!(total, 1458, "band boxes");
        assert_eq!(golden, now, "every band box prints as at dcc6386a");
    }

    /// The 0.02 pt allowance ([`FIT_EPSILON_PT`]): a value the model puts
    /// 0.01 pt over its input's room prints at its class size (unshrunk);
    /// 0.03 pt over, it shrinks to set within the room less 1 pt.
    #[test]
    fn a_value_a_hair_over_its_room_keeps_its_size() {
        let slug = "0605-1999";
        let loaded = bundle(slug).unwrap();
        let sheet = SheetType::of_sheet(slug, loaded.css);
        let [(room, classes, tag)] = plain_inputs(loaded.html, "p1c33")[..] else {
            panic!("0605 p1c33 has one input");
        };
        let width = plain_cell_width(loaded.html, "p1c33").unwrap();
        let inset = width - room;
        let size = sheet.size_for(classes).unwrap();
        let letter = sheet.letter_for(classes);
        let text = "UNIT 1203 TWELFTH";
        let measured = sheet.width(letter, text, size);
        // A cell whose input's room is the value's width less `over` pt.
        let at = |over: f64| plain_input_size(&sheet, measured - over + inset, tag, text);
        assert_eq!(at(0.0), None);
        assert_eq!(at(0.01), None, "0.01 pt over: unshrunk");
        let shrunk = at(0.03).expect("0.03 pt over: shrunk");
        let room = measured - 0.03;
        assert!(room >= 2.0);
        assert_eq!(shrunk, sheet.fit(letter, room - 1.0, text, size));
        assert!(shrunk < size);
        assert!(sheet.width(letter, text, shrunk) <= room - 1.0);
    }

    /// Round 6, D4: a split one-character cell (1600WP's attachment-period
    /// dates, 2550M's sheets count, RDO and ZIP) prints a value of no more
    /// characters than boxes one character per box at its class's size,
    /// like a comb, and a longer one once, as a single plain line measured
    /// across the whole cell, its boxes empty. The split cells listed are
    /// every joined cell of one-character inputs on the frozen sheets.
    #[test]
    fn split_cells_print_one_character_per_box_or_one_line() {
        let mut listed = Vec::new();
        let mut found = Vec::new();
        for slug in sheet_slugs() {
            let loaded = bundle(&slug).unwrap();
            let sheet = SheetType::of_sheet(&slug, loaded.css);
            let cells = writer_cells(&slug).unwrap();
            let tags = input_tags(loaded.html);
            let grouped = grouped_input_indices(&tags);
            for ids in cells.joins.values() {
                for cell in ids {
                    if grouped
                        .get(cell.as_str())
                        .is_some_and(|indices| splits(&tags, indices))
                    {
                        found.push(format!("{slug} {cell}"));
                    }
                }
            }
            for (cell, _) in sheet.split {
                listed.push(format!("{slug} {cell}"));
                let indices = &grouped[*cell];
                let (key, _) = cells
                    .joins
                    .iter()
                    .find(|(_, ids)| ids.iter().any(|id| id == cell))
                    .unwrap();
                let fill = |value: &str| {
                    filled_document(&slug, &BTreeMap::from([(key.clone(), value.to_string())]))
                        .unwrap()
                };
                // As many characters as boxes: one per box.
                let value = &"1234567890"[..indices.len()];
                let document = fill(value);
                let inputs = plain_inputs(&document, cell);
                assert_eq!(inputs.len(), indices.len(), "{slug} {cell}");
                for (offset, (_, _, tag)) in inputs.iter().enumerate() {
                    assert!(!tag.contains("font-size"), "{slug} {cell}: {tag}");
                    assert_eq!(attr(tag, "value"), Some(&value[offset..=offset]));
                }
                assert!(!document.contains(&format!("data-overflow-of=\"{cell}\"")));
                // Longer: one line over the cell, the boxes empty.
                let long = "1234567890";
                let document = fill(long);
                for (_, _, tag) in plain_inputs(&document, cell) {
                    assert_eq!(attr(tag, "value"), Some(""), "{slug} {cell}: {tag}");
                }
                let line = printed_cells(&document).remove(*cell);
                let (text, size) = line.expect("an overflow line");
                assert_eq!(text, long);
                let size = size.unwrap();
                let width = plain_cell_width(&document, cell).unwrap();
                assert!(1.0 + sheet.width(sheet.letter, long, size) <= width);
                assert_eq!(
                    document
                        .matches(&format!("data-overflow-of=\"{cell}\""))
                        .count(),
                    1
                );
            }
        }
        found.sort();
        listed.sort();
        assert_eq!(found, listed, "every joined split cell is listed");
        assert_eq!(listed.len(), 9);
        // 1600WP's dates: "01" and "2024" one per box, "20240101" one line.
        let slug = "1600wp-2010";
        let loaded = bundle(slug).unwrap();
        let sheet = SheetType::of_sheet(slug, loaded.css);
        let fields = BTreeMap::from([
            ("derived:att_from_mm".to_string(), "01".to_string()),
            ("derived:att_from_yyyy".to_string(), "2024".to_string()),
            ("derived:att_to_yyyy".to_string(), "20240101".to_string()),
        ]);
        let document = filled_document(slug, &fields).unwrap();
        let boxes = |cell: &str| -> Vec<String> {
            plain_inputs(&document, cell)
                .iter()
                .map(|(_, _, tag)| {
                    assert!(!tag.contains("font-size"), "{tag}");
                    attr(tag, "value").unwrap().to_string()
                })
                .collect()
        };
        assert_eq!(boxes("p1c62"), ["0", "1"]);
        assert_eq!(boxes("p1c64"), ["2", "0", "2", "4"]);
        assert_eq!(boxes("p1c67"), ["", "", "", ""]);
        let (text, size) = printed_cells(&document).remove("p1c67").unwrap();
        assert_eq!(text, "20240101");
        let width = plain_cell_width(&document, "p1c67").unwrap();
        assert!(1.0 + sheet.width(sheet.letter, &text, size.unwrap()) <= width);
        assert_eq!(document.matches("data-overflow-of=\"p1c6").count(), 1);
    }

    /// The writer key of the text join that prints in `cell` of `slug`.
    fn key_of(slug: &str, cell: &str) -> String {
        writer_cells(slug)
            .unwrap()
            .joins
            .into_iter()
            .find(|(_, ids)| ids.iter().any(|id| id == cell))
            .unwrap()
            .0
    }

    /// The `<div id="{cell}" ...>...</div>` of `cell` in `document`.
    fn cell_html<'a>(document: &'a str, cell: &str) -> &'a str {
        let open = document.find(&format!("<div id=\"{cell}\"")).unwrap();
        let close = open + document[open..].find("</div>").unwrap() + "</div>".len();
        &document[open..close]
    }

    /// Round 6, D3: values with a tab or a no-break space, printed. On a
    /// Tinos sheet (0605, 1604F) and an Arimo sheet (2200AN), a value that
    /// fits prints its cell byte for byte as dcc6386a's fill did (the
    /// literals are that fill's output, from a scratch binary on `git
    /// archive dcc6386a`); a longer one shrinks to set within its room less
    /// 1 pt, measured with the tab's stop and the no-break space's
    /// word-spacing.
    #[test]
    fn tab_and_no_break_space_values_print_as_measured() {
        let fitting: [(&str, &str, &str, &str, &str); 6] = [
            (
                "0605-1999",
                "frm0605:txtLineBus",
                "p1c33",
                "UNIT\t1203 TWELFTH FLOOR",
                P1C33_TAB,
            ),
            (
                "0605-1999",
                "frm0605:txtLineBus",
                "p1c33",
                "UNIT\u{a0}1203 TWELFTH FLOOR",
                P1C33_NBSP,
            ),
            (
                "1604f-2018",
                "frm1604f:txtSched1Date1",
                "p1c48",
                "UNIT\t1203 TWELFTH F",
                P1C48_TAB,
            ),
            (
                "1604f-2018",
                "frm1604f:txtSched1Date1",
                "p1c48",
                "UNIT\u{a0}1203 TWELFTH FLOO",
                P1C48_NBSP,
            ),
            (
                "2200an-2018",
                "frm2200ANv2018:txtXGOthers1Name",
                "p2c246",
                "UNIT\t1203 TWELFTH FLOOR CURVE BUILDIN",
                P2C246_TAB,
            ),
            (
                "2200an-2018",
                "frm2200ANv2018:txtXGOthers1Name",
                "p2c246",
                "UNIT\u{a0}1203 TWELFTH FLOOR CURVE BUILDIN",
                P2C246_NBSP,
            ),
        ];
        for (slug, key, cell, value, dcc6386a) in fitting {
            let document = filled_document(
                slug,
                &BTreeMap::from([(key.to_string(), value.to_string())]),
            )
            .unwrap();
            assert_eq!(
                cell_html(&document, cell),
                dcc6386a,
                "{slug} {cell} {value:?}"
            );
        }
        // Headless Chrome's widths (pt) of such values at their class
        // sizes, each set at 100 times its size, spacing included (an
        // `<input>`'s scrollWidth px × 0.75 / 100): the model's are within
        // 0.01 pt. 1604F's "UNIT<TAB>1203 TWELFTH FLOO" overflows its
        // 99.6 pt room: "UNIT" (16.62 pt) is past the first stop (16.51
        // pt), so the tab runs to the second.
        for (slug, cell, value, chrome) in [
            ("1604f-2018", "p1c48", "UNIT\t1203 TWELFTH FLOO", 101.715),
            (
                "1604f-2018",
                "p1c48",
                "UNIT\u{a0}1203 TWELFTH FLOO",
                87.3825,
            ),
            ("0605-1999", "p1c33", "UNIT\t1203 TWELFTH FLOOR", 149.94),
            (
                "0605-1999",
                "p1c33",
                "UNIT\u{a0}1203 TWELFTH FLOOR",
                137.145,
            ),
            (
                "2200an-2018",
                "p2c246",
                "UNIT\t1203 TWELFTH FLOOR CURVE BUILDIN",
                187.26,
            ),
            (
                "2200an-2018",
                "p2c246",
                "UNIT\u{a0}1203 TWELFTH FLOOR CURVE BUILDIN",
                172.7025,
            ),
        ] {
            let loaded = bundle(slug).unwrap();
            let sheet = SheetType::of_sheet(slug, loaded.css);
            let [(room, classes, _)] = plain_inputs(loaded.html, cell)[..] else {
                panic!("{slug} {cell}: one input");
            };
            let size = sheet.size_for(classes).unwrap();
            let model = sheet.width(sheet.letter_for(classes), value, size);
            assert!(
                (model - chrome).abs() <= 0.01,
                "{slug} {cell} {value:?}: {model} against Chrome's {chrome}"
            );
            let document = filled_document(
                slug,
                &BTreeMap::from([(key_of(slug, cell), value.to_string())]),
            )
            .unwrap();
            let [(_, _, tag)] = plain_inputs(&document, cell)[..] else {
                panic!("{slug} {cell}: one input");
            };
            assert_eq!(
                tag.contains("font-size"),
                chrome > room,
                "{slug} {cell} {value:?}: shrunk only past its {room} pt room"
            );
        }
        for (slug, key, cell, face) in [
            ("0605-1999", "frm0605:txtLineBus", "p1c33", Face::Tinos),
            (
                "2200an-2018",
                "frm2200ANv2018:txtXGOthers1Name",
                "p2c246",
                Face::Arimo,
            ),
        ] {
            let loaded = bundle(slug).unwrap();
            let sheet = SheetType::of_sheet(slug, loaded.css);
            assert_eq!(sheet.face, face);
            for space in ["\t", "\u{a0}"] {
                let value = format!(
                    "UNIT{space}1203 TWELFTH FLOOR CURVE BUILDING BONIFACIO GLOBAL CITY TAGUIG"
                );
                let document =
                    filled_document(slug, &BTreeMap::from([(key.to_string(), value.clone())]))
                        .unwrap();
                let [(room, classes, tag)] = plain_inputs(&document, cell)[..] else {
                    panic!("{slug} {cell}: one input");
                };
                assert_eq!(attr(tag, "value"), Some(value.as_str()));
                let letter = sheet.letter_for(classes);
                let class_size = sheet.size_for(classes).unwrap();
                assert!(sheet.width(letter, &value, class_size) > room + FIT_EPSILON_PT);
                let style = attr(tag, "style").unwrap();
                let at = style.find("font-size:").expect("an inline size") + "font-size:".len();
                let size: f64 = style[at..].split("pt").next().unwrap().parse().unwrap();
                assert!(size < class_size, "{slug} {cell} {space:?}");
                assert!(room >= 2.0);
                assert!(
                    sheet.width(letter, &value, size) <= room - 1.0,
                    "{slug} {cell} {space:?}: at {size}pt"
                );
                assert!(
                    sheet.width(letter, &value, size + 0.01) > room - 1.0,
                    "{slug} {cell} {space:?}: the largest size that sets"
                );
            }
        }
    }

    // dcc6386a's fill of the fitting values of
    // `tab_and_no_break_space_values_print_as_measured`, cell by cell.
    const P1C33_TAB: &str = "<div id=\"p1c33\" class=\"c f\" data-cell-kind=\"field\" data-row=\"14\" data-col=\"72\" data-field-kind=\"text\" data-field-name=\"p1c33\" style=\"left:418.44pt;top:246.12pt;width:158.4pt;height:18.96pt\"><input type=\"text\" class=\"fi fh22\" id=\"p1c33-i\" name=\"p1c33\" style=\"inset:0.75pt 0.75pt 0.75pt 0.75pt\" autocomplete=\"off\" spellcheck=\"false\" value=\"UNIT\t1203 TWELFTH FLOOR\" data-writer-value=\"UNIT\t1203 TWELFTH FLOOR\"></div>";
    const P1C33_NBSP: &str = "<div id=\"p1c33\" class=\"c f\" data-cell-kind=\"field\" data-row=\"14\" data-col=\"72\" data-field-kind=\"text\" data-field-name=\"p1c33\" style=\"left:418.44pt;top:246.12pt;width:158.4pt;height:18.96pt\"><input type=\"text\" class=\"fi fh22\" id=\"p1c33-i\" name=\"p1c33\" style=\"inset:0.75pt 0.75pt 0.75pt 0.75pt\" autocomplete=\"off\" spellcheck=\"false\" value=\"UNIT\u{a0}1203 TWELFTH FLOOR\" data-writer-value=\"UNIT\u{a0}1203 TWELFTH FLOOR\"></div>";
    const P1C48_TAB: &str = "<div id=\"p1c48\" class=\"c f\" data-cell-kind=\"field\" data-row=\"22\" data-col=\"1\" data-field-kind=\"text\" data-field-name=\"p1c48\" style=\"left:86.42pt;top:360.17pt;width:100.56pt;height:24.6pt\"><input type=\"text\" class=\"fi fh25\" id=\"p1c48-i\" name=\"p1c48\" style=\"inset:0.48pt 0.48pt 0.48pt 0.48pt\" autocomplete=\"off\" spellcheck=\"false\" value=\"UNIT\t1203 TWELFTH F\" data-writer-value=\"UNIT\t1203 TWELFTH F\"></div>";
    const P1C48_NBSP: &str = "<div id=\"p1c48\" class=\"c f\" data-cell-kind=\"field\" data-row=\"22\" data-col=\"1\" data-field-kind=\"text\" data-field-name=\"p1c48\" style=\"left:86.42pt;top:360.17pt;width:100.56pt;height:24.6pt\"><input type=\"text\" class=\"fi fh25\" id=\"p1c48-i\" name=\"p1c48\" style=\"inset:0.48pt 0.48pt 0.48pt 0.48pt\" autocomplete=\"off\" spellcheck=\"false\" value=\"UNIT\u{a0}1203 TWELFTH FLOO\" data-writer-value=\"UNIT\u{a0}1203 TWELFTH FLOO\"></div>";
    const P2C246_TAB: &str = "<div id=\"p2c246\" class=\"c f\" data-cell-kind=\"field\" data-row=\"38\" data-col=\"1\" data-field-kind=\"text\" data-field-name=\"p2c246\" style=\"left:60.72pt;top:685.18pt;width:189.41pt;height:14.76pt\"><input type=\"text\" class=\"fi fh7\" id=\"p2c246-i\" name=\"p2c246\" style=\"inset:0.48pt 0.48pt 0.48pt 0.48pt\" autocomplete=\"off\" spellcheck=\"false\" value=\"UNIT\t1203 TWELFTH FLOOR CURVE BUILDIN\" data-writer-value=\"UNIT\t1203 TWELFTH FLOOR CURVE BUILDIN\"></div>";
    const P2C246_NBSP: &str = "<div id=\"p2c246\" class=\"c f\" data-cell-kind=\"field\" data-row=\"38\" data-col=\"1\" data-field-kind=\"text\" data-field-name=\"p2c246\" style=\"left:60.72pt;top:685.18pt;width:189.41pt;height:14.76pt\"><input type=\"text\" class=\"fi fh7\" id=\"p2c246-i\" name=\"p2c246\" style=\"inset:0.48pt 0.48pt 0.48pt 0.48pt\" autocomplete=\"off\" spellcheck=\"false\" value=\"UNIT\u{a0}1203 TWELFTH FLOOR CURVE BUILDIN\" data-writer-value=\"UNIT\u{a0}1203 TWELFTH FLOOR CURVE BUILDIN\"></div>";
}

#[cfg(test)]
mod donor_estate_print_tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Every writer key a sheet's writer-cells.json joins, with a value
    /// each join kind prints (a deterministic stand-in for a draft).
    fn every_join_filled(slug: &str) -> BTreeMap<String, String> {
        let doc: serde_json::Value =
            serde_json::from_str(writer_cells_json(slug).unwrap()).unwrap();
        let mut fields = BTreeMap::new();
        for (section, value) in [
            ("joins", "ABC123"),
            ("money_joins", "1,234.56"),
            ("peso_joins", "1,234.56"),
            ("xbox_joins", "true"),
            ("date_joins", "06/15/2025"),
        ] {
            for row in doc[section].as_array().into_iter().flatten() {
                if let Some(key) = row["writer_key"].as_str() {
                    fields.insert(key.to_string(), value.to_string());
                }
            }
        }
        // PR 10's joins read their sources too: a value each prints, and
        // each date part as its form writes it.
        for (sheet, _, value) in DONOR_ESTATE_JOINS {
            if *sheet != slug {
                continue;
            }
            for key in donor_estate_sources(*value) {
                fields.insert(key.to_string(), "123".to_string());
            }
            if let SheetValue::Key(key) | SheetValue::Tick(key) = value {
                let shown = if matches!(value, SheetValue::Tick(_)) {
                    "true"
                } else {
                    "12"
                };
                fields.insert(key.to_string(), shown.to_string());
            }
        }
        for (key, part) in [
            ("frm1801v2018:txtDateMonth", "06"),
            ("frm1801v2018:txtDateDay", "15"),
            ("frm1801v2018:txtDateYear", "2025"),
            ("frm1800v2018:txtMonth", "06"),
            ("frm1800v2018:txtDate", "15"),
            ("frm1800v2018:txtYear", "2025"),
        ] {
            if fields.contains_key(key) || key.contains(&slug[..4]) {
                fields.insert(key.to_string(), part.to_string());
            }
        }
        fields
    }

    fn digest(text: &str) -> String {
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// A deterministic value in every key writer-cells.json joins (and, on
    /// 1800 and 1801, every source PR 10's joins read, with each date part
    /// as the form writes it), filled and hashed. All four digests were
    /// re-pinned on the 12b merge into integration: 12b's per-sheet fitting
    /// (P4-7) moved 1706's and 2000-OT's long-text print, and PR 10's joins
    /// moved 1800's and 1801's. With the donor joins disabled, the merged
    /// fill reproduces every pre-merge golden byte for byte — the merge
    /// changed nothing else.
    #[test]
    fn pr10_joins_leave_every_other_print_byte_for_byte() {
        for (slug, golden) in [
            (
                "1706-2018",
                "99f6a45573ad85d627f9632165d5c78f70af88c63e651cd79756dad2cf7d46a3",
            ),
            (
                "2000-ot-2018",
                "86f497510e136ed8fc5d40825db719298fba02a4c0d9b62f3546c14423eea198",
            ),
            (
                "1801-2018",
                "d752c2d04bfce02853f926e45862a30a6ae8371b7cf52c82e1064acc85d44d44",
            ),
            (
                "1800-2018",
                "51fd641f9791344fd5ae0c836061fbe06acd42b97bd0a7d827d4295ded37d76f",
            ),
        ] {
            let html = filled_document(slug, &every_join_filled(slug)).unwrap();
            assert_eq!(digest(&html), golden, "{slug}");
        }
        // Only 1800 and 1801 have PR 10 joins.
        assert!(DONOR_ESTATE_JOINS
            .iter()
            .all(|(slug, _, _)| matches!(*slug, "1800-2018" | "1801-2018")));
    }

    fn fixture_profile() -> bir_core::profile::TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null, "full_name": "Print Fixture Estate", "tin": {"segment1": "123",
            "segment2": "456", "segment3": "788", "branch": "00000"}, "rdo_code": "039",
            "line_of_business": "Brokerage", "registered_address": "1 Fixture St",
            "zip_code": "1100", "phone": "0281234567", "email": "fixture@example.com",
            "default_form_type": "1801", "taxpayer_type": "Individual"
        }))
        .unwrap()
    }

    fn estate() -> bir_core::forms::form_1801::Form1801Draft {
        use bir_core::forms::form_1801::{Form1801Draft, Form1801Frequency};
        let mut draft = Form1801Draft::new_from_profile(&fixture_profile(), 2026, 1);
        draft.death_month = 6;
        draft.death_day = 15;
        draft.death_year = 2025;
        draft.administrator_tin = "987654321".into();
        draft.administrator_branch_code = "00001".into();
        draft.tax_relief = Some("RA 9504".into());
        draft.installment_granted = true;
        draft.installment_frequency = Some(Form1801Frequency::Others);
        draft.installment_frequency_other = "Annual".into();
        draft.installment_year = "2027".into();
        draft.recompute();
        draft
    }

    fn comb_text(html: &str, name: &str) -> String {
        named_values(html, name).concat()
    }

    fn named_values(html: &str, name: &str) -> Vec<String> {
        input_tags(html)
            .into_iter()
            .filter(|tag| tag.name == name)
            .map(|tag| attr(tag.tag, "value").unwrap_or("").to_string())
            .collect()
    }

    /// PR 10 (1801 manual-0): item 1 prints the date of death in its one
    /// 6-slot comb p1c4 as the sheet's MM/DD/YY; a date not chosen prints
    /// nothing.
    #[test]
    fn pr10_1801_item_1_prints_the_date_of_death() {
        let draft = estate();
        let html = filled_document("1801-2018", &draft.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c4"), "061525");
        let mut blank = estate();
        blank.death_month = 0;
        blank.death_day = 0;
        blank.death_year = 0;
        let fields = blank.to_print_field_map();
        assert_eq!(fields["frm1801v2018:txtDateMonth"], "00");
        let html = filled_document("1801-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c4"), "");
    }

    /// PR 10: what the 1801 page holds prints in its cell: the executor's
    /// TIN and branch (item 11), the contact number (12), 14A, 15D's
    /// frequency tick and its Others text, and item 21's year. Every value
    /// the print map carries still has a box or a reviewed reason.
    #[test]
    fn pr10_1801_page_values_print_in_their_cells() {
        use bir_core::forms::form_1801::Form1801Frequency;
        let draft = estate();
        let fields = draft.to_print_field_map();
        let html = filled_document("1801-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c31"), "987654321");
        assert_eq!(comb_text(&html, "p1c32"), "00001");
        assert_eq!(comb_text(&html, "p1c33"), "0281234567");
        assert_eq!(comb_text(&html, "p1c38"), "RA 9504");
        assert_eq!(named_values(&html, "p1c52"), vec!["X".to_string()]);
        for other in ["p1c50", "p1c51", "p1c53"] {
            assert_eq!(named_values(&html, other), vec![String::new()], "{other}");
        }
        assert_eq!(named_values(&html, "p1c197"), vec!["ANNUAL".to_string()]);
        assert_eq!(named_values(&html, "p1c201"), vec!["2027".to_string()]);
        assert!(
            unprinted_keys("1801-2018", &fields).unwrap().is_empty(),
            "{:?}",
            unprinted_keys("1801-2018", &fields)
        );
        // Each choice ticks its own box.
        for (frequency, cell) in [
            (Form1801Frequency::Monthly, "p1c50"),
            (Form1801Frequency::Quarterly, "p1c53"),
            (Form1801Frequency::SemiAnnual, "p1c51"),
        ] {
            let mut picked = estate();
            picked.installment_frequency = Some(frequency);
            let html = filled_document("1801-2018", &picked.to_print_field_map()).unwrap();
            assert_eq!(named_values(&html, cell), vec!["X".to_string()], "{cell}");
            assert_eq!(named_values(&html, "p1c52"), vec![String::new()]);
            assert_eq!(named_values(&html, "p1c197"), vec![String::new()]);
        }
        // No executor TIN, no relief: the cells stay blank.
        let mut none = estate();
        none.administrator_tin.clear();
        none.tax_relief = None;
        let html = filled_document("1801-2018", &none.to_print_field_map()).unwrap();
        assert_eq!(comb_text(&html, "p1c31"), "");
        assert_eq!(comb_text(&html, "p1c38"), "");
    }

    /// PR 10: 1800's 13A and 8A ZIP print in their cells.
    #[test]
    fn pr10_1800_13a_and_8a_print_in_their_cells() {
        use bir_core::forms::form_1800::Form1800Draft;
        let mut draft = Form1800Draft::new_from_profile(&fixture_profile(), 2026, 1);
        draft.tax_relief = Some("RA 9504".into());
        draft.recompute();
        let fields = draft.to_print_field_map();
        let html = filled_document("1800-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c57"), "RA 9504");
        assert_eq!(comb_text(&html, "p1c26"), "1100");
        assert!(unprinted_keys("1800-2018", &fields).unwrap().is_empty());
        // A relief longer than the comb prints whole on one line.
        draft.tax_relief = Some("REPUBLIC ACT NO. 9504 SECTION 4".into());
        let html = filled_document("1800-2018", &draft.to_print_field_map()).unwrap();
        assert!(html.contains("value=\"REPUBLIC ACT NO. 9504 SECTION 4\""));
    }

    fn donor(month: u8, day: u8, year: u16) -> BTreeMap<String, String> {
        use bir_core::forms::form_1800::Form1800Draft;
        let mut draft = Form1800Draft::new_from_profile(&fixture_profile(), 2026, 1);
        draft.donation_month = month;
        draft.donation_day = day;
        draft.donation_year = year;
        draft.recompute();
        draft.to_print_field_map()
    }

    /// Second fix check, 1800 item 1: the YY comb p1c7 prints the year's
    /// last two digits (form_1800.rs writes the 4-digit year); a part not
    /// chosen prints nothing, as the page shows it blank; a typed part
    /// prints in its own slots.
    #[test]
    fn pr10_1800_item_1_prints_as_the_page_shows_it() {
        let parts = |fields: &BTreeMap<String, String>| {
            let html = filled_document("1800-2018", fields).unwrap();
            ["p1c5", "p1c6", "p1c7"].map(|cell| comb_text(&html, cell))
        };
        let whole = donor(6, 15, 2025);
        assert_eq!(whole["frm1800v2018:txtYear"], "2025", "the upload keeps it");
        assert_eq!(parts(&whole), ["06", "15", "25"]);
        assert_eq!(parts(&donor(3, 1, 2009)), ["03", "01", "09"]);
        let fresh = donor(0, 0, 0);
        assert_eq!(
            [
                &fresh["frm1800v2018:txtMonth"],
                &fresh["frm1800v2018:txtDate"],
                &fresh["frm1800v2018:txtYear"]
            ],
            ["00", "00", "0"]
        );
        assert_eq!(parts(&fresh), ["", "", ""]);
        assert_eq!(parts(&donor(6, 0, 0)), ["06", "", ""]);
        assert!(unprinted_keys("1800-2018", &fresh).unwrap().is_empty());
    }

    /// Second fix check, 1801 item 1: each typed part prints in its own
    /// MM / DD / YY slots of p1c4; a part not chosen leaves its slots
    /// blank and is no unprinted value (the page shows it blank); a part
    /// that cannot print is reported.
    #[test]
    fn pr10_1801_item_1_prints_each_typed_part() {
        let slots = |draft: &bir_core::forms::form_1801::Form1801Draft| {
            let html = filled_document("1801-2018", &draft.to_print_field_map()).unwrap();
            named_values(&html, "p1c4")
        };
        let mut month = estate();
        month.death_day = 0;
        month.death_year = 0;
        assert_eq!(slots(&month), ["0", "6", "", "", "", ""]);
        let fields = month.to_print_field_map();
        assert!(unprinted_keys("1801-2018", &fields).unwrap().is_empty());
        let mut year = estate();
        year.death_month = 0;
        year.death_day = 0;
        assert_eq!(slots(&year), ["", "", "", "", "2", "5"]);
        let mut odd = fields.clone();
        odd.insert("frm1801v2018:txtDateMonth".into(), "123".into());
        assert_eq!(
            unprinted_keys("1801-2018", &odd).unwrap(),
            ["frm1801v2018:txtDateMonth"]
        );
    }

    /// Second fix check: 1800's 8A ZIP prints trimmed, and 1801's executor
    /// branch code fills its 5 slots ("1" prints "00001"); the upload keeps
    /// what the draft holds.
    #[test]
    fn pr10_zip_and_branch_print_in_their_slots() {
        let mut fields = donor(6, 15, 2025);
        fields.insert("frm1800v2018:txtZipCode".into(), " 1100 ".into());
        let html = filled_document("1800-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c26"), "1100");
        let mut draft = estate();
        draft.administrator_branch_code = "1".into();
        let fields = draft.to_print_field_map();
        assert_eq!(fields["frm1801v2018:txtBranchCodeE"], "1");
        assert_eq!(draft.to_bir_field_map()["frm1801v2018:txtBranchCodeE"], "1");
        let html = filled_document("1801-2018", &fields).unwrap();
        assert_eq!(comb_text(&html, "p1c32"), "00001");
    }
}
