//! Non-promotional runtime observations for a macOS certification candidate.
//!
//! This contract is deliberately smaller than a platform attestation. The
//! running candidate can bind native output to its own immutable WebView
//! identity, nonce, geometry, callback payloads, and validated PDF. It cannot
//! attest its own package, the visible toolbar/save chooser, or a completed
//! CUPS job; those remain external collector responsibilities.
//! The canonical closed JSON contract is the committed
//! `macos-candidate-runtime-observation-v1.schema.json`; these types do not
//! expose a broader generated schema.

use crate::html_output::PdfValidationReport;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RendererPageRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub client_width: f64,
    pub client_height: f64,
    pub scroll_width: f64,
    pub scroll_height: f64,
    pub descendant_overflow_x: usize,
    pub descendant_overflow_y: usize,
    pub descendant_clipped_x: usize,
    pub descendant_clipped_y: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RendererGeometryReport {
    pub page_count: usize,
    pub page_width_pt: f64,
    pub page_height_pt: f64,
    pub pages: Vec<RendererPageRect>,
}

pub const MACOS_CANDIDATE_RUNTIME_OBSERVATION_SCHEMA_VERSION: u8 = 1;
pub const MACOS_CANDIDATE_RUNTIME_OBSERVATION_SCOPE: &str = "macos_candidate_runtime_observation";
const FORM_CODE: &str = "2551Q";
const FORM_REVISION: &str = "2018";
const EXPECTED_PAGE_COUNT: usize = 2;
const EXPECTED_WIDTH_POINTS: f64 = 612.0;
const EXPECTED_HEIGHT_POINTS: f64 = 936.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosCandidateRuntimeObservationV1 {
    pub schema_version: u8,
    pub scope: String,
    pub promotion_eligible: bool,
    pub trusted_producer: bool,
    /// SHA-256 of the collector-generated challenge supplied at process launch.
    pub collector_challenge_sha256: String,
    pub form_code: String,
    pub form_revision: String,
    /// SHA-256 of the host-generated random document run identifier.
    pub document_run_id_sha256: String,
    pub envelope_sha256: String,
    pub render_epoch: u64,
    pub readiness_revision: u64,
    pub issued_nonce: u64,
    pub preflight_consumptions: Vec<u64>,
    pub backend_completion_nonce: u64,
    pub started_at_unix_ms: u64,
    pub completed_at_unix_ms: u64,
    pub geometry_reports: [CertificationGeometryReportV1; 2],
    pub output: MacosCandidateOutputObservationV1,
    /// These exact gaps are mandatory. This record can never promote a renderer.
    pub strict_verifier_gaps: [CertificationVerifierGapV1; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificationVerifierGapV1 {
    RuntimeSelfAuthored,
    ExternalUiAndPrintRequired,
    ExternalCandidateBindingRequired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MacosCandidateOutputObservationV1 {
    PdfExportSucceeded {
        wkpdf_pages: Vec<CertificationPagePayloadV1>,
        output_pdf_sha256: String,
        output_pdf_byte_count: u64,
        pdf_validation: CertificationPdfValidationV1,
        destination_before: CertificationDestinationSnapshotV1,
        destination_after: CertificationDestinationSnapshotV1,
        temporary_file_remaining: bool,
    },
    SystemPrintCompleted {
        appkit_completion_succeeded: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum CertificationDestinationSnapshotV1 {
    Absent,
    File {
        sha256: String,
    },
    Unavailable {
        reason_code: CertificationDestinationUnavailableReasonV1,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificationDestinationUnavailableReasonV1 {
    MetadataReadFailed,
    NotRegularFile,
    FileReadFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationPagePayloadV1 {
    pub page: usize,
    pub byte_count: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationPdfValidationV1 {
    pub page_count: usize,
    pub width_points: f64,
    pub height_points: f64,
    pub content_nonempty: bool,
    pub validated_by: String,
}

impl From<&PdfValidationReport> for CertificationPdfValidationV1 {
    fn from(value: &PdfValidationReport) -> Self {
        Self {
            page_count: value.page_count,
            width_points: value.width_points,
            height_points: value.height_points,
            content_nonempty: true,
            validated_by: "bir-print::html_output::validate_pdf_file".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationGeometryReportV1 {
    pub page_count: usize,
    pub page_width_pt: f64,
    pub page_height_pt: f64,
    pub pages: Vec<CertificationGeometryPageV1>,
}

impl From<&RendererGeometryReport> for CertificationGeometryReportV1 {
    fn from(report: &RendererGeometryReport) -> Self {
        Self {
            page_count: report.page_count,
            page_width_pt: report.page_width_pt,
            page_height_pt: report.page_height_pt,
            pages: report
                .pages
                .iter()
                .map(CertificationGeometryPageV1::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationGeometryPageV1 {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub client_width: f64,
    pub client_height: f64,
    pub scroll_width: f64,
    pub scroll_height: f64,
    pub descendant_overflow_x: usize,
    pub descendant_overflow_y: usize,
    pub descendant_clipped_x: usize,
    pub descendant_clipped_y: usize,
}

impl From<&RendererPageRect> for CertificationGeometryPageV1 {
    fn from(page: &RendererPageRect) -> Self {
        Self {
            x: page.x,
            y: page.y,
            width: page.width,
            height: page.height,
            client_width: page.client_width,
            client_height: page.client_height,
            scroll_width: page.scroll_width,
            scroll_height: page.scroll_height,
            descendant_overflow_x: page.descendant_overflow_x,
            descendant_overflow_y: page.descendant_overflow_y,
            descendant_clipped_x: page.descendant_clipped_x,
            descendant_clipped_y: page.descendant_clipped_y,
        }
    }
}

impl MacosCandidateRuntimeObservationV1 {
    pub fn validate_non_promotional(&self) -> Result<(), String> {
        if self.schema_version != MACOS_CANDIDATE_RUNTIME_OBSERVATION_SCHEMA_VERSION
            || self.scope != MACOS_CANDIDATE_RUNTIME_OBSERVATION_SCOPE
        {
            return Err("runtime observation has an unsupported schema or scope".to_string());
        }
        if self.promotion_eligible || self.trusted_producer {
            return Err(
                "runtime observation must remain non-promotional and untrusted".to_string(),
            );
        }
        require_sha256(&self.collector_challenge_sha256, "collector challenge hash")?;
        if self.form_code != FORM_CODE || self.form_revision != FORM_REVISION {
            return Err("runtime observation must target exactly 2551Q:2018".to_string());
        }
        require_sha256(&self.document_run_id_sha256, "document run identifier hash")?;
        require_sha256(&self.envelope_sha256, "envelope hash")?;
        if self.render_epoch == 0 || self.readiness_revision == 0 {
            return Err("runtime observation requires a validated renderer epoch".to_string());
        }
        if self.issued_nonce == 0
            || self.preflight_consumptions.as_slice() != [self.issued_nonce]
            || self.backend_completion_nonce != self.issued_nonce
        {
            return Err(
                "runtime observation nonce was not consumed and completed exactly once".to_string(),
            );
        }
        if self.started_at_unix_ms == 0 || self.completed_at_unix_ms < self.started_at_unix_ms {
            return Err("runtime observation timestamps are invalid".to_string());
        }
        validate_geometry(&self.geometry_reports)?;
        validate_output(&self.output, self.geometry_reports[0].page_count)?;
        if self.strict_verifier_gaps
            != [
                CertificationVerifierGapV1::RuntimeSelfAuthored,
                CertificationVerifierGapV1::ExternalUiAndPrintRequired,
                CertificationVerifierGapV1::ExternalCandidateBindingRequired,
            ]
        {
            return Err("runtime observation omitted or reordered a mandatory gap".to_string());
        }
        Ok(())
    }
}

fn validate_geometry(reports: &[CertificationGeometryReportV1; 2]) -> Result<(), String> {
    if reports[0] != reports[1] {
        return Err("runtime geometry observations are not identical".to_string());
    }
    let report = &reports[0];
    if report.page_count != EXPECTED_PAGE_COUNT
        || report.pages.len() != EXPECTED_PAGE_COUNT
        || report.page_width_pt != EXPECTED_WIDTH_POINTS
        || report.page_height_pt != EXPECTED_HEIGHT_POINTS
    {
        return Err("runtime geometry is not the exact two-page 2551Q layout".to_string());
    }
    for page in &report.pages {
        let numbers = [
            page.x,
            page.y,
            page.width,
            page.height,
            page.client_width,
            page.client_height,
            page.scroll_width,
            page.scroll_height,
        ];
        if numbers.iter().any(|value| !value.is_finite())
            || page.width <= 0.0
            || page.height <= 0.0
            || page.descendant_overflow_x != 0
            || page.descendant_overflow_y != 0
            || page.descendant_clipped_x != 0
            || page.descendant_clipped_y != 0
        {
            return Err(
                "runtime geometry contains clipping, overflow, or invalid values".to_string(),
            );
        }
    }
    Ok(())
}

fn validate_output(
    output: &MacosCandidateOutputObservationV1,
    page_count: usize,
) -> Result<(), String> {
    match output {
        MacosCandidateOutputObservationV1::PdfExportSucceeded {
            wkpdf_pages,
            output_pdf_sha256,
            output_pdf_byte_count,
            pdf_validation,
            destination_before,
            destination_after,
            temporary_file_remaining,
        } => {
            validate_destination_snapshot(destination_before)?;
            validate_destination_snapshot(destination_after)?;
            require_sha256(output_pdf_sha256, "output PDF hash")?;
            if *output_pdf_byte_count == 0 || *temporary_file_remaining {
                return Err("PDF observation has empty output or a remaining temp file".to_string());
            }
            if wkpdf_pages.len() != page_count {
                return Err("PDF observation omitted a WKPDF callback page".to_string());
            }
            for (index, page) in wkpdf_pages.iter().enumerate() {
                if page.page != index + 1 || page.byte_count == 0 {
                    return Err("WKPDF page payload metadata is not canonical".to_string());
                }
                require_sha256(&page.sha256, "WKPDF page hash")?;
            }
            if pdf_validation.page_count != page_count
                || pdf_validation.width_points != EXPECTED_WIDTH_POINTS
                || pdf_validation.height_points != EXPECTED_HEIGHT_POINTS
                || !pdf_validation.content_nonempty
                || pdf_validation.validated_by != "bir-print::html_output::validate_pdf_file"
            {
                return Err("PDF observation lacks the owned validation result".to_string());
            }
            match destination_after {
                CertificationDestinationSnapshotV1::File { sha256 }
                    if sha256 == output_pdf_sha256 => {}
                _ => {
                    return Err(
                        "PDF destination snapshot differs from the finalized output".to_string()
                    );
                }
            }
        }
        MacosCandidateOutputObservationV1::SystemPrintCompleted {
            appkit_completion_succeeded,
        } if !appkit_completion_succeeded => {
            return Err("system print observation did not complete successfully".to_string());
        }
        MacosCandidateOutputObservationV1::SystemPrintCompleted { .. } => {}
    }
    Ok(())
}

fn validate_destination_snapshot(
    snapshot: &CertificationDestinationSnapshotV1,
) -> Result<(), String> {
    if let CertificationDestinationSnapshotV1::File { sha256 } = snapshot {
        require_sha256(sha256, "destination snapshot hash")?;
    }
    Ok(())
}

fn require_sha256(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{label} is not a canonical SHA-256 digest"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> [CertificationGeometryReportV1; 2] {
        let report = CertificationGeometryReportV1 {
            page_count: 2,
            page_width_pt: 612.0,
            page_height_pt: 936.0,
            pages: (0..2)
                .map(|page| CertificationGeometryPageV1 {
                    x: 0.0,
                    y: page as f64 * 1248.0,
                    width: 816.0,
                    height: 1248.0,
                    client_width: 816.0,
                    client_height: 1248.0,
                    scroll_width: 816.0,
                    scroll_height: 1248.0,
                    descendant_overflow_x: 0,
                    descendant_overflow_y: 0,
                    descendant_clipped_x: 0,
                    descendant_clipped_y: 0,
                })
                .collect(),
        };
        [report.clone(), report]
    }

    fn observation() -> MacosCandidateRuntimeObservationV1 {
        MacosCandidateRuntimeObservationV1 {
            schema_version: 1,
            scope: MACOS_CANDIDATE_RUNTIME_OBSERVATION_SCOPE.to_string(),
            promotion_eligible: false,
            trusted_producer: false,
            collector_challenge_sha256: "a".repeat(64),
            form_code: "2551Q".to_string(),
            form_revision: "2018".to_string(),
            document_run_id_sha256: "f".repeat(64),
            envelope_sha256: "b".repeat(64),
            render_epoch: 4,
            readiness_revision: 2,
            issued_nonce: 7,
            preflight_consumptions: vec![7],
            backend_completion_nonce: 7,
            started_at_unix_ms: 1,
            completed_at_unix_ms: 2,
            geometry_reports: geometry(),
            output: MacosCandidateOutputObservationV1::PdfExportSucceeded {
                wkpdf_pages: vec![
                    CertificationPagePayloadV1 {
                        page: 1,
                        byte_count: 10,
                        sha256: "c".repeat(64),
                    },
                    CertificationPagePayloadV1 {
                        page: 2,
                        byte_count: 20,
                        sha256: "d".repeat(64),
                    },
                ],
                output_pdf_sha256: "e".repeat(64),
                output_pdf_byte_count: 30,
                pdf_validation: CertificationPdfValidationV1 {
                    page_count: 2,
                    width_points: 612.0,
                    height_points: 936.0,
                    content_nonempty: true,
                    validated_by: "bir-print::html_output::validate_pdf_file".to_string(),
                },
                destination_before: CertificationDestinationSnapshotV1::Absent,
                destination_after: CertificationDestinationSnapshotV1::File {
                    sha256: "e".repeat(64),
                },
                temporary_file_remaining: false,
            },
            strict_verifier_gaps: [
                CertificationVerifierGapV1::RuntimeSelfAuthored,
                CertificationVerifierGapV1::ExternalUiAndPrintRequired,
                CertificationVerifierGapV1::ExternalCandidateBindingRequired,
            ],
        }
    }

    #[test]
    fn accepts_closed_non_promotional_pdf_observation() {
        let observation = observation();
        observation
            .validate_non_promotional()
            .expect("valid runtime observation");
        let encoded = serde_json::to_value(&observation).expect("serialize observation");
        assert!(encoded.get("destination_path").is_none());
        assert!(encoded.get("envelope_json").is_none());
        assert!(encoded.get("taxpayer").is_none());
    }

    #[test]
    fn rejects_promotion_nonce_reuse_and_geometry_drift() {
        let mut promoted = observation();
        promoted.promotion_eligible = true;
        assert!(promoted.validate_non_promotional().is_err());

        let mut reused = observation();
        reused.preflight_consumptions.push(7);
        assert!(reused.validate_non_promotional().is_err());

        let mut drifted = observation();
        drifted.geometry_reports[1].pages[0].width += 1.0;
        assert!(drifted.validate_non_promotional().is_err());
    }

    #[test]
    fn rejects_unknown_fields_and_missing_mandatory_gap() {
        let mut value = serde_json::to_value(observation()).expect("serialize observation");
        value
            .as_object_mut()
            .expect("observation object")
            .insert("unexpected".to_string(), true.into());
        assert!(serde_json::from_value::<MacosCandidateRuntimeObservationV1>(value).is_err());

        let mut missing = observation();
        missing.strict_verifier_gaps[0] = CertificationVerifierGapV1::ExternalUiAndPrintRequired;
        assert!(missing.validate_non_promotional().is_err());
    }

    #[test]
    fn accepts_only_successful_system_print_callback() {
        let mut value = observation();
        value.output = MacosCandidateOutputObservationV1::SystemPrintCompleted {
            appkit_completion_succeeded: true,
        };
        value.validate_non_promotional().expect("successful print");
        value.output = MacosCandidateOutputObservationV1::SystemPrintCompleted {
            appkit_completion_succeeded: false,
        };
        assert!(value.validate_non_promotional().is_err());
    }
}
