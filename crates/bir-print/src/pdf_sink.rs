//! The seam that turns a filled, print-ready sheet into PDF bytes.
//!
//! The source app made PDFs with a macOS WebView (`WKWebView::createPDF`);
//! that code is not ported. Each platform supplies its own sink later
//! (macOS WKWebView, Windows WebView2 `PrintToPdfAsync`, Linux WebKitGTK).
//! No implementation lives in this crate yet.
//!
//! Feed a sink the output of [`crate::print_sheet::prepare_html_for`] run
//! through [`crate::static_html::staticize`]: script-free markup laid out
//! one A4 page per form page. Validate the bytes it returns with
//! [`crate::html_output`] before handing them to the user.

/// Renders a complete HTML document to PDF bytes.
pub trait PdfSink {
    /// The platform's rendering failure.
    type Error: std::error::Error;

    /// Render `html` (a whole document, all assets inlined) to a PDF.
    fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, Self::Error>;
}
