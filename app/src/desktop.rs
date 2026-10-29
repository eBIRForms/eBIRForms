//! The desktop print and file glue, one uniform set per OS, for the print
//! preview and Settings (reached from worker threads, never the UI thread):
//!
//! - [`SheetPdf`] `{ form_code }`: bir-print's `PdfSink` over the OS web
//!   view (WebView2 `PrintToPdf` on Windows, a WebKitGTK print operation
//!   on Linux, WKWebView `createPDF` on macOS), A4 with no margins;
//! - `print_pdf(path) -> Result<(), String>`: the PDF to the system's
//!   printing (print verb / default handler / print panel);
//! - `save_panel(name, ext)` and `open_panel(ext) -> Option<PathBuf>`.
//!
//! Below the re-exports: the page geometry and small helpers the OS
//! modules share, testable without a display.

// The screens wire these in; until then some go unused on a given OS.
#![cfg_attr(not(test), allow(dead_code, unused_imports))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use bir_app_core::bir_print::print_sheet::{PAPER_HEIGHT_PT, PAPER_WIDTH_PT};

#[cfg(windows)]
pub(crate) use crate::windows::{SheetPdf, open_panel, print_pdf, save_panel};

#[cfg(target_os = "linux")]
pub(crate) use crate::linux::{SheetPdf, open_panel, print_pdf, save_panel};

// TODO(P6 merge): macOS arm, once app/src/mac.rs is on this branch:
// #[cfg(target_os = "macos")] pub(crate) use crate::mac::{WebKitPdf as SheetPdf, open_panel, print_pdf, save_panel};

/// Why a PDF could not be made.
#[derive(Debug)]
pub(crate) struct PdfError(pub String);

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PdfError {}

/// A paper size and its (uniform) margin, in PDF points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Page {
    pub width_pt: f64,
    pub height_pt: f64,
    pub margin_pt: f64,
}

/// bir-print's A4 sheet, edge to edge: the sheet CSS lays out one form
/// page per A4 page and keeps its own safety margin inside.
pub(crate) const A4: Page = Page {
    width_pt: PAPER_WIDTH_PT as f64,
    height_pt: PAPER_HEIGHT_PT as f64,
    margin_pt: 0.0,
};

impl Page {
    /// Width in inches (WebView2's print settings).
    pub fn width_in(&self) -> f64 {
        self.width_pt / 72.0
    }

    /// Height in inches.
    pub fn height_in(&self) -> f64 {
        self.height_pt / 72.0
    }

    /// Width in millimetres (GTK's paper sizes).
    pub fn width_mm(&self) -> f64 {
        self.width_pt * 25.4 / 72.0
    }

    /// Height in millimetres.
    pub fn height_mm(&self) -> f64 {
        self.height_pt * 25.4 / 72.0
    }

    /// The page at 96 CSS pixels per inch, rounded up to whole pixels (the
    /// hidden web view's size).
    pub fn css_px(&self) -> (i32, i32) {
        ((self.width_pt * 96.0 / 72.0).ceil() as i32, (self.height_pt * 96.0 / 72.0).ceil() as i32)
    }
}

/// Print backgrounds as drawn (shaded boxes, the sheet's white): the
/// printing engines drop them by default.
const PRINT_CSS: &str =
    "<style id=\"pdf-print\">html,body,.sheet{-webkit-print-color-adjust:exact;print-color-adjust:exact}</style>";

/// `html` with [`PRINT_CSS`] at the end of its head.
pub(crate) fn with_print_css(html: &str) -> String {
    match html.rfind("</head>") {
        Some(at) => format!("{}{PRINT_CSS}{}", &html[..at], &html[at..]),
        None => format!("{PRINT_CSS}{html}"),
    }
}

/// A fresh temp path for a capture of `form_code` with extension `ext`.
pub(crate) fn temp_file(form_code: &str, ext: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let code: String = form_code
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .take(32)
        .collect();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("buwiz-{code}-{}-{n}.{ext}", std::process::id()))
}

/// Whether `bytes` look like a PDF.
pub(crate) fn is_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// The `file://` URL of an absolute path (Windows or Unix), with the
/// bytes outside the URL-safe set percent-encoded.
pub(crate) fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = if text.starts_with('/') { text } else { format!("/{text}") };
    let mut url = String::from("file://");
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

/// A file dialog's filter for `ext`: its label and glob.
pub(crate) fn file_filter(ext: &str) -> (String, String) {
    let label = match ext {
        "pdf" => "PDF document".to_owned(),
        "zip" => "Zip archive".to_owned(),
        other => format!("{} file", other.to_uppercase()),
    };
    (label, format!("*.{ext}"))
}

/// `name` with the extension `ext` (added when missing).
pub(crate) fn with_extension(name: &str, ext: &str) -> String {
    let suffix = format!(".{ext}");
    if name.to_ascii_lowercase().ends_with(&suffix) { name.to_owned() } else { format!("{name}{suffix}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_is_210_by_297_mm_with_no_margin() {
        assert!((A4.width_mm() - 210.0).abs() < 0.01, "{}", A4.width_mm());
        assert!((A4.height_mm() - 297.0).abs() < 0.01, "{}", A4.height_mm());
        assert_eq!(A4.margin_pt, 0.0);
    }

    #[test]
    fn a4_in_inches_and_css_pixels() {
        assert!((A4.width_in() - 8.2678).abs() < 0.001);
        assert!((A4.height_in() - 11.6929).abs() < 0.001);
        assert_eq!(A4.css_px(), (794, 1123));
    }

    #[test]
    fn print_css_goes_at_the_end_of_the_head() {
        let out = with_print_css("<html><head><style>a{}</style></head><body></body></html>");
        assert!(out.contains("a{}</style><style id=\"pdf-print\">"));
        assert!(out.ends_with("</style></head><body></body></html>"));
        assert!(with_print_css("<p>x</p>").starts_with("<style id=\"pdf-print\">"));
    }

    #[test]
    fn temp_files_are_unique_and_safe() {
        let a = temp_file("1701Q/../x", "pdf");
        let b = temp_file("1701Q/../x", "pdf");
        assert_ne!(a, b);
        assert_eq!(a.parent(), Some(std::env::temp_dir().as_path()));
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("buwiz-1701Q____x-"), "{name}");
        assert!(name.ends_with(".pdf"));
    }

    #[test]
    fn pdf_magic() {
        assert!(is_pdf(b"%PDF-1.7\n"));
        assert!(!is_pdf(b"<html>"));
    }

    #[test]
    fn file_urls() {
        assert_eq!(file_url(Path::new("/tmp/a b.html")), "file:///tmp/a%20b.html");
        assert_eq!(
            file_url(Path::new(r"C:\Users\RUNNER~1\Temp\x#1.html")),
            "file:///C:/Users/RUNNER~1/Temp/x%231.html"
        );
    }

    #[test]
    fn filters_and_extensions() {
        assert_eq!(file_filter("pdf"), ("PDF document".to_owned(), "*.pdf".to_owned()));
        assert_eq!(file_filter("zip").1, "*.zip");
        assert_eq!(file_filter("csv").0, "CSV file");
        assert_eq!(with_extension("Return", "pdf"), "Return.pdf");
        assert_eq!(with_extension("Return.PDF", "pdf"), "Return.PDF");
    }
}
