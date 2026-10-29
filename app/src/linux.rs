//! Linux glue the print preview and Settings need, reached from worker
//! threads: [`SheetPdf`] (bir-print's `PdfSink` over a WebKitGTK print
//! operation writing a PDF through GTK's file printer), the PDF to the
//! default handler, and GTK's file dialogs.
//!
//! GTK and WebKitGTK live on the main thread. Every function here runs on
//! a worker, posts its GTK work to the main context and waits for the
//! answer, so the main loop never blocks on WebKit's loading.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use bir_app_core::bir_print::pdf_sink::PdfSink;
use webkit6::prelude::*;
use webkit6::{LoadEvent, PrintOperation, WebView, gio, glib, gtk};

use crate::desktop::{A4, Page, PdfError, file_filter, is_pdf, temp_file, with_extension, with_print_css};

/// How long a sheet may take to load and print.
const PDF_TIMEOUT: Duration = Duration::from_secs(60);

/// GTK's print-to-file printer (its file backend).
const FILE_PRINTER: &str = "Print to File";

thread_local! {
    /// The hidden window, web view and print operation of the capture
    /// running on the main thread.
    static HELD: RefCell<Option<(gtk::Window, WebView, PrintOperation)>> = const { RefCell::new(None) };
}

/// bir-print's `PdfSink` on Linux: a hidden WebKitGTK web view loads the
/// sheet and a print operation writes it to a PDF file, A4, no margins,
/// one form page per A4 page (the sheet CSS's page breaks).
pub(crate) struct SheetPdf {
    pub form_code: String,
}

/// One capture at a time (one hidden web view).
static CAPTURE: Mutex<()> = Mutex::new(());

impl PdfSink for SheetPdf {
    type Error = PdfError;

    fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, PdfError> {
        let _one = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
        let out = temp_file(&self.form_code, "pdf");
        let uri = glib::filename_to_uri(&out, None).map_err(|e| PdfError(e.to_string()))?.to_string();
        let html = with_print_css(html);
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        glib::MainContext::default().invoke(move || start(html, uri, tx));
        let answer = rx.recv_timeout(PDF_TIMEOUT);
        glib::MainContext::default().invoke(|| {
            if let Some((window, _, _)) = HELD.with(|h| h.borrow_mut().take()) {
                window.destroy();
            }
        });
        let made = match answer {
            Ok(result) => result,
            Err(_) => Err("the sheet took too long to print".into()),
        };
        let bytes = made.and_then(|()| std::fs::read(&out).map_err(|e| format!("cannot read the PDF: {e}")));
        let _ = std::fs::remove_file(&out);
        let bytes = bytes.map_err(PdfError)?;
        if !is_pdf(&bytes) {
            return Err(PdfError("WebKitGTK wrote no PDF".into()));
        }
        Ok(bytes)
    }
}

/// On the main thread: load `html` in a hidden web view, print it to `uri`
/// once loaded; the outcome goes to `tx`.
fn start(html: String, uri: String, tx: mpsc::Sender<Result<(), String>>) {
    let web = WebView::new();
    let (w, h) = A4.css_px();
    web.set_size_request(w, h);
    let window = gtk::Window::new();
    window.set_default_size(w, h);
    window.set_child(Some(&web));

    let op = PrintOperation::new(&web);
    op.set_page_setup(&page_setup(&A4));
    op.set_print_settings(&print_settings(&A4, &uri));
    let failed = tx.clone();
    op.connect_failed(move |_, err| {
        let _ = failed.send(Err(format!("printing failed: {err}")));
    });
    // `finished` follows `failed` too; the worker takes the first answer.
    let finished = tx.clone();
    op.connect_finished(move |_| {
        let _ = finished.send(Ok(()));
    });
    let printer = op.downgrade();
    web.connect_load_changed(move |_, event| {
        if event == LoadEvent::Finished
            && let Some(op) = printer.upgrade()
        {
            op.print();
        }
    });
    web.connect_load_failed(move |_, _, _, err| {
        let _ = tx.send(Err(format!("the sheet did not load: {err}")));
        false
    });
    web.load_html(&html, None);
    HELD.with(|h| *h.borrow_mut() = Some((window, web, op)));
}

/// GTK's paper size for `page`: A4 by name when it is A4, else custom.
fn paper(page: &Page) -> gtk::PaperSize {
    if *page == A4 {
        gtk::PaperSize::new(Some(gtk::PAPER_NAME_A4.as_str()))
    } else {
        gtk::PaperSize::new_custom("buwiz", "Buwiz", page.width_mm(), page.height_mm(), gtk::Unit::Mm)
    }
}

/// A page setup for `page`: portrait, its paper and margins.
fn page_setup(page: &Page) -> gtk::PageSetup {
    let setup = gtk::PageSetup::new();
    setup.set_orientation(gtk::PageOrientation::Portrait);
    setup.set_paper_size(&paper(page));
    setup.set_top_margin(page.margin_pt, gtk::Unit::Points);
    setup.set_bottom_margin(page.margin_pt, gtk::Unit::Points);
    setup.set_left_margin(page.margin_pt, gtk::Unit::Points);
    setup.set_right_margin(page.margin_pt, gtk::Unit::Points);
    setup
}

/// Print settings that write a PDF of `page` to `uri` through GTK's file
/// printer, at scale 100, one copy.
fn print_settings(page: &Page, uri: &str) -> gtk::PrintSettings {
    let settings = gtk::PrintSettings::new();
    settings.set_printer(FILE_PRINTER);
    settings.set(gtk::PRINT_SETTINGS_OUTPUT_FILE_FORMAT.as_str(), Some("pdf"));
    settings.set(gtk::PRINT_SETTINGS_OUTPUT_URI.as_str(), Some(uri));
    settings.set_paper_size(&paper(page));
    settings.set_orientation(gtk::PageOrientation::Portrait);
    settings.set_scale(100.0);
    settings.set_n_copies(1);
    settings
}

/// Hands the PDF at `path` to the default PDF app (where Print is one
/// click); GTK has no print dialog for a PDF it cannot render.
pub(crate) fn print_pdf(path: &Path) -> Result<(), String> {
    let uri = glib::filename_to_uri(path, None).map_err(|e| e.to_string())?.to_string();
    let (tx, rx) = mpsc::channel();
    glib::MainContext::default().invoke(move || {
        let answer = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>)
            .map_err(|e| format!("no app opens PDF files: {e}"));
        let _ = tx.send(answer);
    });
    rx.recv_timeout(Duration::from_secs(10)).unwrap_or(Ok(()))
}

/// A save dialog suggesting `name` (with `ext`); the chosen path, `None`
/// when cancelled.
pub(crate) fn save_panel(name: &str, ext: &'static str) -> Option<PathBuf> {
    let name = with_extension(name, ext);
    let (tx, rx) = mpsc::channel();
    glib::MainContext::default().invoke(move || {
        let dialog = dialog(ext);
        dialog.set_initial_name(Some(&name));
        dialog.save(app_window().as_ref(), None::<&gio::Cancellable>, move |file| {
            let _ = tx.send(file.ok().and_then(|f| f.path()));
        });
    });
    rx.recv().ok().flatten()
}

/// An open dialog for one file with extension `ext`.
pub(crate) fn open_panel(ext: &'static str) -> Option<PathBuf> {
    let (tx, rx) = mpsc::channel();
    glib::MainContext::default().invoke(move || {
        dialog(ext).open(app_window().as_ref(), None::<&gio::Cancellable>, move |file| {
            let _ = tx.send(file.ok().and_then(|f| f.path()));
        });
    });
    rx.recv().ok().flatten()
}

/// A modal file dialog filtered to `ext`.
fn dialog(ext: &str) -> gtk::FileDialog {
    let (label, _) = file_filter(ext);
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&label));
    filter.add_suffix(ext);
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::new();
    dialog.set_modal(true);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&filter));
    dialog
}

/// The app's active window (the dialog's parent), else its first.
fn app_window() -> Option<gtk::Window> {
    let windows: Vec<gtk::Window> =
        gtk::Window::list_toplevels().into_iter().filter_map(|w| w.downcast::<gtk::Window>().ok()).filter(|w| w.is_visible()).collect();
    windows.iter().find(|w| w.is_active()).or(windows.first()).cloned()
}
