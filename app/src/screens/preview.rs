//! The print preview: the filled BIR sheets (bir-print's static HTML) in a
//! web view, the one place AGENTS.md allows one, with Print and Save as
//! PDF. On macOS both go through [`crate::mac::WebKitPdf`], bir-print's
//! `PdfSink` over WKWebView `createPDF`; elsewhere, and when that fails, the
//! sheet opens in the browser (the fallback P4a had).

use bir_app_core::{PrintedReturn, ReturnRow};
use nsh_core::prelude::*;

use crate::icons::{Icon, icon};
use crate::live::store;
use crate::screens::Route;
use crate::store::{NoticeTone, Preview, call, open_of, task};
use crate::theme;
use crate::widgets::{Variant, button_sm};

/// Show `printed` in the preview.
pub(crate) fn show(printed: PrintedReturn, form_code: &str) {
    let st = store();
    let p = Preview { title: printed.title, form_code: form_code.to_owned(), html: printed.html };
    st.preview.set(Some(p.clone()));
    st.go(Route::Preview);
    pdf_hook(p);
}

/// `BUWIZ_PDF_OUT=<file>` (a check hook, off otherwise): write the
/// preview's PDF, made by the platform's `PdfSink`, to that file.
fn pdf_hook(p: Preview) {
    let Some(out) = std::env::var("BUWIZ_PDF_OUT").ok().filter(|v| !v.is_empty()) else { return };
    task(async move {
        let made = blocking(move || -> Result<Vec<u8>, String> {
            #[cfg(target_os = "macos")]
            {
                use bir_app_core::bir_print::pdf_sink::PdfSink as _;
                crate::mac::WebKitPdf { form_code: p.form_code.clone() }.html_to_pdf(&p.html).map_err(|e| e.to_string())
            }
            #[cfg(not(target_os = "macos"))]
            {
                crate::mobile::printer().ok_or("no printer")?.html_to_pdf(&p.html)
            }
        })
        .await;
        match made {
            Ok(Ok(bytes)) => {
                let _ = std::fs::write(&out, &bytes);
                log::warn!("BUWIZ_PDF_OUT: wrote {} bytes to {out}", bytes.len());
            }
            Ok(Err(why)) => log::warn!("BUWIZ_PDF_OUT: {why}"),
            Err(why) => log::warn!("BUWIZ_PDF_OUT: {why}"),
        }
    });
}

/// Print a saved or filed return without opening its form.
pub(crate) fn print_row(row: ReturnRow) {
    let st = store();
    task(async move {
        let open = open_of(&row);
        let code = row.form_code.clone();
        let answer = call(move |app| {
            let opened = app.open_return(&open)?;
            app.print_html(&code, &opened.draft)
        })
        .await;
        match answer {
            Ok(printed) => show(printed, &row.form_code),
            Err(why) => st.notify(NoticeTone::Danger, format!("Could not print: {why}")),
        }
    });
}

/// A file name for the sheet: `BIR Form 1701Q — Juana Santos` reads
/// `BIR Form 1701Q - Juana Santos`.
fn file_stem(title: &str) -> String {
    title.chars().map(|c| if matches!(c, '/' | ':' | '\u{2014}') { '-' } else { c }).collect()
}

/// What the preview's buttons do.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    Print,
    SavePdf,
    Browser,
    /// A phone's share sheet with the PDF.
    Share,
}

pub(crate) fn run(act: Act, busy: Signal<bool>) {
    let st = store();
    let Some(p) = st.preview.get_untracked() else { return };
    if busy.get_untracked() {
        return;
    }
    busy.set(true);
    task(async move {
        let done = blocking(move || act_now(act, &p)).await.unwrap_or_else(|e| Err(e.to_string()));
        busy.set(false);
        match done {
            Ok(Some(note)) => st.notify(NoticeTone::Success, note),
            Ok(None) => {}
            Err(why) => st.notify(NoticeTone::Danger, why),
        }
    });
}

/// Run `act` on a worker; a note for the filer.
fn act_now(act: Act, p: &Preview) -> Result<Option<String>, String> {
    // A phone: the system print dialog, or the PDF in the share sheet.
    if let Some(printer) = crate::mobile::printer() {
        return match act {
            Act::Share => {
                let pdf = printer.html_to_pdf(&p.html).map_err(|e| format!("Could not make the PDF: {e}"))?;
                printer.share_pdf(&pdf, &file_stem(&p.title)).map_err(|e| format!("Could not share: {e}"))?;
                Ok(None)
            }
            _ => printer.print(&p.html, &p.title).map(|()| None).map_err(|e| format!("Could not print: {e}")),
        };
    }
    #[cfg(target_os = "macos")]
    {
        use bir_app_core::bir_print::pdf_sink::PdfSink as _;
        if act != Act::Browser {
            let sink = crate::mac::WebKitPdf { form_code: p.form_code.clone() };
            let pdf = sink.html_to_pdf(&p.html);
            match (act, pdf) {
                (Act::Print, Ok(bytes)) => {
                    let path = std::env::temp_dir().join(format!("{}.pdf", file_stem(&p.title)));
                    std::fs::write(&path, bytes).map_err(|e| format!("Could not write the PDF: {e}"))?;
                    crate::mac::print_pdf(&path).map_err(|e| format!("Could not print: {e}"))?;
                    return Ok(None);
                }
                (Act::SavePdf, Ok(bytes)) => {
                    let Some(path) = crate::mac::save_panel(&format!("{}.pdf", file_stem(&p.title)), "pdf") else { return Ok(None) };
                    std::fs::write(&path, bytes).map_err(|e| format!("Could not save the PDF: {e}"))?;
                    return Ok(Some(format!("Saved {}.", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())));
                }
                (_, Err(why)) => {
                    log::warn!("PDF failed, opening the browser instead: {why}");
                }
                _ => {}
            }
        }
    }
    let _ = act;
    open_in_browser(p)
}

/// The fallback: the sheet as an HTML file, opened in the system browser.
fn open_in_browser(p: &Preview) -> Result<Option<String>, String> {
    let path = std::env::temp_dir().join(format!("{}.html", file_stem(&p.title)));
    std::fs::write(&path, &p.html).map_err(|e| format!("Could not write the sheet: {e}"))?;
    #[cfg(target_os = "macos")]
    let opened = std::process::Command::new("open").arg(&path).status();
    #[cfg(not(target_os = "macos"))]
    let opened: std::io::Result<std::process::ExitStatus> = Err(std::io::Error::other("no browser"));
    match opened {
        Ok(_) => Ok(Some("The print sheet opened in your browser.".to_owned())),
        Err(_) => Ok(Some(format!("The print sheet is saved at {}.", path.display()))),
    }
}

/// On a phone the A4 sheets (794 CSS px wide) start fitted to the screen's
/// width, page by page; pinching zooms where the platform's web view allows
/// it. The desktop shows them at their size.
fn fit_to_phone(html: String) -> String {
    if !crate::mobile::phone() {
        return html;
    }
    let style = if os() == nsh_core::element::Os::Ios {
        // WKWebView scales an 820 px layout viewport to the screen and
        // pinches natively; text keeps its size (no autosizing).
        "<meta name=\"viewport\" content=\"width=820, user-scalable=yes, maximum-scale=4\"><style id=\"phone-fit\">@media screen{html{-webkit-text-size-adjust:100% !important}body{padding:8px 0 96px !important}.sheet{margin:0 auto 12px auto !important}}</style>".to_owned()
    } else {
        // Android's web view lays out at the device width: zoom the sheets.
        let width = untracked(|| window().width.0 - safe_area().left.0 - safe_area().right.0);
        let zoom = ((width - 16.0) / 794.0).clamp(0.2, 1.0);
        format!("<style id=\"phone-fit\">@media screen{{html{{zoom:{zoom:.4}}}body{{padding:8px 0 96px !important}}.sheet{{margin:0 auto 12px auto !important}}}}</style>")
    };
    match html.rfind("</head>") {
        Some(at) => format!("{}{style}{}", &html[..at], &html[at..]),
        None => format!("{style}{html}"),
    }
}

pub(crate) fn screen() -> impl IntoElement {
    let st = store();
    let busy = signal(false);
    let sheet = move || {
        each(move || vec![st.preview.get().map(|p| p.html.len())], |k| *k, move |_, _| match st.preview.get_untracked() {
            Some(p) => web_view_html(fit_to_phone(p.html), None).flex_1().into_element(),
            None => text("Nothing to preview.").m(px(24.0)).text_size(px(13.0)).text_color(theme::FG2).into_element(),
        })
    };
    if crate::mobile::phone() {
        return view()
            .flex_1()
            .bg(theme::BG)
            .child(view().flex_1().child(sheet()))
            .child(
                view()
                    .absolute()
                    .left(move || px(12.0 + safe_area().left.0))
                    .right(move || px(12.0 + safe_area().right.0))
                    .bottom(move || px(if safe_area().bottom.0 > 0.0 { safe_area().bottom.0 + 4.0 } else { 16.0 }))
                    .h(px(56.0))
                    .px(px(8.0))
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(px(22.0))
                    .bg(theme::ACCENT)
                    .shadow_lg()
                    .cursor_pointer()
                    .label("Print or save as PDF")
                    .active(|s| s.opacity(0.7))
                    .on_press(move || run(Act::Print, busy))
                    .child(view().size(px(40.0)).rounded(px(14.0)).items_center().justify_center().child(icon(Icon::Printer, 18.0, || theme::PRIMARY_FG)))
                    .child(
                        view()
                            .flex_1()
                            .child(text(move || st.preview.get().map(|p| p.title).unwrap_or_default()).text_size(px(11.0)).text_color(theme::PRIMARY_FG).opacity(0.75).truncate())
                            .child(text(move || if busy.get() { "Preparing…" } else { "Print or save as PDF" }).text_size(px(15.0)).font_weight(FontWeight::BOLD).text_color(theme::PRIMARY_FG)),
                    ),
            )
            .into_element();
    }
    view()
        .flex_1()
        .h_full()
        .child(
            view()
                .h(px(52.0))
                .px(px(16.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .border_b(px(1.0))
                .border_color(theme::BORDER)
                .child(button_sm("Back", Variant::Ghost, Some(Icon::ArrowLeft), || false, move || st.back()))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(text(move || st.preview.get().map(|p| p.title).unwrap_or_default()).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate())
                        .child(text(move || if busy.get() { "Preparing the PDF…" } else { "A4, one page per form page" }).text_size(px(11.0)).text_color(theme::FG2)),
                )
                .child(button_sm("Open in browser", Variant::Ghost, None, move || busy.get(), move || run(Act::Browser, busy)).label("Open in browser"))
                .child(button_sm("Save as PDF", Variant::Secondary, None, move || busy.get(), move || run(Act::SavePdf, busy)).label("Save as PDF"))
                .child(button_sm("Print", Variant::Primary, Some(Icon::Printer), move || busy.get(), move || run(Act::Print, busy)).label("Print sheet")),
        )
        .child(view().flex_1().bg(theme::BG).child(sheet()))
        .into_element()
}
