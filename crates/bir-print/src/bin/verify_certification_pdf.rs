//! Re-run the Rust-owned PDF validator for a 2551Q platform candidate export.
//!
//! The output is deterministic so an external collector can retain it and the
//! candidate-certification verifier can later compare the exact bytes. This is
//! an operator-only verifier: it cannot register evidence or promote a form.

use bir_print::html_output::{validate_pdf_file, PdfExpectation};
use lopdf::Document;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{env, fs, path::Path, process::ExitCode};

const FORM_CODE: &str = "2551Q";
const REVISION: &str = "2018";
const PAGE_COUNT: usize = 2;
const WIDTH_POINTS: f64 = 612.0;
const HEIGHT_POINTS: f64 = 936.0;

#[derive(Debug, Serialize)]
struct PageReport {
    page: u32,
    media_width_pt: f64,
    media_height_pt: f64,
    crop_width_pt: f64,
    crop_height_pt: f64,
    rotation: i32,
    content_byte_count: usize,
}

#[derive(Debug, Serialize)]
struct CertificationPdfReport {
    schema_version: u8,
    scope: &'static str,
    promotion_eligible: bool,
    form: FormIdentity,
    envelope_sha256: String,
    output_sha256: String,
    expected_page_count: usize,
    actual_page_count: usize,
    width_points: f64,
    height_points: f64,
    content_nonempty: bool,
    validated_by: &'static str,
    pages: Vec<PageReport>,
}

#[derive(Debug, Serialize)]
struct FormIdentity {
    code: &'static str,
    revision: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CertificationPlatform {
    Macos,
    Windows,
    Linux,
}

impl CertificationPlatform {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "macos" => Ok(Self::Macos),
            "windows" => Ok(Self::Windows),
            "linux" => Ok(Self::Linux),
            _ => Err(format!(
                "unsupported certification platform {value:?}; expected macos, windows, or linux"
            )),
        }
    }

    fn scope(self) -> &'static str {
        match self {
            Self::Macos => "owned_macos_candidate_pdf_validation",
            Self::Windows => "owned_windows_candidate_pdf_validation",
            Self::Linux => "owned_linux_candidate_pdf_validation",
        }
    }
}

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn run_for_platform(
    pdf_path: &Path,
    envelope_sha256: &str,
    platform: CertificationPlatform,
) -> Result<CertificationPdfReport, String> {
    if !canonical_sha256(envelope_sha256) {
        return Err("envelope hash must be 64 lowercase hexadecimal characters".to_string());
    }
    if !pdf_path.is_file() {
        return Err(format!("PDF does not exist: {}", pdf_path.display()));
    }

    let expectation = PdfExpectation {
        form_code: FORM_CODE.to_string(),
        revision: REVISION.to_string(),
        envelope_hash: envelope_sha256.to_string(),
        expected_page_count: PAGE_COUNT,
        width_points: WIDTH_POINTS,
        height_points: HEIGHT_POINTS,
    };
    let validation = validate_pdf_file(pdf_path, &expectation)
        .map_err(|error| format!("owned PDF validation failed: {error}"))?;
    let pdf_bytes = fs::read(pdf_path)
        .map_err(|error| format!("could not read {}: {error}", pdf_path.display()))?;
    let document = Document::load_mem(&pdf_bytes)
        .map_err(|error| format!("could not reopen validated PDF: {error}"))?;
    let pages = document
        .get_pages()
        .into_iter()
        .map(|(page, page_id)| {
            let content = document
                .get_page_content(page_id)
                .map_err(|error| format!("could not read page {page} content: {error}"))?;
            if content.iter().all(u8::is_ascii_whitespace) {
                return Err(format!(
                    "validated page {page} unexpectedly has empty content"
                ));
            }
            Ok(PageReport {
                page,
                media_width_pt: validation.width_points,
                media_height_pt: validation.height_points,
                crop_width_pt: validation.width_points,
                crop_height_pt: validation.height_points,
                rotation: 0,
                content_byte_count: content.len(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(CertificationPdfReport {
        schema_version: 1,
        scope: platform.scope(),
        promotion_eligible: false,
        form: FormIdentity {
            code: FORM_CODE,
            revision: REVISION,
        },
        envelope_sha256: envelope_sha256.to_string(),
        output_sha256: format!("{:x}", Sha256::digest(&pdf_bytes)),
        expected_page_count: PAGE_COUNT,
        actual_page_count: validation.page_count,
        width_points: validation.width_points,
        height_points: validation.height_points,
        content_nonempty: true,
        validated_by: "bir-print::html_output::validate_pdf_file",
        pages,
    })
}

#[cfg(test)]
fn run(pdf_path: &Path, envelope_sha256: &str) -> Result<CertificationPdfReport, String> {
    run_for_platform(pdf_path, envelope_sha256, CertificationPlatform::Macos)
}

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let Some(pdf_path) = arguments.next() else {
        eprintln!("usage: verify_certification_pdf <export.pdf> <envelope-sha256>");
        return ExitCode::from(2);
    };
    let Some(envelope_sha256) = arguments.next() else {
        eprintln!("usage: verify_certification_pdf <export.pdf> <envelope-sha256>");
        return ExitCode::from(2);
    };
    let platform = match arguments.next() {
        Some(value) => match CertificationPlatform::parse(&value.to_string_lossy()) {
            Ok(platform) => platform,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
        },
        None => CertificationPlatform::Macos,
    };
    if arguments.next().is_some() {
        eprintln!(
            "usage: verify_certification_pdf <export.pdf> <envelope-sha256> [macos|windows|linux]"
        );
        return ExitCode::from(2);
    }
    let envelope_sha256 = envelope_sha256.to_string_lossy();
    match run_for_platform(Path::new(&pdf_path), &envelope_sha256, platform) {
        Ok(report) => match serde_json::to_string(&report) {
            Ok(encoded) => {
                println!("{encoded}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("could not encode validation report: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_sha256, run, CertificationPlatform, HEIGHT_POINTS, PAGE_COUNT, WIDTH_POINTS,
    };
    use bir_print::html_output::{create_pdf_export_temp, finalize_pdf_export, PdfExpectation};
    use lopdf::{dictionary, Document, Object, Stream};

    #[test]
    fn accepts_only_canonical_lowercase_sha256() {
        let mixed = "abcdef".repeat(10) + "abcd";
        assert!(canonical_sha256(&"0".repeat(64)));
        assert!(canonical_sha256(&mixed));
        assert!(!canonical_sha256(&"A".repeat(64)));
        assert!(!canonical_sha256(&"0".repeat(63)));
        assert!(!canonical_sha256(&"g".repeat(64)));
    }

    #[test]
    fn validates_a_real_owned_two_page_export() {
        let directory = tempfile::tempdir().expect("temporary PDF directory");
        let destination = directory.path().join("export.pdf");
        let temporary = create_pdf_export_temp(&destination).expect("export temporary path");
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let mut page_ids = Vec::new();
        for _ in 0..PAGE_COUNT {
            let content_id = document.add_object(Stream::new(dictionary! {}, b"BT ET".to_vec()));
            let page_id = document.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 936.into()],
                "CropBox" => vec![0.into(), 0.into(), 612.into(), 936.into()],
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
                "Count" => PAGE_COUNT as i64,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(&temporary).expect("raw PDF should save");
        let envelope_hash = "d".repeat(64);
        let expectation = PdfExpectation {
            form_code: "2551Q".to_string(),
            revision: "2018".to_string(),
            envelope_hash: envelope_hash.clone(),
            expected_page_count: PAGE_COUNT,
            width_points: WIDTH_POINTS,
            height_points: HEIGHT_POINTS,
        };
        finalize_pdf_export(&temporary, &destination, &expectation)
            .expect("owned PDF should finalize");

        let report = run(&destination, &envelope_hash).expect("owned verifier should pass");

        assert_eq!(report.actual_page_count, PAGE_COUNT);
        assert_eq!(report.width_points, WIDTH_POINTS);
        assert_eq!(report.height_points, HEIGHT_POINTS);
        assert_eq!(report.pages.len(), PAGE_COUNT);
        assert!(report.pages.iter().all(|page| page.content_byte_count > 0));
    }

    #[test]
    fn platform_scopes_are_explicit_and_the_legacy_cli_default_remains_macos() {
        assert_eq!(
            CertificationPlatform::Macos.scope(),
            "owned_macos_candidate_pdf_validation"
        );
        assert_eq!(
            CertificationPlatform::Windows.scope(),
            "owned_windows_candidate_pdf_validation"
        );
        assert_eq!(
            CertificationPlatform::parse("windows").expect("Windows should parse"),
            CertificationPlatform::Windows
        );
        assert_eq!(
            CertificationPlatform::Linux.scope(),
            "owned_linux_candidate_pdf_validation"
        );
        assert_eq!(
            CertificationPlatform::parse("linux").expect("Linux should parse"),
            CertificationPlatform::Linux
        );
        assert!(CertificationPlatform::parse("unknown").is_err());
    }
}
