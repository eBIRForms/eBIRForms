#![recursion_limit = "256"]

//! Frozen HTML fill/print and validated native PDF output.
//!
//! Rust owns printable values. Frozen HTML in `assets/html-frozen/` is the
//! print surface: [`frozen_html`] fills it, [`print_sheet`] lays it out on
//! A4 (with the per-form `assets/print-overrides/`), [`static_html`] makes
//! it script-free. A platform [`PdfSink`] turns that into PDF bytes,
//! validated through [`html_output`].

pub mod certification_observation;
pub mod frozen_html;
pub mod html_output;
mod packaged;
pub mod pdf_sink;
pub mod pdf_util;
pub mod print_sheet;
pub mod static_html;

pub use pdf_sink::PdfSink;

pub use pdf_util::{append_text_pages_to_pdf, build_simple_confirmation_pdf, PdfUtilityError};
