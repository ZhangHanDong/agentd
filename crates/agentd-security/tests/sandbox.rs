use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentd_core::ports::{
    AuditPage, AuditReadRequest, CommandError, CommandOutput, CommandRunner, ExecutionAuditAppend,
    ExecutionAuditPort, ExecutionAuditRecord, ExecutionEvidenceError, ExecutionSandboxPort,
    RunOpts,
};
use agentd_core::types::{
    AttemptCapabilityId, AuthenticatedWorkload, AuthorityKey, CapabilityAdmission, EgressPolicy,
    ExecutionSandboxProfile, FencingToken, LeaseId, OciSandboxRuntime, OrganizationRef,
    ProjectExecutionSnapshotRef, ProjectRef, ProtectedAction, ProtectedResource,
    ProtectedResourceKind, RbacPolicyVersionRef, RunId, SandboxCacheSharing, SandboxCleanupRequest,
    SandboxExecuteRequest, SandboxLimits, SandboxLinuxCapabilities, SandboxMount,
    SandboxMountAccess, SandboxPrepareRequest, SandboxPrivilegeEscalation, SandboxRootFilesystem,
    SandboxTerminalReason, SandboxWorkspace, SecurityAuditContext, TaskLeaseClaim, TaskRunId,
    WorkerId, WorkerIncarnationId, WorkloadRole,
};
use agentd_security::sandbox::{OciSandboxAdapter, OciSandboxConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedCommand {
    program: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct RecordingRunner {
    calls: Mutex<Vec<RecordedCommand>>,
}

#[async_trait::async_trait]
impl CommandRunner for RecordingRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        opts: RunOpts,
    ) -> Result<CommandOutput, CommandError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(RecordedCommand {
                program: program.to_string(),
                args: args.to_vec(),
                cwd: opts.cwd,
            });
        Ok(CommandOutput {
            stdout: "sandbox output".to_string(),
            stderr: String::new(),
            status: 0,
        })
    }
}

#[derive(Debug, Default)]
struct RecordingAudit {
    events: Mutex<Vec<ExecutionAuditAppend>>,
}

#[async_trait::async_trait]
impl ExecutionAuditPort for RecordingAudit {
    async fn append_audit(
        &self,
        request: &ExecutionAuditAppend,
    ) -> Result<ExecutionAuditRecord, ExecutionEvidenceError> {
        self.events
            .lock()
            .expect("events lock")
            .push(request.clone());
        Ok(ExecutionAuditRecord {
            append: request.clone(),
            sequence: 1,
            recorded_at: request.occurred_at,
        })
    }

    async fn read_audit(
        &self,
        _request: &AuditReadRequest,
    ) -> Result<AuditPage, ExecutionEvidenceError> {
        Ok(AuditPage {
            records: Vec::new(),
            next_after_sequence: None,
        })
    }
}

fn authority_key() -> AuthorityKey {
    AuthorityKey::new("specify:sandbox-test").expect("authority")
}

fn admission(action: ProtectedAction) -> CapabilityAdmission {
    let worker_incarnation_id = WorkerIncarnationId::from_string("wi_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    let organization_ref =
        OrganizationRef::new(authority_key(), "org-a", "1").expect("organization");
    let project_ref = ProjectRef::new(authority_key(), "project-a", "2").expect("project");
    let execution_snapshot_ref =
        ProjectExecutionSnapshotRef::new(authority_key(), "snapshot-a", "3").expect("snapshot");
    CapabilityAdmission {
        id: AttemptCapabilityId::new(),
        workload: AuthenticatedWorkload {
            spiffe_uri: format!("spiffe://agents.example/worker/{worker_incarnation_id}"),
            role: WorkloadRole::Worker,
            trust_domain: "agents.example".to_string(),
            certificate_sha256: "a".repeat(64),
            not_before: 100,
            not_after: 500,
            worker_id: Some(WorkerId::from_string("wk_01ARZ3NDEKTSV4RRFFQ69G5FAW")),
            worker_incarnation_id: Some(worker_incarnation_id.clone()),
        },
        scope: agentd_core::types::ExecutionSecurityScope {
            authority_key: authority_key(),
            organization_ref: organization_ref.clone(),
            project_ref: project_ref.clone(),
            execution_snapshot_ref: execution_snapshot_ref.clone(),
            rbac_policy_version_ref: RbacPolicyVersionRef::new(authority_key(), "rbac-a", "4")
                .expect("rbac"),
            worker_incarnation_id: worker_incarnation_id.clone(),
            task_lease_claim: TaskLeaseClaim {
                execution_task_id: TaskRunId::from_string("tr_01ARZ3NDEKTSV4RRFFQ69G5FAX"),
                worker_incarnation_id,
                lease_id: LeaseId::from_string("ls_01ARZ3NDEKTSV4RRFFQ69G5FAY"),
                fencing_token: FencingToken::new(9).expect("fencing token"),
            },
            sandbox_profile_id: "oci-restricted-v1".to_string(),
            egress_profile_id: "deny-all-v1".to_string(),
            policy_revocation_epoch: 1,
            valid_until: 450,
            audit_context: SecurityAuditContext {
                execution_run_id: RunId::from_string("r_01ARZ3NDEKTSV4RRFFQ69G5FAZ"),
                snapshot_content_sha256: "b".repeat(64),
                target_repository_id: "repository-a".to_string(),
                target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
        },
        action,
        resource: ProtectedResource {
            organization_ref,
            project_ref,
            execution_snapshot_ref,
            kind: ProtectedResourceKind::Execution,
        },
        issued_at: 120,
        expires_at: 400,
    }
}

fn profile() -> ExecutionSandboxProfile {
    ExecutionSandboxProfile {
        profile_id: "oci-restricted-v1".to_string(),
        runtime: OciSandboxRuntime::Docker,
        image_digest: format!("sha256:{}", "c".repeat(64)),
        root_filesystem: SandboxRootFilesystem::ReadOnly,
        workspace: SandboxWorkspace::Ephemeral,
        mounts: vec![SandboxMount {
            source_id: "input-bundle".to_string(),
            target: "/workspace/input".to_string(),
            access: SandboxMountAccess::ReadOnly,
        }],
        linux_capabilities: SandboxLinuxCapabilities::DropAll,
        privilege_escalation: SandboxPrivilegeEscalation::Denied,
        seccomp_profile: "runtime-default".to_string(),
        limits: SandboxLimits {
            pids: 64,
            memory_bytes: 512 * 1024 * 1024,
            cpu_millis: 1_000,
        },
        tenant_cache_namespace: "specify:sandbox-test/org-a/project-a".to_string(),
        cache_sharing: SandboxCacheSharing::TenantOnly,
        egress: EgressPolicy::DenyAll,
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    runner: Arc<RecordingRunner>,
    audit: Arc<RecordingAudit>,
    adapter: OciSandboxAdapter,
    workspace_root: PathBuf,
}

fn harness() -> Harness {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace_root = temp.path().join("workspaces");
    let cache_root = temp.path().join("cache");
    let input_root = temp.path().join("inputs");
    let input_bundle = input_root.join("bundle");
    std::fs::create_dir_all(&input_bundle).expect("input bundle");
    let runner = Arc::new(RecordingRunner::default());
    let audit = Arc::new(RecordingAudit::default());
    let adapter = OciSandboxAdapter::new(
        Arc::clone(&runner) as Arc<dyn CommandRunner>,
        Arc::clone(&audit) as Arc<dyn ExecutionAuditPort>,
        OciSandboxConfig {
            runtime_bin: "docker".to_string(),
            workspace_root: workspace_root.clone(),
            cache_root,
            allowed_input_root: input_root,
            input_sources: HashMap::from([("input-bundle".to_string(), input_bundle)]),
            command_timeout: Duration::from_secs(30),
        },
    )
    .expect("sandbox adapter");
    Harness {
        _temp: temp,
        runner,
        audit,
        adapter,
        workspace_root,
    }
}

#[tokio::test]
async fn oci_sandbox_request_is_bounded_read_only_and_default_deny() {
    let harness = harness();
    let prepared = harness
        .adapter
        .prepare_sandbox(&SandboxPrepareRequest {
            admission: admission(ProtectedAction::SandboxPrepare),
            profile: profile(),
        })
        .await
        .expect("prepare sandbox");
    let result = harness
        .adapter
        .execute_sandbox(&SandboxExecuteRequest {
            admission: admission(ProtectedAction::SandboxExecute),
            sandbox: prepared,
            argv: vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf separate-argv".to_string(),
            ],
            env: BTreeMap::from([("CI".to_string(), "1".to_string())]),
            observed_at: 200,
        })
        .await
        .expect("execute sandbox");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"sandbox output");

    let calls = harness.runner.calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.program, "docker");
    assert!(call.cwd.is_none());
    for required in [
        "run",
        "--rm",
        "--read-only",
        "--cap-drop",
        "ALL",
        "no-new-privileges",
        "seccomp=runtime-default",
        "--pids-limit",
        "64",
        "--memory",
        "536870912",
        "--cpus",
        "1.000",
        "--network",
        "none",
        "AGENTD_TENANT_CACHE_NAMESPACE=specify:sandbox-test/org-a/project-a",
        &profile().image_digest,
        "printf separate-argv",
    ] {
        assert!(
            call.args.iter().any(|arg| arg == required),
            "missing {required}: {:?}",
            call.args
        );
    }
    assert!(!call.args.iter().any(|arg| arg.contains(".aws")));
    assert!(!call.args.iter().any(|arg| arg.contains(".ssh")));
    assert!(!call.args.iter().any(|arg| arg.contains("docker.sock")));
    assert!(
        !call
            .args
            .iter()
            .any(|arg| arg.contains("; printf separate-argv"))
    );
}

#[tokio::test]
async fn sandbox_cleanup_is_idempotent_for_all_terminal_paths() {
    let harness = harness();
    for terminal_reason in [
        SandboxTerminalReason::Success,
        SandboxTerminalReason::Failure,
        SandboxTerminalReason::Cancelled,
        SandboxTerminalReason::TimedOut,
        SandboxTerminalReason::Recovery,
    ] {
        let prepared = harness
            .adapter
            .prepare_sandbox(&SandboxPrepareRequest {
                admission: admission(ProtectedAction::SandboxPrepare),
                profile: profile(),
            })
            .await
            .expect("prepare sandbox");
        let workspace = harness.workspace_root.join(&prepared.sandbox_id);
        assert!(workspace.is_dir());
        let cleanup = SandboxCleanupRequest {
            sandbox_id: prepared.sandbox_id,
            observed_at: 300,
            terminal_reason,
        };
        harness
            .adapter
            .cleanup_sandbox(&cleanup)
            .await
            .expect("first cleanup");
        harness
            .adapter
            .cleanup_sandbox(&cleanup)
            .await
            .expect("idempotent cleanup");
        assert!(!workspace.exists());
    }

    let orphan = harness.workspace_root.join("sb_orphaned");
    std::fs::create_dir_all(orphan.join("secrets")).expect("orphan workspace");
    let recovered = harness
        .adapter
        .recover_orphans(320)
        .expect("recover orphan");
    assert_eq!(recovered, 1);
    assert!(!orphan.exists());
    assert!(harness.adapter.teardown_records().is_empty());
    assert!(harness.audit.events.lock().expect("events lock").is_empty());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let prepared = harness
            .adapter
            .prepare_sandbox(&SandboxPrepareRequest {
                admission: admission(ProtectedAction::SandboxPrepare),
                profile: profile(),
            })
            .await
            .expect("prepare teardown failure sandbox");
        let mut locked = std::fs::metadata(&harness.workspace_root)
            .expect("workspace root metadata")
            .permissions();
        locked.set_mode(0o500);
        std::fs::set_permissions(&harness.workspace_root, locked).expect("lock workspace root");
        let cleanup = SandboxCleanupRequest {
            sandbox_id: prepared.sandbox_id.clone(),
            observed_at: 330,
            terminal_reason: SandboxTerminalReason::Failure,
        };
        let cleanup_error = harness
            .adapter
            .cleanup_sandbox(&cleanup)
            .await
            .expect_err("injected teardown failure");
        let mut restored = std::fs::metadata(&harness.workspace_root)
            .expect("workspace root metadata")
            .permissions();
        restored.set_mode(0o700);
        std::fs::set_permissions(&harness.workspace_root, restored)
            .expect("restore workspace root");
        assert_eq!(
            cleanup_error.denial_reason(),
            Some(agentd_core::types::SecurityDenialReason::SandboxCleanupFailed)
        );
        assert_eq!(harness.adapter.teardown_records().len(), 1);
        {
            let events = harness.audit.events.lock().expect("events lock");
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event_type, "execution.sandbox_cleanup_failed");
        }
        harness
            .adapter
            .cleanup_sandbox(&cleanup)
            .await
            .expect("recover teardown after permission restore");
    }
}
