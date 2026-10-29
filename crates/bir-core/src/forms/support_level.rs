//! Rust-owned form capability registry.
//!
//! This module is the single source of truth for whether a form may be edited,
//! queued, rendered, or advertised as implemented.  A form code appearing in
//! the registry is inventory, not proof that it is production-ready.

use serde::{Deserialize, Serialize};

/// Independently reviewed capabilities for one exact form revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormCapabilities {
    pub typed_model: bool,
    pub xml_round_trip: bool,
    pub formula_evidence: bool,
    pub persistence: bool,
    pub queue_submission: bool,
    pub editor: bool,
    pub render_contract: bool,
    pub html_component: bool,
    pub html_spec: bool,
    pub pagination: bool,
    pub visual_parity: bool,
    pub native_preview: bool,
    pub native_print: bool,
    pub pdf_export: bool,
    pub packaged_offline: bool,
}

impl FormCapabilities {
    /// Semantic evidence required before a draft may cross the submission
    /// queue boundary.  Layout support is intentionally not part of this gate.
    pub const fn can_queue(self) -> bool {
        self.typed_model
            && self.xml_round_trip
            && self.formula_evidence
            && self.persistence
            && self.queue_submission
    }

    /// Every capability required for an HTML-only production form.
    pub const fn satisfies_release_gate(self) -> bool {
        self.can_queue()
            && self.editor
            && self.render_contract
            && self.html_component
            && self.html_spec
            && self.pagination
            && self.visual_parity
            && self.native_preview
            && self.native_print
            && self.pdf_export
            && self.packaged_offline
    }

    /// Evidence required to open a draft while an HTML-only form is still
    /// completing XML, platform, or package certification. XML round-trip is
    /// intentionally independent here: a reviewed semantic form may be used
    /// for manual/external filing without gaining queue authority. This gate
    /// must only be consumed by an explicitly non-production desktop build.
    pub const fn can_open_certification_draft(self) -> bool {
        self.typed_model
            && self.formula_evidence
            && self.persistence
            && self.editor
            && self.render_contract
            && self.html_component
            && self.html_spec
            && self.pagination
    }

    const fn has_in_app_scaffold(self) -> bool {
        self.typed_model || self.persistence || self.editor || self.html_component
    }
}

/// Capability evidence for one exact BIR form revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormCapabilityRecord {
    pub code: &'static str,
    pub revision: &'static str,
    pub form_id: &'static str,
    pub capabilities: FormCapabilities,
    /// Set only after the release evidence is reviewed and recorded.  This
    /// flag cannot override a missing capability.
    pub release_ready: bool,
}

impl FormCapabilityRecord {
    pub const fn support_level(self) -> FormSupportLevel {
        if self.release_ready && self.capabilities.satisfies_release_gate() {
            FormSupportLevel::ImplementedInApp
        } else if self.capabilities.has_in_app_scaffold() {
            FormSupportLevel::ScaffoldOnly
        } else {
            FormSupportLevel::ExternalOrManualOnly
        }
    }

    pub const fn can_queue(self) -> bool {
        self.capabilities.can_queue()
    }

    pub const fn can_open_certification_draft(self) -> bool {
        !self.release_ready && self.capabilities.can_open_certification_draft()
    }
}

const SCAFFOLD: FormCapabilities = FormCapabilities {
    typed_model: true,
    xml_round_trip: false,
    formula_evidence: false,
    persistence: true,
    queue_submission: false,
    editor: true,
    render_contract: false,
    html_component: false,
    html_spec: false,
    pagination: false,
    visual_parity: false,
    native_preview: false,
    native_print: false,
    pdf_export: false,
    packaged_offline: false,
};

/// Exact form revisions currently known to the desktop application.
///
/// The 2551Q semantic submission path is proven independently from its still
/// incomplete HTML release evidence. Other current views remain honest
/// manual/external scaffolds until every release and submission gate relevant
/// to their exact revision is completed.
pub const FORM_CAPABILITY_REGISTRY: &[FormCapabilityRecord] = &[
    FormCapabilityRecord {
        code: "2551Q",
        revision: "2018",
        form_id: "2551Qv2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            visual_parity: false,
            native_preview: false,
            native_print: false,
            pdf_export: false,
            packaged_offline: false,
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1601C",
        revision: "2018",
        form_id: "1601Cv2018",
        // XML evidence is an exact 100-field replay of the hash-locked
        // plaintext editable save. Its hash-locked encrypted companion proves
        // the reviewed #email# filename and the shared audited IAF crypto path.
        // Queue persistence freezes that exact field map behind a fingerprint
        // and durable claim token. Formula evidence covers the printed
        // arithmetic; penalty components remain reviewed manual inputs.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            visual_parity: false,
            native_preview: false,
            native_print: false,
            pdf_export: false,
            packaged_offline: false,
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2553",
        revision: "1999",
        // `form_id` is the official formType and PROD SFTP folder. The submit
        // plaintext is the official layout replayed through the full page
        // runtime; queueing goes through the generic `QueueableForm` path.
        form_id: "2553",
        // The desktop editor is views::form_2553_view; print preview fills the
        // one-page frozen 2553-1999 sheet through its writer-cells map.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1604F",
        revision: "2018",
        // Official formType and PROD SFTP folder (`ftpTargetFolder.PROD`).
        // Exact submit plaintext via the official layout; generic queue.
        form_id: "1604F",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1604C",
        revision: "2018",
        // Official formType and PROD SFTP folder (`ftpTargetFolder.PROD`).
        // Exact submit plaintext via the official layout; generic queue.
        form_id: "1604C",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1604E",
        revision: "2018",
        // Official formType and PROD SFTP folder (`ftpTargetFolder.PROD`).
        // Exact submit plaintext via the official layout; generic queue.
        form_id: "1604Ev2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "0619E",
        revision: "2018",
        // Official formType and PROD SFTP folder. Queueing goes through the
        // generic `QueueableForm` path with the official layout.
        form_id: "0619E",
        capabilities: FormCapabilities {
            queue_submission: true,
            xml_round_trip: true,
            formula_evidence: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            editor: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "0619F",
        revision: "2018",
        // Official formType and PROD SFTP folder. Queueing goes through the
        // generic `QueueableForm` path with the official layout.
        form_id: "0619F",
        capabilities: FormCapabilities {
            queue_submission: true,
            xml_round_trip: true,
            formula_evidence: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            editor: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "0605",
        revision: "1999",
        // Official formType and PROD SFTP folder; queued through the generic
        // `QueueableForm` path with the official layout plus the Item 6/8
        // popup radios the page adds.
        form_id: "0605",
        capabilities: FormCapabilities {
            queue_submission: true,
            xml_round_trip: true,
            formula_evidence: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            editor: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1701Q",
        revision: "2018",
        // Official formType and PROD SFTP folder. Queueing goes through the
        // generic `QueueableForm` path with the official layout.
        form_id: "1701Qv2018",
        capabilities: FormCapabilities {
            queue_submission: true,
            xml_round_trip: true,
            formula_evidence: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            editor: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2550Q",
        revision: "2024",
        // Official formType; queued through the generic `QueueableForm` path
        // with the official saveEncryptedProfile layout plus `<dateFiled>`.
        form_id: "2550Qv2024",
        capabilities: FormCapabilities {
            xml_round_trip: true,
            formula_evidence: true,
            queue_submission: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1701",
        revision: "2018",
        // Official formType and PROD SFTP folder; queued through the generic
        // `QueueableForm` path with the official submit layout.
        form_id: "1701v2018",
        capabilities: FormCapabilities {
            queue_submission: true,
            xml_round_trip: true,
            formula_evidence: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            editor: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1702RT",
        revision: "2018C",
        // Official formType and PROD SFTP folder. Queueing goes through the
        // generic `QueueableForm` path with the official layout.
        form_id: "1702RTv2018C",
        capabilities: FormCapabilities {
            queue_submission: true,
            xml_round_trip: true,
            formula_evidence: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            editor: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1702MX",
        revision: "2018C",
        // Official formType and PROD SFTP folder. Queueing goes through the
        // generic `QueueableForm` path (four base pages; the mandatory
        // attachments are not supported).
        form_id: "1702MXv2018C",
        capabilities: FormCapabilities {
            queue_submission: true,
            xml_round_trip: true,
            formula_evidence: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            editor: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1606",
        revision: "2018",
        // Official formType and PROD SFTP folder; submit plaintext replayed
        // through the full page runtime, queued on the generic path.
        form_id: "1606",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1600VT",
        revision: "2018",
        // Official formType and PROD SFTP folder; submit plaintext replayed
        // through the full page runtime, queued on the generic path.
        form_id: "1600VTv2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1600PT",
        revision: "2018",
        // Official formType and PROD SFTP folder; submit plaintext replayed
        // through the full page runtime, queued on the generic path.
        form_id: "1600PTv2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1600WP",
        revision: "2010",
        // Official formType and PROD SFTP folder; submit plaintext replayed
        // through the full page runtime, queued on the generic path.
        form_id: "1600WP",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1601EQ",
        revision: "2018",
        // `form_id` is the official formType and PROD SFTP folder; queueing
        // goes through the generic `QueueableForm` path.
        form_id: "1601EQ",
        // Print preview uses the 2019 sheet; print map joins fields by printed label.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1601FQ",
        revision: "2018",
        // `form_id` is the official formType and PROD SFTP folder; queueing
        // goes through the generic `QueueableForm` path.
        form_id: "1601FQ",
        // Print preview uses the 2020 sheet; print map joins fields by printed label.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1603Q",
        revision: "2018",
        // `form_id` is the official formType and PROD SFTP folder; queueing
        // goes through the generic `QueueableForm` path.
        form_id: "1603Qv2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1602Q",
        revision: "2018",
        // `form_id` is the official formType and PROD SFTP folder; queueing
        // goes through the generic `QueueableForm` path.
        form_id: "1602Qv2018",
        // Print preview uses the 2019 sheet; print map joins fields by printed label.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2552",
        revision: "2018",
        // Official formType and PROD SFTP folder; queued through the generic
        // `QueueableForm` path with the official layout replayed exactly.
        form_id: "2552v2018",
        // Editor: views::form_2552_view; print preview fills the
        // frozen sheet through its writer-cells map.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2550M",
        revision: "2007",
        // Official formType and PROD SFTP folder; every schedule (1–8) is
        // modelled and replayed byte for byte.
        form_id: "2550M",
        // Editor: views::form_2550m_view; print preview fills the
        // frozen sheet through its writer-cells map.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1706",
        revision: "2018",
        // Official formType and PROD SFTP folder; queued on the generic path.
        form_id: "1706",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1707",
        revision: "2021",
        // Official formType and PROD SFTP folder; queued on the generic path.
        form_id: "1707v2021",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1707A",
        revision: "2021",
        // Official formType and PROD SFTP folder; queued on the generic path.
        form_id: "1707Av2021",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1800",
        revision: "2018",
        // Official formType and PROD SFTP folder; queued on the generic path.
        form_id: "1800v2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1801",
        revision: "2018",
        // Official formType and PROD SFTP folder; queued on the generic path.
        form_id: "1801v2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2000",
        revision: "2018",
        // `form_id` is the official formType and PROD SFTP folder; the submit plaintext replays the official layout and queueing uses the generic `QueueableForm` path.
        form_id: "2000v2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2000OT",
        revision: "2018",
        // `form_id` is the official formType and PROD SFTP folder; the submit plaintext replays the official layout and queueing uses the generic `QueueableForm` path.
        form_id: "2000OTv2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2200S",
        revision: "2018",
        // `form_id` is the official formType and PROD SFTP folder; the submit plaintext replays the official layout and queueing uses the generic `QueueableForm` path.
        form_id: "2200S",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2200C",
        revision: "2018",
        // `form_id` is the official formType and PROD SFTP folder; the submit plaintext replays the official layout and queueing uses the generic `QueueableForm` path.
        form_id: "2200Cv2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2200AN",
        revision: "2018",
        // `form_id` is the official formType. Queueing stays off:
        // ftpTargetFolder.PROD has no '2200ANv2018' folder (getFtpFolder
        // returns undefined), so the SFTP target is unconfirmed.
        form_id: "2200ANv2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: false,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2200M",
        revision: "2018",
        // `form_id` is the official formType (filename segment). PROD
        // `ftpTargetFolder` has no `2200Mv2018` key; see form_2200m.
        // Queueing stays off: ftpTargetFolder.PROD has no '2200Mv2018' folder
        // (getFtpFolder returns undefined), so the SFTP target is unconfirmed.
        form_id: "2200Mv2018",
        // The desktop editor is views::form_2200m_view; print preview fills the
        // two-page frozen 2200m-2018 sheet through its writer-cells map.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: false,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2200P",
        revision: "2020",
        // `form_id` is the official formType (filename segment).
        // Queueing stays off: ftpTargetFolder.PROD has no '2200Pv2020' folder
        // (getFtpFolder returns undefined), so the SFTP target is unconfirmed.
        form_id: "2200Pv2020",
        // The desktop editor is views::form_2200p_view.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: false,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2200A",
        revision: "2020",
        // `form_id` is the official formType (filename segment).
        // Queueing stays off: ftpTargetFolder.PROD has no '2200Av2020' folder
        // (getFtpFolder returns undefined), so the SFTP target is unconfirmed.
        form_id: "2200Av2020",
        // The desktop editor is views::form_2200a_view.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: false,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "2200T",
        revision: "2020",
        // `form_id` is the official formType (filename segment).
        form_id: "2200Tv2020",
        // Print preview uses the 2022 sheet; print map joins fields by printed label.
        // The desktop editor is views::form_2200t_view.
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1701MS",
        revision: "2024",
        // Official formType and PROD SFTP folder. The submit plaintext is the
        // official layout (radios, check boxes and the name) replayed through
        // the full page runtime; queueing uses the generic path.
        form_id: "1701MS",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1702Q",
        revision: "2018C",
        // Official formType and PROD SFTP folder. The submit plaintext is the
        // official layout replayed through the full page runtime; queueing
        // uses the generic path.
        form_id: "1702Qv2018C",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1701A",
        revision: "2018",
        // Official formType and PROD SFTP folder. The submit plaintext is the
        // official layout replayed through the full page runtime; queueing
        // uses the generic path.
        form_id: "1701A",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1700",
        revision: "2018",
        // Official formType and PROD SFTP folder. The submit plaintext is the
        // official layout replayed through the full page runtime; queueing
        // uses the generic path.
        form_id: "1700v2018",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
    FormCapabilityRecord {
        code: "1702EX",
        revision: "2018C",
        // Official formType and PROD SFTP folder. The submit plaintext is the
        // official layout replayed through the full page runtime; queueing
        // uses the generic path.
        form_id: "1702EXv2018C",
        capabilities: FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            ..SCAFFOLD
        },
        release_ready: false,
    },
];

/// Whether the app can actually draft/file a given form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormSupportLevel {
    ImplementedInApp,
    ScaffoldOnly,
    ExternalOrManualOnly,
    Planned,
}

impl FormSupportLevel {
    pub fn action_label(&self) -> &'static str {
        match self {
            Self::ImplementedInApp => "File in App",
            Self::ScaffoldOnly | Self::ExternalOrManualOnly => "Manual / external filing",
            Self::Planned => "Coming soon",
        }
    }

    pub fn is_fileable_in_app(&self) -> bool {
        matches!(self, Self::ImplementedInApp)
    }
}

pub fn find_form_capability(form_code: &str) -> Option<&'static FormCapabilityRecord> {
    FORM_CAPABILITY_REGISTRY
        .iter()
        .find(|record| record.code == form_code)
}

pub fn find_form_capability_by_id(form_id: &str) -> Option<&'static FormCapabilityRecord> {
    FORM_CAPABILITY_REGISTRY
        .iter()
        .find(|record| record.form_id == form_id)
}

pub fn form_support_level(form_code: &str) -> FormSupportLevel {
    find_form_capability(form_code)
        .map(|record| record.support_level())
        .unwrap_or(FormSupportLevel::ExternalOrManualOnly)
}

pub fn can_queue_for_submission(form_code: &str) -> bool {
    find_form_capability(form_code).is_some_and(|record| record.can_queue())
}

/// Whether a non-production desktop build may open this form for HTML release
/// certification. Production callers must continue to use
/// [`FormSupportLevel::is_fileable_in_app`].
pub fn can_open_certification_draft(form_code: &str) -> bool {
    find_form_capability(form_code).is_some_and(|record| record.can_open_certification_draft())
}

/// Resolve the exact transport identifier only when the semantic submission
/// queue gate is satisfied. This does not claim HTML release readiness.
pub fn queue_authorized_form_type_id(form_code: &str) -> Option<&'static str> {
    find_form_capability(form_code)
        .filter(|record| record.can_queue())
        .map(|record| record.form_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_has_unique_exact_codes_and_form_ids() {
        let mut codes = HashSet::new();
        let mut form_ids = HashSet::new();
        for record in FORM_CAPABILITY_REGISTRY {
            assert!(codes.insert(record.code), "duplicate code {}", record.code);
            assert!(
                form_ids.insert(record.form_id),
                "duplicate form id {}",
                record.form_id
            );
            assert_eq!(find_form_capability_by_id(record.form_id), Some(record));
        }
    }

    #[test]
    fn implemented_requires_every_release_capability() {
        let complete = FormCapabilities {
            typed_model: true,
            xml_round_trip: true,
            formula_evidence: true,
            persistence: true,
            queue_submission: true,
            editor: true,
            render_contract: true,
            html_component: true,
            html_spec: true,
            pagination: true,
            visual_parity: true,
            native_preview: true,
            native_print: true,
            pdf_export: true,
            packaged_offline: true,
        };
        let complete_record = FormCapabilityRecord {
            code: "TEST",
            revision: "1",
            form_id: "TESTv1",
            capabilities: complete,
            release_ready: true,
        };
        assert_eq!(
            complete_record.support_level(),
            FormSupportLevel::ImplementedInApp
        );

        let gates = [
            |c: &mut FormCapabilities| c.typed_model = false,
            |c: &mut FormCapabilities| c.xml_round_trip = false,
            |c: &mut FormCapabilities| c.formula_evidence = false,
            |c: &mut FormCapabilities| c.persistence = false,
            |c: &mut FormCapabilities| c.queue_submission = false,
            |c: &mut FormCapabilities| c.editor = false,
            |c: &mut FormCapabilities| c.render_contract = false,
            |c: &mut FormCapabilities| c.html_component = false,
            |c: &mut FormCapabilities| c.html_spec = false,
            |c: &mut FormCapabilities| c.pagination = false,
            |c: &mut FormCapabilities| c.visual_parity = false,
            |c: &mut FormCapabilities| c.native_preview = false,
            |c: &mut FormCapabilities| c.native_print = false,
            |c: &mut FormCapabilities| c.pdf_export = false,
            |c: &mut FormCapabilities| c.packaged_offline = false,
        ];
        for remove_gate in gates {
            let mut capabilities = complete;
            remove_gate(&mut capabilities);
            assert_ne!(
                FormCapabilityRecord {
                    capabilities,
                    ..complete_record
                }
                .support_level(),
                FormSupportLevel::ImplementedInApp
            );
        }
    }

    #[test]
    fn queue_gate_is_semantic_and_registry_owned() {
        // Every record that claims the queue capabilities opens the gate with
        // its own official form type; everything else fails closed.
        let mut queueable = 0;
        for record in FORM_CAPABILITY_REGISTRY {
            if record.capabilities.can_queue() {
                queueable += 1;
                assert!(can_queue_for_submission(record.code), "{}", record.code);
                assert_eq!(
                    queue_authorized_form_type_id(record.code),
                    Some(record.form_id),
                    "{}",
                    record.code
                );
            } else {
                assert!(
                    !can_queue_for_submission(record.code),
                    "{} must fail closed",
                    record.code
                );
                assert_eq!(queue_authorized_form_type_id(record.code), None);
            }
        }
        assert!(queueable >= 3);
        assert_eq!(queue_authorized_form_type_id("2551Q"), Some("2551Qv2018"));
        assert_eq!(queue_authorized_form_type_id("1601C"), Some("1601Cv2018"));
        // 2550Q left the application freeze through the generic queue.
        assert!(can_queue_for_submission("2550Q"));
        assert_eq!(queue_authorized_form_type_id("2550Q"), Some("2550Qv2024"));
        assert!(!can_queue_for_submission("9999"), "9999 must fail closed");
        assert_eq!(queue_authorized_form_type_id("9999"), None);
        // No PROD ftpTargetFolder key for these formTypes: queueing stays off.
        for code in ["2200AN", "2200M", "2200P", "2200A"] {
            assert!(!can_queue_for_submission(code), "{code} must not queue");
            assert_eq!(queue_authorized_form_type_id(code), None, "{code}");
        }
    }

    #[test]
    fn certification_draft_gate_does_not_claim_release_readiness() {
        assert!(can_open_certification_draft("2551Q"));
        for code in [
            "1601C", "0619E", "0619F", "0605", "1701Q", "2550Q", "1701", "1702RT", "1702MX",
            "2553", "1601C", "0619E", "0619F", "0605", "1701Q", "2550Q", "1701", "1702RT",
            "1702MX", "2553", "2552", "2550M", "1601C", "0619E", "0619F", "0605", "1701Q", "2550Q",
            "1701", "1702RT", "1702MX", "2553", "2553", "1606", "1600VT", "1600PT", "1600WP",
            "2553", "1701MS", "1702Q", "1701A", "1700", "1702EX", "1604F", "1604C", "1604E",
            "1601EQ", "1601FQ", "1602Q", "2200T",
        ] {
            assert!(
                can_open_certification_draft(code),
                "{code} has a semantic HTML certification path"
            );
        }
        assert_eq!(form_support_level("2551Q"), FormSupportLevel::ScaffoldOnly);
        assert_eq!(form_support_level("1601C"), FormSupportLevel::ScaffoldOnly);
        assert_eq!(form_support_level("0619E"), FormSupportLevel::ScaffoldOnly);
        assert_eq!(form_support_level("0619F"), FormSupportLevel::ScaffoldOnly);
        assert_eq!(form_support_level("0605"), FormSupportLevel::ScaffoldOnly);
        assert_eq!(form_support_level("1701Q"), FormSupportLevel::ScaffoldOnly);

        assert!(!can_open_certification_draft("9999"));
    }

    #[test]
    fn current_matrix_is_honest_until_html_release_evidence_exists() {
        for record in FORM_CAPABILITY_REGISTRY {
            assert!(!record.release_ready);
            assert_eq!(record.support_level(), FormSupportLevel::ScaffoldOnly);
            assert_eq!(
                form_support_level(record.code).action_label(),
                "Manual / external filing"
            );
        }
    }

    #[test]
    fn payment_form_uses_canonical_1999_identity() {
        let form = find_form_capability("0605").expect("0605 inventory record");
        assert_eq!(form.revision, "1999");
        assert_eq!(form.form_id, "0605");
        assert!(find_form_capability_by_id("0605v2018").is_none());
    }
}
