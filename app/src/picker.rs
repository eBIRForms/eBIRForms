//! `Picker`: a field whose value comes from a long list (the New taxpayer
//! dialog's city, ZIP code and RDO), as an autocomplete: the field is the
//! search. Typing in it opens the list floating under it (an overlay, so
//! the dialog never grows) and filters it, the first match highlighted so
//! Return picks it; the field then shows the pick. Text typed but not
//! picked is never a value: leaving the field puts the pick back. The
//! search matches titles, so a title carries what a filer types: "1101
//! Diliman", "43A · East Pasig".

use std::rc::Rc;

use bir_app_core::Choice;
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::theme;

/// Rows the list shows before it scrolls.
const ROWS: usize = 5;

/// Which list a picker shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum List {
    City,
    Zip,
    Rdo,
}

impl List {
    fn placeholder(self) -> &'static str {
        match self {
            List::City => "Search a city or municipality",
            List::Zip => "Search a ZIP code or area",
            List::Rdo => "Search an RDO by code or name",
        }
    }

    fn empty(self, query: &str) -> String {
        match self {
            List::City => format!("No city matches “{query}”."),
            List::Zip => format!("No ZIP code matches “{query}”."),
            List::Rdo => format!("No RDO matches “{query}”. Search by code, like 43A."),
        }
    }

    /// What screen readers call it.
    pub(crate) fn label(self) -> &'static str {
        match self {
            List::City => "City or municipality",
            List::Zip => "ZIP code",
            List::Rdo => "RDO",
        }
    }

    /// The list's entries.
    pub(crate) fn choices(self) -> Vec<Choice> {
        match self {
            List::City => bir_app_core::BirApp::cities(),
            List::Zip => bir_app_core::BirApp::zip_codes(),
            List::Rdo => bir_app_core::BirApp::rdos(),
        }
    }

    /// A code is what a filer types first for these: its matches lead.
    fn code_first(self) -> bool {
        self != List::City
    }
}

/// The entries of `choices` the list shows for `query`: those whose title
/// holds it (ignoring case; all for none), in the table's order but with
/// a title that is exactly the query first, so Return picks it ("Quirino"
/// before "Quirino (Angkaki)"), and, with `code_first`, the ones whose
/// title starts with it next ("43" puts 43A before 043).
pub(crate) fn matches(choices: &[Choice], query: &str, code_first: bool) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<usize> = (0..choices.len()).filter(|&i| q.is_empty() || choices[i].title.to_lowercase().contains(&q)).collect();
    if !q.is_empty() {
        out.sort_by_key(|&i| {
            let title = choices[i].title.to_lowercase();
            (title != q, code_first && !title.starts_with(&q))
        });
    }
    out
}

/// The value a field left holding `typed` keeps, when the filer left it
/// without picking: none for an empty field, the entry whose title (or
/// code, for ZIP codes and RDOs) is exactly what was typed when only one
/// entry has it, else `had`.
pub(crate) fn kept_on_leave(choices: &[Choice], typed: &str, had: Option<String>, code_first: bool) -> Option<String> {
    let t = typed.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    // A title or a code names an entry only when one entry has it: 107
    // city names repeat across provinces ("San Isidro" is in nine), and 49
    // ZIP codes serve several areas.
    let mut titled = choices.iter().filter(|c| c.title.to_lowercase() == t);
    match (titled.next(), titled.next()) {
        (Some(c), None) => return Some(c.value.clone()),
        (Some(_), Some(_)) => return had,
        _ => {}
    }
    let code = |c: &Choice| c.title.split([' ', '·']).next().unwrap_or("").to_lowercase();
    let mut coded = choices.iter().filter(|c| code_first && code(c) == t);
    match (coded.next(), coded.next()) {
        (Some(c), None) => Some(c.value.clone()),
        _ => had,
    }
}

/// A picker's state the dialog reads and drives: whether its list is open,
/// whether the field has the keyboard focus, what it shows, and the field
/// itself (to focus it).
#[derive(Clone, Copy)]
pub(crate) struct Picker {
    pub(crate) open: Signal<bool>,
    pub(crate) focused: Signal<bool>,
    /// The field's text: the pick's title, or what is being typed.
    text: Signal<String>,
    /// The text is the filer's, not the pick's: it filters the list.
    editing: Signal<bool>,
    /// The highlighted row, among the matches.
    highlight: Signal<usize>,
    at: ElementRef,
}

impl Picker {
    pub(crate) fn new() -> Self {
        Self { open: signal(false), focused: signal(false), text: signal(String::new()), editing: signal(false), highlight: signal(0), at: element_ref() }
    }

    pub(crate) fn focus(self) {
        self.at.focus();
    }

    /// Focuses the field with `query` typed in it, its list open.
    pub(crate) fn search(self, query: &str) {
        self.at.focus();
        self.text.set(query.to_owned());
        self.editing.set(true);
        self.highlight.set(0);
        self.open.set(true);
    }
}

/// The picker field over `value` (a [`Choice::value`], or none): `phone`
/// draws it as a row of the phone's form (the desktop's list is as wide
/// as the field, 280 to 480). `left` runs when the filer leaves it, so the dialog can say
/// what is missing.
pub(crate) fn picker(list: List, value: Signal<Option<String>>, state: Picker, phone: bool, left: impl Fn() + 'static) -> impl IntoElement {
    let width = signal(0.0_f32);
    let choices = Rc::new(list.choices());
    let Picker { open, focused, text: typed, editing, highlight, at } = state;
    let find = {
        let choices = choices.clone();
        move |v: &str| choices.iter().find(|c| c.value == v).cloned()
    };
    let title_of = {
        let find = find.clone();
        move |v: Option<String>| v.and_then(|v| find(&v)).map(|c| c.title).unwrap_or_default()
    };
    // The field shows the pick whenever the filer is not typing.
    {
        let title_of = title_of.clone();
        effect(move || {
            let v = value.get();
            if !editing.get() {
                typed.set(title_of(v));
            }
        });
    }
    let rows = {
        let choices = choices.clone();
        computed(move || {
            let q = if editing.get() { typed.get() } else { String::new() };
            matches(&choices, &q, list.code_first())
        })
    };
    let pick = {
        let choices = choices.clone();
        move |row: usize| {
            if let Some(i) = rows.with_untracked(|r| r.get(row).copied()) {
                value.set(Some(choices[i].value.clone()));
            }
            editing.set(false);
            typed.set(title_of(value.get_untracked()));
            open.set(false);
        }
    };
    // Leaving the field: the list closes, and the text goes back to the
    // pick, or picks the entry it names exactly (an empty field clears).
    // After a moment, so a press on a row that took the focus picks first.
    let leave = {
        let choices = choices.clone();
        let left = Rc::new(left);
        move || {
            let (choices, left) = (choices.clone(), left.clone());
            crate::live::store().owner.run(|| {
                set_timeout(std::time::Duration::from_millis(150), move || {
                    if focused.get_untracked() {
                        return;
                    }
                    if editing.get_untracked() {
                        value.set(kept_on_leave(&choices, &typed.get_untracked(), value.get_untracked(), list.code_first()));
                        editing.set(false);
                    }
                    open.set(false);
                    left();
                })
            });
        }
    };

    // Down on the closed field opens the list at the pick.
    let all = choices.clone();
    effect(move || {
        if !focused.get() || open.get() {
            return;
        }
        let choices = all.clone();
        on_key_with("down", KeyOptions { when: When::Always, repeat: false }, move |_| {
            let at_pick = value.get_untracked().and_then(|v| rows.with_untracked(|r| r.iter().position(|&i| choices[i].value == v)));
            highlight.set(at_pick.unwrap_or(0));
            open.set(true);
        });
    });

    let field = crate::widgets::field_box(list.placeholder())
        .value(typed)
        .element_ref(&at)
        .on_focus(move || {
            focused.set(true);
            // Typing replaces the pick.
            let n = typed.get_untracked().encode_utf16().count() as u32;
            at.set_selection(0, n);
        })
        .on_blur(move || {
            focused.set(false);
            leave();
        })
        .on_change(move |v: &str| {
            typed.set(v.to_owned());
            editing.set(true);
            highlight.set(0);
            open.set(true);
        })
        .pr(move || px(if value.get().is_some() { 32.0 } else { 10.0 }))
        .w_full();
    let field = if phone { field.h(px(40.0)).text_size(px(17.0)).border(px(0.0)).pl(px(0.0)).bg(crate::widgets::CLEAR) } else { field };

    // The pick's detail (a city's province), quiet at the field's end
    // while the filer is not typing.
    let detail = {
        let find = find.clone();
        move || if editing.get() { String::new() } else { value.get().and_then(|v| find(&v)).map(|c| c.detail).unwrap_or_default() }
    };
    let detail_shown = detail.clone();
    let clear = view()
        .absolute()
        .top(px(if phone { 8.0 } else { 4.0 }))
        .right(px(4.0))
        .size(px(if phone { 32.0 } else { 24.0 }))
        .items_center()
        .justify_center()
        .rounded(px(theme::R_CTL))
        .cursor_pointer()
        .focusable(false)
        .label(format!("Clear the {}", list.label().to_lowercase()))
        .hover(|s| s.bg(theme::HOVER))
        .on_press(move || {
            editing.set(false);
            value.set(None);
            open.set(false);
        })
        .child(text("✕").text_size(px(12.0)).text_color(theme::FG2));

    view()
        .relative()
        .w_full()
        .on_layout(move |b: &Bounds<Points>| width.set(b.size.width.0))
        .child(field)
        .child(when(move || !detail_shown().is_empty(), move || {
            let detail = detail.clone();
            view().absolute().top(px(0.0)).bottom(px(0.0)).right(px(34.0)).justify_center().pointer_events(PointerEvents::None).child(text(detail).text_size(px(12.0)).text_color(theme::FG2))
        }))
        .child(when(move || value.get().is_some(), move || clear.clone()))
        .child(when(move || open.get(), move || list_popup(list, rows, highlight, typed, at, phone, width, pick.clone(), move || {
            editing.set(false);
            typed.set(String::new());
            open.set(false);
        })))
}

/// The open list: floating against the field, at most [`ROWS`] rows, the
/// rest scrolling. Its keys exist while it is open: Up and Down (and the
/// pages) move the highlight, Return picks it, Escape closes it and puts
/// the pick back in the field (`cancel`, which the field's text follows).
#[allow(clippy::too_many_arguments)]
fn list_popup(list: List, rows: Memo<Vec<usize>>, highlight: Signal<usize>, typed: Signal<String>, at: ElementRef, phone: bool, width: Signal<f32>, pick: impl Fn(usize) + Clone + 'static, cancel: impl Fn() + 'static) -> impl IntoElement {
    let choices = Rc::new(list.choices());
    let scroller = element_ref();
    let shown = stored(None::<(usize, usize)>);
    // The highlighted row comes into view when a key moves it.
    let reveal = move |row: usize| match shown.get() {
        Some((first, _)) if row < first => scroller.scroll_to_index(row as u32, Percentage(0.0), px(0.0), false),
        Some((first, last)) if row > last && first <= last => scroller.scroll_to_index(row as u32, Percentage(1.0), px(0.0), false),
        _ => {}
    };
    let step = move |by: isize| {
        let n = rows.with_untracked(Vec::len);
        if n == 0 {
            return;
        }
        let next = (highlight.get_untracked() as isize + by).clamp(0, n as isize - 1) as usize;
        highlight.set(next);
        reveal(next);
    };
    let keys = KeyOptions { when: When::Always, repeat: true };
    on_key_with("down", keys, move |_| step(1));
    on_key_with("up", keys, move |_| step(-1));
    on_key_with("pagedown", keys, move |_| step(ROWS as isize));
    on_key_with("pageup", keys, move |_| step(-(ROWS as isize)));
    {
        let pick = pick.clone();
        on_key_with("enter", KeyOptions { when: When::Always, repeat: false }, move |_| {
            if rows.with_untracked(|r| !r.is_empty()) {
                pick(highlight.get_untracked());
            }
        });
    }
    on_key_with("escape", KeyOptions { when: When::Always, repeat: false }, move |_| cancel());
    // A new query starts at the top.
    effect(move || {
        rows.with(|_| ());
        untracked(|| scroller.scroll_to_index(0, Percentage(0.0), px(0.0), false));
    });

    let row_h = if phone { 44.0 } else { theme::CONTROL };
    let rows_list = {
        let (choices, keyed) = (choices.clone(), choices.clone());
        virtual_list(
            move || rows.with(Vec::len),
            move |pos: Position| {
                let choices = choices.clone();
                let pick = pick.clone();
                let row = pos.index();
                let entry = {
                    let choices = choices.clone();
                    move || rows.with(|r| r.get(row).map(|&i| choices[i].clone()))
                };
                let (title, detail) = (entry.clone(), entry);
                view()
                    .h(px(row_h))
                    .px(px(if phone { 16.0 } else { 10.0 }))
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .bg(move || if highlight.get() == row { theme::ACCENT_SOFT } else { crate::widgets::CLEAR })
                    .on_mouse_move(move |e: &MouseEvent| {
                        if e.pressed_button.is_none() && highlight.get_untracked() != row {
                            highlight.set(row);
                        }
                    })
                    .on_press(move || pick(row))
                    .child(text(move || title().map(|c| c.title).unwrap_or_default()).flex_shrink(1.0).min_w(px(0.0)).text_size(px(if phone { 15.0 } else { 13.0 })).text_color(theme::FG).truncate())
                    .child(text(move || detail().map(|c| c.detail).unwrap_or_default()).flex_1().min_w(px(0.0)).text_size(px(12.0)).text_color(theme::FG2).truncate())
            },
        )
        .keys(move || rows.with(|r| r.iter().map(|&i| keyed[i].value.clone()).collect::<Vec<_>>()))
        .viewport(ListViewport::Parent)
        .on_viewable_items_change(move |v: &ViewableItems| {
            shown.replace(v.viewable);
        })
        .element_ref(&scroller)
    };
    let mut popup = overlay()
        .anchor(&at)
        .placement(move || if phone && keyboard_height().0 > 0.0 { Placement::Top } else { Placement::Bottom })
        .align(Align::Start)
        .offset(px(4.0))
        .margin(px(8.0))
        .max_h(px(ROWS as f32 * row_h + 2.0))
        .bg(theme::RAISED)
        .border_1()
        .border_color(theme::BORDER)
        .rounded(px(theme::R_ROW))
        .shadow_lg()
        .overflow_hidden();
    popup = if phone { popup.left(px(12.0)).right(px(12.0)) } else { popup.w(move || px(width.get().clamp(280.0, 480.0))) };
    popup
        .child(scroll().flex_shrink(1.0).min_h(px(0.0)).child(rows_list))
        .child(when(move || rows.with(Vec::is_empty), move || {
            text(move || list.empty(&typed.get())).h(px(row_h)).px(px(10.0)).text_size(px(13.0)).text_color(theme::FG3).truncate()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(title: &str) -> Choice {
        Choice { value: title.to_owned(), title: title.to_owned(), detail: String::new() }
    }

    #[test]
    fn typing_filters_titles_and_puts_codes_starting_with_it_first() {
        let list: Vec<Choice> = ["043", "43A · East Pasig", "044 · Taguig"].into_iter().map(choice).collect();
        assert_eq!(matches(&list, "43", true), [1, 0]);
        assert_eq!(matches(&list, "", true), [0, 1, 2], "no query lists all, in the table's order");
        assert_eq!(matches(&list, "43", false), [0, 1], "cities keep their order");
        assert_eq!(matches(&list, "PASIG", true), [1], "case is ignored");
        assert!(matches(&list, "zzz", true).is_empty());
    }

    #[test]
    fn a_title_typed_exactly_comes_first() {
        let cities = List::City.choices();
        let first = |q: &str| cities[matches(&cities, q, false)[0]].value.clone();
        assert_eq!(first("Quirino"), "Quirino, Isabela", "not Quirino (Angkaki), which comes first in the table");
        assert_eq!(first("bauan"), "Bauan, Batangas", "not Bauang");
        assert_eq!(first("Alaminos"), "Alaminos, Laguna", "not City of Alaminos");
    }

    #[test]
    fn a_zip_code_typed_leads_its_list() {
        let zips = List::Zip.choices();
        let lead: Vec<&str> = matches(&zips, "1101", true).iter().take(3).map(|&i| zips[i].title.as_str()).collect();
        assert_eq!(lead, ["1101 Botocan", "1101 Diliman", "1101 Krus na Ligas"]);
        assert!(matches(&zips, "diliman", true).iter().any(|&i| zips[i].title == "1101 Diliman"), "an area finds its code too");
        let rdos = List::Rdo.choices();
        assert_eq!(rdos[matches(&rdos, "43a", true)[0]].title, "43A · East Pasig");
    }

    #[test]
    fn leaving_keeps_the_pick_unless_the_text_names_an_entry() {
        let rdos = List::Rdo.choices();
        let had = Some("001".to_owned());
        assert_eq!(kept_on_leave(&rdos, "pasi", had.clone(), true), had, "typed but not picked is no value");
        assert_eq!(kept_on_leave(&rdos, "  ", had.clone(), true), None, "an emptied field clears");
        assert_eq!(kept_on_leave(&rdos, "43a", had.clone(), true).as_deref(), Some("43A"), "a code typed exactly picks it");
        assert_eq!(kept_on_leave(&rdos, "43A · East Pasig", None, true).as_deref(), Some("43A"));
        let cities = List::City.choices();
        assert_eq!(kept_on_leave(&cities, "city of pasig", None, false).as_deref(), Some("City of Pasig, Metro Manila"));
        assert_eq!(kept_on_leave(&cities, "San Isidro", had.clone(), false), had, "a name nine provinces share picks none");
        let zips = List::Zip.choices();
        assert_eq!(kept_on_leave(&zips, "1101", None, true), None, "a code several areas share picks none");
        assert_eq!(kept_on_leave(&zips, "1101 diliman", None, true).as_deref(), Some("1101 - Diliman"));
    }
}
