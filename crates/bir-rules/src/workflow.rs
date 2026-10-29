use crate::{
    ContextFingerprint, FormRevisionKey, InputRevision, ValidationContext, WorkflowStateId,
    WorkflowTransitionId,
};
use serde::{Deserialize, Serialize};

/// A user-visible workflow action selected against one exact evaluated input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowAction {
    Edit,
    Save,
    Validate,
    FinalCopy,
    Submit,
    PrintPreview,
}

/// The presentation channel required by an official workflow transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowNotificationChannel {
    Alert,
}

/// Exact notification emitted as part of one reviewed workflow transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNotification {
    channel: WorkflowNotificationChannel,
    message: String,
    official_message: Option<String>,
}

impl WorkflowNotification {
    pub(crate) fn new(
        channel: WorkflowNotificationChannel,
        message: impl Into<String>,
        official_message: Option<String>,
    ) -> Self {
        Self {
            channel,
            message: message.into(),
            official_message,
        }
    }

    pub const fn channel(&self) -> WorkflowNotificationChannel {
        self.channel
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn official_message(&self) -> Option<&str> {
        self.official_message.as_deref()
    }
}

/// Request-bound transition output returned only after a complete successful
/// evaluation and an explicit current-state/action selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowTransitionResult {
    rule_set: FormRevisionKey,
    context: ValidationContext,
    input_revision: InputRevision,
    context_fingerprint: ContextFingerprint,
    transition_id: WorkflowTransitionId,
    from_state: WorkflowStateId,
    action: WorkflowAction,
    to_state: WorkflowStateId,
    notifications: Vec<WorkflowNotification>,
}

impl WorkflowTransitionResult {
    pub(crate) fn new(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        context_fingerprint: ContextFingerprint,
        transition_id: WorkflowTransitionId,
        from_state: WorkflowStateId,
        action: WorkflowAction,
        to_state: WorkflowStateId,
        notifications: Vec<WorkflowNotification>,
    ) -> Self {
        Self {
            rule_set,
            context,
            input_revision,
            context_fingerprint,
            transition_id,
            from_state,
            action,
            to_state,
            notifications,
        }
    }

    pub fn rule_set(&self) -> &FormRevisionKey {
        &self.rule_set
    }

    pub const fn context(&self) -> ValidationContext {
        self.context
    }

    pub const fn input_revision(&self) -> InputRevision {
        self.input_revision
    }

    pub const fn context_fingerprint(&self) -> ContextFingerprint {
        self.context_fingerprint
    }

    pub fn transition_id(&self) -> &WorkflowTransitionId {
        &self.transition_id
    }

    pub fn from_state(&self) -> &WorkflowStateId {
        &self.from_state
    }

    pub const fn action(&self) -> WorkflowAction {
        self.action
    }

    pub fn to_state(&self) -> &WorkflowStateId {
        &self.to_state
    }

    pub fn notifications(&self) -> &[WorkflowNotification] {
        &self.notifications
    }
}
