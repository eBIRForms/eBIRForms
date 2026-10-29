//! Printing on iOS: a WKWebView's `viewPrintFormatter` (the sheet's CSS
//! and fonts survive) drawn by a UIPrintPageRenderer into an A4 PDF
//! context (bir-print's `PdfSink`); the system print dialog
//! (UIPrintInteractionController, with AirPrint and Save to Files) and the
//! share sheet (UIActivityViewController) both take those pages.
//!
//! UIKit and WebKit live on the main thread; these run on a worker, post
//! their work to the main queue and wait for it.

use std::cell::RefCell;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bir_app_core::bir_print::pdf_sink::PdfSink;
use bir_app_core::bir_print::print_sheet::{PAPER_HEIGHT_PT, PAPER_WIDTH_PT};
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::msg_send;
use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSSize, NSString, NSURL};

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    fn UIGraphicsBeginPDFContextToData(data: *mut AnyObject, bounds: NSRect, info: *const AnyObject);
    fn UIGraphicsBeginPDFPage();
    fn UIGraphicsGetPDFContextBounds() -> NSRect;
    fn UIGraphicsEndPDFContext();
}

thread_local! {
    /// The web view of the sheet being printed (kept while the dialog is up).
    static WEB: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

fn paper() -> NSRect {
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(f64::from(PAPER_WIDTH_PT), f64::from(PAPER_HEIGHT_PT)))
}

/// Load `html` in a web view on the main thread and wait until it is drawn.
fn load(html: &str) -> Result<(), String> {
    let html = html.to_owned();
    DispatchQueue::main().exec_sync(move || {
        // SAFETY: WebKit's documented API, on the main thread.
        unsafe {
            let Some(class) = AnyClass::get(c"WKWebView") else { return };
            let web: Retained<AnyObject> = msg_send![class, new];
            let _: () = msg_send![&web, setFrame: paper()];
            let page = NSString::from_str(&html);
            let _: Option<Retained<AnyObject>> = msg_send![&web, loadHTMLString: &*page, baseURL: Option::<&NSURL>::None];
            WEB.with(|cell| *cell.borrow_mut() = Some(web));
        }
    });
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
            None => return Err("WebKit is not available".into()),
            Some(true) => {
                // Let it lay out once more before the formatter reads it.
                std::thread::sleep(Duration::from_millis(150));
                return Ok(());
            }
            Some(false) if started.elapsed() > Duration::from_secs(20) => return Err("the sheet took too long to load".into()),
            Some(false) => {}
        }
    }
}

/// The view controller showing on top, to present from.
unsafe fn top_controller() -> Option<Retained<AnyObject>> {
    unsafe {
        let app: Retained<AnyObject> = msg_send![AnyClass::get(c"UIApplication")?, sharedApplication];
        let windows: Retained<NSArray<AnyObject>> = msg_send![&app, windows];
        let window = windows.iter().find(|w| {
            let key: Bool = msg_send![&**w, isKeyWindow];
            key.as_bool()
        });
        let window = window.or_else(|| windows.iter().next())?;
        let mut top: Option<Retained<AnyObject>> = msg_send![&window, rootViewController];
        while let Some(t) = top.clone() {
            let next: Option<Retained<AnyObject>> = msg_send![&t, presentedViewController];
            match next {
                Some(n) => top = Some(n),
                None => break,
            }
        }
        top
    }
}

/// bir-print's `PdfSink` on iOS.
pub(crate) struct UiKitPdf;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct PdfError(String);

impl PdfSink for UiKitPdf {
    type Error = PdfError;

    fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, PdfError> {
        load(html).map_err(PdfError)?;
        let (tx, rx) = mpsc::channel::<Result<Vec<u8>, String>>();
        DispatchQueue::main().exec_sync(move || {
            let answer = WEB.with(|cell| {
                let web = cell.borrow().clone().ok_or_else(|| "the web view went away".to_owned())?;
                // SAFETY: UIKit's documented printing and PDF context API,
                // on the main thread.
                unsafe {
                    let renderer: Retained<AnyObject> = msg_send![AnyClass::get(c"UIPrintPageRenderer").ok_or("no UIPrintPageRenderer")?, new];
                    let formatter: Retained<AnyObject> = msg_send![&web, viewPrintFormatter];
                    let _: () = msg_send![&renderer, addPrintFormatter: &*formatter, startingAtPageAtIndex: 0isize];
                    let rect: Retained<AnyObject> = msg_send![AnyClass::get(c"NSValue").ok_or("no NSValue")?, valueWithCGRect: paper()];
                    let _: () = msg_send![&renderer, setValue: &*rect, forKey: &*NSString::from_str("paperRect")];
                    let _: () = msg_send![&renderer, setValue: &*rect, forKey: &*NSString::from_str("printableRect")];
                    let data: Retained<AnyObject> = msg_send![AnyClass::get(c"NSMutableData").ok_or("no NSMutableData")?, new];
                    let pages: isize = msg_send![&renderer, numberOfPages];
                    UIGraphicsBeginPDFContextToData(Retained::as_ptr(&data) as *mut AnyObject, paper(), std::ptr::null());
                    for i in 0..pages {
                        UIGraphicsBeginPDFPage();
                        let bounds = UIGraphicsGetPDFContextBounds();
                        let _: () = msg_send![&renderer, drawPageAtIndex: i, inRect: bounds];
                    }
                    UIGraphicsEndPDFContext();
                    let bytes: &NSData = &*(Retained::as_ptr(&data) as *const NSData);
                    Ok(bytes.to_vec())
                }
            });
            let _ = tx.send(answer);
        });
        rx.recv_timeout(Duration::from_secs(30)).map_err(|_| PdfError("the PDF took too long".into()))?.map_err(PdfError)
    }
}

/// What the app calls.
pub(crate) struct IosPrint;

impl crate::mobile::PhonePrint for IosPrint {
    fn print(&self, html: &str, title: &str) -> Result<(), String> {
        // The dialog gets the A4 pages the web view's print formatter drew
        // (the sheet's CSS and fonts kept); handing it the formatter itself
        // lets the printer's paper (US Letter by default) re-paginate them.
        let pdf = UiKitPdf.html_to_pdf(html).map_err(|e| e.to_string())?;
        let title = title.to_owned();
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        DispatchQueue::main().exec_sync(move || {
            let answer = (|| -> Result<(), String> {
                // SAFETY: UIKit's documented UIPrintInteractionController API,
                // on the main thread.
                unsafe {
                    let class = AnyClass::get(c"UIPrintInteractionController").ok_or("no UIPrintInteractionController")?;
                    let can: Bool = msg_send![class, isPrintingAvailable];
                    let _ = can;
                    let controller: Retained<AnyObject> = msg_send![class, sharedPrintController];
                    let info: Retained<AnyObject> = msg_send![AnyClass::get(c"UIPrintInfo").ok_or("no UIPrintInfo")?, printInfo];
                    // UIPrintInfoOutputGeneral = 0.
                    let _: () = msg_send![&info, setOutputType: 0isize];
                    let _: () = msg_send![&info, setJobName: &*NSString::from_str(&title)];
                    let _: () = msg_send![&controller, setPrintInfo: &*info];
                    let data = NSData::with_bytes(&pdf);
                    let _: () = msg_send![&controller, setPrintingItem: &*data];
                    let none: *const AnyObject = std::ptr::null();
                    let shown: Bool = msg_send![&controller, presentAnimated: Bool::YES, completionHandler: none];
                    if shown.as_bool() { Ok(()) } else { Err("the print dialog did not open".to_owned()) }
                }
            })();
            let _ = tx.send(answer);
        });
        rx.recv_timeout(Duration::from_secs(10)).unwrap_or(Ok(()))
    }

    fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, String> {
        UiKitPdf.html_to_pdf(html).map_err(|e| e.to_string())
    }

    fn share_pdf(&self, pdf: &[u8], title: &str) -> Result<(), String> {
        let name: String = format!("{title}.pdf").chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '.' | '_') { c } else { '-' }).collect();
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, pdf).map_err(|e| e.to_string())?;
        let path = path.display().to_string();
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        DispatchQueue::main().exec_async(move || {
            // SAFETY: UIKit's documented UIActivityViewController API, on
            // the main thread.
            let answer = unsafe {
                (|| -> Result<(), String> {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
                    let items = NSArray::from_retained_slice(&[url]);
                    let alloc: objc2::rc::Allocated<AnyObject> = msg_send![AnyClass::get(c"UIActivityViewController").ok_or("no share sheet")?, alloc];
                    let none: *const AnyObject = std::ptr::null();
                    let sheet: Retained<AnyObject> = msg_send![alloc, initWithActivityItems: &*items, applicationActivities: none];
                    let top = top_controller().ok_or("nothing to present from")?;
                    // An iPad shows it as a popover from the top view.
                    let pop: Option<Retained<AnyObject>> = msg_send![&sheet, popoverPresentationController];
                    if let Some(pop) = pop {
                        let view: Retained<AnyObject> = msg_send![&top, view];
                        let _: () = msg_send![&pop, setSourceView: &*view];
                    }
                    let _: () = msg_send![&top, presentViewController: &*sheet, animated: Bool::YES, completion: none];
                    Ok(())
                })()
            };
            let _ = tx.send(answer);
        });
        rx.recv_timeout(Duration::from_secs(10)).unwrap_or(Ok(()))
    }
}
