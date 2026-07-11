//! `send_message`: agent-facing direct-message write. P218 covers durable
//! direct messages; p221 adds local attachment metadata.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SurfaceError;
use crate::host::{DirectMessageInput, RunHost};
use crate::tools::attachments::normalize_local_attachments;

#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageInput {
    #[serde(default, alias = "from")]
    pub from_agent: Option<String>,
    pub to: String,
    pub summary: String,
    pub full: String,
    #[serde(default, rename = "type", alias = "messageType")]
    pub message_type: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SendMessageOutput {
    pub ok: bool,
    pub message: Value,
}

/// Send one durable direct message.
///
/// # Errors
/// [`SurfaceError::BadRequest`] for invalid input; [`SurfaceError::Internal`]
/// for host/store or encoding failures.
pub async fn send_message(
    host: &dyn RunHost,
    input: SendMessageInput,
) -> Result<SendMessageOutput, SurfaceError> {
    let from = clean_required(input.from_agent, "from_agent required")?;
    let to = clean_required(Some(input.to), "to required")?;
    let summary = clean_required(Some(input.summary), "summary required")?;
    let full = clean_required(Some(input.full), "full required")?;
    let message_type = match clean_opt(input.message_type) {
        Some(value) if matches!(value.as_str(), "request" | "inform" | "reply") => value,
        Some(value) => {
            return Err(SurfaceError::BadRequest(format!(
                "type must be one of request, inform, reply; got {value}"
            )));
        }
        None => default_message_type(),
    };
    let priority = match clean_opt(input.priority) {
        Some(value) if matches!(value.as_str(), "normal" | "high" | "urgent") => value,
        Some(value) => {
            return Err(SurfaceError::BadRequest(format!(
                "priority must be one of normal, high, urgent; got {value}"
            )));
        }
        None => default_priority(),
    };
    let attachments = normalize_local_attachments(input.attachments)?;

    let message = host
        .post_direct_message(DirectMessageInput {
            message_id: None,
            ts: None,
            from,
            to,
            message_type: Some(message_type),
            priority: Some(priority),
            summary,
            full,
            reply_to: clean_opt(input.reply_to),
            source: Some("api".to_string()),
            source_room: None,
            sender_mxid: None,
            trust_level: None,
            from_id: None,
            schema: None,
            attachments,
        })
        .await?;
    let message = serde_json::to_value(message)
        .map_err(|e| SurfaceError::Internal(format!("encode sent message: {e}")))?;
    Ok(SendMessageOutput { ok: true, message })
}

pub(crate) fn clean_required(value: Option<String>, message: &str) -> Result<String, SurfaceError> {
    clean_opt(value).ok_or_else(|| SurfaceError::BadRequest(message.to_string()))
}

pub(crate) fn clean_opt(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

pub(crate) fn default_message_type() -> String {
    "inform".to_string()
}

pub(crate) fn default_priority() -> String {
    "normal".to_string()
}
