//! `check_group`: read one group's message history for a member.

use serde::{Deserialize, Serialize};

use crate::error::SurfaceError;
use crate::host::{GroupReadAdvance, GroupReadRequest, InboxMessage, RunHost};
use crate::tools::post::group_has_member;
use crate::tools::send_message::clean_required;

#[derive(Debug, Clone, Deserialize)]
pub struct CheckGroupInput {
    pub group: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub unread_limit: Option<usize>,
    #[serde(default)]
    pub read_all: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckGroupOutput {
    pub group: String,
    pub unread: Vec<InboxMessage>,
    pub read: Vec<InboxMessage>,
    pub unread_total: usize,
    pub unread_returned: usize,
    pub unread_omitted: usize,
    pub advance: String,
}

/// Read group history for one group member.
///
/// # Errors
/// [`SurfaceError::BadRequest`] for missing fields, [`SurfaceError::NotFound`]
/// for unknown agent/group, [`SurfaceError::Forbidden`] for non-members.
pub async fn check_group(
    host: &dyn RunHost,
    input: CheckGroupInput,
) -> Result<CheckGroupOutput, SurfaceError> {
    let group_name = clean_required(Some(input.group), "group required")?;
    let agent_id = clean_required(input.agent_id, "agent_id required")?;
    if host.get_agent(&agent_id).await?.is_none() {
        return Err(SurfaceError::NotFound);
    }
    let Some(group) = host.get_group(&group_name).await? else {
        return Err(SurfaceError::NotFound);
    };
    if !group_has_member(&group, &agent_id) {
        return Err(SurfaceError::Forbidden);
    }

    let advance = if input.read_all.unwrap_or(false) {
        GroupReadAdvance::All
    } else {
        GroupReadAdvance::None
    };
    let result = host
        .read_group_messages(GroupReadRequest {
            group: group_name,
            agent_id,
            limit: input.limit.unwrap_or(10).min(200),
            unread_limit: (advance == GroupReadAdvance::None)
                .then_some(input.unread_limit.unwrap_or(10).min(500)),
            advance,
        })
        .await?;
    Ok(CheckGroupOutput {
        group: result.group,
        unread: result.unread,
        read: result.read,
        unread_total: result.unread_total,
        unread_returned: result.unread_returned,
        unread_omitted: result.unread_omitted,
        advance: result.advance,
    })
}
