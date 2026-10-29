//! Shared state and fail-closed validation for HTML print and PDF output.
//!
//! Platform WebViews create the PDF bytes, but every platform hands the result
//! through this module before a user-selected destination can be replaced.

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const GEOMETRY_TOLERANCE_POINTS: f64 = 0.25;
const WKPDF_CSS_PIXELS_PER_POINT: f64 = 96.0 / 72.0;
const WKPDF_NORMALIZATION_MATRIX: &[u8] = b"q\n0.75 0 0 0.75 0 0 cm\n";
const FORM_CODE_INFO_KEY: &[u8] = b"BirFormCode";
const REVISION_INFO_KEY: &[u8] = b"BirFormRevision";
const ENVELOPE_HASH_INFO_KEY: &[u8] = b"BirEnvelopeSha256";
const RENDER_BINDING_HASH_INFO_KEY: &[u8] = b"BirRenderBindingSha256";
const RENDER_BINDING_HASH_DOMAIN: &[u8] = b"ebirforms-envelope-render-binding-v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HtmlOutputKind {
    SystemPrint,
    PdfExport,
}

/// A bounded phase in the native HTML output lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlOutputTimeoutStage {
    Preflight,
    PdfExportBackend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HtmlOutputNonceError {
    #[error("native HTML output nonce space is exhausted")]
    Exhausted,
}

/// Issue the next one-use native-output nonce without ever wrapping or reusing a value.
///
/// A nonce binds one immutable renderer preflight to one platform backend invocation. Once the
/// counter reaches `u64::MAX`, output must fail closed instead of wrapping to `1`, because a reused
/// nonce could be mistaken for a stale completion from an earlier operation.
pub fn issue_html_output_nonce(current: u64) -> Result<u64, HtmlOutputNonceError> {
    current
        .checked_add(1)
        .ok_or(HtmlOutputNonceError::Exhausted)
}

/// Classifies elapsed native-output work without timing out an active system print dialog.
///
/// System print is completion-callback-bound after its platform backend starts because the user
/// may legitimately keep the native dialog open. PDF export remains bounded because it has no
/// interactive native-dialog phase.
pub fn html_output_timeout_stage(
    kind: HtmlOutputKind,
    backend_started: bool,
    elapsed: Duration,
    readiness_timeout: Duration,
    pdf_export_timeout: Duration,
) -> Option<HtmlOutputTimeoutStage> {
    if !backend_started && elapsed >= readiness_timeout {
        return Some(HtmlOutputTimeoutStage::Preflight);
    }
    if kind == HtmlOutputKind::PdfExport && elapsed >= pdf_export_timeout {
        return Some(HtmlOutputTimeoutStage::PdfExportBackend);
    }
    None
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HtmlOutputState {
    #[default]
    Idle,
    Validating {
        kind: HtmlOutputKind,
        nonce: String,
        destination: Option<PathBuf>,
    },
    Running {
        kind: HtmlOutputKind,
        temp_path: Option<PathBuf>,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfExpectation {
    pub form_code: String,
    pub revision: String,
    pub envelope_hash: String,
    pub expected_page_count: usize,
    pub width_points: f64,
    pub height_points: f64,
}

impl PdfExpectation {
    pub fn validate(&self) -> Result<(), HtmlOutputError> {
        if self.form_code.trim().is_empty() {
            return Err(HtmlOutputError::InvalidExpectation(
                "form code must not be empty".to_string(),
            ));
        }
        if self.revision.trim().is_empty() {
            return Err(HtmlOutputError::InvalidExpectation(
                "revision must not be empty".to_string(),
            ));
        }
        if self.envelope_hash.len() != 64
            || !self
                .envelope_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(HtmlOutputError::InvalidExpectation(
                "envelope hash must be a 64-character SHA-256 hex digest".to_string(),
            ));
        }
        if self.expected_page_count == 0 {
            return Err(HtmlOutputError::InvalidExpectation(
                "expected page count must be greater than zero".to_string(),
            ));
        }
        if !self.width_points.is_finite()
            || !self.height_points.is_finite()
            || self.width_points <= 0.0
            || self.height_points <= 0.0
        {
            return Err(HtmlOutputError::InvalidExpectation(
                "paper geometry must be finite and positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PdfValidationReport {
    pub page_count: usize,
    pub width_points: f64,
    pub height_points: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum HtmlOutputError {
    #[error("invalid PDF expectation: {0}")]
    InvalidExpectation(String),
    #[error("invalid PDF bytes: {0}")]
    InvalidPdf(String),
    #[error("PDF page count mismatch: expected {expected}, found {actual}")]
    PageCount { expected: usize, actual: usize },
    #[error("PDF page {page} has invalid {box_name}: {reason}")]
    InvalidPageBox {
        page: u32,
        box_name: &'static str,
        reason: String,
    },
    #[error(
        "PDF page {page} {box_name} mismatch: expected [0, 0, {expected_width}, {expected_height}], found {actual:?}"
    )]
    PageGeometry {
        page: u32,
        box_name: &'static str,
        expected_width: f64,
        expected_height: f64,
        actual: [f64; 4],
    },
    #[error("PDF page {page} has unexpected rotation {rotation}")]
    UnexpectedRotation { page: u32, rotation: f64 },
    #[error("PDF page {page} has no drawing content")]
    EmptyPage { page: u32 },
    #[error("PDF evidence {field} mismatch: expected {expected:?}, found {actual:?}")]
    EvidenceMismatch {
        field: &'static str,
        expected: String,
        actual: Option<String>,
    },
    #[error("platform PDF unexpectedly contains reserved eBIRForms evidence field {field}")]
    PreexistingEvidence { field: &'static str },
    #[error("PDF output path is invalid: {0}")]
    InvalidPath(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Create a unique, hidden sibling file for a platform PDF backend.
///
/// The backend may overwrite this empty file. Call [`finalize_pdf_export`] only
/// after the platform reports success. Failed callers should use
/// [`discard_pdf_export_temp`].
pub fn create_pdf_export_temp(destination: &Path) -> Result<PathBuf, HtmlOutputError> {
    let parent = destination.parent().ok_or_else(|| {
        HtmlOutputError::InvalidPath(format!("{} has no parent directory", destination.display()))
    })?;
    if !parent.is_dir() {
        return Err(HtmlOutputError::InvalidPath(format!(
            "destination directory does not exist: {}",
            parent.display()
        )));
    }
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            HtmlOutputError::InvalidPath(format!(
                "destination has no valid file name: {}",
                destination.display()
            ))
        })?;
    let temporary = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".partial.pdf")
        .tempfile_in(parent)?;
    let (file, path) = temporary
        .keep()
        .map_err(|error| HtmlOutputError::Io(error.error))?;
    drop(file);
    Ok(path)
}

pub fn discard_pdf_export_temp(path: &Path) -> Result<(), HtmlOutputError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Test helper for constructing a finalized PDF without exercising a native backend.
#[cfg(test)]
fn stamp_pdf_evidence(path: &Path, expectation: &PdfExpectation) -> Result<(), HtmlOutputError> {
    expectation.validate()?;
    let mut document =
        Document::load(path).map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
    ensure_reserved_evidence_absent(&document)?;
    validate_pdf_structure(&document, expectation)?;
    let render_binding_hash = render_binding_hash(&document, expectation)?;
    apply_pdf_evidence(&mut document, expectation, &render_binding_hash);
    document
        .save(path)
        .map_err(|error| HtmlOutputError::Io(std::io::Error::other(error.to_string())))?;
    Ok(())
}

pub fn validate_pdf_file(
    path: &Path,
    expectation: &PdfExpectation,
) -> Result<PdfValidationReport, HtmlOutputError> {
    expectation.validate()?;
    let document =
        Document::load(path).map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
    validate_pdf_document(&document, expectation)
}

pub fn validate_pdf_bytes(
    bytes: &[u8],
    expectation: &PdfExpectation,
) -> Result<PdfValidationReport, HtmlOutputError> {
    expectation.validate()?;
    let document = Document::load_mem(bytes)
        .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
    validate_pdf_document(&document, expectation)
}

/// Normalize one raw macOS `WKWebView.createPDF` page from CSS pixels to PDF points.
///
/// WebKit emits the configured DOM capture rectangle using one PDF point per
/// CSS pixel. The renderer measures paper at 96 CSS pixels per inch, while PDF
/// paper uses 72 points per inch. This function accepts only that exact 4:3
/// source geometry, materializes the expected page boxes, and wraps the
/// decoded vector content in a 0.75 coordinate transform.
///
/// # Errors
///
/// Returns an error for malformed or multi-page PDFs, pre-stamped evidence,
/// arbitrary page geometry, nonzero rotation, external streams, interactive
/// or alternate visual state, empty content, or serialization failure.
pub fn normalize_wkpdf_page_from_css_pixels(
    raw_page: &[u8],
    expectation: &PdfExpectation,
) -> Result<Vec<u8>, HtmlOutputError> {
    expectation.validate()?;
    let mut document = Document::load_mem(raw_page)
        .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
    ensure_reserved_evidence_absent(&document)?;
    reject_unsupported_wkpdf_state(&document)?;

    let pages = document.get_pages();
    if pages.len() != 1 {
        return Err(HtmlOutputError::PageCount {
            expected: 1,
            actual: pages.len(),
        });
    }
    let (page_number, page_id) = pages
        .into_iter()
        .next()
        .ok_or_else(|| HtmlOutputError::InvalidPdf("WKPDF capture has no page".to_string()))?;

    let raw_width = expectation.width_points * WKPDF_CSS_PIXELS_PER_POINT;
    let raw_height = expectation.height_points * WKPDF_CSS_PIXELS_PER_POINT;
    let media_box = inherited_page_box(&document, page_id, b"MediaBox", page_number)?;
    validate_page_geometry(page_number, "MediaBox", media_box, raw_width, raw_height)?;
    // Per ISO 32000, an absent CropBox inherits the MediaBox. That is the
    // exact shape emitted by WKWebView.createPDF on current macOS, so validate
    // the effective CropBox rather than requiring a redundant dictionary key.
    let crop_box = match inherited_page_value(&document, page_id, b"CropBox") {
        Some(value) => parse_page_box(&document, value, page_number, "CropBox")?,
        None => media_box,
    };
    validate_page_geometry(page_number, "CropBox", crop_box, raw_width, raw_height)?;
    validate_zero_rotation(&document, page_id, page_number)?;

    let content = document
        .get_page_content(page_id)
        .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
    if content.iter().all(u8::is_ascii_whitespace) {
        return Err(HtmlOutputError::EmptyPage { page: page_number });
    }
    let mut normalized_content =
        Vec::with_capacity(WKPDF_NORMALIZATION_MATRIX.len() + content.len() + b"\nQ\n".len());
    normalized_content.extend_from_slice(WKPDF_NORMALIZATION_MATRIX);
    normalized_content.extend_from_slice(&content);
    if !content.ends_with(b"\n") {
        normalized_content.push(b'\n');
    }
    normalized_content.extend_from_slice(b"Q\n");
    let content_id = document.add_object(Stream::new(dictionary! {}, normalized_content));
    let page = document
        .get_dictionary_mut(page_id)
        .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
    let normalized_box = vec![
        Object::Integer(0),
        Object::Integer(0),
        pdf_number(expectation.width_points),
        pdf_number(expectation.height_points),
    ];
    page.set("MediaBox", normalized_box.clone());
    page.set("CropBox", normalized_box);
    page.set("Rotate", 0);
    page.set("Contents", content_id);

    let mut normalized = Vec::new();
    document
        .save_to(&mut normalized)
        .map_err(|error| HtmlOutputError::Io(std::io::Error::other(error.to_string())))?;
    let normalized_document = Document::load_mem(&normalized)
        .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
    let mut page_expectation = expectation.clone();
    page_expectation.expected_page_count = 1;
    validate_pdf_structure(&normalized_document, &page_expectation)?;
    Ok(normalized)
}

/// Merge platform captures that each contain exactly one page.
///
/// WKWebView is intentionally captured one validated DOM rectangle at a time;
/// this keeps CSS page geometry from becoming one tall PDF page. Inherited
/// page attributes are materialized before the source page tree is discarded.
pub fn merge_single_page_pdfs(page_pdfs: &[Vec<u8>]) -> Result<Vec<u8>, HtmlOutputError> {
    if page_pdfs.is_empty() {
        return Err(HtmlOutputError::InvalidPdf(
            "cannot merge an empty page list".to_string(),
        ));
    }

    let mut merged = Document::with_version("1.7");
    let pages_root_id = merged.new_object_id();
    let catalog_id = merged.new_object_id();
    let mut next_object_id = catalog_id.0.saturating_add(1);
    let mut merged_pages = BTreeMap::new();

    for (index, page_bytes) in page_pdfs.iter().enumerate() {
        let mut source = Document::load_mem(page_bytes).map_err(|error| {
            HtmlOutputError::InvalidPdf(format!("captured page {}: {error}", index + 1))
        })?;
        if source.get_pages().len() != 1 {
            return Err(HtmlOutputError::InvalidPdf(format!(
                "captured page {} must contain exactly one PDF page",
                index + 1
            )));
        }

        source.renumber_objects_with(next_object_id);
        next_object_id = source.max_id.saturating_add(1);
        let (_, page_id) =
            source.get_pages().into_iter().next().ok_or_else(|| {
                HtmlOutputError::InvalidPdf("captured page disappeared".to_string())
            })?;
        let mut page = source
            .get_dictionary(page_id)
            .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?
            .clone();

        for key in [b"MediaBox".as_slice(), b"CropBox", b"Resources", b"Rotate"] {
            if !page.has(key) {
                if let Some(value) = inherited_page_value(&source, page_id, key) {
                    page.set(key, value.clone());
                }
            }
        }
        page.set("Parent", pages_root_id);
        merged_pages.insert(page_id, Object::Dictionary(page));

        for (object_id, object) in source.objects {
            if object_id == page_id {
                continue;
            }
            match object.type_name().unwrap_or("") {
                "Catalog" | "Pages" | "Page" | "Outlines" | "Outline" => {}
                _ => {
                    merged.objects.insert(object_id, object);
                }
            }
        }
    }

    let page_ids = merged_pages.keys().copied().collect::<Vec<_>>();
    merged.objects.extend(merged_pages);
    merged.objects.insert(
        pages_root_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids.iter().copied().map(Object::Reference).collect::<Vec<_>>(),
            "Count" => page_ids.len() as i64,
        }),
    );
    merged.objects.insert(
        catalog_id,
        Object::Dictionary(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_root_id,
        }),
    );
    merged.trailer.set("Root", catalog_id);
    merged.max_id = merged
        .objects
        .keys()
        .map(|(object_id, _)| *object_id)
        .max()
        .unwrap_or_default();
    merged.renumber_objects();

    let mut bytes = Vec::new();
    merged
        .save_to(&mut bytes)
        .map_err(|error| HtmlOutputError::Io(std::io::Error::other(error.to_string())))?;
    Ok(bytes)
}

/// Stamp, validate, flush, and atomically replace the chosen destination.
///
/// Any failure removes the sibling temporary file and leaves a pre-existing
/// destination untouched.
pub fn finalize_pdf_export(
    temp_path: &Path,
    destination: &Path,
    expectation: &PdfExpectation,
) -> Result<PdfValidationReport, HtmlOutputError> {
    if temp_path == destination {
        return Err(HtmlOutputError::InvalidPath(
            "temporary and destination paths must differ".to_string(),
        ));
    }

    let result = (|| {
        expectation.validate()?;
        validate_export_paths(temp_path, destination)?;

        // Validate the untouched platform output before adding evidence. Keeping
        // the validated document in memory avoids a path-based TOCTOU between
        // validation and stamping.
        let mut document = Document::load(temp_path)
            .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
        ensure_reserved_evidence_absent(&document)?;
        let raw_report = validate_pdf_structure(&document, expectation)?;
        let render_binding_hash = render_binding_hash(&document, expectation)?;
        apply_pdf_evidence(&mut document, expectation, &render_binding_hash);
        document
            .save(temp_path)
            .map_err(|error| HtmlOutputError::Io(std::io::Error::other(error.to_string())))?;

        // Reload the exact bytes that will be renamed. This proves the evidence
        // is still bound to the validated page/render graph after serialization.
        let report = validate_pdf_file(temp_path, expectation)?;
        debug_assert_eq!(report, raw_report);
        // Write access is required, not incidental: `sync_all` is
        // `FlushFileBuffers` on Windows, which needs GENERIC_WRITE and fails
        // with ERROR_ACCESS_DENIED on a read-only handle. On Unix `fsync`
        // would accept a read-only descriptor, which is why this only ever
        // failed on Windows - including for real exports, not just tests.
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(temp_path)?
            .sync_all()?;
        atomic_replace(temp_path, destination)?;
        Ok(report)
    })();

    if result.is_err() {
        let _ = discard_pdf_export_temp(temp_path);
    }
    result
}

fn validate_pdf_document(
    document: &Document,
    expectation: &PdfExpectation,
) -> Result<PdfValidationReport, HtmlOutputError> {
    let report = validate_pdf_structure(document, expectation)?;
    validate_pdf_evidence(document, expectation)?;
    Ok(report)
}

fn validate_pdf_structure(
    document: &Document,
    expectation: &PdfExpectation,
) -> Result<PdfValidationReport, HtmlOutputError> {
    let pages = document.get_pages();
    if pages.len() != expectation.expected_page_count {
        return Err(HtmlOutputError::PageCount {
            expected: expectation.expected_page_count,
            actual: pages.len(),
        });
    }

    for (page_number, page_id) in pages {
        let media_box = inherited_page_box(document, page_id, b"MediaBox", page_number)?;
        validate_page_geometry(
            page_number,
            "MediaBox",
            media_box,
            expectation.width_points,
            expectation.height_points,
        )?;

        let crop_box = match inherited_page_value(document, page_id, b"CropBox") {
            Some(value) => parse_page_box(document, value, page_number, "CropBox")?,
            None => media_box,
        };
        validate_page_geometry(
            page_number,
            "CropBox",
            crop_box,
            expectation.width_points,
            expectation.height_points,
        )?;

        validate_zero_rotation(document, page_id, page_number)?;

        let content = document
            .get_page_content(page_id)
            .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
        if content.iter().all(u8::is_ascii_whitespace) {
            return Err(HtmlOutputError::EmptyPage { page: page_number });
        }
    }

    Ok(PdfValidationReport {
        page_count: expectation.expected_page_count,
        width_points: expectation.width_points,
        height_points: expectation.height_points,
    })
}

fn validate_pdf_evidence(
    document: &Document,
    expectation: &PdfExpectation,
) -> Result<(), HtmlOutputError> {
    let info = document_info_dictionary(document);
    for (key, field, expected) in [
        (
            FORM_CODE_INFO_KEY,
            "form code",
            expectation.form_code.as_str(),
        ),
        (REVISION_INFO_KEY, "revision", expectation.revision.as_str()),
        (
            ENVELOPE_HASH_INFO_KEY,
            "envelope hash",
            expectation.envelope_hash.as_str(),
        ),
    ] {
        let actual = info
            .as_ref()
            .and_then(|dictionary| dictionary.get(key).ok())
            .and_then(|value| resolved_object(document, value))
            .and_then(|value| value.as_string().ok())
            .map(|value| value.into_owned());
        if actual.as_deref() != Some(expected) {
            return Err(HtmlOutputError::EvidenceMismatch {
                field,
                expected: expected.to_string(),
                actual,
            });
        }
    }
    let expected_render_binding_hash = render_binding_hash(document, expectation)?;
    let actual_render_binding_hash = info
        .as_ref()
        .and_then(|dictionary| dictionary.get(RENDER_BINDING_HASH_INFO_KEY).ok())
        .and_then(|value| resolved_object(document, value))
        .and_then(|value| value.as_string().ok())
        .map(|value| value.into_owned());
    if actual_render_binding_hash.as_deref() != Some(expected_render_binding_hash.as_str()) {
        return Err(HtmlOutputError::EvidenceMismatch {
            field: "render binding hash",
            expected: expected_render_binding_hash,
            actual: actual_render_binding_hash,
        });
    }
    Ok(())
}

fn apply_pdf_evidence(
    document: &mut Document,
    expectation: &PdfExpectation,
    render_binding_hash: &str,
) {
    let mut info = document_info_dictionary(document).unwrap_or_default();
    info.set(
        FORM_CODE_INFO_KEY,
        Object::string_literal(expectation.form_code.as_bytes()),
    );
    info.set(
        REVISION_INFO_KEY,
        Object::string_literal(expectation.revision.as_bytes()),
    );
    info.set(
        ENVELOPE_HASH_INFO_KEY,
        Object::string_literal(expectation.envelope_hash.as_bytes()),
    );
    info.set(
        RENDER_BINDING_HASH_INFO_KEY,
        Object::string_literal(render_binding_hash.as_bytes()),
    );
    let info_id = document.add_object(info);
    document.trailer.set("Info", info_id);
}

fn ensure_reserved_evidence_absent(document: &Document) -> Result<(), HtmlOutputError> {
    let Some(info) = document_info_dictionary(document) else {
        return Ok(());
    };
    for (key, field) in [
        (FORM_CODE_INFO_KEY, "form code"),
        (REVISION_INFO_KEY, "revision"),
        (ENVELOPE_HASH_INFO_KEY, "envelope hash"),
        (RENDER_BINDING_HASH_INFO_KEY, "render binding hash"),
    ] {
        if info.has(key) {
            return Err(HtmlOutputError::PreexistingEvidence { field });
        }
    }
    Ok(())
}

fn reject_unsupported_wkpdf_state(document: &Document) -> Result<(), HtmlOutputError> {
    let catalog = document
        .catalog()
        .map_err(|error| HtmlOutputError::InvalidPdf(format!("WKPDF catalog: {error}")))?;
    for (key, description) in [
        (b"AcroForm".as_slice(), "catalog AcroForm state"),
        (b"OCProperties".as_slice(), "catalog optional-content state"),
        (b"OutputIntents".as_slice(), "catalog output-intent state"),
        (b"OpenAction".as_slice(), "catalog open action"),
        (b"AA".as_slice(), "catalog additional actions"),
        (
            b"ViewerPreferences".as_slice(),
            "catalog viewer/print preferences",
        ),
    ] {
        if catalog.has(key) {
            return Err(HtmlOutputError::InvalidPdf(format!(
                "WKPDF capture contains unsupported {description}"
            )));
        }
    }
    for (page_number, page_id) in document.get_pages() {
        for (key, description) in [
            (b"Annots".as_slice(), "annotations"),
            (b"AA".as_slice(), "page additional actions"),
            (b"UserUnit".as_slice(), "custom user units"),
            (b"BleedBox".as_slice(), "BleedBox"),
            (b"TrimBox".as_slice(), "TrimBox"),
            (b"ArtBox".as_slice(), "ArtBox"),
            (b"PresSteps".as_slice(), "presentation steps"),
            (b"Trans".as_slice(), "page transitions"),
            (b"Dur".as_slice(), "page duration"),
        ] {
            if inherited_page_value(document, page_id, key).is_some() {
                return Err(HtmlOutputError::InvalidPdf(format!(
                    "WKPDF page {page_number} contains unsupported {description}"
                )));
            }
        }
    }
    if document
        .objects
        .values()
        .chain(document.trailer.iter().map(|(_, value)| value))
        .any(contains_external_stream)
    {
        return Err(HtmlOutputError::InvalidPdf(
            "WKPDF capture contains an unsupported stream backed by external file data".to_string(),
        ));
    }
    Ok(())
}

fn contains_external_stream(root: &Object) -> bool {
    let mut pending = vec![root];
    while let Some(object) = pending.pop() {
        match object {
            Object::Array(values) => pending.extend(values),
            Object::Dictionary(dictionary) => {
                pending.extend(dictionary.iter().map(|(_, value)| value));
            }
            Object::Stream(stream) => {
                if stream.dict.has(b"F")
                    || stream.dict.has(b"FFilter")
                    || stream.dict.has(b"FDecodeParms")
                {
                    return true;
                }
                pending.extend(stream.dict.iter().map(|(_, value)| value));
            }
            Object::Null
            | Object::Boolean(_)
            | Object::Integer(_)
            | Object::Real(_)
            | Object::Name(_)
            | Object::String(_, _)
            | Object::Reference(_) => {}
        }
    }
    false
}

fn render_binding_hash(
    document: &Document,
    expectation: &PdfExpectation,
) -> Result<String, HtmlOutputError> {
    let pages = document.get_pages();
    let mut hasher = Sha256::new();
    hasher.update(RENDER_BINDING_HASH_DOMAIN);
    hash_bytes(&mut hasher, expectation.form_code.as_bytes());
    hash_bytes(&mut hasher, expectation.revision.as_bytes());
    hash_bytes(&mut hasher, expectation.envelope_hash.as_bytes());
    hasher.update((expectation.expected_page_count as u64).to_be_bytes());
    hasher.update(expectation.width_points.to_bits().to_be_bytes());
    hasher.update(expectation.height_points.to_bits().to_be_bytes());
    hash_length(&mut hasher, pages.len());

    let mut visited = BTreeSet::new();
    for (page_number, page_id) in pages {
        hasher.update(b"page\0");
        hasher.update(page_number.to_be_bytes());
        let page = document
            .get_dictionary(page_id)
            .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
        hash_dictionary(document, page, &mut hasher, &mut visited, &[b"Parent"])?;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_object(
    document: &Document,
    object: &Object,
    hasher: &mut Sha256,
    visited: &mut BTreeSet<ObjectId>,
) -> Result<(), HtmlOutputError> {
    match object {
        Object::Null => hasher.update(b"null\0"),
        Object::Boolean(value) => {
            hasher.update(b"bool\0");
            hasher.update([u8::from(*value)]);
        }
        Object::Integer(value) => {
            hasher.update(b"integer\0");
            hasher.update(value.to_be_bytes());
        }
        Object::Real(value) => {
            hasher.update(b"real\0");
            hasher.update(value.to_bits().to_be_bytes());
        }
        Object::Name(value) => {
            hasher.update(b"name\0");
            hash_bytes(hasher, value);
        }
        Object::String(value, format) => {
            hasher.update(b"string\0");
            hasher.update([match format {
                lopdf::StringFormat::Literal => 0,
                lopdf::StringFormat::Hexadecimal => 1,
            }]);
            hash_bytes(hasher, value);
        }
        Object::Array(values) => {
            hasher.update(b"array\0");
            hash_length(hasher, values.len());
            for value in values {
                hash_object(document, value, hasher, visited)?;
            }
        }
        Object::Dictionary(dictionary) => {
            hasher.update(b"dictionary\0");
            hash_dictionary(document, dictionary, hasher, visited, &[])?;
        }
        Object::Stream(stream) => {
            hasher.update(b"stream\0");
            hash_dictionary(document, &stream.dict, hasher, visited, &[])?;
            hash_bytes(hasher, &stream.content);
        }
        Object::Reference(object_id) => {
            hasher.update(b"reference\0");
            hasher.update(object_id.0.to_be_bytes());
            hasher.update(object_id.1.to_be_bytes());
            if visited.insert(*object_id) {
                let resolved = document
                    .get_object(*object_id)
                    .map_err(|error| HtmlOutputError::InvalidPdf(error.to_string()))?;
                hash_object(document, resolved, hasher, visited)?;
            } else {
                hasher.update(b"visited\0");
            }
        }
    }
    Ok(())
}

fn hash_dictionary(
    document: &Document,
    dictionary: &Dictionary,
    hasher: &mut Sha256,
    visited: &mut BTreeSet<ObjectId>,
    ignored_keys: &[&[u8]],
) -> Result<(), HtmlOutputError> {
    let mut entries = dictionary
        .iter()
        .filter(|(key, _)| !ignored_keys.contains(&key.as_slice()))
        .collect::<Vec<_>>();
    entries.sort_unstable_by_key(|(left, _)| *left);
    hash_length(hasher, entries.len());
    for (key, value) in entries {
        hash_bytes(hasher, key);
        hash_object(document, value, hasher, visited)?;
    }
    Ok(())
}

fn hash_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hash_length(hasher, bytes.len());
    hasher.update(bytes);
}

fn hash_length(hasher: &mut Sha256, length: usize) {
    hasher.update((length as u64).to_be_bytes());
}

fn validate_export_paths(temp_path: &Path, destination: &Path) -> Result<(), HtmlOutputError> {
    let temp_parent = temp_path.parent().ok_or_else(|| {
        HtmlOutputError::InvalidPath(format!(
            "temporary PDF has no parent directory: {}",
            temp_path.display()
        ))
    })?;
    let destination_parent = destination.parent().ok_or_else(|| {
        HtmlOutputError::InvalidPath(format!(
            "destination PDF has no parent directory: {}",
            destination.display()
        ))
    })?;
    let temp_parent = fs::canonicalize(temp_parent)?;
    let destination_parent = fs::canonicalize(destination_parent)?;
    if temp_parent != destination_parent {
        return Err(HtmlOutputError::InvalidPath(format!(
            "temporary PDF must be a sibling of the destination: {}",
            temp_path.display()
        )));
    }

    let temp_metadata = fs::symlink_metadata(temp_path)?;
    if !temp_metadata.file_type().is_file() {
        return Err(HtmlOutputError::InvalidPath(format!(
            "temporary PDF must be a regular file: {}",
            temp_path.display()
        )));
    }

    if destination.exists() {
        let canonical_temp = fs::canonicalize(temp_path)?;
        let canonical_destination = fs::canonicalize(destination)?;
        if canonical_temp == canonical_destination {
            return Err(HtmlOutputError::InvalidPath(
                "temporary PDF aliases the destination".to_string(),
            ));
        }
        reject_same_file(
            temp_path,
            destination,
            &temp_metadata,
            &fs::metadata(destination)?,
        )?;
    }
    Ok(())
}

#[cfg(unix)]
fn reject_same_file(
    _temp_path: &Path,
    _destination: &Path,
    temp_metadata: &fs::Metadata,
    destination_metadata: &fs::Metadata,
) -> Result<(), HtmlOutputError> {
    use std::os::unix::fs::MetadataExt;
    if temp_metadata.dev() == destination_metadata.dev()
        && temp_metadata.ino() == destination_metadata.ino()
    {
        return Err(HtmlOutputError::InvalidPath(
            "temporary PDF aliases the destination".to_string(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn reject_same_file(
    temp_path: &Path,
    destination: &Path,
    _temp_metadata: &fs::Metadata,
    _destination_metadata: &fs::Metadata,
) -> Result<(), HtmlOutputError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    fn identity(path: &Path) -> Result<(u32, u64), HtmlOutputError> {
        let file = fs::File::open(path)?;
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `file` owns a valid handle for this synchronous query and
        // `information` is a live, correctly-sized output buffer.
        let succeeded =
            unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut information) };
        if succeeded == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let file_index =
            (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
        Ok((information.dwVolumeSerialNumber, file_index))
    }

    if identity(temp_path)? == identity(destination)? {
        return Err(HtmlOutputError::InvalidPath(
            "temporary PDF aliases the destination".to_string(),
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn reject_same_file(
    _temp_path: &Path,
    _destination: &Path,
    _temp_metadata: &fs::Metadata,
    _destination_metadata: &fs::Metadata,
) -> Result<(), HtmlOutputError> {
    // Fail closed on platforms where std does not expose a stable file identity.
    // Creating a new destination still works; replacing an existing destination
    // requires proving the temporary file is not a hard-link alias.
    Err(HtmlOutputError::InvalidPath(
        "stable file identity is unavailable for PDF replacement on this platform".to_string(),
    ))
}

fn document_info_dictionary(document: &Document) -> Option<Dictionary> {
    let info = document.trailer.get(b"Info").ok()?;
    resolved_object(document, info)?.as_dict().ok().cloned()
}

fn inherited_page_box(
    document: &Document,
    page_id: ObjectId,
    key: &[u8],
    page_number: u32,
) -> Result<[f64; 4], HtmlOutputError> {
    let value = inherited_page_value(document, page_id, key).ok_or_else(|| {
        HtmlOutputError::InvalidPageBox {
            page: page_number,
            box_name: "MediaBox",
            reason: "missing inherited page box".to_string(),
        }
    })?;
    parse_page_box(document, value, page_number, "MediaBox")
}

fn inherited_page_value<'a>(
    document: &'a Document,
    mut object_id: ObjectId,
    key: &[u8],
) -> Option<&'a Object> {
    for _ in 0..64 {
        let dictionary = document.get_dictionary(object_id).ok()?;
        if let Ok(value) = dictionary.get(key) {
            return Some(value);
        }
        object_id = dictionary.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn parse_page_box(
    document: &Document,
    value: &Object,
    page_number: u32,
    box_name: &'static str,
) -> Result<[f64; 4], HtmlOutputError> {
    let value =
        resolved_object(document, value).ok_or_else(|| HtmlOutputError::InvalidPageBox {
            page: page_number,
            box_name,
            reason: "indirect page box could not be resolved".to_string(),
        })?;
    let values = value
        .as_array()
        .map_err(|_| HtmlOutputError::InvalidPageBox {
            page: page_number,
            box_name,
            reason: "page box must be an array".to_string(),
        })?;
    if values.len() != 4 {
        return Err(HtmlOutputError::InvalidPageBox {
            page: page_number,
            box_name,
            reason: format!(
                "page box must have four coordinates, found {}",
                values.len()
            ),
        });
    }
    let mut parsed = [0.0; 4];
    for (index, coordinate) in values.iter().enumerate() {
        parsed[index] =
            object_number(document, coordinate).ok_or_else(|| HtmlOutputError::InvalidPageBox {
                page: page_number,
                box_name,
                reason: format!("coordinate {index} is not a finite number"),
            })?;
    }
    if parsed[2] <= parsed[0] || parsed[3] <= parsed[1] {
        return Err(HtmlOutputError::InvalidPageBox {
            page: page_number,
            box_name,
            reason: "upper-right coordinates must exceed lower-left coordinates".to_string(),
        });
    }
    Ok(parsed)
}

fn validate_page_geometry(
    page: u32,
    box_name: &'static str,
    actual: [f64; 4],
    expected_width: f64,
    expected_height: f64,
) -> Result<(), HtmlOutputError> {
    let expected = [0.0, 0.0, expected_width, expected_height];
    if actual
        .iter()
        .zip(expected)
        .any(|(actual, expected)| (actual - expected).abs() > GEOMETRY_TOLERANCE_POINTS)
    {
        return Err(HtmlOutputError::PageGeometry {
            page,
            box_name,
            expected_width,
            expected_height,
            actual,
        });
    }
    Ok(())
}

fn validate_zero_rotation(
    document: &Document,
    page_id: ObjectId,
    page_number: u32,
) -> Result<(), HtmlOutputError> {
    let Some(rotation) = inherited_page_value(document, page_id, b"Rotate") else {
        return Ok(());
    };
    let rotation =
        object_number(document, rotation).ok_or_else(|| HtmlOutputError::InvalidPageBox {
            page: page_number,
            box_name: "Rotate",
            reason: "rotation must be a finite number".to_string(),
        })?;
    if rotation.abs() > f64::EPSILON {
        return Err(HtmlOutputError::UnexpectedRotation {
            page: page_number,
            rotation,
        });
    }
    Ok(())
}

fn pdf_number(value: f64) -> Object {
    if value.fract().abs() <= f64::EPSILON && value <= i64::MAX as f64 {
        Object::Integer(value as i64)
    } else {
        Object::Real(value as f32)
    }
}

fn object_number(document: &Document, value: &Object) -> Option<f64> {
    let value = resolved_object(document, value)?;
    let number = match value {
        Object::Integer(value) => *value as f64,
        Object::Real(value) => f64::from(*value),
        _ => return None,
    };
    number.is_finite().then_some(number)
}

fn resolved_object<'a>(document: &'a Document, mut value: &'a Object) -> Option<&'a Object> {
    for _ in 0..64 {
        match value {
            Object::Reference(object_id) => value = document.get_object(*object_id).ok()?,
            _ => return Some(value),
        }
    }
    None
}

#[cfg(not(windows))]
fn atomic_replace(temp_path: &Path, destination: &Path) -> Result<(), HtmlOutputError> {
    fs::rename(temp_path, destination)?;
    Ok(())
}

#[cfg(windows)]
fn atomic_replace(temp_path: &Path, destination: &Path) -> Result<(), HtmlOutputError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: both arguments are owned, NUL-terminated UTF-16 buffers that
    // remain alive for the duration of this synchronous Win32 call.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Stream};

    const TEST_READINESS_TIMEOUT: Duration = Duration::from_secs(5);
    const TEST_PDF_EXPORT_TIMEOUT: Duration = Duration::from_secs(30);

    #[test]
    fn output_nonces_start_at_one_and_advance_monotonically() {
        assert_eq!(issue_html_output_nonce(0), Ok(1));
        assert_eq!(issue_html_output_nonce(1), Ok(2));
        assert_eq!(issue_html_output_nonce(u64::MAX - 1), Ok(u64::MAX));
    }

    #[test]
    fn output_nonce_exhaustion_fails_closed_instead_of_reusing_one() {
        assert_eq!(
            issue_html_output_nonce(u64::MAX),
            Err(HtmlOutputNonceError::Exhausted)
        );
    }

    #[test]
    fn active_system_print_has_no_backend_deadline() {
        let stage = html_output_timeout_stage(
            HtmlOutputKind::SystemPrint,
            true,
            Duration::from_secs(3_600),
            TEST_READINESS_TIMEOUT,
            TEST_PDF_EXPORT_TIMEOUT,
        );
        assert_eq!(stage, None);
    }

    #[test]
    fn active_pdf_export_expires_at_its_backend_deadline() {
        let stage = html_output_timeout_stage(
            HtmlOutputKind::PdfExport,
            true,
            TEST_PDF_EXPORT_TIMEOUT,
            TEST_READINESS_TIMEOUT,
            TEST_PDF_EXPORT_TIMEOUT,
        );
        assert_eq!(stage, Some(HtmlOutputTimeoutStage::PdfExportBackend));
    }

    #[test]
    fn system_print_preflight_expires_before_backend_start() {
        let stage = html_output_timeout_stage(
            HtmlOutputKind::SystemPrint,
            false,
            TEST_READINESS_TIMEOUT,
            TEST_READINESS_TIMEOUT,
            TEST_PDF_EXPORT_TIMEOUT,
        );
        assert_eq!(stage, Some(HtmlOutputTimeoutStage::Preflight));
    }

    fn expectation(page_count: usize) -> PdfExpectation {
        PdfExpectation {
            form_code: "2551Q".to_string(),
            revision: "2018".to_string(),
            envelope_hash: "a".repeat(64),
            expected_page_count: page_count,
            width_points: 612.0,
            height_points: 936.0,
        }
    }

    fn write_pdf(path: &Path, page_count: usize, width: i64, height: i64, content: &[u8]) {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let mut page_ids = Vec::new();
        for _ in 0..page_count {
            let content_id = document.add_object(Stream::new(dictionary! {}, content.to_vec()));
            let page_id = document.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), width.into(), height.into()],
                "CropBox" => vec![0.into(), 0.into(), width.into(), height.into()],
                "Resources" => dictionary! {},
                "Contents" => content_id,
            });
            page_ids.push(page_id);
        }
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids.iter().copied().map(Object::Reference).collect::<Vec<_>>(),
                "Count" => page_count as i64,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(path).expect("test PDF should save");
    }

    fn pdf_bytes(page_count: usize, width: i64, height: i64, content: &[u8]) -> Vec<u8> {
        let directory = tempfile::tempdir().expect("test PDF directory");
        let path = directory.path().join("page.pdf");
        write_pdf(&path, page_count, width, height, content);
        fs::read(path).expect("test PDF bytes")
    }

    #[test]
    fn normalizes_exact_wkpdf_css_pixel_page_to_expected_points() {
        let expectation = expectation(2);
        let raw = pdf_bytes(1, 816, 1248, b"0 0 m 100 100 l S");
        let normalized = normalize_wkpdf_page_from_css_pixels(&raw, &expectation)
            .expect("exact WKPDF CSS-pixel capture should normalize");
        let document = Document::load_mem(&normalized).expect("normalized PDF");
        let (page_number, page_id) = document
            .get_pages()
            .into_iter()
            .next()
            .expect("normalized page");
        let media_box = inherited_page_box(&document, page_id, b"MediaBox", page_number)
            .expect("normalized MediaBox");
        let crop_box = parse_page_box(
            &document,
            inherited_page_value(&document, page_id, b"CropBox").expect("normalized CropBox"),
            page_number,
            "CropBox",
        )
        .expect("parse normalized CropBox");
        let content = document
            .get_page_content(page_id)
            .expect("normalized vector content");

        assert_eq!(
            (media_box, crop_box, content),
            (
                [0.0, 0.0, 612.0, 936.0],
                [0.0, 0.0, 612.0, 936.0],
                b"q\n0.75 0 0 0.75 0 0 cm\n0 0 m 100 100 l S\nQ\n".to_vec(),
            )
        );
    }

    #[test]
    fn rejects_already_normalized_wkpdf_page() {
        let error = normalize_wkpdf_page_from_css_pixels(
            &pdf_bytes(1, 612, 936, b"0 0 m 100 100 l S"),
            &expectation(1),
        )
        .expect_err("already-normalized input must not be scaled twice");
        assert!(matches!(error, HtmlOutputError::PageGeometry { .. }));
    }

    #[test]
    fn rejects_wkpdf_page_with_arbitrary_geometry() {
        let error = normalize_wkpdf_page_from_css_pixels(
            &pdf_bytes(1, 800, 1200, b"0 0 m 100 100 l S"),
            &expectation(1),
        )
        .expect_err("arbitrary WKPDF geometry must fail closed");
        assert!(matches!(error, HtmlOutputError::PageGeometry { .. }));
    }

    #[test]
    fn rejects_rotated_wkpdf_page_before_normalization() {
        let raw = pdf_bytes(1, 816, 1248, b"0 0 m 100 100 l S");
        let mut document = Document::load_mem(&raw).expect("raw WKPDF test page");
        let page_id = *document
            .get_pages()
            .values()
            .next()
            .expect("raw WKPDF test page ID");
        document
            .get_dictionary_mut(page_id)
            .expect("raw WKPDF page dictionary")
            .set("Rotate", 90);
        let mut rotated = Vec::new();
        document.save_to(&mut rotated).expect("rotated WKPDF bytes");

        let error = normalize_wkpdf_page_from_css_pixels(&rotated, &expectation(1))
            .expect_err("rotated WKPDF capture must fail closed");
        assert!(matches!(
            error,
            HtmlOutputError::UnexpectedRotation { rotation, .. } if rotation == 90.0
        ));
    }

    #[test]
    fn rejects_wkpdf_catalog_visual_state_before_normalization() {
        let raw = pdf_bytes(1, 816, 1248, b"0 0 m 100 100 l S");
        let mut document = Document::load_mem(&raw).expect("raw WKPDF test page");
        document
            .catalog_mut()
            .expect("raw WKPDF catalog")
            .set("AcroForm", dictionary! {});
        let mut interactive = Vec::new();
        document
            .save_to(&mut interactive)
            .expect("interactive WKPDF bytes");

        let error = normalize_wkpdf_page_from_css_pixels(&interactive, &expectation(1))
            .expect_err("interactive WKPDF capture must fail closed");
        assert!(error.to_string().contains("unsupported catalog AcroForm"));
    }

    fn replace_first_page_content(path: &Path, content: &[u8]) {
        let mut document = Document::load(path).expect("test PDF should load");
        let page_id = document
            .get_pages()
            .into_values()
            .next()
            .expect("test PDF should contain a page");
        let content_id = document
            .get_dictionary(page_id)
            .expect("page dictionary should exist")
            .get(b"Contents")
            .expect("page content should exist")
            .as_reference()
            .expect("test page content should be indirect");
        let stream = document
            .get_object_mut(content_id)
            .expect("content object should exist")
            .as_stream_mut()
            .expect("content should be a stream");
        stream.set_plain_content(content.to_vec());
        document.save(path).expect("mutated PDF should save");
    }

    fn replace_first_page_geometry(path: &Path, width: i64, height: i64) {
        let mut document = Document::load(path).expect("test PDF should load");
        let page_id = document
            .get_pages()
            .into_values()
            .next()
            .expect("test PDF should contain a page");
        let page = document
            .get_dictionary_mut(page_id)
            .expect("page dictionary should exist");
        let page_box = vec![0.into(), 0.into(), width.into(), height.into()];
        page.set("MediaBox", page_box.clone());
        page.set("CropBox", page_box);
        document.save(path).expect("mutated PDF should save");
    }

    fn replace_info_string(path: &Path, key: &[u8], value: &[u8]) {
        let mut document = Document::load(path).expect("test PDF should load");
        let info_id = document
            .trailer
            .get(b"Info")
            .expect("test PDF should have an Info dictionary")
            .as_reference()
            .expect("Info dictionary should be indirect");
        document
            .get_dictionary_mut(info_id)
            .expect("Info dictionary should exist")
            .set(key, Object::string_literal(value));
        document.save(path).expect("mutated PDF should save");
    }

    fn stamped_pdf(page_count: usize) -> (tempfile::TempDir, PathBuf, PdfExpectation) {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("form.pdf");
        let expectation = expectation(page_count);
        write_pdf(&path, page_count, 612, 936, b"q 1 0 0 1 0 0 cm Q");
        stamp_pdf_evidence(&path, &expectation).expect("evidence should be stamped");
        (directory, path, expectation)
    }

    #[test]
    fn validates_expected_page_geometry_and_evidence() {
        let (_directory, path, expectation) = stamped_pdf(2);
        let report = validate_pdf_file(&path, &expectation).expect("PDF should validate");
        assert_eq!(report.page_count, 2);
        assert_eq!(report.width_points, 612.0);
        assert_eq!(report.height_points, 936.0);
    }

    #[test]
    fn rejects_incorrect_page_count() {
        let (_directory, path, mut expectation) = stamped_pdf(1);
        expectation.expected_page_count = 2;
        assert!(matches!(
            validate_pdf_file(&path, &expectation),
            Err(HtmlOutputError::PageCount {
                expected: 2,
                actual: 1
            })
        ));
    }

    #[test]
    fn rejects_incorrect_page_geometry() {
        let (_directory, path, expectation) = stamped_pdf(1);
        replace_first_page_geometry(&path, 612, 935);
        assert!(matches!(
            validate_pdf_file(&path, &expectation),
            Err(HtmlOutputError::PageGeometry { .. })
        ));
    }

    #[test]
    fn rejects_empty_page_content() {
        let (_directory, path, expectation) = stamped_pdf(1);
        replace_first_page_content(&path, b" \n\t");
        assert!(matches!(
            validate_pdf_file(&path, &expectation),
            Err(HtmlOutputError::EmptyPage { page: 1 })
        ));
    }

    #[test]
    fn rejects_mismatched_envelope_evidence() {
        let (_directory, path, mut expectation) = stamped_pdf(1);
        expectation.envelope_hash = "b".repeat(64);
        assert!(matches!(
            validate_pdf_file(&path, &expectation),
            Err(HtmlOutputError::EvidenceMismatch {
                field: "envelope hash",
                ..
            })
        ));
    }

    #[test]
    fn render_binding_cryptographically_includes_envelope_identity() {
        let (_directory, path, mut different_expectation) = stamped_pdf(1);
        different_expectation.envelope_hash = "b".repeat(64);
        replace_info_string(
            &path,
            ENVELOPE_HASH_INFO_KEY,
            different_expectation.envelope_hash.as_bytes(),
        );

        assert!(matches!(
            validate_pdf_file(&path, &different_expectation),
            Err(HtmlOutputError::EvidenceMismatch {
                field: "render binding hash",
                ..
            })
        ));
    }

    #[test]
    fn failed_export_preserves_existing_destination() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let destination = directory.path().join("selected.pdf");
        fs::write(&destination, b"existing destination").expect("destination should be written");
        let temp_path = create_pdf_export_temp(&destination).expect("temporary path should exist");
        fs::write(&temp_path, b"not a PDF").expect("invalid PDF should be written");

        assert!(finalize_pdf_export(&temp_path, &destination, &expectation(1)).is_err());
        assert_eq!(
            fs::read(&destination).expect("destination should remain readable"),
            b"existing destination"
        );
        assert!(!temp_path.exists());
    }

    #[test]
    fn raw_pdf_is_validated_before_evidence_is_added() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let destination = directory.path().join("selected.pdf");
        fs::write(&destination, b"existing destination").expect("destination should be written");
        let temp_path = create_pdf_export_temp(&destination).expect("temporary path should exist");
        write_pdf(&temp_path, 1, 612, 936, b"q Q");

        assert!(matches!(
            finalize_pdf_export(&temp_path, &destination, &expectation(2)),
            Err(HtmlOutputError::PageCount {
                expected: 2,
                actual: 1
            })
        ));
        assert_eq!(
            fs::read(&destination).expect("destination should remain readable"),
            b"existing destination"
        );
        assert!(!temp_path.exists());
    }

    #[test]
    fn pre_stamped_platform_pdf_is_rejected_without_rewriting_destination() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let destination = directory.path().join("selected.pdf");
        fs::write(&destination, b"existing destination").expect("destination should be written");
        let temp_path = create_pdf_export_temp(&destination).expect("temporary path should exist");
        write_pdf(&temp_path, 1, 612, 936, b"q Q");
        stamp_pdf_evidence(&temp_path, &expectation(1)).expect("test evidence should be stamped");

        assert!(matches!(
            finalize_pdf_export(&temp_path, &destination, &expectation(1)),
            Err(HtmlOutputError::PreexistingEvidence { .. })
        ));
        assert_eq!(
            fs::read(&destination).expect("destination should remain readable"),
            b"existing destination"
        );
        assert!(!temp_path.exists());
    }

    #[test]
    fn non_sibling_temp_is_rejected_without_rewriting_destination() {
        let destination_directory =
            tempfile::tempdir().expect("destination directory should be created");
        let temp_directory = tempfile::tempdir().expect("temp directory should be created");
        let destination = destination_directory.path().join("selected.pdf");
        let temp_path = temp_directory.path().join("backend.pdf");
        fs::write(&destination, b"existing destination").expect("destination should be written");
        write_pdf(&temp_path, 1, 612, 936, b"q Q");

        assert!(matches!(
            finalize_pdf_export(&temp_path, &destination, &expectation(1)),
            Err(HtmlOutputError::InvalidPath(_))
        ));
        assert_eq!(
            fs::read(&destination).expect("destination should remain readable"),
            b"existing destination"
        );
        assert!(!temp_path.exists());
    }

    #[test]
    fn hard_linked_temp_is_rejected_without_rewriting_destination() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let destination = directory.path().join("selected.pdf");
        let temp_path = directory.path().join("backend.pdf");
        write_pdf(&destination, 1, 612, 936, b"q Q");
        let original_destination =
            fs::read(&destination).expect("destination snapshot should be readable");
        fs::hard_link(&destination, &temp_path).expect("hard link should be created");

        assert!(matches!(
            finalize_pdf_export(&temp_path, &destination, &expectation(1)),
            Err(HtmlOutputError::InvalidPath(_))
        ));
        assert_eq!(
            fs::read(&destination).expect("destination should remain readable"),
            original_destination
        );
        assert!(!temp_path.exists());
    }

    #[test]
    fn valid_export_atomically_replaces_destination() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let destination = directory.path().join("selected.pdf");
        fs::write(&destination, b"old").expect("destination should be written");
        let temp_path = create_pdf_export_temp(&destination).expect("temporary path should exist");
        write_pdf(&temp_path, 1, 612, 936, b"q Q");

        let report = finalize_pdf_export(&temp_path, &destination, &expectation(1))
            .expect("valid export should replace destination");
        assert_eq!(report.page_count, 1);
        assert!(!temp_path.exists());
        validate_pdf_file(&destination, &expectation(1))
            .expect("final destination should remain valid");
    }

    #[test]
    fn finalized_evidence_detects_render_content_tampering() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let destination = directory.path().join("selected.pdf");
        let temp_path = create_pdf_export_temp(&destination).expect("temporary path should exist");
        write_pdf(&temp_path, 1, 612, 936, b"q 1 0 0 1 0 0 cm Q");
        finalize_pdf_export(&temp_path, &destination, &expectation(1))
            .expect("valid export should finalize");

        replace_first_page_content(&destination, b"q 1 0 0 1 4 4 cm Q");
        assert!(matches!(
            validate_pdf_file(&destination, &expectation(1)),
            Err(HtmlOutputError::EvidenceMismatch {
                field: "render binding hash",
                ..
            })
        ));
    }

    #[test]
    fn merges_one_capture_per_page_in_renderer_order() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let first = directory.path().join("first.pdf");
        let second = directory.path().join("second.pdf");
        write_pdf(&first, 1, 612, 936, b"q 1 0 0 1 1 1 cm Q");
        write_pdf(&second, 1, 612, 936, b"q 1 0 0 1 2 2 cm Q");
        let bytes = merge_single_page_pdfs(&[
            fs::read(first).expect("first capture should be readable"),
            fs::read(second).expect("second capture should be readable"),
        ])
        .expect("captures should merge");
        let merged = Document::load_mem(&bytes).expect("merged PDF should load");
        assert_eq!(merged.get_pages().len(), 2);
    }

    #[test]
    fn merges_normalized_wkpdf_pages_without_catalog_id_collision() {
        let expectation = expectation(2);
        let first = normalize_wkpdf_page_from_css_pixels(
            &pdf_bytes(1, 816, 1248, b"q 1 0 0 1 1 1 cm Q"),
            &expectation,
        )
        .expect("normalize first WKPDF page");
        let second = normalize_wkpdf_page_from_css_pixels(
            &pdf_bytes(1, 816, 1248, b"q 1 0 0 1 2 2 cm Q"),
            &expectation,
        )
        .expect("normalize second WKPDF page");

        let bytes =
            merge_single_page_pdfs(&[first, second]).expect("normalized pages should merge");
        let merged = Document::load_mem(&bytes).expect("merged PDF should load");
        assert_eq!(merged.get_pages().len(), 2);
    }
}
