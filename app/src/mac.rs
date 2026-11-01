//! macOS glue the print preview and Settings need, reached from worker
//! threads: [`WebKitPdf`] (bir-print's `PdfSink` over WKWebView
//! `createPDF`, ported from bir-shell's `print_preview_window.rs`), the
//! system print panel for a PDF, and save / open panels.
//!
//! AppKit and WebKit live on the main thread. Every function here runs on a
//! worker (`blocking`), posts its AppKit work to the main queue and waits
//! for the answer, so neither the engine nor the main thread ever blocks on
//! WebKit's loading.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bir_app_core::bir_print::html_output::{PdfExpectation, merge_single_page_pdfs, normalize_wkpdf_page_from_css_pixels};
use bir_app_core::bir_print::pdf_sink::PdfSink;
use bir_app_core::bir_print::print_sheet::{PAPER_HEIGHT_PT, PAPER_WIDTH_PT};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::msg_send;
use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSSize, NSString, NSURL};

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "PDFKit", kind = "framework")]
unsafe extern "C" {}

/// CSS pixels per PDF point: WebKit captures one point per CSS pixel.
const CSS_PER_PT: f64 = 96.0 / 72.0;

/// Pins every sheet to A4 with no screen gaps, so sheet `i` starts at
/// `i` × the paper height and each capture is exactly one page.
const CAPTURE_CSS: &str = "<style id=\"pdf-capture\">html,body{width:595.28pt !important;margin:0 !important;padding:0 !important;background:#fff !important}\
.sheet{margin:0 !important;box-shadow:none !important;height:841.89pt !important}.sheet::after{content:none !important}</style>";

thread_local! {
    /// The hidden web view of the capture running on the main thread.
    static WEB: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

/// Why a PDF could not be made.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct PdfError(String);

/// bir-print's `PdfSink` on macOS: a hidden WKWebView loads the sheet, then
/// `createPDF` captures each A4 sheet as one page; bir-print normalizes the
/// pages from CSS pixels to points and merges them.
pub(crate) struct WebKitPdf {
    pub form_code: String,
}

/// One capture at a time (the hidden web view is shared).
static CAPTURE: Mutex<()> = Mutex::new(());

impl PdfSink for WebKitPdf {
    type Error = PdfError;

    fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, PdfError> {
        let _one = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
        let html = match html.rfind("</head>") {
            Some(at) => format!("{}{CAPTURE_CSS}{}", &html[..at], &html[at..]),
            None => format!("{CAPTURE_CSS}{html}"),
        };
        let pages = capture(html);
        DispatchQueue::main().exec_sync(|| WEB.with(|w| w.borrow_mut().take()).map_or((), drop));
        let pages = pages?;
        let expectation = PdfExpectation {
            form_code: self.form_code.clone(),
            revision: "preview".to_owned(),
            envelope_hash: "0".repeat(64),
            expected_page_count: pages.len(),
            // WebKit captures whole CSS pixels: the page is the A4 rect
            // rounded down (793 × 1122 px, 594.75 × 841.5 pt).
            width_points: capture_size().0 / CSS_PER_PT,
            height_points: capture_size().1 / CSS_PER_PT,
        };
        let normalized = pages
            .iter()
            .map(|page| normalize_wkpdf_page_from_css_pixels(page, &expectation))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PdfError(format!("a page did not come out as A4: {e}")))?;
        merge_single_page_pdfs(&normalized).map_err(|e| PdfError(e.to_string()))
    }
}

/// The capture rect's size in CSS pixels: A4, rounded down to whole
/// pixels as WebKit emits it.
fn capture_size() -> (f64, f64) {
    ((f64::from(PAPER_WIDTH_PT) * CSS_PER_PT).floor(), (f64::from(PAPER_HEIGHT_PT) * CSS_PER_PT).floor())
}

/// Load `html` in a hidden web view and capture each sheet.
fn capture(html: String) -> Result<Vec<Vec<u8>>, PdfError> {
    let (w, h) = (f64::from(PAPER_WIDTH_PT) * CSS_PER_PT, f64::from(PAPER_HEIGHT_PT) * CSS_PER_PT);
    DispatchQueue::main().exec_sync(move || {
        // SAFETY: WebKit's documented API, on the main thread.
        unsafe {
            let Some(class) = AnyClass::get(c"WKWebView") else { return };
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h));
            let web: Retained<AnyObject> = msg_send![class, new];
            let _: () = msg_send![&web, setFrame: frame];
            let page = NSString::from_str(&html);
            let _: Option<Retained<AnyObject>> = msg_send![&web, loadHTMLString: &*page, baseURL: Option::<&NSURL>::None];
            WEB.with(|cell| *cell.borrow_mut() = Some(web));
        }
    });
    // Loaded: not loading and the progress at its end.
    let started = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(60));
        let (tx, rx) = mpsc::channel();
        DispatchQueue::main().exec_sync(move || {
            let ready = WEB.with(|cell| {
                cell.borrow().as_ref().map(|web| unsafe {
                    let loading: Bool = msg_send![&**web, isLoading];
                    let progress: f64 = msg_send![&**web, estimatedProgress];
                    !loading.as_bool() && progress >= 1.0
                })
            });
            let _ = tx.send(ready);
        });
        match rx.recv().ok().flatten() {
            None => return Err(PdfError("WebKit is not available".into())),
            Some(true) => break,
            Some(false) if started.elapsed() > Duration::from_secs(20) => return Err(PdfError("the sheet took too long to load".into())),
            Some(false) => {}
        }
    }
    let count = eval_number("document.querySelectorAll('.sheet').length")?.max(1.0) as usize;
    let mut pages = Vec::with_capacity(count);
    for i in 0..count {
        let (tx, rx) = mpsc::channel::<Result<Vec<u8>, String>>();
        DispatchQueue::main().exec_sync(move || {
            WEB.with(|cell| {
                let Some(web) = cell.borrow().clone() else {
                    let _ = tx.send(Err("the web view went away".into()));
                    return;
                };
                // SAFETY: WebKit's documented API, on the main thread; the
                // block runs on the main thread once, with NSData or an error.
                unsafe {
                    let Some(class) = AnyClass::get(c"WKPDFConfiguration") else {
                        let _ = tx.send(Err("WKPDFConfiguration is missing (macOS 11 or later)".into()));
                        return;
                    };
                    let config: Retained<AnyObject> = msg_send![class, new];
                    let (cw, ch) = capture_size();
                    let rect = NSRect::new(NSPoint::new(0.0, (h * i as f64).round()), NSSize::new(cw, ch));
                    let _: () = msg_send![&config, setRect: rect];
                    let done = RcBlock::new(move |data: *mut NSData, error: *mut AnyObject| {
                        let answer = match data.as_ref() {
                            Some(data) => Ok(data.to_vec()),
                            None if !error.is_null() => {
                                let text: Option<Retained<NSString>> = msg_send![&*error, localizedDescription];
                                Err(text.map(|t| t.to_string()).unwrap_or_else(|| "createPDF failed".into()))
                            }
                            None => Err("createPDF gave nothing".into()),
                        };
                        let _ = tx.send(answer);
                    });
                    let _: () = msg_send![&*web, createPDFWithConfiguration: &*config, completionHandler: &*done];
                }
            });
        });
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Ok(bytes)) => pages.push(bytes),
            Ok(Err(why)) => return Err(PdfError(format!("page {}: {why}", i + 1))),
            Err(_) => return Err(PdfError(format!("page {} timed out", i + 1))),
        }
    }
    Ok(pages)
}

/// Evaluate `script` in the hidden web view; its number.
fn eval_number(script: &'static str) -> Result<f64, PdfError> {
    let (tx, rx) = mpsc::channel::<Option<f64>>();
    DispatchQueue::main().exec_sync(move || {
        WEB.with(|cell| {
            let Some(web) = cell.borrow().clone() else {
                let _ = tx.send(None);
                return;
            };
            // SAFETY: WebKit's documented API on the main thread; a number
            // result is an NSNumber.
            unsafe {
                let js = NSString::from_str(script);
                let done = RcBlock::new(move |result: *mut AnyObject, _error: *mut AnyObject| {
                    let n = result.as_ref().map(|r| {
                        let v: f64 = msg_send![r, doubleValue];
                        v
                    });
                    let _ = tx.send(n);
                });
                let _: () = msg_send![&*web, evaluateJavaScript: &*js, completionHandler: &*done];
            }
        });
    });
    rx.recv_timeout(Duration::from_secs(10)).ok().flatten().ok_or_else(|| PdfError("the sheet did not answer".into()))
}

/// The system print panel for the PDF at `path`, A4, no scaling, as a sheet
/// of the key window. Returns once the panel is up.
pub(crate) fn print_pdf(path: &Path) -> Result<(), String> {
    let path = path.display().to_string();
    let (tx, rx) = mpsc::channel();
    DispatchQueue::main().exec_async(move || {
        // SAFETY: AppKit and PDFKit's documented API, on the main thread.
        let answer = unsafe {
            (|| -> Result<(), String> {
                let doc_class = AnyClass::get(c"PDFDocument").ok_or("PDFKit is missing")?;
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
                let alloc: objc2::rc::Allocated<AnyObject> = msg_send![doc_class, alloc];
                let doc: Option<Retained<AnyObject>> = msg_send![alloc, initWithURL: &*url];
                let doc = doc.ok_or("the PDF did not open")?;
                let info_class = AnyClass::get(c"NSPrintInfo").ok_or("no NSPrintInfo")?;
                let info: Retained<AnyObject> = msg_send![info_class, new];
                let _: () = msg_send![&info, setPaperSize: NSSize::new(f64::from(PAPER_WIDTH_PT), f64::from(PAPER_HEIGHT_PT))];
                let _: () = msg_send![&info, setTopMargin: 0.0f64];
                let _: () = msg_send![&info, setBottomMargin: 0.0f64];
                let _: () = msg_send![&info, setLeftMargin: 0.0f64];
                let _: () = msg_send![&info, setRightMargin: 0.0f64];
                // kPDFPrintPageScaleNone = 0.
                let op: Option<Retained<AnyObject>> = msg_send![&doc, printOperationForPrintInfo: &*info, scalingMode: 0isize, autoRotate: Bool::NO];
                let op = op.ok_or("PDFKit gave no print operation")?;
                let _: () = msg_send![&op, setShowsPrintPanel: Bool::YES];
                let app_class = AnyClass::get(c"NSApplication").ok_or("no NSApplication")?;
                let app: Retained<AnyObject> = msg_send![app_class, sharedApplication];
                let window: Option<Retained<AnyObject>> = msg_send![&app, keyWindow];
                let window = match window {
                    Some(w) => Some(w),
                    None => msg_send![&app, mainWindow],
                };
                match window {
                    Some(window) => {
                        let none: *const AnyObject = std::ptr::null();
                        let _: () = msg_send![&op, runOperationModalForWindow: &*window, delegate: none, didRunSelector: Option::<objc2::runtime::Sel>::None, contextInfo: std::ptr::null_mut::<std::ffi::c_void>()];
                    }
                    None => {
                        let _: Bool = msg_send![&op, runOperation];
                    }
                }
                Ok(())
            })()
        };
        let _ = tx.send(answer);
    });
    rx.recv_timeout(Duration::from_secs(10)).unwrap_or(Ok(()))
}

/// A save panel suggesting `name` (with `ext`); the chosen path, `None`
/// when cancelled.
pub(crate) fn save_panel(name: &str, ext: &'static str) -> Option<PathBuf> {
    let name = name.to_owned();
    let (tx, rx) = mpsc::channel();
    DispatchQueue::main().exec_async(move || {
        // SAFETY: AppKit's documented NSSavePanel API, on the main thread.
        let path = unsafe {
            AnyClass::get(c"NSSavePanel").and_then(|class| {
                let panel: Retained<AnyObject> = msg_send![class, savePanel];
                let _: () = msg_send![&panel, setNameFieldStringValue: &*NSString::from_str(&name)];
                let types = NSArray::from_retained_slice(&[NSString::from_str(ext)]);
                let _: () = msg_send![&panel, setAllowedFileTypes: &*types];
                let answer: isize = msg_send![&panel, runModal];
                if answer != 1 {
                    return None;
                }
                let url: Option<Retained<NSURL>> = msg_send![&panel, URL];
                url.and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string()))
            })
        };
        let _ = tx.send(path);
    });
    rx.recv().ok().flatten()
}

/// An open panel for one file with extension `ext`.
pub(crate) fn open_panel(ext: &'static str) -> Option<PathBuf> {
    let (tx, rx) = mpsc::channel();
    DispatchQueue::main().exec_async(move || {
        // SAFETY: AppKit's documented NSOpenPanel API, on the main thread.
        let path = unsafe {
            AnyClass::get(c"NSOpenPanel").and_then(|class| {
                let panel: Retained<AnyObject> = msg_send![class, openPanel];
                let _: () = msg_send![&panel, setCanChooseFiles: Bool::YES];
                let _: () = msg_send![&panel, setCanChooseDirectories: Bool::NO];
                let types = NSArray::from_retained_slice(&[NSString::from_str(ext)]);
                let _: () = msg_send![&panel, setAllowedFileTypes: &*types];
                let answer: isize = msg_send![&panel, runModal];
                if answer != 1 {
                    return None;
                }
                let urls: Retained<NSArray<NSURL>> = msg_send![&panel, URLs];
                urls.iter().next().and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string()))
            })
        };
        let _ = tx.send(path);
    });
    rx.recv().ok().flatten()
}

/// Keep the system from napping the app (timers coalesced, frames slowed)
/// while its window is hidden, for a measurement that must run at full
/// rate; held until the process ends.
pub(crate) fn hold_off_app_nap(reason: &str) {
    // NSActivityUserInitiated | NSActivityLatencyCritical.
    const OPTIONS: u64 = 0x00FF_FFFF | 0xFF_0000_0000;
    let reason = NSString::from_str(reason);
    // SAFETY: NSProcessInfo's documented API; the token is kept (leaked) so
    // the activity lasts for the process.
    unsafe {
        let Some(class) = AnyClass::get(c"NSProcessInfo") else { return };
        let info: Retained<AnyObject> = msg_send![class, processInfo];
        let token: Option<Retained<AnyObject>> = msg_send![&info, beginActivityWithOptions: OPTIONS, reason: &*reason];
        std::mem::forget(token);
    }
}

/// `BUWIZ_OFFSCREEN=1` (screenshot scripts): keep the app's windows off
/// every screen, at their size, so a run never shows on the screen someone
/// is using even when the window manager has not picked the window up yet.
/// `screencapture -l` still captures them. Repeats for a few seconds, as
/// the window may come after the root mounts; a tiling window manager can
/// resize the window before the script floats it, so the size is put back.
pub(crate) fn park_offscreen_for_a_while() {
    let (w, h) = crate::window_size();
    let size = NSSize::new(f64::from(w), f64::from(h));
    std::thread::spawn(move || {
        for _ in 0..40 {
            DispatchQueue::main().exec_async(move || {
                // SAFETY: AppKit's documented NSApplication / NSWindow API,
                // on the main thread.
                unsafe {
                    let Some(class) = AnyClass::get(c"NSApplication") else { return };
                    let app: Retained<AnyObject> = msg_send![class, sharedApplication];
                    let windows: Retained<NSArray<AnyObject>> = msg_send![&app, windows];
                    for w in windows.iter() {
                        let frame: NSRect = msg_send![&*w, frame];
                        // The main window only (the print preview is a window too).
                        let title: Retained<NSString> = msg_send![&*w, title];
                        let content: NSRect = msg_send![&*w, contentRectForFrameRect: frame];
                        if title.to_string() == crate::APP_NAME && (content.size.width != size.width || content.size.height != size.height) {
                            let _: () = msg_send![&*w, setContentSize: size];
                        }
                        let frame: NSRect = msg_send![&*w, frame];
                        if frame.origin.x > -10000.0 {
                            let _: () = msg_send![&*w, setFrameOrigin: NSPoint::new(-20000.0, frame.origin.y)];
                        }
                    }
                }
            });
            std::thread::sleep(Duration::from_millis(100));
        }
    });
}
