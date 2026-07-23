//! The remote worker loop (M1): pull a fenced lease over HTTP, execute the
//! task natively, upload + acknowledge the transcript, release the lease.
//! The worker never opens the daemon database; its local store only backs
//! disposable scratch state.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agentd_core::ports::{
    Clock, ExecutionArtifactKind, ExecutionArtifactPublish, ExecutionEvidenceLinks,
    TaskLeaseCloseRequest, TaskLeasePort, WorkerArtifactReport, WorkerFleetPullRequest,
    WorkerFleetRegisterRequest,
};
use agentd_core::types::{ExecutionArtifactId, TaskLeaseGrant, WorkerId, WorkerIncarnationId};
use agentd_store::SqliteStore;
use serde_json::json;

use crate::native_runtime_client::NativeRuntimeHttpClient;
use crate::native_worker::{AgentdWorker, NativeWorkerError, native_process_config_from_spec};
use crate::worker_fleet_client::{WorkerFleetHttpClient, WorkerFleetRetryPolicy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRunReport {
    pub executed: u32,
    pub released: u32,
}

/// Register a fresh incarnation, pull until one grant arrives (or the
/// deadline passes), execute it, and return. `--once` semantics.
pub async fn run_worker_once(
    daemon_url: &str,
    auth_proof: &str,
    state_dir: &Path,
    poll: Duration,
    deadline: Duration,
) -> Result<WorkerRunReport, NativeWorkerError> {
    std::fs::create_dir_all(state_dir)
        .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;
    let scratch = SqliteStore::connect(&state_dir.join("worker-scratch.db"))
        .await
        .map_err(NativeWorkerError::Store)?;

    let fleet = WorkerFleetHttpClient::new(daemon_url, auth_proof)
        .map_err(|error| NativeWorkerError::Fleet(error.to_string()))?;
    let runtime = NativeRuntimeHttpClient::new(daemon_url, auth_proof)?;
    let policy = WorkerFleetRetryPolicy::default();

    let incarnation_id = WorkerIncarnationId::new();
    let registration = WorkerFleetRegisterRequest {
        auth_proof: auth_proof.to_string(),
        worker_id: WorkerId::new(),
        trust_domain: "corp-coding".to_string(),
        labels: json!({}),
        incarnation_id: incarnation_id.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        host_name: hostname_or_default(),
        network_zone: None,
        capabilities: json!({"runtime": ["codex", "claude-code"]}),
    };
    fleet
        .register_with_retry(&registration, policy)
        .await
        .map_err(|error| NativeWorkerError::Fleet(error.to_string()))?;

    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > deadline {
            return Ok(WorkerRunReport {
                executed: 0,
                released: 0,
            });
        }
        let observed_at = crate::clock::SystemClock.now_unix();
        let request = WorkerFleetPullRequest {
            auth_proof: auth_proof.to_string(),
            worker_incarnation_id: incarnation_id.clone(),
            observed_at,
            expires_at: observed_at.saturating_add(60),
        };
        match fleet
            .pull_native_with_scope(&request, policy)
            .await
            .map_err(|error| NativeWorkerError::Fleet(error.to_string()))?
        {
            None => tokio::time::sleep(poll).await,
            Some(grant) => {
                return execute_grant(&scratch, &fleet, &runtime, incarnation_id, grant).await;
            }
        }
    }
}

async fn execute_grant(
    scratch: &SqliteStore,
    fleet: &WorkerFleetHttpClient,
    runtime: &NativeRuntimeHttpClient,
    incarnation_id: WorkerIncarnationId,
    grant: TaskLeaseGrant,
) -> Result<WorkerRunReport, NativeWorkerError> {
    let claim = grant.claim();
    let spec = grant.execution_spec.as_ref().ok_or_else(|| {
        NativeWorkerError::InvalidRecovery("pulled grant has no execution spec".into())
    })?;
    let config = native_process_config_from_spec(spec)?;

    // Resolve the daemon-bound runtime session for this task; a grant
    // without one cannot execute (release so the daemon can requeue).
    let Some(view) = runtime.session_for_task(&grant.execution_task_id).await? else {
        let observed_at = crate::clock::SystemClock.now_unix();
        let _ = fleet
            .release(&TaskLeaseCloseRequest {
                claim: claim.clone(),
                observed_at,
                reason: "no runtime session bound to task".to_string(),
            })
            .await;
        return Err(NativeWorkerError::InvalidRecovery(
            "no runtime session bound to pulled task".into(),
        ));
    };

    let worker = AgentdWorker::with_control_planes(
        scratch.clone(),
        Arc::new(runtime.clone()),
        Arc::new(fleet.clone()),
    );
    let handle = worker
        .start_for_task(
            view.session_id.clone(),
            grant.execution_task_id.clone(),
            incarnation_id,
            config,
        )
        .await?;
    let renewal = handle.spawn_lease_renewal(
        claim.clone(),
        Duration::from_secs(10),
        Duration::from_secs(60),
    );

    let event = handle.wait(Duration::from_secs(3600)).await?;
    renewal.abort();

    // Upload the bounded transcript and acknowledge it under the claim.
    let output = handle.output_snapshot();
    let stored = runtime.upload_artifact(output).await?;
    let observed_at = crate::clock::SystemClock.now_unix();
    let report = WorkerArtifactReport {
        claim: claim.clone(),
        observed_at,
        artifact: ExecutionArtifactPublish {
            id: ExecutionArtifactId::new(),
            kind: ExecutionArtifactKind::Transcript,
            content_sha256: stored.content_sha256,
            size_bytes: stored.size_bytes,
            media_type: "text/plain".to_string(),
            storage_ref: stored.storage_ref,
            provenance: json!({
                "source": "agentd-worker",
                "event": format!("{event:?}"),
            }),
            links: ExecutionEvidenceLinks {
                execution_run_id: view.run_id.clone(),
                execution_task_id: Some(grant.execution_task_id.clone()),
                runtime_session_id: Some(view.session_id.clone()),
                runtime_attempt_id: Some(handle.attempt_id().clone()),
                worker_incarnation_id: Some(claim.worker_incarnation_id.clone()),
                snapshot: view.snapshot.clone(),
                target_repository_id: "repo_test".to_string(),
                target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
        },
    };
    runtime.acknowledge_artifact(&report).await?;

    let observed_at = crate::clock::SystemClock.now_unix();
    handle
        .release_lease(&claim, observed_at, "worker execution complete")
        .await?;
    Ok(WorkerRunReport {
        executed: 1,
        released: 1,
    })
}

fn hostname_or_default() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "agentd-worker".to_string())
}
