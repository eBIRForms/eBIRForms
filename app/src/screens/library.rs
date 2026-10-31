//! The Forms library: the selected taxpayer's saved returns, then every
//! form, searchable. A form's row opens its periods; a period opens (or
//! starts) that return.

use bir_app_core::display::{current_year, period_choices};
use bir_app_core::{FormCatalogEntry, OpenReturn, ReturnRow};
use nsh_base::input;
use nsh_core::prelude::*;

use crate::live::store;
use crate::store::{Catalog, Db, Loaded};
use crate::theme;
use crate::widgets::{Variant, button, button_sm, chip};

pub(crate) fn screen() -> impl IntoElement {
    let st = store();
    crate::screens::content(
        view()
            .gap(px(20.0))
            .child(if crate::mobile::phone() {
                // The phone's nav bar names the page: the search takes the row.
                search().w_full().into_element()
            } else {
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .child(crate::screens::page_title("Form library"))
                    .child(view().flex_1())
                    .child(search())
                    .into_element()
            })
            .child(when(move || matches!(st.db.get(), Db::Failed(_)), db_error))
            .child(when(move || matches!(st.db.get(), Db::Ready(_)) && st.selected.get().is_none(), no_taxpayer))
            .child(when(move || st.selected.get().is_some(), saved_returns))
            .child(catalog()),
    )
}

fn search() -> Input {
    let st = store();
    input()
        .value(st.library_query)
        .on_change(move |v: &str| st.library_query.set(v.to_owned()))
        .placeholder("Find a form, like 1601C or VAT")
        .placeholder_color(|| color(theme::FG3))
        .auto_correct(false)
        .w(px(280.0))
        .h(px(theme::CONTROL))
        .px(px(10.0))
        .text_size(px(13.0))
        .text_color(theme::FG)
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::BORDER_STRONG)
        .rounded(px(theme::R_ROW))
}

pub(crate) fn db_error() -> impl IntoElement {
    let st = store();
    view()
        .p(px(16.0))
        .gap(px(6.0))
        .rounded(px(theme::R_CARD))
        .bg(theme::DANGER_SOFT)
        .child(text("The database could not be opened").text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::DANGER))
        .child(
            text(move || match st.db.get() {
                Db::Failed(why) => why,
                _ => String::new(),
            })
            .text_size(px(12.0))
            .text_color(theme::FG),
        )
        .child(text("Check BUWIZ_DATABASE_PATH and the database key (BUWIZ_DB_KEY or the keychain), then restart eBIRForms. Another running copy also holds the database.").text_size(px(12.0)).text_color(theme::FG2))
}

fn no_taxpayer() -> impl IntoElement {
    let st = store();
    view()
        .p(px(16.0))
        .gap(px(10.0))
        .rounded(px(theme::R_CARD))
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::BORDER)
        .child(text("No taxpayer selected").text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
        .child(text("A return belongs to a taxpayer's branch. Add a taxpayer to open forms for it.").text_size(px(12.0)).text_color(theme::FG2))
        .child(view().flex_row().child(button("New taxpayer", Variant::Primary, || false, move || st.new_taxpayer_open.set(true))))
}

/// The selected branch's saved and filed returns.
fn saved_returns() -> impl IntoElement {
    let st = store();
    let rows = move || match (st.returns.get(), st.selected.get()) {
        (Loaded::Ready(rows), Some(sel)) => rows.into_iter().filter(|r| r.tin == sel.tin).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let count = move || rows().len();
    view()
        .gap(px(6.0))
        .child(section_title(move || format!("Saved returns · {}", count())))
        .child(when(move || count() == 0, || {
            text("None yet. Open a form below; it saves itself and shows here.").text_size(px(12.0)).text_color(theme::FG2)
        }))
        .child(
            view()
                .rounded(px(theme::R_CARD))
                .bg(theme::SURFACE)
                .border_1()
                .border_color(move || if count() == 0 { theme::BG } else { theme::BORDER })
                .overflow_hidden()
                .child(each(rows, |r| r.id, |row, index| return_row(row.get_untracked(), index))),
        )
}

fn return_row(row: ReturnRow, index: ReadSignal<usize>) -> impl IntoElement {
    let st = store();
    let open = OpenReturn {
        tin: row.tin.clone(),
        form_code: row.form_code.clone(),
        year: row.taxable_year,
        period: row.period,
        key: row.return_key,
    };
    let title = format!("{} · {}", row.form_code, row.period_label);
    view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .px(px(16.0))
        .py(px(8.0))
        .min_h(px(52.0))
        .border_t(move || px(if index.get() == 0 { 0.0 } else { 1.0_f32 }))
        .border_color(theme::BORDER)
        .cursor_pointer()
        .hover(|s| s.bg(theme::HOVER))
        .on_press(move || st.form.open(open.clone(), title.clone()))
        .child(text(row.form_code.clone()).w(px(72.0)).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).tabular_nums().text_color(theme::FG))
        .child(text(row.period_label.clone()).flex_1().flex_shrink(1.0).text_size(px(13.0)).text_color(theme::FG))
        .child(text(format!("edited {}", row.updated_at.get(..10).unwrap_or(&row.updated_at))).text_size(px(11.0)).text_color(theme::FG2))
        .child(chip(row.status.clone(), crate::widgets::status_tone(&row.status)))
}

fn section_title(label: impl IntoReactive<SharedString>) -> impl IntoElement {
    text(label).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG3)
}

fn normalized(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '-' | ' ')).collect::<String>().to_lowercase()
}

/// Every form, filtered by the search.
fn catalog() -> impl IntoElement {
    let st = store();
    let forms = move || match st.catalog.get() {
        Catalog::Ready(forms) => {
            let query = st.library_query.get();
            let q = query.trim().to_lowercase();
            let qn = normalized(&q);
            forms
                .into_iter()
                .filter(|f| q.is_empty() || normalized(&f.code).contains(&qn) || f.title.to_lowercase().contains(&q))
                .collect::<Vec<_>>()
        }
        _ => Vec::new(),
    };
    let status = move || match st.catalog.get() {
        Catalog::Loading => "Loading forms…".to_owned(),
        Catalog::Ready(all) => {
            let shown = forms().len();
            if shown == all.len() { format!("All forms · {shown}") } else { format!("Forms · {shown} of {}", all.len()) }
        }
        Catalog::Failed(error) => format!("Could not load the forms: {error}"),
    };
    view()
        .gap(px(6.0))
        .child(section_title(status))
        .child(
            view()
                .rounded(px(theme::R_CARD))
                .bg(theme::SURFACE)
                .border_1()
                .border_color(theme::BORDER)
                .overflow_hidden()
                .child(each(forms.clone(), |f| f.code.clone(), |form, index| form_row(form.get_untracked(), index))),
        )
        .child(when(move || matches!(st.catalog.get(), Catalog::Ready(_)) && forms().is_empty(), move || {
            text(move || format!("No form matches “{}”. Try a form code like 1601C, or a word like VAT.", st.library_query.get().trim())).text_size(px(13.0)).text_color(theme::FG3)
        }))
}

fn form_row(form: FormCatalogEntry, index: ReadSignal<usize>) -> impl IntoElement {
    let st = store();
    let code = form.code.clone();
    let picked = {
        let code = code.clone();
        move || st.library_pick.get().as_deref() == Some(code.as_str())
    };
    let pick = {
        let code = code.clone();
        move || {
            // A phone picks the period in the start-a-return sheet.
            if crate::mobile::phone() {
                if st.selected.get_untracked().is_some() {
                    st.start_sheet.set(Some(code.clone()));
                } else {
                    st.new_taxpayer_open.set(true);
                }
                return;
            }
            let now = st.library_pick.get_untracked();
            st.library_pick.set(if now.as_deref() == Some(code.as_str()) { None } else { Some(code.clone()) });
        }
    };
    let freq = form.frequency.clone().unwrap_or_default();
    let meta = match (&form.revision, form.pages) {
        (Some(rev), Some(pages)) => format!("{freq} · rev. {rev} · {pages} page{}", if pages == 1 { "" } else { "s" }),
        _ => freq.clone(),
    };
    let picked_bg = picked.clone();
    view()
        .border_t(move || px(if index.get() == 0 { 0.0 } else { 1.0_f32 }))
        .border_color(theme::BORDER)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .px(px(16.0))
                .py(px(8.0))
                .min_h(px(52.0))
                .cursor_pointer()
                .bg(move || if picked_bg() { theme::BG } else { theme::SURFACE })
                .hover(|s| s.bg(theme::HOVER))
                .on_press(pick)
                .child(text(code.clone()).w(px(72.0)).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).tabular_nums().text_color(theme::FG))
                .child(if crate::mobile::phone() {
                    // A phone stacks the title over its frequency and pages.
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(text(form.title.clone()).text_size(px(14.0)).text_color(theme::FG))
                        .child(text(meta).text_size(px(11.5)).text_color(theme::FG2))
                        .into_element()
                } else {
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .child(text(form.title.clone()).flex_1().flex_shrink(1.0).min_w(px(0.0)).text_size(px(13.0)).text_color(theme::FG))
                        .child(text(meta).text_size(px(11.0)).text_color(theme::FG2))
                        .into_element()
                }),
        )
        .child(when(picked, move || periods(form.clone())))
}

/// A form's years and periods; a period opens the return.
fn periods(form: FormCatalogEntry) -> impl IntoElement {
    let st = store();
    let freq = form.frequency.clone().unwrap_or_default();
    let this_year = current_year();
    let mut years = view().flex_row().gap(px(6.0)).items_center().child(text("Year").w(px(56.0)).text_size(px(11.0)).text_color(theme::FG2));
    for year in (this_year - 3..=this_year).rev() {
        years = years.child(
            view()
                .h(px(24.0))
                .px(px(9.0))
                .rounded(px(12.0))
                .justify_center()
                .cursor_pointer()
                .bg(move || if st.library_year.get() == year { theme::ACCENT_SOFT } else { theme::SURFACE })
                .border_1()
                .border_color(move || if st.library_year.get() == year { theme::ACCENT } else { theme::BORDER_STRONG })
                .on_press(move || st.library_year.set(year))
                .child(text(year.to_string()).text_size(px(12.0)).text_color(theme::FG)),
        );
    }
    let choices = {
        let freq = freq.clone();
        move || period_choices(&freq, st.library_year.get())
    };
    let code = form.code.clone();
    let sample_code = form.code.clone();
    view()
        .px(px(12.0))
        .pb(px(10.0))
        .pt(px(4.0))
        .pl(px(if crate::mobile::phone() { 16.0 } else { 96.0 }))
        .gap(px(8.0))
        .bg(theme::BG)
        .child(when(move || st.selected.get().is_some(), move || {
            let code = sample_code.clone();
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(text("Sample").w(px(56.0)).text_size(px(11.0)).text_color(theme::FG2))
                .child(button_sm("Open a sample", Variant::Ghost, None, || false, move || {
                    if let Some(sel) = st.selected.get_untracked() {
                        st.form.open_sample(sel.tin, code.clone());
                    }
                }))
                .child(text("Type in it and print it; nothing is saved.").text_size(px(11.0)).text_color(theme::FG3))
        }))
        .child(years)
        .child(
            view()
                .flex_row()
                .gap(px(6.0))
                .items_start()
                .child(text("Period").w(px(56.0)).pt(px(5.0)).text_size(px(11.0)).text_color(theme::FG2))
                .child(when(move || st.selected.get().is_none(), || {
                    text("Pick or add a taxpayer first (top of the sidebar).").pt(px(5.0)).text_size(px(12.0)).text_color(theme::DANGER)
                }))
                .child(when(move || st.selected.get().is_some(), move || {
                    let code = code.clone();
                    view().flex_row().flex_wrap().flex_1().gap(px(6.0)).child(each(
                        choices.clone(),
                        |c| c.label.clone(),
                        move |choice, _| {
                            let choice = choice.get_untracked();
                            let code = code.clone();
                            let label = choice.label.clone();
                            button_sm(choice.label.clone(), Variant::Secondary, None, || false, move || {
                                let Some(sel) = st.selected.get_untracked() else { return };
                                let open = OpenReturn {
                                    tin: sel.tin,
                                    form_code: code.clone(),
                                    year: st.library_year.get_untracked(),
                                    period: choice.period,
                                    key: None,
                                };
                                st.form.open(open, format!("{code} · {label}"));
                            })
                        },
                    ))
                })),
        )
}
