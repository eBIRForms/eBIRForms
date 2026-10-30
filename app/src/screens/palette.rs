//! The command palette (⌘K, bir-shell `chrome.js` `renderPalette`): one
//! search over taxpayers (a TIN typed as digits finds its branches), pages,
//! forms and the selected taxpayer's returns. ↑ ↓ move, Enter runs, Esc
//! closes.

use bir_app_core::OpenReturn;
use nsh_core::prelude::*;

use crate::icons::{Icon, icon};
use crate::live::store;
use crate::screens::Route;
use crate::store::{Catalog, Loaded};
use crate::theme;
use crate::widgets::{CLEAR, avatar};

/// What an item does.
#[derive(Clone, PartialEq, Debug)]
enum Run {
    Select(String),
    Go(Route),
    Form(String),
    Open(OpenReturn, String),
    NewTaxpayer,
}

#[derive(Clone, PartialEq, Debug)]
struct Item {
    group: &'static str,
    label: String,
    hint: String,
    icon: Icon,
    /// A taxpayer's avatar (name, id) instead of the icon.
    avatar: Option<(String, String)>,
    run: Run,
}

fn items(q: &str) -> Vec<Item> {
    let st = store();
    let q = q.trim().to_lowercase();
    let digits: String = q.chars().filter(|c| !matches!(c, '-' | ' ')).collect();
    let by_tin = !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
    let hit = |text: &str| q.is_empty() || text.to_lowercase().contains(&q);
    let mut out = Vec::new();
    if let Loaded::Ready(list) = st.taxpayers.get_untracked() {
        for t in &list {
            if by_tin {
                for b in t.branches.iter().filter(|b| b.tin.starts_with(&digits)) {
                    let label = if t.branches.len() > 1 { format!("{} · {}", t.name, b.name) } else { t.name.clone() };
                    out.push(Item { group: "Taxpayers", label, hint: b.tin_display.clone(), icon: Icon::User, avatar: Some((t.name.clone(), t.tin_root.clone())), run: Run::Select(b.tin.clone()) });
                }
            } else if hit(&t.name) {
                let head = t.branches.first().map(|b| b.tin.clone()).unwrap_or_default();
                out.push(Item { group: "Taxpayers", label: t.name.clone(), hint: t.tin_display.clone(), icon: Icon::User, avatar: Some((t.name.clone(), t.tin_root.clone())), run: Run::Select(head) });
            }
        }
    }
    for (route, i) in [
        (Route::Home, Icon::Home),
        (Route::Library, Icon::Library),
        (Route::Calendar, Icon::Calendar),
        (Route::Filings, Icon::FileText),
        (Route::Queue, Icon::Send),
        (Route::Taxpayer, Icon::Users),
        (Route::Settings, Icon::Settings),
    ] {
        if hit(route.label()) {
            out.push(Item { group: "Go to", label: route.label().to_owned(), hint: String::new(), icon: i, avatar: None, run: Run::Go(route) });
        }
    }
    if hit("new taxpayer") {
        out.push(Item { group: "Go to", label: "New taxpayer".to_owned(), hint: String::new(), icon: Icon::Plus, avatar: None, run: Run::NewTaxpayer });
    }
    if let Catalog::Ready(forms) = st.catalog.get_untracked() {
        let qn: String = digits.to_lowercase();
        for f in forms.iter().filter(|f| q.is_empty() || f.code.to_lowercase().replace('-', "").contains(&qn) || f.title.to_lowercase().contains(&q)).take(if q.is_empty() { 6 } else { 12 }) {
            out.push(Item { group: "Forms", label: format!("{} · {}", f.code, f.title), hint: f.frequency.clone().unwrap_or_default(), icon: Icon::FileText, avatar: None, run: Run::Form(f.code.clone()) });
        }
    }
    if let (Loaded::Ready(rows), Some(sel)) = (st.returns.get_untracked(), st.selected.get_untracked()) {
        for r in rows.iter().filter(|r| r.tin == sel.tin).filter(|r| hit(&format!("{} {}", r.form_code, r.period_label))).take(6) {
            let title = format!("{} · {}", r.form_code, r.period_label);
            out.push(Item { group: "Returns", label: title.clone(), hint: r.status.clone(), icon: Icon::Clock, avatar: None, run: Run::Open(crate::store::open_of(r), title) });
        }
    }
    out
}

fn run(item: &Item) {
    let st = store();
    st.palette_open.set(false);
    match &item.run {
        Run::Select(tin) => {
            st.select(tin);
            st.go(Route::Taxpayer);
        }
        Run::Go(route) => st.go(*route),
        Run::Form(code) => {
            st.library_query.set(code.clone());
            st.library_pick.set(Some(code.clone()));
            st.go(Route::Library);
        }
        Run::Open(open, title) => st.form.open(open.clone(), title.clone()),
        Run::NewTaxpayer => st.new_taxpayer_open.set(true),
    }
}

pub(crate) fn palette() -> impl IntoElement {
    let st = store();
    let query = signal(String::new());
    let index = signal(0usize);
    let list = move || items(&query.get());
    let list2 = list.clone();
    let list3 = list.clone();
    let close = move || st.palette_open.set(false);
    on_key_with("down", KeyOptions { when: nsh_core::keymap::When::Always, ..Default::default() }, move |_| {
        let n = untracked(&list2).len();
        index.update(|i| *i = (*i + 1).min(n.saturating_sub(1)));
    });
    on_key_with("up", KeyOptions { when: nsh_core::keymap::When::Always, ..Default::default() }, move |_| index.update(|i| *i = i.saturating_sub(1)));
    let enter = move || {
        let all = untracked(&list3);
        if let Some(item) = all.get(index.get_untracked().min(all.len().saturating_sub(1))) {
            run(item);
        }
    };
    let enter2 = enter.clone();
    on_key_with("enter", KeyOptions { when: nsh_core::keymap::When::Always, ..Default::default() }, move |_| enter2());
    on_key("escape", move |_| close());
    let rows = move || {
        let mut last = "";
        let mut out: Vec<(usize, Option<&'static str>, Item)> = Vec::new();
        for (i, item) in list().into_iter().enumerate() {
            let head = (item.group != last).then_some(item.group);
            last = item.group;
            out.push((i, head, item));
        }
        out
    };
    let rows2 = rows.clone();
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(50)
        .items_center()
        .pt(px(72.0))
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(close))
        .child(
            view()
                .w(move || px((window().width.0 - 32.0).min(560.0)))
                .rounded(px(theme::R_DIALOG))
                .bg(theme::SURFACE)
                .border_1()
                .border_color(theme::BORDER)
                .shadow_lg()
                .overflow_hidden()
                .child(
                    view()
                        .h(px(48.0))
                        .px(px(16.0))
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .border_b(px(1.0))
                        .border_color(theme::BORDER)
                        .child(icon(Icon::Search, 16.0, || theme::FG3))
                        .child(
                            nsh_base::input()
                                .value(query)
                                .on_change(move |v: &str| {
                                    query.set(v.to_owned());
                                    index.set(0);
                                })
                                .on_submit(move |_: &str| enter())
                                .placeholder("Search taxpayers, forms, returns or a TIN")
                                .placeholder_color(|| color(theme::FG3))
                                .auto_focus(true)
                                .auto_correct(false)
                                .flex_1()
                                .h(px(40.0))
                                .text_size(px(14.0))
                                .text_color(theme::FG)
                                .bg(CLEAR),
                        )
                        .child(text("Esc").px(px(6.0)).py(px(2.0)).rounded(px(4.0)).bg(theme::HOVER).text_size(px(11.0)).text_color(theme::FG3)),
                )
                .child(
                    scroll().max_h(px(420.0)).child(
                        view()
                            .p(px(12.0))
                            .gap(px(2.0))
                            .child(when(move || rows2().is_empty(), || view().py(px(40.0)).items_center().child(text("No matches. Try a form code like 2550Q.").text_size(px(13.0)).text_color(theme::FG3))))
                            .child(each(rows, |r| (r.0, r.2.label.clone()), move |r, _| {
                                let (i, head, item) = r.get_untracked();
                                let picked = item.clone();
                                let lead: AnyElement = match &item.avatar {
                                    Some((name, id)) => avatar(name, id, 20.0).into_element(),
                                    None => icon(item.icon, 16.0, || theme::FG2),
                                };
                                view()
                                    .child(when(head.is_some(), move || text(head.unwrap_or("").to_uppercase()).pt(px(10.0)).pb(px(4.0)).px(px(10.0)).text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG3)))
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(10.0))
                                            .min_h(px(36.0))
                                            .py(px(6.0))
                                            .px(px(10.0))
                                            .rounded(px(theme::R_ROW))
                                            .cursor_pointer()
                                            .label(item.label.clone())
                                            .bg(move || if index.get() == i { theme::HOVER } else { CLEAR })
                                            .hover(|s| s.bg(theme::HOVER))
                                            .on_press(move || run(&picked))
                                            .child(lead)
                                            .child(text(item.label.clone()).flex_1().min_w(px(0.0)).text_size(px(13.0)).text_color(theme::FG).truncate())
                                            .child(text(item.hint.clone()).text_size(px(12.0)).tabular_nums().text_color(theme::FG3)),
                                    )
                            })),
                    ),
                ),
        )
}
