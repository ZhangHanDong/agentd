//! `query_run` (design §4.12.1): read a run's status / current node / completed
//! nodes / context.

use agentd_core::types::RunId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SurfaceError;
use crate::host::RunHost;

#[derive(Debug, Clone, Deserialize)]
pub struct QueryRunInput {
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryRunOutput {
    pub status: String,
    pub current_node: Option<String>,
    pub completed_nodes: Vec<String>,
    pub context: Value,
}

/// Read a run snapshot through the host.
///
/// # Errors
/// [`SurfaceError::NotFound`] when the run is unknown.
pub async fn query_run(
    host: &dyn RunHost,
    input: QueryRunInput,
) -> Result<QueryRunOutput, SurfaceError> {
    let run_id = RunId::from_string(input.run_id);
    let snapshot = host
        .run_snapshot(&run_id)
        .await?
        .ok_or(SurfaceError::NotFound)?;
    Ok(QueryRunOutput {
        status: snapshot.status,
        current_node: snapshot.current_node,
        completed_nodes: snapshot.completed_nodes,
        context: snapshot.context,
    })
}
