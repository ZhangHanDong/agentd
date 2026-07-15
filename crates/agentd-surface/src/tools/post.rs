//! `post`: create one durable group message.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::SurfaceError;
use crate::host::{GroupMessageInput, GroupRecord, RunHost};
use crate::tools::attachments::normalize_local_attachments;
use crate::tools::send_message::{
    clean_opt, clean_required, default_message_type, default_priority,
};

#[derive(Debug, Clone, Deserialize)]
pub struct PostInput {
    #[serde(default, alias = "from")]
    pub from_agent: Option<String>,
    pub group: String,
    pub summary: String,
    pub full: String,
    #[serde(default, rename = "type", alias = "messageType")]
    pub message_type: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
    #[serde(default)]
    pub attachments: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PostOutput {
    pub ok: bool,
    pub id: String,
    pub warnings: Vec<Value>,
    pub delivery: GroupMessageDelivery,
    #[serde(rename = "taskGraph")]
    pub task_graph: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupMessageDelivery {
    pub suppressed: Vec<String>,
    #[serde(rename = "targetKind")]
    pub target_kind: Option<String>,
}

/// Post one group message.
///
/// # Errors
/// [`SurfaceError::BadRequest`] for invalid input, [`SurfaceError::NotFound`]
/// for unknown sender/group, [`SurfaceError::Forbidden`] for non-members.
pub async fn post(host: &dyn RunHost, input: PostInput) -> Result<PostOutput, SurfaceError> {
    let mut input = input;
    let attachments = normalize_local_attachments(std::mem::take(&mut input.attachments))?;
    post_with_normalized_attachments(host, input, attachments).await
}

pub(crate) async fn post_with_normalized_attachments(
    host: &dyn RunHost,
    input: PostInput,
    attachments: Vec<Value>,
) -> Result<PostOutput, SurfaceError> {
    let from = clean_required(input.from_agent, "from_agent required")?;
    let sender_is_system = from == "system";
    if !sender_is_system && host.get_agent(&from).await?.is_none() {
        return Err(SurfaceError::NotFound);
    }

    let group_name = clean_required(Some(input.group), "group required")?;
    let Some(group) = host.get_group(&group_name).await? else {
        return Err(SurfaceError::NotFound);
    };
    if !sender_is_system && !group_has_member(&group, &from) {
        return Err(SurfaceError::Forbidden);
    }

    let summary = clean_required(Some(input.summary), "summary required")?;
    let full = clean_required(Some(input.full), "full required")?;
    let message_type = match clean_opt(input.message_type) {
        Some(value) if matches!(value.as_str(), "request" | "inform" | "reply" | "human") => value,
        Some(value) => {
            return Err(SurfaceError::BadRequest(format!(
                "type must be one of request, inform, reply, human; got {value}"
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

    let mentions = clean_strings(input.mentions);
    let MentionResolution {
        delivered,
        suppressed,
        warnings,
    } = resolve_mentions(host, &group, &from, &mentions).await;
    let message = host
        .post_group_message(GroupMessageInput {
            message_id: None,
            ts: None,
            from,
            group: group_name,
            message_type: Some(message_type),
            priority: Some(priority),
            summary,
            full,
            mentions: delivered,
            reply_to: clean_opt(input.reply_to),
            source: Some("api".to_string()),
            schema: input.schema,
            attachments,
        })
        .await?;

    Ok(PostOutput {
        ok: true,
        id: message.id,
        warnings,
        delivery: GroupMessageDelivery {
            suppressed,
            target_kind: None,
        },
        task_graph: None,
    })
}

pub(crate) fn group_has_member(group: &GroupRecord, agent_id: &str) -> bool {
    group
        .members
        .iter()
        .any(|member| member.eq_ignore_ascii_case(agent_id))
}

pub(crate) fn clean_strings(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let Some(value) = clean_opt(Some(value)) else {
            continue;
        };
        if !out
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&value))
        {
            out.push(value);
        }
    }
    out
}

pub(crate) struct MentionResolution {
    pub delivered: Vec<String>,
    pub suppressed: Vec<String>,
    pub warnings: Vec<Value>,
}

pub(crate) async fn resolve_mentions(
    host: &dyn RunHost,
    group: &GroupRecord,
    sender: &str,
    mentions: &[String],
) -> MentionResolution {
    let mut delivered = Vec::new();
    let mut suppressed = Vec::new();
    let mut out_of_group = Vec::new();
    let mut unknown = Vec::new();
    for mention in mentions {
        if mention.eq_ignore_ascii_case(sender) {
            continue;
        }
        let is_member = group_has_member(group, mention);
        let exists = matches!(host.get_agent(mention).await, Ok(Some(_)));
        if is_member {
            if !delivered
                .iter()
                .any(|value: &String| value.eq_ignore_ascii_case(mention))
            {
                delivered.push(mention.clone());
            }
        } else if exists {
            out_of_group.push(json!({ "target": mention, "reason": "not-in-group" }));
            if !suppressed
                .iter()
                .any(|value: &String| value.eq_ignore_ascii_case(mention))
            {
                suppressed.push(mention.clone());
            }
        } else {
            unknown.push(json!({ "target": mention, "reason": "not-found" }));
        }
    }
    let mut warnings = Vec::new();
    if !out_of_group.is_empty() {
        warnings.push(json!({ "code": "mentions_not_in_group", "targets": out_of_group }));
    }
    if !unknown.is_empty() {
        warnings.push(json!({ "code": "mentions_unknown", "targets": unknown }));
    }
    MentionResolution {
        delivered,
        suppressed,
        warnings,
    }
}
