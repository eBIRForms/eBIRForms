//! `Picker`: a field whose value comes from a long list (the New taxpayer
//! dialog's city, ZIP code and RDO), on nsh's `Combobox`. Closed, it looks
//! like a text field holding the pick; open, the list floats under it (a
//! sheet on phones) with a search above five rows, so the dialog never
//! grows. The search matches titles, so a title carries what a filer
//! types: "1101 Diliman", "43A · East Pasig".

use std::collections::HashMap;
use std::rc::Rc;

use bir_app_core::Choice;
use nsh_base::{ComboboxEvent, SelectItem, SelectItemState};
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::icons::{Icon, icon};
use crate::theme;
use crate::widgets::focus_ring;

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

/// `choices` in the order the list shows them for `query`: with
/// `code_first`, the ones whose title starts with it first ("43" puts 43A
/// before 043), the rest as they come. Search itself is the combobox's (a
/// substring of the title), so nothing is dropped here.
pub(crate) fn ordered(choices: &[Choice], query: &str, code_first: bool) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<usize> = (0..choices.len()).collect();
    if code_first && !q.is_empty() {
        out.sort_by_key(|&i| !choices[i].title.to_lowercase().starts_with(&q));
    }
    out
}

/// A picker's state the dialog reads and drives: whether its list is open,
/// whether the field has the keyboard focus, and its query.
#[derive(Clone, Copy)]
pub(crate) struct Picker {
    pub(crate) open: Signal<bool>,
    pub(crate) focused: Signal<bool>,
    query: Signal<SharedString>,
    /// A letter typed on the closed field: the query once the list shows
    /// (set as it opens, the list would not highlight its first match).
    typed: Signal<Option<String>>,
}

impl Picker {
    pub(crate) fn new() -> Self {
        Self { open: signal(false), focused: signal(false), query: signal(SharedString::default()), typed: signal(None) }
    }

    /// Opens the list searching `query`, as typing it on the field would.
    pub(crate) fn search(self, query: &str) {
        self.typed.set(Some(query.to_owned()));
        self.query.set(SharedString::default());
        self.open.set(true);
    }
}

/// The picker field over `value` (a [`Choice::value`], or none): `phone`
/// draws it as a row of the phone's form, `popup_w` is the desktop list's
/// width. `left` runs when the filer leaves it, by Tab or by closing the
/// list, so the dialog can say what is missing.
pub(crate) fn picker(list: List, value: Signal<Option<String>>, state: Picker, phone: bool, popup_w: f32, left: impl Fn() + 'static) -> impl IntoElement {
    let choices = Rc::new(list.choices());
    let by_value: Rc<HashMap<String, Choice>> = Rc::new(choices.iter().map(|c| (c.value.clone(), c.clone())).collect());
    let Picker { open, focused, query, typed } = state;
    let left = Rc::new(left);

    let items = {
        let choices = choices.clone();
        move || {
            let q = query.get();
            ordered(&choices, &q, list.code_first()).into_iter().map(|i| SelectItem::new(choices[i].value.clone(), choices[i].title.clone())).collect::<Vec<_>>()
        }
    };

    // Typed on the closed, focused field: the letter opens the list and
    // starts its search; Backspace or Delete clears the pick.
    effect(move || {
        if !focused.get() || open.get() {
            return;
        }
        let keys = KeyOptions { when: When::NotEditing, repeat: false };
        for c in ('a'..='z').chain('0'..='9') {
            on_key_with(&c.to_string(), keys, move |_| {
                typed.set(Some(c.to_string()));
                query.set(SharedString::default());
                open.set(true);
            });
        }
        for key in ["backspace", "delete"] {
            on_key_with(key, keys, move |_| value.set(None));
        }
    });
    // Tab in the open list's search closes it, the pick kept; the focus
    // goes back to the field, and the next Tab moves on.
    effect(move || {
        if !open.get() || phone {
            return;
        }
        for key in ["tab", "shift+tab"] {
            on_key_with(key, KeyOptions { when: When::Always, repeat: false }, move |_| open.set(false));
        }
    });

    let shown = by_value.clone();
    let trigger_text = move || value.get().and_then(|v| shown.get(&v).cloned());
    let trigger = if phone { phone_trigger(list, Rc::new(trigger_text)) } else { desktop_trigger(list, Rc::new(trigger_text), value) };
    let trigger = {
        let left = left.clone();
        trigger.on_focus(move || focused.set(true)).on_blur(move || {
            focused.set(false);
            // Leaving by Tab (opening the list moves the focus into its
            // search, which is not leaving).
            if !open.get_untracked() {
                left();
            }
        })
    };

    let details = by_value.clone();
    let row = move |s: &SelectItemState| {
        let detail = details.get(s.value.as_ref()).map(|c| c.detail.clone()).unwrap_or_default();
        list_row(s.title.to_string(), detail, s.selected, s.highlighted, phone)
    };

    let mut combo = nsh_base::combobox()
        .items(items)
        .value(move || Some(value.get().into_iter().map(SharedString::from).collect::<Vec<_>>()))
        .open(move || Some(open.get()))
        .query(move || Some(query.get()))
        .searchable(true)
        .cleanable(true)
        .search_placeholder(list.placeholder())
        .accessibility_label(list.label())
        .clear_button_accessibility_label(format!("Clear the {}", list.label().to_lowercase()))
        .clear_button(text("✕").text_size(px(12.0)).text_color(theme::FG2))
        .trigger(trigger)
        .render_item(row)
        .empty(text(move || list.empty(&query.get())).px(px(12.0)).py(px(10.0)).text_size(px(13.0)).text_color(theme::FG3))
        .on_change(move |v: &Vec<SharedString>| value.set(v.first().map(|s| s.to_string())))
        .on_query_change(move |q: &SharedString| query.set(q.clone()))
        .on_open_change(move |e: &ComboboxEvent| {
            if let ComboboxEvent::OpenChange(next, _) = e {
                open.set(*next);
                if !*next {
                    query.set(SharedString::default());
                    typed.set(None);
                    left();
                }
            }
        })
        // The list is shown: the letter that opened it becomes the query,
        // which highlights the first match, so Return picks it.
        .on_open(move || {
            if let Some(letter) = typed.get_untracked() {
                typed.set(None);
                query.set(letter.into());
            }
        })
        .placement(Placement::Bottom)
        .align(Align::Start)
        .offset(px(4.0))
        .relative()
        .w_full();
    combo = if phone {
        combo
            .popup_style(|s| s.bg(theme::SURFACE).rounded_t(px(14.0)).pt(px(8.0)).pb(px(24.0)).max_h(relative(0.7)))
            .backdrop_style(|s| s.bg(theme::SCRIM))
            .search_style(|s| s.h(px(44.0)).mx(px(16.0)).my(px(8.0)).px(px(14.0)).rounded(px(12.0)).bg(theme::HOVER).text_size(px(16.0)).text_color(theme::FG))
            .clear_button_style(|s| s.absolute().top(px(10.0)).right(px(18.0)).size(px(44.0)).items_center().justify_center())
    } else {
        combo
            // Five rows under a 40 px search (and the 1 px border round
            // them); the rest scroll.
            .popup_style(move |s| {
                s.w(px(popup_w))
                    .min_w(px(popup_w))
                    .max_h(px(5.0 * theme::CONTROL + 40.0 + 2.0))
                    .bg(theme::RAISED)
                    .border_1()
                    .border_color(theme::BORDER)
                    .rounded(px(theme::R_ROW))
                    .shadow_lg()
                    .overflow_hidden()
            })
            .search_style(|s| s.h(px(40.0)).px(px(10.0)).border_b(px(1.0)).border_color(theme::BORDER).text_size(px(13.0)).text_color(theme::FG))
            .clear_button_style(|s| s.absolute().top(px(4.0)).right(px(4.0)).size(px(24.0)).items_center().justify_center().rounded(px(theme::R_CTL)).cursor_pointer())
    };
    combo
}

/// The desktop's closed field: a text field's look holding the pick's
/// title and detail, or the placeholder; room on the right for the clear
/// button while there is a pick.
fn desktop_trigger(list: List, pick: Rc<dyn Fn() -> Option<Choice>>, value: Signal<Option<String>>) -> View {
    let (title, detail) = (pick.clone(), pick);
    view()
        .h(px(theme::CONTROL))
        .w_full()
        .pl(px(10.0))
        .pr(move || px(if value.get().is_some() { 32.0 } else { 10.0 }))
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::BORDER_STRONG)
        .rounded(px(theme::R_ROW))
        .cursor_pointer()
        .focus(focus_ring)
        .child(
            text(move || title().map(|c| c.title).unwrap_or_else(|| list.placeholder().to_owned()))
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .text_size(px(13.0))
                .text_color(move || if value.get().is_some() { theme::FG } else { theme::FG3 })
                .truncate(),
        )
        .child(
            text(move || detail().map(|c| if c.detail.is_empty() { String::new() } else { format!("· {}", c.detail) }).unwrap_or_default())
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .text_size(px(12.0))
                .text_color(theme::FG2)
                .truncate(),
        )
}

/// The phone's closed field: a row of the form, the label over the pick.
fn phone_trigger(list: List, pick: Rc<dyn Fn() -> Option<Choice>>) -> View {
    let (title, has) = (pick.clone(), pick);
    view()
        .min_h(px(64.0))
        .px(px(18.0))
        .py(px(10.0))
        .pr(px(64.0))
        .justify_center()
        .cursor_pointer()
        .child(text(list.label()).text_size(px(13.0)).text_color(theme::FG2))
        .child(
            text(move || title().map(|c| if c.detail.is_empty() { c.title } else { format!("{} · {}", c.title, c.detail) }).unwrap_or_else(|| list.placeholder().to_owned()))
                .text_size(px(17.0))
                .text_color(move || if has().is_some() { theme::FG } else { theme::FG3 })
                .truncate(),
        )
}

/// A row of the open list: the title and its detail, a check on the pick,
/// the highlight (the keys or the pointer) tinted.
fn list_row(title: String, detail: String, selected: bool, highlighted: bool, phone: bool) -> View {
    view()
        .h(px(if phone { 48.0 } else { theme::CONTROL }))
        .px(px(if phone { 18.0 } else { 10.0 }))
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .bg(if highlighted { theme::ACCENT_SOFT } else { crate::widgets::CLEAR })
        .child(text(title).flex_shrink(1.0).min_w(px(0.0)).text_size(px(if phone { 15.0 } else { 13.0 })).text_color(theme::FG).truncate())
        .child(text(detail).flex_1().min_w(px(0.0)).text_size(px(12.0)).text_color(theme::FG2).truncate())
        .child(if selected { icon(Icon::Check, 16.0, || theme::ACCENT) } else { view().size(px(16.0)).into_element() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(title: &str) -> Choice {
        Choice { value: title.to_owned(), title: title.to_owned(), detail: String::new() }
    }

    #[test]
    fn a_code_typed_puts_the_codes_it_starts_first() {
        let list: Vec<Choice> = ["043", "43A · East Pasig", "044 · Taguig"].into_iter().map(choice).collect();
        assert_eq!(ordered(&list, "43", true), [1, 0, 2]);
        assert_eq!(ordered(&list, "", true), [0, 1, 2], "no query keeps the table's order");
        assert_eq!(ordered(&list, "43", false), [0, 1, 2], "cities keep theirs");
    }

    #[test]
    fn a_zip_code_typed_leads_its_list() {
        let zips = List::Zip.choices();
        let order = ordered(&zips, "1101", true);
        let lead: Vec<&str> = order.iter().take(3).map(|&i| zips[i].title.as_str()).collect();
        assert_eq!(lead, ["1101 Botocan", "1101 Diliman", "1101 Krus na Ligas"]);
        assert!(ordered(&zips, "diliman", true).iter().any(|&i| zips[i].title == "1101 Diliman"), "an area finds its code too");
        let rdos = List::Rdo.choices();
        assert_eq!(rdos[ordered(&rdos, "43a", true)[0]].title, "43A · East Pasig");
    }
}
