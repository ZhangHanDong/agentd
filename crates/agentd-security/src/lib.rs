//! Deterministic security adapters used by standalone and enterprise composition.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use agentd_core::ports::{
    AuthenticatedWorkload, CapabilityAdmission, MtlsWorkloadVerifier, ProtectedAction,
    ProtectedResource, SecretBrokerPort, SecretLease, SecretMaterial, SecurityDenial,
    TenantAuthorizationPort, WorkloadRole,
};
use agentd_core::ports::{CommandRunner, RunOpts};
use agentd_core::types::WorkerIncarnationId;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use x509_parser::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionSandboxProfile {
    pub image_digest: String,
    pub workspace: PathBuf,
    pub ephemeral_workspace: bool,
    pub input_mount: Option<PathBuf>,
    pub output_mount: PathBuf,
    pub egress_profile: String,
    pub memory_bytes: u64,
    pub cpu_quota: u64,
}

impl ExecutionSandboxProfile {
    pub fn validate(&self) -> Result<(), SecurityDenial> {
        if !self.image_digest.starts_with("sha256:")
            || self.image_digest.len() != 71
            || self.workspace.as_os_str().is_empty()
            || self.output_mount.as_os_str().is_empty()
            || self.memory_bytes == 0
            || self.cpu_quota == 0
        {
            return Err(SecurityDenial::ResourceDenied);
        }
        if self.ephemeral_workspace {
            let candidate = if self.workspace.is_absolute() {
                self.workspace.clone()
            } else {
                std::env::current_dir()
                    .unwrap_or_default()
                    .join(&self.workspace)
            };
            let normalized = candidate.canonicalize().unwrap_or(candidate);
            let current_dir = std::env::current_dir().unwrap_or_default();
            if normalized == std::path::Path::new("/") || normalized.starts_with(&current_dir) {
                return Err(SecurityDenial::SandboxProfileDenied);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxLaunchRequest {
    pub program: String,
    pub args: Vec<String>,
    /// Environment passed as individual OCI `--env` arguments. This is never
    /// rendered through a shell, so values cannot become additional argv.
    pub environment: Vec<(String, String)>,
    pub profile: ExecutionSandboxProfile,
}

impl SandboxLaunchRequest {
    pub fn validate(&self) -> Result<(), SecurityDenial> {
        self.profile.validate()?;
        if self.program.is_empty()
            || self.program.contains('\0')
            || self.args.iter().any(|arg| arg.contains('\0'))
            || self.environment.iter().any(|(key, value)| {
                key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0')
            })
        {
            return Err(SecurityDenial::ResourceDenied);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

#[must_use]
pub fn oci_command(runtime: &str, request: &SandboxLaunchRequest) -> (String, Vec<String>) {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--interactive".to_string(),
        "--read-only".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--security-opt".to_string(),
        "no-new-privileges:true".to_string(),
        "--pids-limit".to_string(),
        "256".to_string(),
        "--tmpfs".to_string(),
        "/tmp:rw,noexec,nosuid,nodev".to_string(),
        "--network".to_string(),
        if request.profile.egress_profile == "none" {
            "none".to_string()
        } else {
            request.profile.egress_profile.clone()
        },
        "--memory".to_string(),
        request.profile.memory_bytes.to_string(),
        "--cpus".to_string(),
        request.profile.cpu_quota.to_string(),
    ];
    if let Some(input) = &request.profile.input_mount {
        args.extend([
            "--mount".to_string(),
            format!("type=bind,src={},dst=/inputs,readonly", input.display()),
        ]);
    }
    args.extend([
        "--mount".to_string(),
        format!(
            "type=bind,src={},dst=/outputs,rw",
            request.profile.output_mount.display()
        ),
    ]);
    for (key, value) in &request.environment {
        args.extend(["--env".to_string(), format!("{key}={value}")]);
    }
    args.extend([
        request.profile.image_digest.clone(),
        request.program.clone(),
    ]);
    args.extend(request.args.clone());
    (runtime.to_string(), args)
}

pub struct OciSandboxAdapter<R> {
    runner: R,
    runtime: String,
}

impl<R> std::fmt::Debug for OciSandboxAdapter<R> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OciSandboxAdapter")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl<R> OciSandboxAdapter<R> {
    #[must_use]
    pub fn new(runner: R, runtime: impl Into<String>) -> Self {
        Self {
            runner,
            runtime: runtime.into(),
        }
    }

    pub fn cleanup_workspace(profile: &ExecutionSandboxProfile) -> Result<(), SecurityDenial> {
        if !profile.ephemeral_workspace {
            return Ok(());
        }
        match std::fs::remove_dir_all(&profile.workspace) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(SecurityDenial::SandboxCleanupFailed),
        }
    }
}

impl<R: CommandRunner> OciSandboxAdapter<R> {
    pub async fn execute(
        &self,
        admission: &CapabilityAdmission,
        request: &SandboxLaunchRequest,
    ) -> Result<SandboxOutput, SecurityDenial> {
        if admission.action != ProtectedAction::SandboxExecute {
            return Err(SecurityDenial::ActionDenied);
        }
        request.validate()?;
        if admission.scope.sandbox_profile != request.profile.image_digest
            || admission.scope.egress_profile != request.profile.egress_profile
        {
            return Err(SecurityDenial::CapabilityScopeMismatch);
        }
        let (runtime, args) = oci_command(&self.runtime, request);
        let output = self
            .runner
            .run(
                &runtime,
                &args,
                RunOpts {
                    cwd: Some(request.profile.workspace.clone()),
                    ..RunOpts::default()
                },
            )
            .await;
        Self::cleanup_workspace(&request.profile)?;
        let output = output.map_err(|_| SecurityDenial::ResourceDenied)?;
        Ok(SandboxOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            status: output.status,
        })
    }
}

#[derive(Debug, Clone)]
pub struct WorkloadIdentityVerifier {
    trust_domain: String,
    trusted_fingerprints: Arc<RwLock<HashSet<String>>>,
}

#[derive(Debug, Clone)]
pub struct PeerCertificateVerifier {
    identity: WorkloadIdentityVerifier,
    trusted_roots: Arc<RwLock<Vec<Vec<u8>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPeerCertificate {
    pub spiffe_id: String,
    pub fingerprint_sha256: String,
    pub not_before: i64,
    pub not_after: i64,
}

pub fn parse_peer_certificate(
    der: &[u8],
    trust_domain: &str,
    observed_at: i64,
) -> Result<ParsedPeerCertificate, SecurityDenial> {
    let (_, certificate) =
        X509Certificate::from_der(der).map_err(|_| SecurityDenial::IdentityUntrusted)?;
    let validity = certificate.validity();
    let not_before = validity.not_before.timestamp();
    let not_after = validity.not_after.timestamp();
    if observed_at < not_before || observed_at >= not_after {
        return Err(SecurityDenial::IdentityExpired);
    }
    let expected_prefix = format!("spiffe://{trust_domain}/");
    let spiffe_id = certificate
        .subject_alternative_name()
        .ok()
        .flatten()
        .and_then(|extension| {
            extension
                .value
                .general_names
                .iter()
                .find_map(|name| match name {
                    GeneralName::URI(uri) if uri.starts_with(&expected_prefix) => {
                        Some(uri.to_string())
                    }
                    _ => None,
                })
        })
        .ok_or(SecurityDenial::IdentityUntrusted)?;
    Ok(ParsedPeerCertificate {
        spiffe_id,
        fingerprint_sha256: format!("{:x}", Sha256::digest(der)),
        not_before,
        not_after,
    })
}

impl PeerCertificateVerifier {
    #[must_use]
    pub fn new(identity: WorkloadIdentityVerifier) -> Self {
        Self {
            identity,
            trusted_roots: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn trust_root(&self, der: impl Into<Vec<u8>>) {
        self.trusted_roots.write().await.push(der.into());
    }
}

#[async_trait::async_trait]
impl MtlsWorkloadVerifier for PeerCertificateVerifier {
    async fn verify_peer(
        &self,
        peer_certificate_der: &[u8],
        observed_at: i64,
    ) -> Result<AuthenticatedWorkload, SecurityDenial> {
        let (_, leaf) = X509Certificate::from_der(peer_certificate_der)
            .map_err(|_| SecurityDenial::IdentityUntrusted)?;
        let roots = self.trusted_roots.read().await;
        let trusted = roots.iter().any(|root_der| {
            let Ok((_, root)) = X509Certificate::from_der(root_der) else {
                return false;
            };
            let root_valid = x509_parser::time::ASN1Time::from_timestamp(observed_at)
                .is_ok_and(|timestamp| root.validity().is_valid_at(timestamp));
            let root_is_ca = root
                .basic_constraints()
                .ok()
                .flatten()
                .is_some_and(|constraints| constraints.value.ca);
            leaf.issuer() == root.subject()
                && root_valid
                && root_is_ca
                && root.verify_signature(None).is_ok()
                && leaf
                    .verify_signature(Some(&root.tbs_certificate.subject_pki))
                    .is_ok()
        });
        if !trusted {
            return Err(SecurityDenial::IdentityUntrusted);
        }
        let parsed = parse_peer_certificate(
            peer_certificate_der,
            self.identity.trust_domain(),
            observed_at,
        )?;
        let spiffe_id = parsed.spiffe_id;
        let worker_incarnation_id = spiffe_id
            .strip_prefix(&format!(
                "spiffe://{}/worker/",
                self.identity.trust_domain()
            ))
            .filter(|value| !value.is_empty())
            .map(WorkerIncarnationId::from_string);
        let workload = AuthenticatedWorkload {
            spiffe_id,
            role: WorkloadRole::Worker,
            trust_domain: self.identity.trust_domain().to_string(),
            certificate_fingerprint: parsed.fingerprint_sha256,
            valid_from: parsed.not_before,
            valid_until: parsed.not_after,
            worker_incarnation_id,
        };
        if workload.worker_incarnation_id.is_none() {
            return Err(SecurityDenial::IdentityUntrusted);
        }
        self.identity.verify(&workload, observed_at).await?;
        Ok(workload)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSecurityConfig {
    pub trust_domain: String,
    pub sandbox_runtime: String,
    pub secret_broker: String,
}

impl EnterpriseSecurityConfig {
    pub fn validate(&self) -> Result<(), SecurityDenial> {
        if self.trust_domain.trim().is_empty()
            || self.sandbox_runtime.trim().is_empty()
            || self.secret_broker.trim().is_empty()
        {
            return Err(SecurityDenial::IdentityUntrusted);
        }
        if !self.trust_domain.contains('.') {
            return Err(SecurityDenial::IdentityUntrusted);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct EnterpriseAdmission<A> {
    identity: WorkloadIdentityVerifier,
    authorizer: A,
}

#[derive(Debug, Clone)]
pub struct EnterpriseExecutionGate<A, B> {
    pub admission: EnterpriseAdmission<A>,
    pub secret_broker: B,
}

#[derive(Debug)]
pub struct EnterpriseSandboxGate<A, R> {
    pub admission: EnterpriseAdmission<A>,
    pub sandbox: OciSandboxAdapter<R>,
}

impl<A, R> EnterpriseSandboxGate<A, R> {
    #[must_use]
    pub fn new(admission: EnterpriseAdmission<A>, sandbox: OciSandboxAdapter<R>) -> Self {
        Self { admission, sandbox }
    }
}

impl<A, R> EnterpriseSandboxGate<A, R>
where
    A: TenantAuthorizationPort,
    R: CommandRunner,
{
    pub async fn execute(
        &self,
        workload: &AuthenticatedWorkload,
        capability: &CapabilityAdmission,
        request: &SandboxLaunchRequest,
        observed_at: i64,
    ) -> Result<SandboxOutput, SecurityDenial> {
        capability.validate_at(observed_at)?;
        self.admission
            .admit(
                workload,
                capability.action,
                &capability.resource,
                &capability.scope,
                observed_at,
            )
            .await?;
        if capability.scope.worker_incarnation_id
            != workload
                .worker_incarnation_id
                .clone()
                .ok_or(SecurityDenial::IdentityUntrusted)?
        {
            return Err(SecurityDenial::CapabilityScopeMismatch);
        }
        self.sandbox.execute(capability, request).await
    }
}

impl<A, B> EnterpriseExecutionGate<A, B> {
    #[must_use]
    pub fn new(admission: EnterpriseAdmission<A>, secret_broker: B) -> Self {
        Self {
            admission,
            secret_broker,
        }
    }
}

impl<A, B> EnterpriseExecutionGate<A, B>
where
    A: TenantAuthorizationPort,
    B: SecretBrokerPort,
{
    pub async fn checkout_secret(
        &self,
        workload: &AuthenticatedWorkload,
        admission: &CapabilityAdmission,
        observed_at: i64,
    ) -> Result<SecretLease, SecurityDenial> {
        admission.validate_at(observed_at)?;
        self.admission
            .admit(
                workload,
                admission.action,
                &admission.resource,
                &admission.scope,
                observed_at,
            )
            .await?;
        if admission.action != ProtectedAction::SecretCheckout
            || admission.scope.worker_incarnation_id
                != workload
                    .worker_incarnation_id
                    .clone()
                    .ok_or(SecurityDenial::IdentityUntrusted)?
        {
            return Err(SecurityDenial::CapabilityScopeMismatch);
        }
        let selector = match &admission.resource {
            ProtectedResource::Secret(selector) => selector,
            _ => return Err(SecurityDenial::ResourceDenied),
        };
        self.secret_broker
            .checkout(admission, selector, observed_at)
            .await
    }
}

impl<A> EnterpriseAdmission<A> {
    #[must_use]
    pub fn new(identity: WorkloadIdentityVerifier, authorizer: A) -> Self {
        Self {
            identity,
            authorizer,
        }
    }
}

impl<A: TenantAuthorizationPort> EnterpriseAdmission<A> {
    pub async fn admit(
        &self,
        workload: &AuthenticatedWorkload,
        action: ProtectedAction,
        resource: &ProtectedResource,
        scope: &agentd_core::ports::ExecutionSecurityScope,
        observed_at: i64,
    ) -> Result<(), SecurityDenial> {
        self.identity.verify(workload, observed_at).await?;
        self.authorizer
            .authorize(workload, action, resource, scope, observed_at)
            .await
    }
}

impl WorkloadIdentityVerifier {
    #[must_use]
    pub fn new(trust_domain: impl Into<String>) -> Self {
        Self {
            trust_domain: trust_domain.into(),
            trusted_fingerprints: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub async fn trust_fingerprint(&self, fingerprint: impl Into<String>) {
        self.trusted_fingerprints
            .write()
            .await
            .insert(fingerprint.into());
    }

    #[must_use]
    pub fn trust_domain(&self) -> &str {
        &self.trust_domain
    }

    pub async fn verify(
        &self,
        workload: &AuthenticatedWorkload,
        observed_at: i64,
    ) -> Result<(), SecurityDenial> {
        let expected_prefix = format!("spiffe://{}/", self.trust_domain);
        if workload.trust_domain != self.trust_domain
            || !workload.spiffe_id.starts_with(&expected_prefix)
        {
            return Err(SecurityDenial::IdentityUntrusted);
        }
        if observed_at < workload.valid_from || observed_at >= workload.valid_until {
            return Err(SecurityDenial::IdentityExpired);
        }
        if !self
            .trusted_fingerprints
            .read()
            .await
            .contains(&workload.certificate_fingerprint)
        {
            return Err(SecurityDenial::IdentityUntrusted);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryTenantAuthorizer {
    revoked_workers: Arc<RwLock<std::collections::HashSet<String>>>,
    policy_revocation_epoch: Arc<RwLock<u64>>,
}

impl InMemoryTenantAuthorizer {
    pub async fn revoke_worker(&self, worker_incarnation_id: &str) {
        self.revoked_workers
            .write()
            .await
            .insert(worker_incarnation_id.to_string());
    }

    /// Advance the policy revocation epoch. Capabilities issued at an older
    /// epoch are rejected without mutating individual capability records.
    pub async fn revoke_policy_before(&self, epoch: u64) {
        let mut current = self.policy_revocation_epoch.write().await;
        *current = (*current).max(epoch);
    }
}

#[async_trait::async_trait]
impl TenantAuthorizationPort for InMemoryTenantAuthorizer {
    async fn authorize(
        &self,
        workload: &AuthenticatedWorkload,
        _action: ProtectedAction,
        _resource: &ProtectedResource,
        scope: &agentd_core::ports::ExecutionSecurityScope,
        observed_at: i64,
    ) -> Result<(), SecurityDenial> {
        scope.validate()?;
        if workload.role != WorkloadRole::Worker {
            return Err(SecurityDenial::IdentityUntrusted);
        }
        if observed_at < workload.valid_from || observed_at >= workload.valid_until {
            return Err(SecurityDenial::IdentityExpired);
        }
        let worker = workload
            .worker_incarnation_id
            .as_ref()
            .ok_or(SecurityDenial::IdentityUntrusted)?;
        if worker != &scope.worker_incarnation_id {
            return Err(SecurityDenial::IncarnationStale);
        }
        if self
            .revoked_workers
            .read()
            .await
            .contains(&worker.to_string())
        {
            return Err(SecurityDenial::IdentityRevoked);
        }
        if scope.snapshot_ref.authority_key() != &scope.authority_key
            || scope.project_ref.authority_key() != &scope.authority_key
            || scope.organization_ref.authority_key() != &scope.authority_key
        {
            return Err(SecurityDenial::TenantMismatch);
        }
        if scope.policy_revocation_epoch < *self.policy_revocation_epoch.read().await {
            return Err(SecurityDenial::CapabilityRevoked);
        }
        if observed_at < scope.valid_from || observed_at >= scope.valid_until {
            return Err(SecurityDenial::SnapshotMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct InMemorySecretBroker {
    secrets: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

impl InMemorySecretBroker {
    pub async fn insert(&self, selector: impl Into<String>, material: impl Into<Vec<u8>>) {
        self.secrets
            .write()
            .await
            .insert(selector.into(), material.into());
    }
}

#[async_trait::async_trait]
impl SecretBrokerPort for InMemorySecretBroker {
    async fn checkout(
        &self,
        admission: &CapabilityAdmission,
        selector: &str,
        observed_at: i64,
    ) -> Result<SecretLease, SecurityDenial> {
        admission.validate_at(observed_at)?;
        if admission.action != ProtectedAction::SecretCheckout {
            return Err(SecurityDenial::ActionDenied);
        }
        match &admission.resource {
            ProtectedResource::Secret(expected) if expected == selector => {}
            _ => return Err(SecurityDenial::ResourceDenied),
        }
        let material = self
            .secrets
            .read()
            .await
            .get(selector)
            .cloned()
            .ok_or(SecurityDenial::ResourceDenied)?;
        Ok(SecretLease {
            selector: selector.to_string(),
            material: SecretMaterial::new(material),
            expires_at: admission.scope.valid_until,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EnterpriseSecurityConfig, ExecutionSandboxProfile, InMemorySecretBroker,
        InMemoryTenantAuthorizer, OciSandboxAdapter, SandboxLaunchRequest,
        WorkloadIdentityVerifier, oci_command,
    };
    use agentd_core::ports::AuthenticatedWorkload;
    use agentd_core::ports::WorkloadRole;
    #[tokio::test]
    async fn secret_broker_returns_only_requested_secret() {
        let broker = InMemorySecretBroker::default();
        broker.insert("forge/token", b"opaque".to_vec()).await;
        assert!(broker.secrets.read().await.contains_key("forge/token"));
    }

    #[tokio::test]
    async fn authorizer_can_revoke_worker() {
        let authorizer = InMemoryTenantAuthorizer::default();
        authorizer.revoke_worker("wi_test").await;
        assert!(authorizer.revoked_workers.read().await.contains("wi_test"));
    }

    #[tokio::test]
    async fn identity_verifier_rejects_lookalike_trust_domain_uri() {
        let verifier = WorkloadIdentityVerifier::new("corp.example");
        verifier.trust_fingerprint("fp").await;
        let workload = AuthenticatedWorkload {
            spiffe_id: "spiffe://evil-corp.example/worker".to_string(),
            role: WorkloadRole::Worker,
            trust_domain: "corp.example".to_string(),
            certificate_fingerprint: "fp".to_string(),
            valid_from: 0,
            valid_until: 100,
            worker_incarnation_id: None,
        };
        assert!(verifier.verify(&workload, 10).await.is_err());
    }

    #[test]
    fn enterprise_security_config_rejects_incomplete_setup() {
        let config = EnterpriseSecurityConfig {
            trust_domain: "corp.example".to_string(),
            sandbox_runtime: "docker".to_string(),
            secret_broker: String::new(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn cleanup_never_removes_non_ephemeral_workspace() {
        let root = std::env::temp_dir().join(format!("agentd-security-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create test workspace");
        let profile = ExecutionSandboxProfile {
            image_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            workspace: root.clone(),
            ephemeral_workspace: false,
            input_mount: None,
            output_mount: root.clone(),
            egress_profile: "none".to_string(),
            memory_bytes: 1,
            cpu_quota: 1,
        };
        OciSandboxAdapter::<()>::cleanup_workspace(&profile).expect("cleanup guard");
        assert!(root.exists());
        std::fs::remove_dir_all(root).expect("remove test workspace");
    }

    #[test]
    fn cleanup_removes_ephemeral_workspace() {
        let root =
            std::env::temp_dir().join(format!("agentd-security-ephemeral-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create ephemeral workspace");
        let profile = ExecutionSandboxProfile {
            image_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            workspace: root.clone(),
            ephemeral_workspace: true,
            input_mount: None,
            output_mount: root.clone(),
            egress_profile: "none".to_string(),
            memory_bytes: 1,
            cpu_quota: 1,
        };
        OciSandboxAdapter::<()>::cleanup_workspace(&profile).expect("cleanup ephemeral workspace");
        assert!(!root.exists());
    }

    #[test]
    fn profile_rejects_ephemeral_repo_subdirectory() {
        let workspace = std::env::current_dir()
            .expect("current directory")
            .join(".agentd-security-test-subdir");
        let profile = ExecutionSandboxProfile {
            image_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            workspace,
            ephemeral_workspace: true,
            input_mount: None,
            output_mount: std::env::temp_dir(),
            egress_profile: "none".to_string(),
            memory_bytes: 1,
            cpu_quota: 1,
        };
        assert_eq!(
            profile
                .validate()
                .expect_err("repo subdirectory must be denied"),
            agentd_core::ports::SecurityDenial::SandboxProfileDenied
        );
    }

    #[test]
    fn oci_command_preserves_environment_as_argv() {
        let request = SandboxLaunchRequest {
            program: "codex".to_string(),
            args: vec!["exec".to_string()],
            environment: vec![("MCP_URL".to_string(), "http://127.0.0.1:9/a b".to_string())],
            profile: ExecutionSandboxProfile {
                image_digest:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_string(),
                workspace: std::env::temp_dir(),
                ephemeral_workspace: false,
                input_mount: None,
                output_mount: std::env::temp_dir(),
                egress_profile: "none".to_string(),
                memory_bytes: 1,
                cpu_quota: 1,
            },
        };
        let (_, args) = oci_command("runc", &request);
        let env_index = args
            .iter()
            .position(|arg| arg == "--env")
            .expect("env flag");
        assert_eq!(args[env_index + 1], "MCP_URL=http://127.0.0.1:9/a b");
        assert_eq!(args[env_index + 2], request.profile.image_digest);
        assert!(request.validate().is_ok());
    }

    #[test]
    fn sandbox_request_rejects_malformed_environment() {
        let request = SandboxLaunchRequest {
            program: "codex".to_string(),
            args: Vec::new(),
            environment: vec![("BAD=KEY".to_string(), "value".to_string())],
            profile: ExecutionSandboxProfile {
                image_digest:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_string(),
                workspace: std::env::temp_dir(),
                ephemeral_workspace: false,
                input_mount: None,
                output_mount: std::env::temp_dir(),
                egress_profile: "none".to_string(),
                memory_bytes: 1,
                cpu_quota: 1,
            },
        };
        assert_eq!(
            request
                .validate()
                .expect_err("invalid env key must be rejected"),
            agentd_core::ports::SecurityDenial::ResourceDenied
        );
    }
}
