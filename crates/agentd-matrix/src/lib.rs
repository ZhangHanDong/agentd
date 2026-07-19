//! Matrix bridge runtime boundary for agentd.
//!
//! This crate intentionally stops at the local process-loop scaffold. A future
//! Matrix SDK adapter can implement [`MatrixBridgeTransport`], while the daemon
//! or HTTP client side implements [`AgentdBridgeBackend`].

#![doc(html_root_url = "https://docs.rs/agentd-matrix/0.0.0")]
// Production-only lint opt-ins. Test files don't pick these up.
#![warn(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Configuration needed by a bridge process to reach agentd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeConfig {
    agentd_api: String,
    operator_token: Option<String>,
}

impl BridgeConfig {
    /// Build a bridge config with a normalized agentd API base URL.
    pub fn new(agentd_api: impl Into<String>) -> Result<Self, BridgeError> {
        let agentd_api = normalize_agentd_api(&agentd_api.into())?;
        Ok(Self {
            agentd_api,
            operator_token: None,
        })
    }

    /// Set an optional operator bearer token for bridge-to-agentd calls.
    #[must_use]
    pub fn with_operator_token(mut self, token: impl Into<String>) -> Self {
        let token = token.into();
        let trimmed = token.trim();
        self.operator_token = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        };
        self
    }

    /// Normalized agentd API base URL, without trailing slash characters.
    #[must_use]
    pub fn agentd_api(&self) -> &str {
        &self.agentd_api
    }

    /// Optional operator bearer token.
    #[must_use]
    pub fn operator_token(&self) -> Option<&str> {
        self.operator_token.as_deref()
    }
}

/// Durable bridge progress that can be persisted by a process wrapper.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeState {
    next_from_seq: i64,
}

impl BridgeState {
    /// Build bridge state from a previously confirmed outbox sequence.
    #[must_use]
    pub const fn new(next_from_seq: i64) -> Self {
        Self { next_from_seq }
    }

    /// Load bridge state from a JSON file. Missing files default to cursor 0.
    pub fn load_json(path: impl AsRef<Path>) -> Result<Self, BridgeError> {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|err| {
                BridgeError::state(format!("decode state JSON {}: {err}", path.display()))
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(BridgeError::state(format!(
                "read state {}: {err}",
                path.display()
            ))),
        }
    }

    /// Save bridge state as JSON, creating parent directories when needed.
    pub fn save_json(&self, path: impl AsRef<Path>) -> Result<(), BridgeError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                BridgeError::state(format!("create state dir {}: {err}", parent.display()))
            })?;
        }
        let contents = serde_json::to_string_pretty(self)
            .map_err(|err| BridgeError::state(format!("encode state JSON: {err}")))?;
        fs::write(path, contents)
            .map_err(|err| BridgeError::state(format!("write state {}: {err}", path.display())))
    }

    /// Last confirmed backend outbox sequence.
    #[must_use]
    pub const fn next_from_seq(&self) -> i64 {
        self.next_from_seq
    }
}

/// Summary of one bridge loop iteration.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeRunReport {
    /// Number of room registrations forwarded to agentd.
    pub registered_rooms: usize,
    /// Number of inbound Matrix events forwarded to agentd.
    pub inbound_forwarded: usize,
    /// Number of backend outbox events sent to Matrix.
    pub outbound_sent: usize,
    /// Number of Matrix bot command replies sent directly by the bridge.
    pub bot_command_replies_sent: usize,
}

/// Summary of a complete one-shot bridge process execution.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeOnceReport {
    /// The underlying deterministic runtime report.
    pub run: BridgeRunReport,
    /// Confirmed outbox cursor after the run completes.
    pub next_from_seq: i64,
    /// Optional Matrix puppet account provisioning report for this one-shot run.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub puppet_account_provisioning: Option<MatrixPuppetAccountProvisioningReport>,
}

/// Matrix room mapping observed by the Matrix-side bridge adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixRoomRegistration {
    /// Matrix room id, for example `!ops:matrix.test`.
    pub room_id: String,
    /// Optional agentd group name this room maps to.
    pub group_name: Option<String>,
    /// Optional single agent name this room maps to.
    pub agent_name: Option<String>,
    /// Whether the room is trusted for inbound delivery.
    pub trusted: bool,
    /// Reason recorded for the trust decision.
    pub trust_reason: String,
    /// Matrix user id that invited the bridge, if known.
    pub inviter_mxid: Option<String>,
    /// Agent names that should be members of a mapped group room.
    pub members: Vec<String>,
}

/// Inbound Matrix event ready to forward to the agentd backend contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixInboundEvent {
    /// Matrix event id.
    pub event_id: String,
    /// Matrix room id.
    pub room_id: String,
    /// Matrix sender user id.
    pub sender_mxid: String,
    /// Message body after Matrix-side normalization.
    pub body: String,
    /// Agent names mentioned by the event.
    pub mentions: Vec<String>,
    /// Optional reply target message or event id.
    pub reply_to: Option<String>,
}

/// Backend outbox event ready to send through the Matrix transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatrixOutboundEvent {
    /// Monotonic backend outbox sequence.
    pub seq: i64,
    /// Matrix room id to send into, if the backend payload is already
    /// bridge-ready.
    pub room_id: Option<String>,
    /// Agent or target name from the backend payload.
    pub target: Option<String>,
    /// Message body to send.
    pub body: String,
    /// Optional originating agentd message id.
    pub message_id: Option<String>,
    /// Optional source marker from the backend outbox payload.
    pub source: Option<String>,
    /// Raw backend relay payload for adapters that need extra routing metadata.
    pub payload: Value,
}

/// File-backed configuration for one deterministic bridge run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeOnceConfig {
    /// Agentd HTTP backend configuration.
    pub bridge_config: BridgeConfig,
    /// JSON state file containing `nextFromSeq`.
    pub state_path: PathBuf,
    /// JSON array of Matrix room registrations.
    pub rooms_json_path: PathBuf,
    /// JSON array of inbound Matrix events.
    pub inbound_json_path: PathBuf,
    /// JSONL file that receives Matrix outbound sends.
    pub sent_log_jsonl_path: PathBuf,
    /// Optional Matrix puppet account provisioning to execute before the bridge run.
    pub puppet_accounts: Option<BridgeOncePuppetAccountConfig>,
}

/// Optional Matrix puppet account provisioning for a one-shot bridge run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeOncePuppetAccountConfig {
    /// Planned local Matrix puppet identities.
    pub directory: MatrixPuppetDirectory,
    /// Password/login/register policy for Matrix puppet accounts.
    pub provisioning_config: MatrixPuppetProvisioningConfig,
    /// Matrix homeserver HTTP account-management configuration.
    pub http_account_config: MatrixPuppetHttpAccountConfig,
    /// Agent-chat-style bridge-state JSON file storing `agentTokens`.
    pub token_state_path: PathBuf,
}

/// SDK-facing configuration for one deterministic Matrix client bridge run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixClientBridgeOnceConfig {
    /// Agentd HTTP backend configuration.
    pub bridge_config: BridgeConfig,
    /// JSON state file containing `nextFromSeq`.
    pub state_path: PathBuf,
    /// Matrix client transport adaptation configuration.
    pub transport_config: MatrixClientTransportConfig,
    /// Optional Matrix puppet account provisioning to execute before Matrix sync.
    pub puppet_accounts: Option<BridgeOncePuppetAccountConfig>,
}

/// Trust handling mode for Matrix room invites and ingress.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixTrustMode {
    /// Join untrusted rooms but mark them untrusted so the backend can enforce.
    #[default]
    Audit,
    /// Reject untrusted rooms by leaving them and omitting registrations.
    Enforce,
}

/// Configuration for SDK-facing Matrix client transport adaptation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixClientTransportConfig {
    /// Optional bot user id override. When omitted, `ensure_logged_in` supplies it.
    pub bot_user_id: Option<String>,
    /// Matrix localpart prefix used by agent puppet users, for example `ac_`.
    pub agent_user_prefix: String,
    /// Matrix server name used for local puppet MXIDs, for example `matrix.example.com`.
    pub matrix_server_name: Option<String>,
    /// Agent names known to agentd and eligible for Matrix puppet accounts.
    pub known_agent_names: Vec<String>,
    /// Agent names that should not receive Matrix puppet accounts.
    pub skip_agent_names: Vec<String>,
    /// Invite trust handling mode.
    pub trust_mode: MatrixTrustMode,
    /// MXIDs allowed to invite the bridge into trusted rooms.
    pub trusted_inviter_mxids: Vec<String>,
    /// MXIDs that should never be forwarded into agentd.
    pub ignored_sender_mxids: Vec<String>,
    /// Agent-chat-compatible Matrix bot command ACL.
    pub bot_command_acl: MatrixBotCommandAcl,
}

impl Default for MatrixClientTransportConfig {
    fn default() -> Self {
        Self {
            bot_user_id: None,
            agent_user_prefix: "ac_".to_owned(),
            matrix_server_name: None,
            known_agent_names: Vec::new(),
            skip_agent_names: Vec::new(),
            trust_mode: MatrixTrustMode::Audit,
            trusted_inviter_mxids: Vec::new(),
            ignored_sender_mxids: Vec::new(),
            bot_command_acl: MatrixBotCommandAcl::default(),
        }
    }
}

/// Planned Matrix puppet account for one local agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixPuppetAccount {
    /// Canonical agent name as known by agentd.
    pub agent_name: String,
    /// Matrix localpart for the puppet user.
    pub localpart: String,
    /// Full local Matrix user id for the puppet user.
    pub mxid: String,
}

/// Local deterministic directory for agent Matrix puppet identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixPuppetDirectory {
    server_name: String,
    agent_user_prefix: String,
    accounts: Vec<MatrixPuppetAccount>,
}

impl MatrixPuppetDirectory {
    /// Build a local puppet directory from known and skipped agent names.
    pub fn new<A, S, B, T>(
        server_name: impl AsRef<str>,
        agent_user_prefix: impl AsRef<str>,
        agent_names: A,
        skip_agent_names: B,
    ) -> Result<Self, BridgeError>
    where
        A: IntoIterator<Item = S>,
        S: AsRef<str>,
        B: IntoIterator<Item = T>,
        T: AsRef<str>,
    {
        let server_name = normalize_matrix_server_name(server_name.as_ref())?;
        let agent_user_prefix = normalize_agent_user_prefix(agent_user_prefix.as_ref())?;
        let skip_keys = skip_agent_names
            .into_iter()
            .map(|name| normalize_matrix_agent_name(name.as_ref()).map(|name| name_key(&name)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut seen_keys = Vec::new();
        let mut accounts = Vec::new();

        for agent_name in agent_names {
            let agent_name = normalize_matrix_agent_name(agent_name.as_ref())?;
            let key = name_key(&agent_name);
            if skip_keys.iter().any(|skip| skip == &key)
                || seen_keys.iter().any(|seen| seen == &key)
            {
                continue;
            }
            seen_keys.push(key);
            let localpart = format!("{agent_user_prefix}{agent_name}");
            let mxid = format!("@{localpart}:{server_name}");
            accounts.push(MatrixPuppetAccount {
                agent_name,
                localpart,
                mxid,
            });
        }

        Ok(Self {
            server_name,
            agent_user_prefix,
            accounts,
        })
    }

    /// Planned puppet accounts in stable input order.
    #[must_use]
    pub fn accounts(&self) -> &[MatrixPuppetAccount] {
        &self.accounts
    }

    /// Find the planned puppet account for an agent name.
    #[must_use]
    pub fn account_for_agent(&self, agent_name: &str) -> Option<&MatrixPuppetAccount> {
        let key = name_key(&normalize_matrix_agent_name(agent_name).ok()?);
        self.accounts
            .iter()
            .find(|account| name_key(&account.agent_name) == key)
    }

    /// Resolve a Matrix MXID back to a known local puppet agent name.
    #[must_use]
    pub fn agent_name_from_mxid<'a>(&'a self, mxid: &str) -> Option<&'a str> {
        let (localpart, server_name) = matrix_user_parts(mxid)?;
        if server_name != self.server_name {
            return None;
        }
        localpart.strip_prefix(&self.agent_user_prefix)?;
        self.accounts
            .iter()
            .find(|account| account.localpart == localpart)
            .map(|account| account.agent_name.as_str())
    }

    /// Whether a Matrix MXID is one of the known non-skipped local puppets.
    #[must_use]
    pub fn is_agent_puppet_mxid(&self, mxid: &str) -> bool {
        self.agent_name_from_mxid(mxid).is_some()
    }
}

/// Local configuration for Matrix puppet login/register planning.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MatrixPuppetProvisioningConfig {
    /// Secret used to derive the preferred agent password.
    pub password_secret: Option<String>,
    /// Optional legacy password template used only when legacy fallback is enabled.
    pub legacy_password_template: Option<String>,
    /// Whether legacy password template fallback is allowed.
    pub allow_legacy_password: bool,
    /// Optional Matrix registration token for UIA registration.
    pub registration_token: Option<String>,
}

impl fmt::Debug for MatrixPuppetProvisioningConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MatrixPuppetProvisioningConfig")
            .field(
                "password_secret",
                &self
                    .password_secret
                    .as_ref()
                    .is_some_and(|secret| !secret.trim().is_empty()),
            )
            .field(
                "legacy_password_template",
                &self
                    .legacy_password_template
                    .as_ref()
                    .is_some_and(|template| !template.trim().is_empty()),
            )
            .field("allow_legacy_password", &self.allow_legacy_password)
            .field(
                "registration_token",
                &self
                    .registration_token
                    .as_ref()
                    .is_some_and(|token| !token.trim().is_empty()),
            )
            .finish()
    }
}

impl MatrixPuppetProvisioningConfig {
    /// Derive ordered password candidates for a Matrix puppet account.
    #[must_use]
    pub fn password_candidates(&self, agent_name: &str) -> Vec<String> {
        let Ok(agent_name) = normalize_matrix_agent_name(agent_name) else {
            return Vec::new();
        };
        let mut candidates = Vec::new();

        if let Some(secret) = non_empty_trimmed(self.password_secret.as_deref()) {
            push_unique(
                &mut candidates,
                sha256_hex(format!("{secret}:{agent_name}").as_bytes()),
            );
        }

        if self.allow_legacy_password
            && let Some(template) = non_empty_trimmed(self.legacy_password_template.as_deref())
        {
            let legacy = template
                .replace("{name}", &agent_name)
                .replace("${name}", &agent_name);
            push_unique(&mut candidates, legacy);
        }

        candidates
    }

    /// Choose the local Matrix UIA registration auth payload strategy.
    pub fn registration_auth(
        &self,
        session: &str,
        supports_dummy: bool,
    ) -> Result<MatrixPuppetRegistrationAuth, BridgeError> {
        let Some(session) = non_empty_trimmed(Some(session)) else {
            return Err(BridgeError::invalid_config(
                "Matrix registration session is required",
            ));
        };

        if let Some(token) = non_empty_trimmed(self.registration_token.as_deref()) {
            return Ok(MatrixPuppetRegistrationAuth::RegistrationToken {
                token: token.to_owned(),
                session: session.to_owned(),
            });
        }

        if supports_dummy {
            return Ok(MatrixPuppetRegistrationAuth::Dummy {
                session: session.to_owned(),
            });
        }

        Err(BridgeError::invalid_config(
            "No usable Matrix registration flow: set MATRIX_REG_TOKEN or enable open registration",
        ))
    }
}

/// Local Matrix UIA auth choice for puppet account registration.
#[derive(Clone, PartialEq, Eq)]
pub enum MatrixPuppetRegistrationAuth {
    /// Use `m.login.registration_token`.
    RegistrationToken { token: String, session: String },
    /// Use `m.login.dummy`.
    Dummy { session: String },
}

impl fmt::Debug for MatrixPuppetRegistrationAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RegistrationToken { session, .. } => f
                .debug_struct("RegistrationToken")
                .field("token", &"***")
                .field("session", session)
                .finish(),
            Self::Dummy { session } => f.debug_struct("Dummy").field("session", session).finish(),
        }
    }
}

/// In-memory view of Matrix puppet access tokens keyed by agent name.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MatrixPuppetTokenState {
    agent_tokens: BTreeMap<String, String>,
}

impl fmt::Debug for MatrixPuppetTokenState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MatrixPuppetTokenState")
            .field(
                "agent_token_names",
                &self.agent_tokens.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl MatrixPuppetTokenState {
    /// Build token state from persisted `agentTokens`-style entries.
    #[must_use]
    pub fn from_agent_tokens<I, K, V>(agent_tokens: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut state = Self::default();
        for (name, token) in agent_tokens {
            let name = name.into();
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            state.agent_tokens.insert(name.to_owned(), token.into());
        }
        state
    }

    /// Find the stored token key for an agent, matching case-insensitively.
    #[must_use]
    pub fn token_name_for_agent(&self, agent_name: &str) -> Option<&str> {
        let key = name_key(&normalize_matrix_agent_name(agent_name).ok()?);
        self.agent_tokens
            .keys()
            .find(|name| name_key(name) == key)
            .map(String::as_str)
    }

    /// Return the stored token value for an agent, matching case-insensitively.
    #[must_use]
    pub fn token_for_agent(&self, agent_name: &str) -> Option<&str> {
        self.token_name_for_agent(agent_name)
            .and_then(|name| self.agent_tokens.get(name))
            .map(String::as_str)
    }

    /// Stored token names that no longer correspond to planned non-skipped puppets.
    #[must_use]
    pub fn stale_token_names(&self, directory: &MatrixPuppetDirectory) -> Vec<String> {
        self.agent_tokens
            .keys()
            .filter(|name| directory.account_for_agent(name).is_none())
            .cloned()
            .collect()
    }
}

/// Planned local action for one Matrix puppet account.
#[derive(Clone, PartialEq, Eq)]
pub enum MatrixPuppetProvisioningAction {
    /// A matching existing token can be reused.
    ReuseToken {
        agent_name: String,
        localpart: String,
        mxid: String,
        token_name: String,
    },
    /// The caller should try Matrix login, then registration if login fails.
    LoginOrRegister {
        agent_name: String,
        localpart: String,
        mxid: String,
        password_candidates: Vec<String>,
    },
    /// No existing token or usable password candidate is available.
    MissingPassword {
        agent_name: String,
        localpart: String,
        mxid: String,
    },
}

impl fmt::Debug for MatrixPuppetProvisioningAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReuseToken {
                agent_name,
                localpart,
                mxid,
                token_name,
            } => f
                .debug_struct("ReuseToken")
                .field("agent_name", agent_name)
                .field("localpart", localpart)
                .field("mxid", mxid)
                .field("token_name", token_name)
                .finish(),
            Self::LoginOrRegister {
                agent_name,
                localpart,
                mxid,
                password_candidates,
            } => f
                .debug_struct("LoginOrRegister")
                .field("agent_name", agent_name)
                .field("localpart", localpart)
                .field("mxid", mxid)
                .field("password_candidate_count", &password_candidates.len())
                .finish(),
            Self::MissingPassword {
                agent_name,
                localpart,
                mxid,
            } => f
                .debug_struct("MissingPassword")
                .field("agent_name", agent_name)
                .field("localpart", localpart)
                .field("mxid", mxid)
                .finish(),
        }
    }
}

/// Local Matrix puppet provisioning plan derived from identities and token state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixPuppetProvisioningPlan {
    actions: Vec<MatrixPuppetProvisioningAction>,
    stale_token_names: Vec<String>,
}

impl MatrixPuppetProvisioningPlan {
    /// Build a local provisioning plan from a p243 puppet directory.
    #[must_use]
    pub fn from_directory(
        directory: &MatrixPuppetDirectory,
        config: &MatrixPuppetProvisioningConfig,
        token_state: &MatrixPuppetTokenState,
    ) -> Self {
        let actions = directory
            .accounts()
            .iter()
            .map(|account| {
                if let Some(token_name) = token_state.token_name_for_agent(&account.agent_name) {
                    return MatrixPuppetProvisioningAction::ReuseToken {
                        agent_name: account.agent_name.clone(),
                        localpart: account.localpart.clone(),
                        mxid: account.mxid.clone(),
                        token_name: token_name.to_owned(),
                    };
                }

                let password_candidates = config.password_candidates(&account.agent_name);
                if password_candidates.is_empty() {
                    MatrixPuppetProvisioningAction::MissingPassword {
                        agent_name: account.agent_name.clone(),
                        localpart: account.localpart.clone(),
                        mxid: account.mxid.clone(),
                    }
                } else {
                    MatrixPuppetProvisioningAction::LoginOrRegister {
                        agent_name: account.agent_name.clone(),
                        localpart: account.localpart.clone(),
                        mxid: account.mxid.clone(),
                        password_candidates,
                    }
                }
            })
            .collect();
        let stale_token_names = token_state.stale_token_names(directory);

        Self {
            actions,
            stale_token_names,
        }
    }

    /// Planned actions in stable puppet directory order.
    #[must_use]
    pub fn actions(&self) -> &[MatrixPuppetProvisioningAction] {
        &self.actions
    }

    /// Stored token names that should be pruned by the caller.
    #[must_use]
    pub fn stale_token_names(&self) -> &[String] {
        &self.stale_token_names
    }
}

/// Result of validating a Matrix access token against `/account/whoami`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixPuppetWhoami {
    /// Matrix user id returned by the homeserver.
    pub user_id: String,
}

/// Result of a Matrix puppet login or registration call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixPuppetAccountSession {
    /// Matrix user id returned by the homeserver.
    pub user_id: String,
    /// Matrix access token returned by the homeserver.
    pub access_token: String,
}

/// Matrix account operations needed to provision local agent puppet users.
pub trait MatrixPuppetAccountPort {
    /// Validate an existing access token.
    fn whoami(&mut self, access_token: &str) -> Result<MatrixPuppetWhoami, BridgeError>;

    /// Attempt password login for a Matrix localpart.
    fn login(
        &mut self,
        localpart: &str,
        password: &str,
    ) -> Result<MatrixPuppetAccountSession, BridgeError>;

    /// Register a Matrix localpart with the preferred password candidate.
    fn register(
        &mut self,
        localpart: &str,
        password: &str,
    ) -> Result<MatrixPuppetAccountSession, BridgeError>;
}

/// Durable update target for Matrix puppet access tokens.
pub trait MatrixPuppetTokenSink {
    /// Save or replace the stored access token for a canonical agent name.
    fn save_agent_token(&mut self, agent_name: &str, access_token: &str)
    -> Result<(), BridgeError>;

    /// Delete a stale stored token entry by its persisted key.
    fn delete_agent_token(&mut self, token_name: &str) -> Result<(), BridgeError>;
}

/// File-backed Matrix puppet token store for agent-chat-style bridge state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixPuppetTokenFileStore {
    path: PathBuf,
}

impl MatrixPuppetTokenFileStore {
    /// Build a file-backed Matrix puppet token store.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Path to the bridge state JSON file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the current `agentTokens` object as a Matrix puppet token state.
    pub fn load_token_state(&self) -> Result<MatrixPuppetTokenState, BridgeError> {
        let value = self.load_state_value()?;
        let tokens = value
            .get("agentTokens")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|tokens| tokens.iter())
            .filter_map(|(name, token)| token.as_str().map(|token| (name.as_str(), token)));
        Ok(MatrixPuppetTokenState::from_agent_tokens(tokens))
    }

    fn load_state_value(&self) -> Result<Value, BridgeError> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|err| {
                BridgeError::state(format!(
                    "decode Matrix puppet token state {}: {err}",
                    self.path.display()
                ))
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(default_matrix_bridge_state())
            }
            Err(err) => Err(BridgeError::state(format!(
                "read Matrix puppet token state {}: {err}",
                self.path.display()
            ))),
        }
    }

    fn write_state_value(&self, value: &Value) -> Result<(), BridgeError> {
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|err| {
                BridgeError::state(format!(
                    "create Matrix puppet token state dir {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let contents = serde_json::to_string_pretty(value).map_err(|err| {
            BridgeError::state(format!("encode Matrix puppet token state: {err}"))
        })?;
        let temp_path = self.temp_path();
        fs::write(&temp_path, contents).map_err(|err| {
            BridgeError::state(format!(
                "write Matrix puppet token temp state {}: {err}",
                temp_path.display()
            ))
        })?;
        fs::rename(&temp_path, &self.path).map_err(|err| {
            BridgeError::state(format!(
                "replace Matrix puppet token state {}: {err}",
                self.path.display()
            ))
        })
    }

    fn temp_path(&self) -> PathBuf {
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bridge-state.json");
        self.path.with_file_name(format!(".{file_name}.tmp"))
    }
}

impl MatrixPuppetTokenSink for MatrixPuppetTokenFileStore {
    fn save_agent_token(
        &mut self,
        agent_name: &str,
        access_token: &str,
    ) -> Result<(), BridgeError> {
        let agent_name = normalize_matrix_agent_name(agent_name)?;
        let access_token = non_empty_trimmed(Some(access_token))
            .ok_or_else(|| BridgeError::invalid_config("Matrix puppet access token is required"))?;
        let mut value = self.load_state_value()?;
        let agent_tokens = ensure_agent_tokens_object(&mut value)?;
        let existing_key = agent_tokens
            .keys()
            .find(|name| name_key(name) == name_key(&agent_name))
            .cloned();
        let token_name = existing_key.unwrap_or(agent_name);
        agent_tokens.insert(token_name, Value::String(access_token.to_owned()));
        self.write_state_value(&value)
    }

    fn delete_agent_token(&mut self, token_name: &str) -> Result<(), BridgeError> {
        let Some(token_name) = non_empty_trimmed(Some(token_name)) else {
            return Ok(());
        };
        let mut value = self.load_state_value()?;
        let agent_tokens = ensure_agent_tokens_object(&mut value)?;
        agent_tokens.remove(token_name);
        self.write_state_value(&value)
    }
}

/// Account-management step that produced a failed outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixPuppetAccountStep {
    /// Existing token validation.
    Whoami,
    /// Password login.
    Login,
    /// Account registration.
    Register,
    /// Token persistence after login or registration.
    SaveToken,
    /// Stale token pruning.
    PruneToken,
}

/// Outcome for one planned Matrix puppet account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixPuppetAccountOutcome {
    /// A stored token was accepted by whoami.
    ReusedToken {
        agent_name: String,
        localpart: String,
        mxid: String,
        token_name: String,
        user_id: String,
    },
    /// Password login succeeded and a new token was saved.
    LoggedIn {
        agent_name: String,
        localpart: String,
        mxid: String,
        user_id: String,
    },
    /// Registration succeeded and a new token was saved.
    Registered {
        agent_name: String,
        localpart: String,
        mxid: String,
        user_id: String,
    },
    /// No existing valid token or usable password candidate was available.
    MissingPassword {
        agent_name: String,
        localpart: String,
        mxid: String,
    },
    /// Provisioning failed for one puppet account without aborting the full run.
    Failed {
        agent_name: String,
        localpart: String,
        mxid: String,
        step: MatrixPuppetAccountStep,
        error: String,
    },
}

/// Failure while pruning a stale Matrix puppet token entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixPuppetTokenPruneFailure {
    /// Persisted token key that could not be deleted.
    pub token_name: String,
    /// Error message returned by the token sink.
    pub error: String,
}

/// Report produced by Matrix puppet account provisioning.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixPuppetAccountProvisioningReport {
    outcomes: Vec<MatrixPuppetAccountOutcome>,
    pruned_token_names: Vec<String>,
    prune_failures: Vec<MatrixPuppetTokenPruneFailure>,
}

impl MatrixPuppetAccountProvisioningReport {
    /// Per-puppet outcomes in stable directory order.
    #[must_use]
    pub fn outcomes(&self) -> &[MatrixPuppetAccountOutcome] {
        &self.outcomes
    }

    /// Stale token names successfully deleted by the token sink.
    #[must_use]
    pub fn pruned_token_names(&self) -> &[String] {
        &self.pruned_token_names
    }

    /// Stale token names that the token sink failed to delete.
    #[must_use]
    pub fn prune_failures(&self) -> &[MatrixPuppetTokenPruneFailure] {
        &self.prune_failures
    }
}

/// Executes local Matrix puppet account provisioning through fakeable ports.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MatrixPuppetAccountExecutor;

impl MatrixPuppetAccountExecutor {
    /// Build a stateless Matrix puppet account executor.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Provision planned Matrix puppet accounts and record token-state updates.
    pub fn provision<P, S>(
        &self,
        directory: &MatrixPuppetDirectory,
        config: &MatrixPuppetProvisioningConfig,
        token_state: &MatrixPuppetTokenState,
        account_port: &mut P,
        token_sink: &mut S,
    ) -> MatrixPuppetAccountProvisioningReport
    where
        P: MatrixPuppetAccountPort,
        S: MatrixPuppetTokenSink,
    {
        let mut report = MatrixPuppetAccountProvisioningReport::default();

        report
            .outcomes
            .extend(directory.accounts().iter().map(|account| {
                Self::provision_account(account, config, token_state, account_port, token_sink)
            }));

        for token_name in token_state.stale_token_names(directory) {
            match token_sink.delete_agent_token(&token_name) {
                Ok(()) => report.pruned_token_names.push(token_name),
                Err(err) => report.prune_failures.push(MatrixPuppetTokenPruneFailure {
                    token_name,
                    error: err.to_string(),
                }),
            }
        }

        report
    }

    fn provision_account<P, S>(
        account: &MatrixPuppetAccount,
        config: &MatrixPuppetProvisioningConfig,
        token_state: &MatrixPuppetTokenState,
        account_port: &mut P,
        token_sink: &mut S,
    ) -> MatrixPuppetAccountOutcome
    where
        P: MatrixPuppetAccountPort,
        S: MatrixPuppetTokenSink,
    {
        if let Some(token_name) = token_state.token_name_for_agent(&account.agent_name)
            && let Some(access_token) = token_state.token_for_agent(&account.agent_name)
            && let Ok(whoami) = account_port.whoami(access_token)
        {
            return MatrixPuppetAccountOutcome::ReusedToken {
                agent_name: account.agent_name.clone(),
                localpart: account.localpart.clone(),
                mxid: account.mxid.clone(),
                token_name: token_name.to_owned(),
                user_id: whoami.user_id,
            };
        }

        let password_candidates = config.password_candidates(&account.agent_name);
        if password_candidates.is_empty() {
            return MatrixPuppetAccountOutcome::MissingPassword {
                agent_name: account.agent_name.clone(),
                localpart: account.localpart.clone(),
                mxid: account.mxid.clone(),
            };
        }

        for password in &password_candidates {
            if let Ok(session) = account_port.login(&account.localpart, password) {
                return Self::save_session(
                    account,
                    session,
                    token_sink,
                    MatrixPuppetAccountSuccess::LoggedIn,
                );
            }
        }

        match account_port.register(&account.localpart, &password_candidates[0]) {
            Ok(session) => Self::save_session(
                account,
                session,
                token_sink,
                MatrixPuppetAccountSuccess::Registered,
            ),
            Err(err) => MatrixPuppetAccountOutcome::Failed {
                agent_name: account.agent_name.clone(),
                localpart: account.localpart.clone(),
                mxid: account.mxid.clone(),
                step: MatrixPuppetAccountStep::Register,
                error: err.to_string(),
            },
        }
    }

    fn save_session<S>(
        account: &MatrixPuppetAccount,
        session: MatrixPuppetAccountSession,
        token_sink: &mut S,
        success: MatrixPuppetAccountSuccess,
    ) -> MatrixPuppetAccountOutcome
    where
        S: MatrixPuppetTokenSink,
    {
        match token_sink.save_agent_token(&account.agent_name, &session.access_token) {
            Ok(()) => success.outcome(account, session.user_id),
            Err(err) => MatrixPuppetAccountOutcome::Failed {
                agent_name: account.agent_name.clone(),
                localpart: account.localpart.clone(),
                mxid: account.mxid.clone(),
                step: MatrixPuppetAccountStep::SaveToken,
                error: err.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatrixPuppetAccountSuccess {
    LoggedIn,
    Registered,
}

impl MatrixPuppetAccountSuccess {
    fn outcome(self, account: &MatrixPuppetAccount, user_id: String) -> MatrixPuppetAccountOutcome {
        match self {
            Self::LoggedIn => MatrixPuppetAccountOutcome::LoggedIn {
                agent_name: account.agent_name.clone(),
                localpart: account.localpart.clone(),
                mxid: account.mxid.clone(),
                user_id,
            },
            Self::Registered => MatrixPuppetAccountOutcome::Registered {
                agent_name: account.agent_name.clone(),
                localpart: account.localpart.clone(),
                mxid: account.mxid.clone(),
                user_id,
            },
        }
    }
}

/// Configuration for the standard-library Matrix puppet account HTTP port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixPuppetHttpAccountConfig {
    homeserver_url: String,
    registration_token: Option<String>,
}

impl MatrixPuppetHttpAccountConfig {
    /// Build a Matrix account HTTP config from a direct homeserver URL.
    pub fn new(homeserver_url: impl Into<String>) -> Result<Self, BridgeError> {
        let homeserver_url = homeserver_url.into();
        let homeserver_url = homeserver_url.trim().trim_end_matches('/').to_owned();
        HttpEndpoint::parse_labeled(&homeserver_url, "matrix_homeserver_url")?;
        Ok(Self {
            homeserver_url,
            registration_token: None,
        })
    }

    /// Configure an optional Matrix registration token for UIA completion.
    #[must_use]
    pub fn with_registration_token(mut self, token: impl Into<String>) -> Self {
        let token = token.into();
        let token = token.trim();
        self.registration_token = if token.is_empty() {
            None
        } else {
            Some(token.to_owned())
        };
        self
    }

    /// Direct Matrix homeserver URL.
    #[must_use]
    pub fn homeserver_url(&self) -> &str {
        &self.homeserver_url
    }

    /// Optional Matrix registration token.
    #[must_use]
    pub fn registration_token(&self) -> Option<&str> {
        self.registration_token.as_deref()
    }
}

/// Standard-library HTTP implementation of [`MatrixPuppetAccountPort`].
#[derive(Debug, Clone)]
pub struct MatrixPuppetHttpAccountPort {
    endpoint: HttpEndpoint,
    registration_token: Option<String>,
}

impl MatrixPuppetHttpAccountPort {
    /// Build a Matrix puppet account HTTP port.
    pub fn new(config: &MatrixPuppetHttpAccountConfig) -> Result<Self, BridgeError> {
        Ok(Self {
            endpoint: HttpEndpoint::parse_labeled(
                config.homeserver_url(),
                "matrix_homeserver_url",
            )?,
            registration_token: config.registration_token().map(ToOwned::to_owned),
        })
    }

    fn request_json(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        bearer_token: Option<&str>,
    ) -> Result<Value, BridgeError> {
        let (status, value) = self.request_json_status(method, path, body, bearer_token)?;
        if !(200..300).contains(&status) {
            return Err(BridgeError::transport(format!(
                "{method} {path} returned status {status}: {value}"
            )));
        }
        Ok(value)
    }

    fn request_json_status(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        bearer_token: Option<&str>,
    ) -> Result<(u16, Value), BridgeError> {
        let request_path = self.endpoint.path(path);
        let body = match body {
            Some(value) => serde_json::to_string(&value)
                .map_err(|err| BridgeError::transport(format!("encode JSON body: {err}")))?,
            None => String::new(),
        };
        let response = self.http_request(method, &request_path, &body, bearer_token)?;
        let value = serde_json::from_slice(&response.body).map_err(|err| {
            BridgeError::transport(format!("decode JSON from {method} {request_path}: {err}"))
        })?;
        Ok((response.status, value))
    }

    fn http_request(
        &self,
        method: &str,
        path: &str,
        body: &str,
        bearer_token: Option<&str>,
    ) -> Result<HttpResponse, BridgeError> {
        let address = self.endpoint.address();
        let mut stream = TcpStream::connect(&address)
            .map_err(|err| BridgeError::transport(format!("connect {address}: {err}")))?;
        let timeout = Some(Duration::from_secs(5));
        stream
            .set_read_timeout(timeout)
            .map_err(|err| BridgeError::transport(format!("set read timeout: {err}")))?;
        stream
            .set_write_timeout(timeout)
            .map_err(|err| BridgeError::transport(format!("set write timeout: {err}")))?;

        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n",
            self.endpoint.host_header
        );
        if let Some(token) = bearer_token {
            let _ = write!(request, "Authorization: Bearer {token}\r\n");
        }
        if !body.is_empty() {
            request.push_str("Content-Type: application/json\r\n");
            let _ = write!(request, "Content-Length: {}\r\n", body.len());
        }
        request.push_str("\r\n");
        request.push_str(body);

        stream
            .write_all(request.as_bytes())
            .map_err(|err| BridgeError::transport(format!("write HTTP request: {err}")))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|err| BridgeError::transport(format!("read HTTP response: {err}")))?;
        HttpResponse::parse(&response).map_err(|err| BridgeError::transport(err.to_string()))
    }

    fn decode_session(
        value: &Value,
        context: &str,
    ) -> Result<MatrixPuppetAccountSession, BridgeError> {
        let user_id = required_json_string(value, "user_id", context)?;
        let access_token = required_json_string(value, "access_token", context)?;
        Ok(MatrixPuppetAccountSession {
            user_id,
            access_token,
        })
    }

    fn complete_registration(
        &self,
        localpart: &str,
        password: &str,
        session: &str,
        flows: Option<&Value>,
    ) -> Result<MatrixPuppetAccountSession, BridgeError> {
        let auth = if let Some(token) = self.registration_token.as_deref() {
            json!({
                "type": "m.login.registration_token",
                "token": token,
                "session": session,
            })
        } else if registration_flows_support_dummy(flows) {
            json!({
                "type": "m.login.dummy",
                "session": session,
            })
        } else {
            return Err(BridgeError::transport(format!(
                "No usable Matrix registration flow for {localpart}: set MATRIX_REG_TOKEN or enable open registration"
            )));
        };

        let value = self.request_json(
            "POST",
            "/_matrix/client/v3/register",
            Some(json!({
                "username": localpart,
                "password": password,
                "auth": auth,
            })),
            None,
        )?;
        Self::decode_session(&value, "Matrix registration completion response")
    }
}

/// Assembles HTTP Matrix puppet account operations with the provisioning executor.
#[derive(Debug, Clone)]
pub struct MatrixPuppetHttpAccountProvisioner {
    account_port: MatrixPuppetHttpAccountPort,
    executor: MatrixPuppetAccountExecutor,
}

impl MatrixPuppetHttpAccountProvisioner {
    /// Build an HTTP-backed Matrix puppet account provisioner.
    pub fn new(config: &MatrixPuppetHttpAccountConfig) -> Result<Self, BridgeError> {
        Ok(Self {
            account_port: MatrixPuppetHttpAccountPort::new(config)?,
            executor: MatrixPuppetAccountExecutor::new(),
        })
    }

    /// Provision Matrix puppet accounts through the HTTP account port.
    pub fn provision<S>(
        &self,
        directory: &MatrixPuppetDirectory,
        config: &MatrixPuppetProvisioningConfig,
        token_state: &MatrixPuppetTokenState,
        token_sink: &mut S,
    ) -> MatrixPuppetAccountProvisioningReport
    where
        S: MatrixPuppetTokenSink,
    {
        let mut account_port = self.account_port.clone();
        self.executor.provision(
            directory,
            config,
            token_state,
            &mut account_port,
            token_sink,
        )
    }
}

impl MatrixPuppetAccountPort for MatrixPuppetHttpAccountPort {
    fn whoami(&mut self, access_token: &str) -> Result<MatrixPuppetWhoami, BridgeError> {
        let value = self.request_json(
            "GET",
            "/_matrix/client/v3/account/whoami",
            None,
            Some(access_token),
        )?;
        Ok(MatrixPuppetWhoami {
            user_id: required_json_string(&value, "user_id", "Matrix whoami response")?,
        })
    }

    fn login(
        &mut self,
        localpart: &str,
        password: &str,
    ) -> Result<MatrixPuppetAccountSession, BridgeError> {
        let value = self.request_json(
            "POST",
            "/_matrix/client/v3/login",
            Some(json!({
                "type": "m.login.password",
                "identifier": {
                    "type": "m.id.user",
                    "user": localpart,
                },
                "password": password,
            })),
            None,
        )?;
        Self::decode_session(&value, "Matrix login response")
    }

    fn register(
        &mut self,
        localpart: &str,
        password: &str,
    ) -> Result<MatrixPuppetAccountSession, BridgeError> {
        let (status, value) = self.request_json_status(
            "POST",
            "/_matrix/client/v3/register",
            Some(json!({
                "username": localpart,
                "password": password,
            })),
            None,
        )?;

        if (200..300).contains(&status) && value.get("access_token").is_some() {
            return Self::decode_session(&value, "Matrix registration probe response");
        }

        if !(200..300).contains(&status) && status != 401 {
            return Err(BridgeError::transport(format!(
                "POST /_matrix/client/v3/register returned status {status}: {value}"
            )));
        }

        let session =
            required_json_string(&value, "session", "Matrix registration probe response")?;
        self.complete_registration(localpart, password, &session, value.get("flows"))
    }
}

fn registration_flows_support_dummy(flows: Option<&Value>) -> bool {
    flows.and_then(Value::as_array).is_some_and(|flows| {
        flows.iter().any(|flow| {
            flow.get("stages")
                .and_then(Value::as_array)
                .is_some_and(|stages| {
                    stages.len() == 1 && stages[0].as_str() == Some("m.login.dummy")
                })
        })
    })
}

fn required_json_string(value: &Value, key: &str, context: &str) -> Result<String, BridgeError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| BridgeError::transport(format!("{context} missing {key}")))
}

/// Configuration for the feature-gated real Matrix SDK client adapter.
#[cfg(feature = "matrix-sdk-adapter")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkMatrixClientConfig {
    /// Direct Matrix homeserver URL, for example `https://matrix.example.com`.
    pub homeserver_url: String,
    /// Optional username for password login.
    pub username: Option<String>,
    /// Optional password for password login.
    pub password: Option<String>,
    /// Optional Matrix user id for access-token session restore.
    pub user_id: Option<String>,
    /// Optional Matrix device id for access-token session restore.
    pub device_id: Option<String>,
    /// Optional Matrix access token for session restore.
    pub access_token: Option<String>,
    /// Timeout for one SDK `/sync` call in milliseconds.
    pub sync_timeout_ms: u64,
    /// Optional `SQLite` store directory for SDK state.
    pub sqlite_store_path: Option<PathBuf>,
}

#[cfg(feature = "matrix-sdk-adapter")]
impl SdkMatrixClientConfig {
    /// Build a config for a direct homeserver URL.
    #[must_use]
    pub fn new(homeserver_url: impl Into<String>) -> Self {
        Self {
            homeserver_url: homeserver_url.into(),
            username: None,
            password: None,
            user_id: None,
            device_id: None,
            access_token: None,
            sync_timeout_ms: 0,
            sqlite_store_path: None,
        }
    }

    /// Configure username/password login.
    #[must_use]
    pub fn with_password_login(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    /// Configure access-token session restore.
    #[must_use]
    pub fn with_access_token(
        mut self,
        user_id: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        self.user_id = Some(user_id.into());
        self.access_token = Some(access_token.into());
        self
    }

    /// Configure a Matrix device id for access-token session restore.
    #[must_use]
    pub fn with_device_id(mut self, device_id: impl Into<String>) -> Self {
        self.device_id = Some(device_id.into());
        self
    }

    /// Configure one-shot sync timeout in milliseconds.
    #[must_use]
    pub const fn with_sync_timeout_ms(mut self, sync_timeout_ms: u64) -> Self {
        self.sync_timeout_ms = sync_timeout_ms;
        self
    }

    /// Configure an SDK `SQLite` store directory.
    #[must_use]
    pub fn with_sqlite_store_path(mut self, sqlite_store_path: impl Into<PathBuf>) -> Self {
        self.sqlite_store_path = Some(sqlite_store_path.into());
        self
    }

    /// Validate the direct homeserver URL and credential mode.
    pub fn validate(&self) -> Result<(), BridgeError> {
        let homeserver_url = self.homeserver_url.trim();
        if homeserver_url.is_empty() {
            return Err(BridgeError::invalid_config(
                "Matrix homeserver URL is required",
            ));
        }
        if !homeserver_url.starts_with("http://") && !homeserver_url.starts_with("https://") {
            return Err(BridgeError::invalid_config(
                "Matrix homeserver URL must start with http:// or https://",
            ));
        }
        matrix_sdk::reqwest::Url::parse(homeserver_url).map_err(|err| {
            BridgeError::invalid_config(format!("Matrix homeserver URL is invalid: {err}"))
        })?;

        let has_password_login = self.username.is_some() || self.password.is_some();
        let has_token_restore = self.access_token.is_some() || self.user_id.is_some();
        if has_password_login && has_token_restore {
            return Err(BridgeError::invalid_config(
                "Matrix password login and access-token restore are mutually exclusive",
            ));
        }
        if has_password_login && (self.username.is_none() || self.password.is_none()) {
            return Err(BridgeError::invalid_config(
                "Matrix password login requires both username and password",
            ));
        }
        if has_token_restore && (self.user_id.is_none() || self.access_token.is_none()) {
            return Err(BridgeError::invalid_config(
                "Matrix token restore requires both user_id and access_token",
            ));
        }
        Ok(())
    }

    fn normalized(mut self) -> Result<Self, BridgeError> {
        self.validate()?;
        self.homeserver_url = self.homeserver_url.trim().trim_end_matches('/').to_owned();
        if let Some(username) = self.username.take() {
            let trimmed = username.trim();
            self.username = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        }
        if let Some(password) = self.password.take() {
            let trimmed = password.trim();
            self.password = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        }
        if let Some(user_id) = self.user_id.take() {
            let trimmed = user_id.trim();
            self.user_id = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        }
        if let Some(device_id) = self.device_id.take() {
            let trimmed = device_id.trim();
            self.device_id = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        }
        if let Some(access_token) = self.access_token.take() {
            let trimmed = access_token.trim();
            self.access_token = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        }
        self.validate()?;
        Ok(self)
    }
}

/// Feature-gated real Matrix SDK implementation of [`MatrixClientPort`].
#[cfg(feature = "matrix-sdk-adapter")]
#[derive(Debug)]
pub struct SdkMatrixClient {
    client: matrix_sdk::Client,
    runtime: tokio::runtime::Runtime,
    config: SdkMatrixClientConfig,
}

#[cfg(feature = "matrix-sdk-adapter")]
impl SdkMatrixClient {
    /// Build a local SDK client from direct homeserver configuration.
    pub fn build(config: SdkMatrixClientConfig) -> Result<Self, BridgeError> {
        let config = config.normalized()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| BridgeError::transport(format!("build Matrix SDK runtime: {err}")))?;
        let client = runtime.block_on(async {
            let mut builder = matrix_sdk::Client::builder()
                .homeserver_url(&config.homeserver_url)
                .respect_login_well_known(false);
            if let Some(path) = &config.sqlite_store_path {
                builder = builder.sqlite_store(path, None);
            }
            builder.build().await
        });
        let client = client
            .map_err(|err| BridgeError::transport(format!("build Matrix SDK client: {err}")))?;
        Ok(Self {
            client,
            runtime,
            config,
        })
    }

    /// Direct homeserver URL used to build the SDK client.
    #[must_use]
    pub fn homeserver_url(&self) -> &str {
        &self.config.homeserver_url
    }

    /// Borrow the underlying Matrix SDK client.
    #[must_use]
    pub const fn sdk_client(&self) -> &matrix_sdk::Client {
        &self.client
    }

    fn sdk_room(&self, room_id: &str) -> Result<matrix_sdk::Room, BridgeError> {
        let room_id = matrix_sdk::ruma::RoomId::parse(room_id).map_err(|err| {
            BridgeError::transport(format!("invalid Matrix room id {room_id}: {err}"))
        })?;
        self.client.get_room(&room_id).ok_or_else(|| {
            BridgeError::transport(format!(
                "Matrix room {room_id} is not known to the SDK client"
            ))
        })
    }

    fn current_user_id(&self) -> Option<String> {
        self.client.user_id().map(ToString::to_string)
    }
}

/// Normalized Matrix client sync snapshot consumed by the bridge transport.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixClientSync {
    /// Room invitations visible to the bot client.
    pub invites: Vec<MatrixClientInvite>,
    /// Joined rooms visible to the bot client.
    pub joined_rooms: Vec<MatrixClientRoom>,
    /// New text events visible in the sync response.
    pub text_events: Vec<MatrixClientTextMessage>,
}

/// Normalized Matrix room invitation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixClientInvite {
    /// Matrix room id.
    pub room_id: String,
    /// Optional group mapping represented by the room.
    pub group_name: Option<String>,
    /// Optional direct-agent mapping represented by the room.
    pub agent_name: Option<String>,
    /// MXID that invited the bot client, if available.
    pub inviter_mxid: Option<String>,
    /// Agent names that should be members of the mapped room.
    pub members: Vec<String>,
}

/// Normalized joined Matrix room metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixClientRoom {
    /// Matrix room id.
    pub room_id: String,
    /// Optional group mapping represented by the room.
    pub group_name: Option<String>,
    /// Optional direct-agent mapping represented by the room.
    pub agent_name: Option<String>,
    /// Whether the room is trusted for ingress and target resolution.
    pub trusted: bool,
    /// Reason recorded for the trust decision.
    pub trust_reason: String,
    /// MXID that invited the bot client, if available.
    pub inviter_mxid: Option<String>,
    /// Agent names that should be members of the mapped room.
    pub members: Vec<String>,
}

/// Normalized Matrix text event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixClientTextMessage {
    /// Matrix event id.
    pub event_id: String,
    /// Matrix room id.
    pub room_id: String,
    /// Matrix sender MXID.
    pub sender_mxid: String,
    /// Plain text body after SDK-side event parsing.
    pub body: String,
    /// Optional Matrix custom-HTML formatted body.
    pub formatted_body: Option<String>,
    /// Agent names mentioned by the event.
    pub mentions: Vec<String>,
    /// Optional reply target message or event id.
    pub reply_to: Option<String>,
}

/// Agent-chat-compatible Matrix bot command ACL configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotCommandAcl {
    /// MXIDs allowed to run operator read and management commands.
    pub operator_mxids: Vec<String>,
    /// MXIDs allowed to run every command, including admin-only controls.
    pub admin_mxids: Vec<String>,
}

/// Matrix room context that agent-chat derives before command dispatch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotCommandContext {
    /// Agent-chat group name mapped to the Matrix room, if any.
    pub group_name: Option<String>,
    /// Agent name mapped to the Matrix direct room, if any.
    pub target_agent: Option<String>,
}

/// Authorization tier for agent-chat Matrix bot commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixBotCommandTier {
    /// Public command that does not require operator/admin ACL membership.
    Public,
    /// Operator read command such as `!status` or `!agents`.
    OperatorRead,
    /// Operator mutation command such as `!dm` or `!identity`.
    OperatorManagement,
    /// Admin-only command such as `!spy` or `!ctl`.
    Admin,
}

/// Agent-chat-compatible reason for a Matrix bot command authorization result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixBotCommandAuthReason {
    /// Public command.
    Public,
    /// Sender is a configured Matrix admin.
    Admin,
    /// Sender is a configured Matrix operator.
    Operator,
    /// No ACL is configured, so agent-chat allows commands for compatibility.
    NoAcl,
    /// Sender must be an operator or admin.
    OperatorRequired,
    /// Sender must be an admin.
    AdminRequired,
}

/// Authorization decision for a planned Matrix bot command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotCommandAuthorization {
    /// Whether the sender may run the command tier.
    pub allowed: bool,
    /// Why the command was allowed or denied.
    pub reason: MatrixBotCommandAuthReason,
}

/// Parsed Matrix bot command plan. This is intentionally side-effect free:
/// execution is a later replacement slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotCommand {
    /// Matrix event id when the command was classified from an inbound event.
    pub event_id: Option<String>,
    /// Matrix room id when the command was classified from an inbound event.
    pub room_id: Option<String>,
    /// Matrix sender MXID.
    pub sender_mxid: String,
    /// Lowercase bang command token, for example `!status`.
    pub command: String,
    /// Whitespace-split command arguments in input order.
    pub args: Vec<String>,
    /// Agent-chat-compatible command tier.
    pub tier: MatrixBotCommandTier,
    /// Sender authorization decision for this tier.
    pub authorization: MatrixBotCommandAuthorization,
    /// Sender MXID localpart, matching agent-chat's human name fallback.
    pub sender_human_localpart: String,
    /// Matrix room group context, if known.
    pub group_name: Option<String>,
    /// Matrix direct-agent context, if known.
    pub target_agent: Option<String>,
}

/// Non-command bot-DM fallback plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotNonCommandPlan {
    /// Agent-chat-compatible fallback reply.
    pub reply_hint: String,
    /// Always `None`; present so callers can handle command/fallback plans
    /// without inventing an execution target.
    pub command: Option<String>,
    /// Always empty for non-command input.
    pub args: Vec<String>,
}

/// Side-effect-free Matrix bot command planning result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixBotCommandPlan {
    /// A bang command was parsed and authorization was evaluated.
    Command(MatrixBotCommand),
    /// The message was not a command and should receive the fallback hint.
    NonCommand(MatrixBotNonCommandPlan),
}

/// Minimal read-only agent summary used by Matrix bot command replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotAgentSummary {
    /// Agent display/name id.
    pub name: String,
    /// Agent status from agentd, for example `online` or `offline`.
    pub status: String,
    /// Optional role, such as `coding` or `review`.
    pub role: Option<String>,
    /// Optional capability tier.
    pub capability: Option<String>,
    /// Optional runtime, such as `codex`.
    pub runtime: Option<String>,
}

/// Minimal read-only group summary used by Matrix bot command replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotGroupSummary {
    /// Group name.
    pub name: String,
    /// Current group members.
    pub members: Vec<String>,
}

/// Read-only snapshot available to Matrix bot command execution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotCommandSnapshot {
    /// Known agents.
    pub agents: Vec<MatrixBotAgentSummary>,
    /// Known groups.
    pub groups: Vec<MatrixBotGroupSummary>,
    /// Optional tmux session count. `None` renders as unavailable.
    pub tmux_sessions: Option<usize>,
    /// Whether the Matrix bridge process is running.
    pub bridge_running: bool,
}

/// Matrix bot reply produced by command execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotCommandReply {
    /// Matrix room id where the reply should be sent.
    pub room_id: String,
    /// Plain text Matrix reply body.
    pub body: String,
    /// Optional Matrix custom-HTML formatted body.
    pub formatted_body: Option<String>,
}

/// Side-effect category declared by a Matrix bot command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixBotCommandSideEffect {
    /// Mutates agentd backend state.
    MutatesBackend,
    /// Creates, joins, leaves, or otherwise changes Matrix rooms.
    ChangesMatrixRooms,
    /// Controls tmux panes or agentctl processes.
    ControlsTmux,
    /// Launches or wakes agent runtimes.
    LaunchesAgents,
}

/// Agent-chat-compatible human DM room status returned by the room effect port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixBotDmRoomStatus {
    /// The human is already joined to the DM room.
    Joined,
    /// The human has been invited to the DM room.
    Invited,
    /// A room exists, but inviting the human failed.
    InviteFailed,
    /// A room exists, but the human membership is unknown.
    Unknown,
    /// No usable room was created or found.
    MissingRoom,
}

/// Result of requesting a human-agent DM room for a Matrix bot command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotDmRoomResult {
    /// Matrix room id, when a room exists.
    pub room_id: Option<String>,
    /// Human membership or invite status.
    pub human_status: MatrixBotDmRoomStatus,
    /// Optional invite failure detail.
    pub invite_error: Option<String>,
}

impl MatrixBotDmRoomResult {
    /// Build a missing-room result.
    #[must_use]
    pub const fn missing_room() -> Self {
        Self {
            room_id: None,
            human_status: MatrixBotDmRoomStatus::MissingRoom,
            invite_error: None,
        }
    }
}

impl Default for MatrixBotDmRoomResult {
    fn default() -> Self {
        Self::missing_room()
    }
}

/// Result of requesting a human Matrix membership in a group room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotGroupRoomResult {
    /// Matrix group room id, when a trusted group room mapping exists.
    pub room_id: Option<String>,
    /// Human membership or invite status.
    pub human_status: MatrixBotDmRoomStatus,
    /// Optional invite failure detail.
    pub invite_error: Option<String>,
}

impl MatrixBotGroupRoomResult {
    /// Build a missing-group-room result.
    #[must_use]
    pub const fn missing_room() -> Self {
        Self {
            room_id: None,
            human_status: MatrixBotDmRoomStatus::MissingRoom,
            invite_error: None,
        }
    }
}

impl Default for MatrixBotGroupRoomResult {
    fn default() -> Self {
        Self::missing_room()
    }
}

/// Result of a backend mutation requested by a Matrix bot management command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotCommandMutationResult {
    /// Agent-chat-compatible error text. `None` means the mutation succeeded.
    pub error: Option<String>,
}

impl MatrixBotCommandMutationResult {
    /// Build a successful mutation result.
    #[must_use]
    pub const fn ok() -> Self {
        Self { error: None }
    }

    /// Build a failed mutation result with agent-chat-compatible text.
    #[must_use]
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            error: Some(error.into()),
        }
    }
}

/// Backend effects needed by Matrix bot management commands.
pub trait MatrixBotCommandBackendEffectPort {
    /// Return a target agent if it exists.
    fn lookup_bot_agent(
        &mut self,
        agent_name: &str,
    ) -> Result<Option<MatrixBotAgentSummary>, BridgeError>;

    /// Update the configured identity text for one agent.
    fn update_bot_agent_identity(
        &mut self,
        agent_name: &str,
        identity: &str,
    ) -> Result<MatrixBotCommandMutationResult, BridgeError>;

    /// Create a durable group.
    fn create_bot_group(
        &mut self,
        name: &str,
        members: &[String],
    ) -> Result<MatrixBotCommandMutationResult, BridgeError>;

    /// Return a target group if it exists.
    fn lookup_bot_group(
        &mut self,
        group_name: &str,
    ) -> Result<Option<MatrixBotGroupSummary>, BridgeError>;

    /// Add and remove durable group members.
    fn update_bot_group_members(
        &mut self,
        group_name: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<MatrixBotCommandMutationResult, BridgeError>;

    /// Delete a durable group.
    fn delete_bot_group(
        &mut self,
        group_name: &str,
    ) -> Result<MatrixBotCommandMutationResult, BridgeError>;
}

/// Matrix room effects needed by Matrix bot management commands.
pub trait MatrixBotCommandRoomEffectPort {
    /// Ensure a human-agent DM room exists and return the human membership state.
    fn ensure_human_dm_room(
        &mut self,
        agent_name: &str,
        human_mxid: &str,
    ) -> Result<MatrixBotDmRoomResult, BridgeError>;

    /// Ensure a human is invited or present in a group Matrix room.
    fn ensure_human_group_room(
        &mut self,
        group_name: &str,
        human_mxid: &str,
    ) -> Result<MatrixBotGroupRoomResult, BridgeError>;
}

/// Side-effect declaration and reply for one executed Matrix bot command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixBotCommandExecution {
    /// Reply to send for the command.
    pub reply: MatrixBotCommandReply,
    /// Declared side-effect categories. Empty means the command is read-only.
    pub side_effects: Vec<MatrixBotCommandSideEffect>,
}

/// Agent-chat-compatible non-command fallback reply.
pub const MATRIX_BOT_COMMAND_FALLBACK_REPLY: &str = "Send !help for available commands.";

const MATRIX_BOT_KNOWN_COMMANDS: &[&str] = &[
    "!help",
    "!status",
    "!agents",
    "!groups",
    "!group",
    "!agent",
    "!sessions",
    "!mcp",
    "!bridge",
    "!mkgroup",
    "!addmember",
    "!rmember",
    "!joingroup",
    "!dm",
    "!identity",
    "!rmgroup",
    "!spy",
    "!agentctl",
    "!ctl",
];

/// Execute the p257 side-effect-free read-only Matrix bot command subset.
pub fn execute_matrix_bot_command(
    command: &MatrixBotCommand,
    snapshot: &MatrixBotCommandSnapshot,
) -> Result<MatrixBotCommandExecution, BridgeError> {
    let room_id = command
        .room_id
        .as_deref()
        .map(str::trim)
        .filter(|room_id| !room_id.is_empty())
        .ok_or_else(|| BridgeError::transport("Matrix bot command has no reply room"))?
        .to_owned();
    let body = render_matrix_bot_command_reply(command, snapshot);
    Ok(MatrixBotCommandExecution {
        reply: MatrixBotCommandReply {
            room_id,
            body,
            formatted_body: None,
        },
        side_effects: Vec::new(),
    })
}

/// Execute Matrix bot commands with the p258 management effect ports enabled.
pub fn execute_matrix_bot_command_with_effects<B, R>(
    command: &MatrixBotCommand,
    snapshot: &MatrixBotCommandSnapshot,
    backend: &mut B,
    rooms: &mut R,
) -> Result<MatrixBotCommandExecution, BridgeError>
where
    B: MatrixBotCommandBackendEffectPort,
    R: MatrixBotCommandRoomEffectPort,
{
    let room_id = command_reply_room_id(command)?;
    if !command.authorization.allowed {
        return Ok(MatrixBotCommandExecution {
            reply: MatrixBotCommandReply {
                room_id,
                body: render_matrix_bot_command_reply(command, snapshot),
                formatted_body: None,
            },
            side_effects: Vec::new(),
        });
    }

    match command.command.as_str() {
        "!dm" => execute_matrix_bot_dm_command(command, backend, rooms, room_id),
        "!identity" => execute_matrix_bot_identity_command(command, snapshot, backend, room_id),
        "!mkgroup" => execute_matrix_bot_mkgroup_command(command, backend, room_id),
        "!addmember" => execute_matrix_bot_group_member_command(command, backend, room_id, true),
        "!rmember" => execute_matrix_bot_group_member_command(command, backend, room_id, false),
        "!rmgroup" => execute_matrix_bot_rmgroup_command(command, backend, room_id),
        "!joingroup" => execute_matrix_bot_joingroup_command(command, backend, rooms, room_id),
        _ => execute_matrix_bot_command(command, snapshot),
    }
}

fn command_reply_room_id(command: &MatrixBotCommand) -> Result<String, BridgeError> {
    command
        .room_id
        .as_deref()
        .map(str::trim)
        .filter(|room_id| !room_id.is_empty())
        .ok_or_else(|| BridgeError::transport("Matrix bot command has no reply room"))
        .map(ToOwned::to_owned)
}

fn execute_matrix_bot_dm_command<B, R>(
    command: &MatrixBotCommand,
    backend: &mut B,
    rooms: &mut R,
    reply_room_id: String,
) -> Result<MatrixBotCommandExecution, BridgeError>
where
    B: MatrixBotCommandBackendEffectPort,
    R: MatrixBotCommandRoomEffectPort,
{
    let Some(agent_name) = command.args.first().map(String::as_str) else {
        return Ok(matrix_bot_execution_without_effects(
            reply_room_id,
            "Usage: !dm <agent>".to_owned(),
        ));
    };

    if backend.lookup_bot_agent(agent_name)?.is_none() {
        return Ok(matrix_bot_execution_without_effects(
            reply_room_id,
            format!("Agent not found: {agent_name}"),
        ));
    }

    let dm = rooms.ensure_human_dm_room(agent_name, &command.sender_mxid)?;
    Ok(MatrixBotCommandExecution {
        reply: MatrixBotCommandReply {
            room_id: reply_room_id,
            body: render_matrix_bot_dm_reply(agent_name, &dm),
            formatted_body: None,
        },
        side_effects: vec![MatrixBotCommandSideEffect::ChangesMatrixRooms],
    })
}

fn execute_matrix_bot_identity_command<B>(
    command: &MatrixBotCommand,
    snapshot: &MatrixBotCommandSnapshot,
    backend: &mut B,
    reply_room_id: String,
) -> Result<MatrixBotCommandExecution, BridgeError>
where
    B: MatrixBotCommandBackendEffectPort,
{
    let Some((agent_name, identity)) = resolve_matrix_bot_identity_target(command, snapshot) else {
        return Ok(matrix_bot_execution_without_effects(
            reply_room_id,
            "Usage: !identity <text> (in agent DM) or !identity <agent> <text>".to_owned(),
        ));
    };
    if identity.trim().is_empty() {
        return Ok(matrix_bot_execution_without_effects(
            reply_room_id,
            "Identity text required.".to_owned(),
        ));
    }

    let result = backend.update_bot_agent_identity(&agent_name, &identity)?;
    let body = if let Some(error) = result.error {
        format!("Failed: {error}")
    } else {
        format!("Identity set for {agent_name}: {identity}")
    };
    Ok(MatrixBotCommandExecution {
        reply: MatrixBotCommandReply {
            room_id: reply_room_id,
            body,
            formatted_body: None,
        },
        side_effects: vec![MatrixBotCommandSideEffect::MutatesBackend],
    })
}

fn execute_matrix_bot_mkgroup_command<B>(
    command: &MatrixBotCommand,
    backend: &mut B,
    reply_room_id: String,
) -> Result<MatrixBotCommandExecution, BridgeError>
where
    B: MatrixBotCommandBackendEffectPort,
{
    let Some(group_name) = command.args.first() else {
        return Ok(matrix_bot_execution_without_effects(
            reply_room_id,
            "Usage: !mkgroup <name> [member1] [member2] ...".to_owned(),
        ));
    };
    let members = command.args.get(1..).unwrap_or(&[]).to_vec();
    let result = backend.create_bot_group(group_name, &members)?;
    let body = if let Some(error) = result.error {
        format!("Failed: {error}")
    } else {
        let members = if members.is_empty() {
            "none".to_owned()
        } else {
            members.join(", ")
        };
        format!("Group \"{group_name}\" created with members: {members}")
    };
    Ok(matrix_bot_execution_with_backend_mutation(
        reply_room_id,
        body,
    ))
}

fn execute_matrix_bot_group_member_command<B>(
    command: &MatrixBotCommand,
    backend: &mut B,
    reply_room_id: String,
    add_member: bool,
) -> Result<MatrixBotCommandExecution, BridgeError>
where
    B: MatrixBotCommandBackendEffectPort,
{
    let Some((group_name, member_name)) = resolve_matrix_bot_group_member_target(command) else {
        let usage = if add_member {
            "Usage: !addmember <group> <name> (or !addmember <name> inside a group room)"
        } else {
            "Usage: !rmember <group> <name> (or !rmember <name> inside a group room)"
        };
        return Ok(matrix_bot_execution_without_effects(
            reply_room_id,
            usage.to_owned(),
        ));
    };

    let add = if add_member {
        vec![member_name.clone()]
    } else {
        Vec::new()
    };
    let remove = if add_member {
        Vec::new()
    } else {
        vec![member_name.clone()]
    };
    let result = backend.update_bot_group_members(&group_name, &add, &remove)?;
    let body = if let Some(error) = result.error {
        format!("Failed: {error}")
    } else if add_member {
        format!("Added {member_name} to {group_name}")
    } else {
        format!("Removed {member_name} from {group_name}")
    };
    Ok(matrix_bot_execution_with_backend_mutation(
        reply_room_id,
        body,
    ))
}

fn execute_matrix_bot_rmgroup_command<B>(
    command: &MatrixBotCommand,
    backend: &mut B,
    reply_room_id: String,
) -> Result<MatrixBotCommandExecution, BridgeError>
where
    B: MatrixBotCommandBackendEffectPort,
{
    let Some(group_name) = command
        .args
        .first()
        .cloned()
        .or_else(|| command.group_name.clone())
    else {
        return Ok(matrix_bot_execution_without_effects(
            reply_room_id,
            "Usage: !rmgroup <group> (or use inside a group room)".to_owned(),
        ));
    };

    if backend.lookup_bot_group(&group_name)?.is_none() {
        return Ok(matrix_bot_execution_without_effects(
            reply_room_id,
            format!("Group not found: {group_name}"),
        ));
    }

    let result = backend.delete_bot_group(&group_name)?;
    let body = if let Some(error) = result.error {
        format!("Failed to delete group: {error}")
    } else {
        format!("Group \"{group_name}\" removed. Matrix room cleanup is not included in p261.")
    };
    Ok(matrix_bot_execution_with_backend_mutation(
        reply_room_id,
        body,
    ))
}

fn execute_matrix_bot_joingroup_command<B, R>(
    command: &MatrixBotCommand,
    backend: &mut B,
    rooms: &mut R,
    reply_room_id: String,
) -> Result<MatrixBotCommandExecution, BridgeError>
where
    B: MatrixBotCommandBackendEffectPort,
    R: MatrixBotCommandRoomEffectPort,
{
    let Some(group_name) = command
        .args
        .first()
        .cloned()
        .or_else(|| command.group_name.clone())
    else {
        return Ok(matrix_bot_execution_without_effects(
            reply_room_id,
            "Usage: !joingroup <group> (or use inside a group room)".to_owned(),
        ));
    };

    let human_name = command.sender_human_localpart.clone();
    let result =
        backend.update_bot_group_members(&group_name, std::slice::from_ref(&human_name), &[])?;
    if let Some(error) = result.error {
        return Ok(matrix_bot_execution_with_backend_mutation(
            reply_room_id,
            format!("Failed: {error}"),
        ));
    }

    let group_room = rooms.ensure_human_group_room(&group_name, &command.sender_mxid)?;
    let body = render_matrix_bot_joingroup_reply(&group_name, &human_name, &group_room);
    let side_effects = if group_room.room_id.is_some() {
        vec![
            MatrixBotCommandSideEffect::MutatesBackend,
            MatrixBotCommandSideEffect::ChangesMatrixRooms,
        ]
    } else {
        vec![MatrixBotCommandSideEffect::MutatesBackend]
    };
    Ok(MatrixBotCommandExecution {
        reply: MatrixBotCommandReply {
            room_id: reply_room_id,
            body,
            formatted_body: None,
        },
        side_effects,
    })
}

fn resolve_matrix_bot_group_member_target(command: &MatrixBotCommand) -> Option<(String, String)> {
    if command.args.len() >= 2 {
        return Some((command.args[0].clone(), command.args[1].clone()));
    }
    if command.args.len() == 1 {
        return command
            .group_name
            .as_ref()
            .map(|group_name| (group_name.clone(), command.args[0].clone()));
    }
    None
}

fn matrix_bot_execution_without_effects(
    room_id: String,
    body: String,
) -> MatrixBotCommandExecution {
    MatrixBotCommandExecution {
        reply: MatrixBotCommandReply {
            room_id,
            body,
            formatted_body: None,
        },
        side_effects: Vec::new(),
    }
}

fn matrix_bot_execution_with_backend_mutation(
    room_id: String,
    body: String,
) -> MatrixBotCommandExecution {
    MatrixBotCommandExecution {
        reply: MatrixBotCommandReply {
            room_id,
            body,
            formatted_body: None,
        },
        side_effects: vec![MatrixBotCommandSideEffect::MutatesBackend],
    }
}

fn render_matrix_bot_dm_reply(agent_name: &str, dm: &MatrixBotDmRoomResult) -> String {
    let Some(room_id) = dm.room_id.as_deref() else {
        return format!(
            "Failed to create DM room. Agent \"{agent_name}\" may not have a Matrix account yet."
        );
    };
    let room_link = format!("https://matrix.to/#/{room_id}");
    match dm.human_status {
        MatrixBotDmRoomStatus::Joined | MatrixBotDmRoomStatus::Unknown => {
            format!("You're already in the DM room with {agent_name}. Room: {room_link}")
        }
        MatrixBotDmRoomStatus::Invited => {
            format!(
                "DM room ready for {agent_name}. Invite sent — check your Matrix invites. Room: {room_link}"
            )
        }
        MatrixBotDmRoomStatus::InviteFailed => {
            let detail = dm
                .invite_error
                .as_deref()
                .filter(|error| !error.is_empty())
                .map_or(String::new(), |error| format!(" ({error})"));
            format!(
                "DM room exists but invite failed{detail}. Open {room_link} or retry !dm {agent_name}."
            )
        }
        MatrixBotDmRoomStatus::MissingRoom => {
            format!(
                "Failed to create DM room. Agent \"{agent_name}\" may not have a Matrix account yet."
            )
        }
    }
}

fn render_matrix_bot_joingroup_reply(
    group_name: &str,
    human_name: &str,
    group_room: &MatrixBotGroupRoomResult,
) -> String {
    let prefix = format!("Added you ({human_name}) to group \"{group_name}\".");
    let Some(room_id) = group_room.room_id.as_deref() else {
        return format!(
            "{prefix} no trusted Matrix group room is mapped yet, so no Matrix invite was sent."
        );
    };
    let room_link = format!("https://matrix.to/#/{room_id}");
    match group_room.human_status {
        MatrixBotDmRoomStatus::Joined | MatrixBotDmRoomStatus::Unknown => {
            format!("{prefix} You're already in the Matrix group room. Room: {room_link}")
        }
        MatrixBotDmRoomStatus::Invited => {
            format!("{prefix} Invite sent — check your Matrix invites. Room: {room_link}")
        }
        MatrixBotDmRoomStatus::InviteFailed => {
            let detail = group_room
                .invite_error
                .as_deref()
                .filter(|error| !error.is_empty())
                .map_or(String::new(), |error| format!(" ({error})"));
            format!("{prefix} Matrix group room invite failed{detail}. Room: {room_link}")
        }
        MatrixBotDmRoomStatus::MissingRoom => format!(
            "{prefix} no trusted Matrix group room is mapped yet, so no Matrix invite was sent."
        ),
    }
}

fn resolve_matrix_bot_identity_target(
    command: &MatrixBotCommand,
    snapshot: &MatrixBotCommandSnapshot,
) -> Option<(String, String)> {
    if let Some(target_agent) = command.target_agent.as_deref() {
        let first = command.args.first()?;
        if snapshot.agents.iter().any(|agent| agent.name == *first) {
            let identity = command.args.get(1..)?.join(" ");
            return Some((first.clone(), identity));
        }
        return Some((target_agent.to_owned(), command.args.join(" ")));
    }

    if command.args.len() < 2 {
        return None;
    }
    let agent_name = command.args[0].clone();
    let identity = command.args[1..].join(" ");
    Some((agent_name, identity))
}

/// Plan a Matrix bot command using the command grammar and ACL behavior from
/// agent-chat's Matrix bridge.
#[must_use]
pub fn plan_matrix_bot_command(
    sender_mxid: &str,
    body: &str,
    formatted_body: Option<&str>,
    context: MatrixBotCommandContext,
    acl: &MatrixBotCommandAcl,
) -> MatrixBotCommandPlan {
    let command_body = normalize_matrix_bot_command_body(body, formatted_body);
    if !command_body.starts_with('!') {
        return MatrixBotCommandPlan::NonCommand(MatrixBotNonCommandPlan {
            reply_hint: MATRIX_BOT_COMMAND_FALLBACK_REPLY.to_owned(),
            command: None,
            args: Vec::new(),
        });
    }

    let mut parts = command_body.split_whitespace();
    let Some(command_token) = parts.next() else {
        return MatrixBotCommandPlan::NonCommand(MatrixBotNonCommandPlan {
            reply_hint: MATRIX_BOT_COMMAND_FALLBACK_REPLY.to_owned(),
            command: None,
            args: Vec::new(),
        });
    };
    let command = command_token.to_ascii_lowercase();
    let args = parts.map(ToOwned::to_owned).collect();
    let tier = classify_matrix_bot_command(&command);
    let authorization = authorize_matrix_bot_command(sender_mxid, tier, acl);
    let sender_human_localpart = matrix_localpart(sender_mxid)
        .unwrap_or(sender_mxid)
        .to_owned();

    MatrixBotCommandPlan::Command(MatrixBotCommand {
        event_id: None,
        room_id: None,
        sender_mxid: sender_mxid.to_owned(),
        command,
        args,
        tier,
        authorization,
        sender_human_localpart,
        group_name: context.group_name,
        target_agent: context.target_agent,
    })
}

/// Classify a Matrix bot command using agent-chat's command tier table.
#[must_use]
pub fn classify_matrix_bot_command(command: &str) -> MatrixBotCommandTier {
    match command {
        "!help" => MatrixBotCommandTier::Public,
        "!mkgroup" | "!addmember" | "!rmember" | "!joingroup" | "!dm" | "!identity"
        | "!rmgroup" => MatrixBotCommandTier::OperatorManagement,
        "!spy" | "!agentctl" | "!ctl" => MatrixBotCommandTier::Admin,
        _ => MatrixBotCommandTier::OperatorRead,
    }
}

/// Authorize a Matrix bot command tier using agent-chat's ACL behavior.
#[must_use]
pub fn authorize_matrix_bot_command(
    sender_mxid: &str,
    tier: MatrixBotCommandTier,
    acl: &MatrixBotCommandAcl,
) -> MatrixBotCommandAuthorization {
    if tier == MatrixBotCommandTier::Public {
        return MatrixBotCommandAuthorization {
            allowed: true,
            reason: MatrixBotCommandAuthReason::Public,
        };
    }

    if acl.admin_mxids.iter().any(|admin| admin == sender_mxid) {
        return MatrixBotCommandAuthorization {
            allowed: true,
            reason: MatrixBotCommandAuthReason::Admin,
        };
    }

    if tier != MatrixBotCommandTier::Admin
        && acl
            .operator_mxids
            .iter()
            .any(|operator| operator == sender_mxid)
    {
        return MatrixBotCommandAuthorization {
            allowed: true,
            reason: MatrixBotCommandAuthReason::Operator,
        };
    }

    if acl.operator_mxids.is_empty() && acl.admin_mxids.is_empty() {
        return MatrixBotCommandAuthorization {
            allowed: true,
            reason: MatrixBotCommandAuthReason::NoAcl,
        };
    }

    MatrixBotCommandAuthorization {
        allowed: false,
        reason: if tier == MatrixBotCommandTier::Admin {
            MatrixBotCommandAuthReason::AdminRequired
        } else {
            MatrixBotCommandAuthReason::OperatorRequired
        },
    }
}

fn render_matrix_bot_command_reply(
    command: &MatrixBotCommand,
    snapshot: &MatrixBotCommandSnapshot,
) -> String {
    if !command.authorization.allowed {
        let required = if command.authorization.reason == MatrixBotCommandAuthReason::AdminRequired
        {
            "admin"
        } else {
            "operator"
        };
        return format!(
            "Access denied: {} requires {required} privileges.",
            command.command
        );
    }

    match command.command.as_str() {
        "!help" => render_matrix_bot_help(),
        "!status" => render_matrix_bot_status(snapshot),
        "!agents" => {
            render_matrix_bot_agents(snapshot, command.args.iter().any(|arg| arg == "all"))
        }
        "!groups" => render_matrix_bot_groups(snapshot),
        known if MATRIX_BOT_KNOWN_COMMANDS.contains(&known) => {
            format!("Command not implemented in agentd Matrix bridge yet: {known}")
        }
        unknown => format!("Unknown command: {unknown}\n{MATRIX_BOT_COMMAND_FALLBACK_REPLY}"),
    }
}

fn render_matrix_bot_help() -> String {
    [
        "=== Agent Bridge Bot Commands ===",
        "",
        "System:",
        "  !status          - System overview",
        "  !agents          - List online agents (!agents all for full list)",
        "  !groups          - List all groups",
        "",
        "The agentd Matrix bridge currently executes only read-only command replies.",
        "Management commands are recognized but not implemented yet.",
    ]
    .join("\n")
}

fn render_matrix_bot_status(snapshot: &MatrixBotCommandSnapshot) -> String {
    let tmux_sessions = snapshot
        .tmux_sessions
        .map_or_else(|| "unavailable".to_owned(), |count| count.to_string());
    let bridge = if snapshot.bridge_running {
        "running"
    } else {
        "stopped"
    };
    [
        "=== System Status ===".to_owned(),
        format!("Agents: {}", snapshot.agents.len()),
        format!("Groups: {}", snapshot.groups.len()),
        format!("Tmux sessions: {tmux_sessions}"),
        format!("Bridge: {bridge}"),
    ]
    .join("\n")
}

fn render_matrix_bot_agents(snapshot: &MatrixBotCommandSnapshot, show_all: bool) -> String {
    if snapshot.agents.is_empty() {
        return "No known agents yet.".to_owned();
    }

    let mut lines = vec![if show_all {
        "=== All Agents ===".to_owned()
    } else {
        "=== Online Agents ===".to_owned()
    }];
    let mut filtered = 0usize;
    for agent in &snapshot.agents {
        let online = agent.status == "online";
        if !show_all && !online {
            filtered += 1;
            continue;
        }
        let mut details = Vec::new();
        if let Some(role) = agent.role.as_deref().filter(|role| !role.is_empty()) {
            details.push(format!("role={role}"));
        }
        if let Some(capability) = agent
            .capability
            .as_deref()
            .filter(|capability| !capability.is_empty())
        {
            details.push(format!("capability={capability}"));
        }
        if let Some(runtime) = agent
            .runtime
            .as_deref()
            .filter(|runtime| !runtime.is_empty())
        {
            details.push(format!("runtime={runtime}"));
        }
        let suffix = if details.is_empty() {
            String::new()
        } else {
            format!(" ({})", details.join(", "))
        };
        lines.push(format!("{} [{}]{}", agent.name, agent.status, suffix));
    }
    if !show_all && filtered > 0 {
        lines.push(String::new());
        lines.push("Use !agents all to see all agents including offline.".to_owned());
    }
    lines.join("\n")
}

fn render_matrix_bot_groups(snapshot: &MatrixBotCommandSnapshot) -> String {
    if snapshot.groups.is_empty() {
        return "No groups.".to_owned();
    }

    let mut lines = vec!["=== Groups ===".to_owned()];
    for group in &snapshot.groups {
        lines.push(format!(
            "{} ({} members): {}",
            group.name,
            group.members.len(),
            group.members.join(", ")
        ));
    }
    lines.join("\n")
}

fn normalize_matrix_bot_command_body(body: &str, formatted_body: Option<&str>) -> String {
    let trimmed = body.trim();
    if trimmed.starts_with('!') {
        return trimmed.to_owned();
    }

    if formatted_body.is_some_and(formatted_body_starts_with_matrix_command_mention) {
        if let Some(command_index) = trimmed.find('!') {
            return trimmed[command_index..].trim().to_owned();
        }
    }

    trimmed.to_owned()
}

fn formatted_body_starts_with_matrix_command_mention(formatted_body: &str) -> bool {
    let trimmed = formatted_body.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("<a") || !lower.contains("href=\"https://matrix.to/#/@") {
        return false;
    }

    let Some(close_index) = lower.find("</a>") else {
        return false;
    };
    let after_link = trimmed[close_index + "</a>".len()..].trim_start();
    let Some(after_colon) = after_link.strip_prefix(':') else {
        return false;
    };
    after_colon.trim_start().starts_with('!')
}

/// Parse Matrix SDK `/sync` timeline events into bridge-ready text messages.
#[must_use]
#[cfg(feature = "matrix-sdk-adapter")]
pub fn sdk_timeline_text_messages(
    room_id: &str,
    events: &[matrix_sdk::deserialized_responses::SyncTimelineEvent],
) -> Vec<MatrixClientTextMessage> {
    events
        .iter()
        .filter_map(|event| sdk_timeline_text_message(room_id, event))
        .collect()
}

#[cfg(feature = "matrix-sdk-adapter")]
fn sdk_timeline_text_message(
    room_id: &str,
    event: &matrix_sdk::deserialized_responses::SyncTimelineEvent,
) -> Option<MatrixClientTextMessage> {
    use matrix_sdk::ruma::events::{
        AnySyncMessageLikeEvent, AnySyncTimelineEvent, SyncMessageLikeEvent,
        room::message::{MessageType, Relation},
    };

    let AnySyncTimelineEvent::MessageLike(message_like) = event.raw().deserialize().ok()? else {
        return None;
    };
    let AnySyncMessageLikeEvent::RoomMessage(SyncMessageLikeEvent::Original(original)) =
        message_like
    else {
        return None;
    };

    let mentions = original
        .content
        .mentions
        .as_ref()
        .map(|mentions| mentions.user_ids.iter().map(ToString::to_string).collect())
        .unwrap_or_default();
    let reply_to = match &original.content.relates_to {
        Some(Relation::Reply { in_reply_to }) => Some(in_reply_to.event_id.to_string()),
        _ => None,
    };
    let formatted_body = match &original.content.msgtype {
        MessageType::Emote(content) => content
            .formatted
            .as_ref()
            .map(|formatted| formatted.body.clone()),
        MessageType::Notice(content) => content
            .formatted
            .as_ref()
            .map(|formatted| formatted.body.clone()),
        MessageType::Text(content) => content
            .formatted
            .as_ref()
            .map(|formatted| formatted.body.clone()),
        _ => None,
    };

    Some(MatrixClientTextMessage {
        event_id: original.event_id.to_string(),
        room_id: room_id.to_owned(),
        sender_mxid: original.sender.to_string(),
        body: original.content.body().to_owned(),
        formatted_body,
        mentions,
        reply_to,
    })
}

/// SDK-facing Matrix client operations needed by the bridge transport.
pub trait MatrixClientPort {
    /// Ensure the bot client is logged in and return its Matrix user id.
    fn ensure_logged_in(&mut self) -> Result<String, BridgeError>;

    /// Return one normalized Matrix sync snapshot.
    fn sync_once(&mut self) -> Result<MatrixClientSync, BridgeError>;

    /// Join a Matrix room.
    fn join_room(&mut self, room_id: &str) -> Result<(), BridgeError>;

    /// Leave a Matrix room.
    fn leave_room(&mut self, room_id: &str) -> Result<(), BridgeError>;

    /// Send one plain text Matrix message.
    fn send_text_message(&mut self, room_id: &str, body: &str) -> Result<(), BridgeError>;

    /// Return a Matrix user's active membership status in a room, if known.
    fn room_member_status(
        &mut self,
        _room_id: &str,
        _user_mxid: &str,
    ) -> Result<Option<MatrixBotDmRoomStatus>, BridgeError> {
        Ok(None)
    }

    /// Create a direct Matrix room and invite the supplied Matrix user ids.
    fn create_direct_room(
        &mut self,
        _name: &str,
        _invite_mxids: &[String],
    ) -> Result<String, BridgeError> {
        Err(BridgeError::transport(
            "Matrix client does not support direct room creation",
        ))
    }

    /// Invite one Matrix user id to an existing room.
    fn invite_user_to_room(&mut self, _room_id: &str, _user_mxid: &str) -> Result<(), BridgeError> {
        Err(BridgeError::transport(
            "Matrix client does not support room invites",
        ))
    }

    /// Ensure a human-agent DM room exists. Real room lifecycle parity is
    /// implemented by future adapters; the default keeps current clients
    /// compiling and reports that no room was created.
    fn ensure_human_dm_room(
        &mut self,
        _agent_name: &str,
        _human_localpart: &str,
    ) -> Result<MatrixBotDmRoomResult, BridgeError> {
        Ok(MatrixBotDmRoomResult::missing_room())
    }
}

impl<C> MatrixClientPort for &mut C
where
    C: MatrixClientPort + ?Sized,
{
    fn ensure_logged_in(&mut self) -> Result<String, BridgeError> {
        (**self).ensure_logged_in()
    }

    fn sync_once(&mut self) -> Result<MatrixClientSync, BridgeError> {
        (**self).sync_once()
    }

    fn join_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        (**self).join_room(room_id)
    }

    fn leave_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        (**self).leave_room(room_id)
    }

    fn send_text_message(&mut self, room_id: &str, body: &str) -> Result<(), BridgeError> {
        (**self).send_text_message(room_id, body)
    }

    fn room_member_status(
        &mut self,
        room_id: &str,
        user_mxid: &str,
    ) -> Result<Option<MatrixBotDmRoomStatus>, BridgeError> {
        (**self).room_member_status(room_id, user_mxid)
    }

    fn create_direct_room(
        &mut self,
        name: &str,
        invite_mxids: &[String],
    ) -> Result<String, BridgeError> {
        (**self).create_direct_room(name, invite_mxids)
    }

    fn invite_user_to_room(&mut self, room_id: &str, user_mxid: &str) -> Result<(), BridgeError> {
        (**self).invite_user_to_room(room_id, user_mxid)
    }

    fn ensure_human_dm_room(
        &mut self,
        agent_name: &str,
        human_mxid: &str,
    ) -> Result<MatrixBotDmRoomResult, BridgeError> {
        (**self).ensure_human_dm_room(agent_name, human_mxid)
    }
}

impl<C> MatrixBotCommandRoomEffectPort for C
where
    C: MatrixClientPort + ?Sized,
{
    fn ensure_human_dm_room(
        &mut self,
        agent_name: &str,
        human_mxid: &str,
    ) -> Result<MatrixBotDmRoomResult, BridgeError> {
        MatrixClientPort::ensure_human_dm_room(self, agent_name, human_mxid)
    }

    fn ensure_human_group_room(
        &mut self,
        _group_name: &str,
        _human_mxid: &str,
    ) -> Result<MatrixBotGroupRoomResult, BridgeError> {
        Ok(MatrixBotGroupRoomResult::missing_room())
    }
}

#[cfg(feature = "matrix-sdk-adapter")]
impl MatrixClientPort for SdkMatrixClient {
    fn ensure_logged_in(&mut self) -> Result<String, BridgeError> {
        if let Some(user_id) = self.current_user_id() {
            return Ok(user_id);
        }

        if let (Some(user_id), Some(access_token)) =
            (&self.config.user_id, &self.config.access_token)
        {
            let user_id = matrix_sdk::ruma::UserId::parse(user_id).map_err(|err| {
                BridgeError::transport(format!("invalid Matrix user id for session restore: {err}"))
            })?;
            let device_id = matrix_sdk::ruma::OwnedDeviceId::from(
                self.config.device_id.as_deref().unwrap_or("AGENTD"),
            );
            let session = matrix_sdk::matrix_auth::MatrixSession {
                meta: matrix_sdk::SessionMeta { user_id, device_id },
                tokens: matrix_sdk::matrix_auth::MatrixSessionTokens {
                    access_token: access_token.clone(),
                    refresh_token: None,
                },
            };
            self.runtime
                .block_on(self.client.restore_session(session))
                .map_err(|err| {
                    BridgeError::transport(format!("restore Matrix SDK session: {err}"))
                })?;
            return self
                .current_user_id()
                .ok_or_else(|| BridgeError::transport("Matrix SDK session has no user id"));
        }

        if let (Some(username), Some(password)) = (&self.config.username, &self.config.password) {
            let response = self.runtime.block_on(async {
                self.client
                    .matrix_auth()
                    .login_username(username, password)
                    .send()
                    .await
            });
            let response = response
                .map_err(|err| BridgeError::transport(format!("Matrix SDK login failed: {err}")))?;
            return Ok(response.user_id.to_string());
        }

        Err(BridgeError::transport(
            "Matrix SDK client is not logged in and no credentials were configured",
        ))
    }

    fn sync_once(&mut self) -> Result<MatrixClientSync, BridgeError> {
        self.ensure_logged_in()?;
        let settings = matrix_sdk::config::SyncSettings::new()
            .timeout(Duration::from_millis(self.config.sync_timeout_ms));
        let sync = self
            .runtime
            .block_on(self.client.sync_once(settings))
            .map_err(|err| BridgeError::transport(format!("Matrix SDK sync_once failed: {err}")))?;
        let sync_joined_rooms = &sync.rooms.join;
        let text_events = sync_joined_rooms
            .iter()
            .flat_map(|(room_id, room)| {
                let room_id = room_id.to_string();
                sdk_timeline_text_messages(&room_id, &room.timeline.events)
            })
            .collect();

        let joined_rooms = self
            .client
            .joined_rooms()
            .into_iter()
            .map(|room| MatrixClientRoom {
                room_id: room.room_id().to_string(),
                group_name: None,
                agent_name: None,
                trusted: true,
                trust_reason: "sdk_joined".to_owned(),
                inviter_mxid: None,
                members: Vec::new(),
            })
            .collect();
        let invites = self
            .client
            .invited_rooms()
            .into_iter()
            .map(|room| MatrixClientInvite {
                room_id: room.room_id().to_string(),
                group_name: None,
                agent_name: None,
                inviter_mxid: None,
                members: Vec::new(),
            })
            .collect();

        Ok(MatrixClientSync {
            invites,
            joined_rooms,
            text_events,
        })
    }

    fn join_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        let room_id = matrix_sdk::ruma::RoomId::parse(room_id).map_err(|err| {
            BridgeError::transport(format!("invalid Matrix room id {room_id}: {err}"))
        })?;
        self.runtime
            .block_on(self.client.join_room_by_id(&room_id))
            .map_err(|err| BridgeError::transport(format!("join Matrix room {room_id}: {err}")))?;
        Ok(())
    }

    fn leave_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        let room = self.sdk_room(room_id)?;
        self.runtime
            .block_on(room.leave())
            .map_err(|err| BridgeError::transport(format!("leave Matrix room {room_id}: {err}")))
    }

    fn send_text_message(&mut self, room_id: &str, body: &str) -> Result<(), BridgeError> {
        let room = self.sdk_room(room_id)?;
        let body = body.to_owned();
        self.runtime
            .block_on(async move {
                let content =
                    matrix_sdk::ruma::events::room::message::RoomMessageEventContent::text_plain(
                        body,
                    );
                room.send(content).await
            })
            .map_err(|err| {
                BridgeError::transport(format!("send Matrix room message to {room_id}: {err}"))
            })?;
        Ok(())
    }

    fn room_member_status(
        &mut self,
        room_id: &str,
        user_mxid: &str,
    ) -> Result<Option<MatrixBotDmRoomStatus>, BridgeError> {
        self.ensure_logged_in()?;
        let user_id = matrix_sdk::ruma::UserId::parse(user_mxid).map_err(|err| {
            BridgeError::transport(format!("invalid Matrix user id {user_mxid}: {err}"))
        })?;
        let room = self.sdk_room(room_id)?;
        let members = self
            .runtime
            .block_on(room.members(matrix_sdk::RoomMemberships::ACTIVE))
            .map_err(|err| BridgeError::transport(format!("read Matrix room members: {err}")))?;
        for member in members {
            if member.user_id().as_str() != user_id.as_str() {
                continue;
            }
            return Ok(match member.membership() {
                matrix_sdk::ruma::events::room::member::MembershipState::Join => {
                    Some(MatrixBotDmRoomStatus::Joined)
                }
                matrix_sdk::ruma::events::room::member::MembershipState::Invite => {
                    Some(MatrixBotDmRoomStatus::Invited)
                }
                _ => Some(MatrixBotDmRoomStatus::Unknown),
            });
        }
        Ok(None)
    }

    fn create_direct_room(
        &mut self,
        name: &str,
        invite_mxids: &[String],
    ) -> Result<String, BridgeError> {
        use matrix_sdk::ruma::api::client::room::create_room;

        self.ensure_logged_in()?;
        let invite = invite_mxids
            .iter()
            .map(|mxid| {
                matrix_sdk::ruma::UserId::parse(mxid.as_str()).map_err(|err| {
                    BridgeError::transport(format!("invalid Matrix invite user id {mxid}: {err}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut request = create_room::v3::Request::new();
        request.invite = invite;
        request.is_direct = true;
        request.name = Some(name.to_owned());
        request.preset = Some(create_room::v3::RoomPreset::TrustedPrivateChat);
        let room = self
            .runtime
            .block_on(self.client.create_room(request))
            .map_err(|err| BridgeError::transport(format!("create Matrix direct room: {err}")))?;
        Ok(room.room_id().to_string())
    }

    fn invite_user_to_room(&mut self, room_id: &str, user_mxid: &str) -> Result<(), BridgeError> {
        self.ensure_logged_in()?;
        let user_id = matrix_sdk::ruma::UserId::parse(user_mxid).map_err(|err| {
            BridgeError::transport(format!("invalid Matrix invite user id {user_mxid}: {err}"))
        })?;
        let room = self.sdk_room(room_id)?;
        self.runtime
            .block_on(room.invite_user_by_id(&user_id))
            .map_err(|err| {
                BridgeError::transport(format!(
                    "invite Matrix user {user_mxid} to {room_id}: {err}"
                ))
            })
    }
}

/// Agentd-side contract used by the Matrix bridge runtime.
pub trait AgentdBridgeBackend {
    /// Register or update a Matrix room mapping in agentd.
    fn register_room(&mut self, room: MatrixRoomRegistration) -> Result<(), BridgeError>;

    /// Post a Matrix inbound event to agentd.
    fn post_inbound(&mut self, event: MatrixInboundEvent) -> Result<(), BridgeError>;

    /// Poll backend outbox events after the supplied sequence.
    fn poll_outbox(&mut self, from_seq: i64) -> Result<Vec<MatrixOutboundEvent>, BridgeError>;

    /// Persist the highest outbox sequence actually delivered to Matrix.
    fn acknowledge_outbox_cursor(&mut self, _last_seq: i64) -> Result<(), BridgeError> {
        Ok(())
    }

    fn outbox_cursor(&mut self) -> Result<Option<i64>, BridgeError> {
        Ok(None)
    }
}

/// Matrix-side adapter contract used by the bridge runtime.
pub trait MatrixBridgeTransport {
    /// Return room registrations observed by the Matrix side.
    fn room_registrations(&mut self) -> Result<Vec<MatrixRoomRegistration>, BridgeError>;

    /// Return inbound Matrix events ready for agentd.
    fn inbound_events(&mut self) -> Result<Vec<MatrixInboundEvent>, BridgeError>;

    /// Send one backend outbox event through Matrix.
    fn send_outbound(&mut self, event: MatrixOutboundEvent) -> Result<(), BridgeError>;
}

/// Deterministic bridge process-loop unit.
#[derive(Debug)]
pub struct BridgeRuntime<B, T> {
    backend: B,
    transport: T,
    state: BridgeState,
}

impl<B, T> BridgeRuntime<B, T> {
    /// Create a bridge runtime from explicit backend, transport, and state.
    #[must_use]
    pub fn new(backend: B, transport: T, state: BridgeState) -> Self {
        Self {
            backend,
            transport,
            state,
        }
    }

    /// Borrow the backend, mostly for tests and embedding code.
    #[must_use]
    pub const fn backend(&self) -> &B {
        &self.backend
    }

    /// Borrow the Matrix transport, mostly for tests and embedding code.
    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    /// Current bridge state.
    #[must_use]
    pub const fn state(&self) -> &BridgeState {
        &self.state
    }
}

impl<B, T> BridgeRuntime<B, T>
where
    B: AgentdBridgeBackend,
    T: MatrixBridgeTransport,
{
    /// Run one deterministic bridge iteration.
    pub fn run_once(&mut self) -> Result<BridgeRunReport, BridgeError> {
        let mut report = BridgeRunReport::default();

        for room in self.transport.room_registrations()? {
            self.backend.register_room(room)?;
            report.registered_rooms += 1;
        }

        for event in self.transport.inbound_events()? {
            self.backend.post_inbound(event)?;
            report.inbound_forwarded += 1;
        }

        let mut acknowledged_seq = None;
        for event in self.backend.poll_outbox(self.state.next_from_seq)? {
            let seq = event.seq;
            self.transport.send_outbound(event)?;
            if seq > self.state.next_from_seq {
                self.state.next_from_seq = seq;
            }
            acknowledged_seq = Some(seq);
            report.outbound_sent += 1;
        }
        if let Some(seq) = acknowledged_seq {
            self.backend.acknowledge_outbox_cursor(seq)?;
        }

        Ok(report)
    }
}

/// Directory for resolving backend outbox targets to Matrix rooms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixRoomDirectory {
    rooms: Vec<MatrixRoomRegistration>,
}

impl MatrixRoomDirectory {
    /// Build a room directory from Matrix-side registrations.
    #[must_use]
    pub fn new(rooms: Vec<MatrixRoomRegistration>) -> Self {
        Self { rooms }
    }

    /// Resolve the Matrix room for an outbound event.
    pub fn resolve_room_id(&self, event: &MatrixOutboundEvent) -> Result<String, BridgeError> {
        if let Some(room_id) = event
            .room_id
            .as_deref()
            .map(str::trim)
            .filter(|room_id| !room_id.is_empty())
        {
            return Ok(room_id.to_owned());
        }

        let target = event
            .target
            .as_deref()
            .map(str::trim)
            .filter(|target| !target.is_empty())
            .ok_or_else(|| {
                BridgeError::transport(format!(
                    "outbound seq {} has no Matrix room id or target",
                    event.seq
                ))
            })?;

        self.rooms
            .iter()
            .find(|room| {
                room.trusted
                    && (room.agent_name.as_deref() == Some(target)
                        || room.group_name.as_deref() == Some(target))
            })
            .map(|room| room.room_id.clone())
            .ok_or_else(|| {
                BridgeError::transport(format!(
                    "outbound seq {} target {target} has no trusted Matrix room mapping",
                    event.seq
                ))
            })
    }
}

/// Deterministic file-backed Matrix-side transport.
#[derive(Debug, Clone)]
pub struct FileMatrixTransport {
    rooms: Vec<MatrixRoomRegistration>,
    inbound: Vec<MatrixInboundEvent>,
    sent_log_jsonl_path: PathBuf,
    directory: MatrixRoomDirectory,
}

impl FileMatrixTransport {
    /// Load a file-backed transport from JSON fixture files.
    pub fn from_files(
        rooms_json_path: impl AsRef<Path>,
        inbound_json_path: impl AsRef<Path>,
        sent_log_jsonl_path: impl Into<PathBuf>,
    ) -> Result<Self, BridgeError> {
        let rooms = read_json_file(rooms_json_path.as_ref(), "room registrations")?;
        let inbound = read_json_file(inbound_json_path.as_ref(), "inbound events")?;
        Ok(Self::new(rooms, inbound, sent_log_jsonl_path))
    }

    /// Build a file-backed transport from already-decoded fixtures.
    #[must_use]
    pub fn new(
        rooms: Vec<MatrixRoomRegistration>,
        inbound: Vec<MatrixInboundEvent>,
        sent_log_jsonl_path: impl Into<PathBuf>,
    ) -> Self {
        let directory = MatrixRoomDirectory::new(rooms.clone());
        Self {
            rooms,
            inbound,
            sent_log_jsonl_path: sent_log_jsonl_path.into(),
            directory,
        }
    }
}

impl MatrixBridgeTransport for FileMatrixTransport {
    fn room_registrations(&mut self) -> Result<Vec<MatrixRoomRegistration>, BridgeError> {
        Ok(self.rooms.clone())
    }

    fn inbound_events(&mut self) -> Result<Vec<MatrixInboundEvent>, BridgeError> {
        Ok(self.inbound.clone())
    }

    fn send_outbound(&mut self, event: MatrixOutboundEvent) -> Result<(), BridgeError> {
        let room_id = self.directory.resolve_room_id(&event)?;
        ensure_parent_dir(&self.sent_log_jsonl_path, "sent log")?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.sent_log_jsonl_path)
            .map_err(|err| {
                BridgeError::transport(format!(
                    "open sent log {}: {err}",
                    self.sent_log_jsonl_path.display()
                ))
            })?;
        let line = json!({
            "seq": event.seq,
            "roomId": room_id,
            "target": event.target,
            "messageId": event.message_id,
            "source": event.source,
            "body": event.body,
            "payload": event.payload,
        });
        serde_json::to_writer(&mut file, &line)
            .map_err(|err| BridgeError::transport(format!("encode sent JSONL: {err}")))?;
        file.write_all(b"\n")
            .map_err(|err| BridgeError::transport(format!("write sent JSONL: {err}")))
    }
}

/// SDK-facing Matrix transport that adapts a Matrix client port into the
/// deterministic bridge runtime contract.
#[derive(Debug, Clone)]
pub struct MatrixClientBridgeTransport<C> {
    client: C,
    config: MatrixClientTransportConfig,
    bot_user_id: Option<String>,
    cache: Option<MatrixClientSyncCache>,
    directory: Option<MatrixRoomDirectory>,
}

struct MatrixClientDmRoomEffects<'a, C> {
    client: &'a mut C,
    config: &'a MatrixClientTransportConfig,
    registrations: Vec<MatrixRoomRegistration>,
}

impl<C> MatrixClientDmRoomEffects<'_, C>
where
    C: MatrixClientPort,
{
    fn puppet_directory(&self) -> Result<Option<MatrixPuppetDirectory>, BridgeError> {
        let Some(server_name) = &self.config.matrix_server_name else {
            return Ok(None);
        };
        MatrixPuppetDirectory::new(
            server_name,
            &self.config.agent_user_prefix,
            self.config.known_agent_names.iter().map(String::as_str),
            self.config.skip_agent_names.iter().map(String::as_str),
        )
        .map(Some)
    }

    fn trusted_direct_room_for_agent(&self, agent_name: &str) -> Option<&str> {
        self.registrations
            .iter()
            .find(|room| {
                room.trusted
                    && room.group_name.is_none()
                    && room.agent_name.as_deref() == Some(agent_name)
            })
            .map(|room| room.room_id.as_str())
    }

    fn trusted_group_room_for_group(&self, group_name: &str) -> Option<&str> {
        self.registrations
            .iter()
            .find(|room| room.trusted && room.group_name.as_deref() == Some(group_name))
            .map(|room| room.room_id.as_str())
    }

    fn ensure_human_membership(
        &mut self,
        room_id: &str,
        human_mxid: &str,
    ) -> MatrixBotDmRoomResult {
        match self.client.room_member_status(room_id, human_mxid) {
            Ok(Some(status @ (MatrixBotDmRoomStatus::Joined | MatrixBotDmRoomStatus::Invited))) => {
                return MatrixBotDmRoomResult {
                    room_id: Some(room_id.to_owned()),
                    human_status: status,
                    invite_error: None,
                };
            }
            Ok(Some(MatrixBotDmRoomStatus::Unknown)) => {
                return MatrixBotDmRoomResult {
                    room_id: Some(room_id.to_owned()),
                    human_status: MatrixBotDmRoomStatus::Unknown,
                    invite_error: None,
                };
            }
            Ok(
                Some(MatrixBotDmRoomStatus::InviteFailed | MatrixBotDmRoomStatus::MissingRoom)
                | None,
            )
            | Err(_) => {}
        }

        match self.client.invite_user_to_room(room_id, human_mxid) {
            Ok(()) => MatrixBotDmRoomResult {
                room_id: Some(room_id.to_owned()),
                human_status: MatrixBotDmRoomStatus::Invited,
                invite_error: None,
            },
            Err(err) => MatrixBotDmRoomResult {
                room_id: Some(room_id.to_owned()),
                human_status: MatrixBotDmRoomStatus::InviteFailed,
                invite_error: Some(err.to_string()),
            },
        }
    }

    fn ensure_human_group_membership(
        &mut self,
        room_id: &str,
        human_mxid: &str,
    ) -> MatrixBotGroupRoomResult {
        let dm = self.ensure_human_membership(room_id, human_mxid);
        MatrixBotGroupRoomResult {
            room_id: dm.room_id,
            human_status: dm.human_status,
            invite_error: dm.invite_error,
        }
    }
}

impl<C> MatrixBotCommandRoomEffectPort for MatrixClientDmRoomEffects<'_, C>
where
    C: MatrixClientPort,
{
    fn ensure_human_dm_room(
        &mut self,
        agent_name: &str,
        human_mxid: &str,
    ) -> Result<MatrixBotDmRoomResult, BridgeError> {
        if matrix_user_parts(human_mxid).is_none() {
            return Ok(MatrixBotDmRoomResult::missing_room());
        }
        let Some(directory) = self.puppet_directory()? else {
            return Ok(MatrixBotDmRoomResult::missing_room());
        };
        let Some(account) = directory.account_for_agent(agent_name).cloned() else {
            return Ok(MatrixBotDmRoomResult::missing_room());
        };

        if let Some(room_id) = self
            .trusted_direct_room_for_agent(&account.agent_name)
            .map(ToOwned::to_owned)
        {
            return Ok(self.ensure_human_membership(&room_id, human_mxid));
        }

        let invite_mxids = vec![human_mxid.to_owned(), account.mxid];
        match self
            .client
            .create_direct_room(&format!("DM: {}", account.agent_name), &invite_mxids)
        {
            Ok(room_id) => Ok(MatrixBotDmRoomResult {
                room_id: Some(room_id),
                human_status: MatrixBotDmRoomStatus::Invited,
                invite_error: None,
            }),
            Err(_) => Ok(MatrixBotDmRoomResult::missing_room()),
        }
    }

    fn ensure_human_group_room(
        &mut self,
        group_name: &str,
        human_mxid: &str,
    ) -> Result<MatrixBotGroupRoomResult, BridgeError> {
        if matrix_user_parts(human_mxid).is_none() {
            return Ok(MatrixBotGroupRoomResult::missing_room());
        }
        let Some(room_id) = self
            .trusted_group_room_for_group(group_name)
            .map(ToOwned::to_owned)
        else {
            return Ok(MatrixBotGroupRoomResult::missing_room());
        };
        Ok(self.ensure_human_group_membership(&room_id, human_mxid))
    }
}

impl<C> MatrixClientBridgeTransport<C> {
    /// Build a Matrix client transport from a normalized client port.
    #[must_use]
    pub fn new(client: C, config: MatrixClientTransportConfig) -> Self {
        Self {
            client,
            bot_user_id: config.bot_user_id.clone(),
            config,
            cache: None,
            directory: None,
        }
    }

    /// Borrow the Matrix client, mostly for tests and embedding code.
    #[must_use]
    pub const fn client(&self) -> &C {
        &self.client
    }
}

impl<C> MatrixClientBridgeTransport<C>
where
    C: MatrixClientPort,
{
    fn ensure_synced(&mut self) -> Result<(), BridgeError> {
        if self.cache.is_some() {
            return Ok(());
        }

        let logged_in_user_id = self.client.ensure_logged_in()?;
        let bot_user_id = self.config.bot_user_id.clone().unwrap_or(logged_in_user_id);
        self.bot_user_id = Some(bot_user_id.clone());

        let sync = self.client.sync_once()?;
        let mut registrations = Vec::new();

        for invite in sync.invites {
            if let Some(registration) = self.handle_invite(invite)? {
                registrations.push(registration);
            }
        }

        registrations.extend(
            sync.joined_rooms
                .into_iter()
                .map(MatrixClientRoom::into_room),
        );

        let mut inbound = Vec::new();
        let mut bot_commands = Vec::new();
        for event in sync
            .text_events
            .into_iter()
            .filter(|event| !self.should_suppress_event(event, &bot_user_id))
        {
            let context = Self::bot_command_context(&registrations, &event.room_id);
            match plan_matrix_bot_command(
                &event.sender_mxid,
                &event.body,
                event.formatted_body.as_deref(),
                context,
                &self.config.bot_command_acl,
            ) {
                MatrixBotCommandPlan::Command(mut command) => {
                    command.event_id = Some(event.event_id);
                    command.room_id = Some(event.room_id);
                    command.sender_mxid = event.sender_mxid;
                    bot_commands.push(command);
                }
                MatrixBotCommandPlan::NonCommand(_) => inbound.push(event.into_inbound()),
            }
        }

        self.directory = Some(MatrixRoomDirectory::new(registrations.clone()));
        self.cache = Some(MatrixClientSyncCache {
            registrations,
            inbound,
            bot_commands,
        });
        Ok(())
    }

    /// Return Matrix bot command plans captured from the current sync snapshot.
    pub fn bot_command_plans(&mut self) -> Result<Vec<MatrixBotCommand>, BridgeError> {
        self.ensure_synced()?;
        Ok(self
            .cache
            .as_ref()
            .map(|cache| cache.bot_commands.clone())
            .unwrap_or_default())
    }

    /// Execute cached Matrix bot command plans and send their replies through
    /// the Matrix client.
    pub fn execute_bot_command_replies(
        &mut self,
        snapshot: &MatrixBotCommandSnapshot,
    ) -> Result<Vec<MatrixBotCommandReply>, BridgeError> {
        self.ensure_synced()?;
        let commands = self
            .cache
            .as_ref()
            .map(|cache| cache.bot_commands.clone())
            .unwrap_or_default();
        let mut replies = Vec::new();
        for command in commands {
            let execution = execute_matrix_bot_command(&command, snapshot)?;
            self.client
                .send_text_message(&execution.reply.room_id, &execution.reply.body)
                .map_err(|err| {
                    BridgeError::transport(format!(
                        "send Matrix bot command reply for {} to {}: {err}",
                        command.command, execution.reply.room_id
                    ))
                })?;
            replies.push(execution.reply);
        }
        Ok(replies)
    }

    /// Execute cached Matrix bot command plans with p258 management effects
    /// enabled, then send their replies through the Matrix client.
    pub fn execute_bot_command_replies_with_effects<B>(
        &mut self,
        snapshot: &MatrixBotCommandSnapshot,
        backend: &mut B,
    ) -> Result<Vec<MatrixBotCommandReply>, BridgeError>
    where
        B: MatrixBotCommandBackendEffectPort,
    {
        self.ensure_synced()?;
        let commands = self
            .cache
            .as_ref()
            .map(|cache| cache.bot_commands.clone())
            .unwrap_or_default();
        let registrations = self
            .cache
            .as_ref()
            .map(|cache| cache.registrations.clone())
            .unwrap_or_default();
        let mut replies = Vec::new();
        for command in commands {
            let mut rooms = MatrixClientDmRoomEffects {
                client: &mut self.client,
                config: &self.config,
                registrations: registrations.clone(),
            };
            let execution =
                execute_matrix_bot_command_with_effects(&command, snapshot, backend, &mut rooms)?;
            self.client
                .send_text_message(&execution.reply.room_id, &execution.reply.body)
                .map_err(|err| {
                    BridgeError::transport(format!(
                        "send Matrix bot command reply for {} to {}: {err}",
                        command.command, execution.reply.room_id
                    ))
                })?;
            replies.push(execution.reply);
        }
        Ok(replies)
    }

    fn handle_invite(
        &mut self,
        invite: MatrixClientInvite,
    ) -> Result<Option<MatrixRoomRegistration>, BridgeError> {
        let trusted = invite
            .inviter_mxid
            .as_deref()
            .is_some_and(|inviter| self.is_trusted_inviter(inviter));
        if trusted || self.config.trust_mode == MatrixTrustMode::Audit {
            self.client.join_room(&invite.room_id)?;
            let trust_reason = if trusted {
                "trusted_inviter"
            } else {
                "untrusted_inviter"
            };
            Ok(Some(invite.into_room(trusted, trust_reason)))
        } else {
            self.client.leave_room(&invite.room_id)?;
            Ok(None)
        }
    }

    fn is_trusted_inviter(&self, inviter: &str) -> bool {
        self.config
            .trusted_inviter_mxids
            .iter()
            .any(|trusted| trusted == inviter)
    }

    fn should_suppress_event(&self, event: &MatrixClientTextMessage, bot_user_id: &str) -> bool {
        event.sender_mxid == bot_user_id
            || self.is_agent_user(&event.sender_mxid)
            || self
                .config
                .ignored_sender_mxids
                .iter()
                .any(|ignored| ignored == &event.sender_mxid)
            || event.body.contains("[AGENTIGNORE]")
    }

    fn is_agent_user(&self, mxid: &str) -> bool {
        if let Some(server_name) = &self.config.matrix_server_name {
            return MatrixPuppetDirectory::new(
                server_name,
                &self.config.agent_user_prefix,
                self.config.known_agent_names.iter().map(String::as_str),
                self.config.skip_agent_names.iter().map(String::as_str),
            )
            .is_ok_and(|directory| directory.is_agent_puppet_mxid(mxid));
        }

        // prefix-only fallback keeps pre-p243 configs compatible when no
        // Matrix server name or known-agent list is available yet.
        let prefix = self.config.agent_user_prefix.trim().trim_start_matches('@');
        if prefix.is_empty() {
            return false;
        }
        matrix_localpart(mxid).is_some_and(|localpart| localpart.starts_with(prefix))
    }

    fn bot_command_context(
        registrations: &[MatrixRoomRegistration],
        room_id: &str,
    ) -> MatrixBotCommandContext {
        registrations
            .iter()
            .find(|registration| registration.room_id == room_id)
            .map(|registration| MatrixBotCommandContext {
                group_name: registration.group_name.clone(),
                target_agent: registration.agent_name.clone(),
            })
            .unwrap_or_default()
    }
}

impl<C> MatrixBridgeTransport for MatrixClientBridgeTransport<C>
where
    C: MatrixClientPort,
{
    fn room_registrations(&mut self) -> Result<Vec<MatrixRoomRegistration>, BridgeError> {
        self.ensure_synced()?;
        Ok(self
            .cache
            .as_ref()
            .map(|cache| cache.registrations.clone())
            .unwrap_or_default())
    }

    fn inbound_events(&mut self) -> Result<Vec<MatrixInboundEvent>, BridgeError> {
        self.ensure_synced()?;
        Ok(self
            .cache
            .as_ref()
            .map(|cache| cache.inbound.clone())
            .unwrap_or_default())
    }

    fn send_outbound(&mut self, event: MatrixOutboundEvent) -> Result<(), BridgeError> {
        self.ensure_synced()?;
        let directory = self
            .directory
            .as_ref()
            .ok_or_else(|| BridgeError::transport("Matrix client sync directory is unavailable"))?;
        let room_id = directory.resolve_room_id(&event)?;
        self.client
            .send_text_message(&room_id, &event.body)
            .map_err(|err| {
                BridgeError::transport(format!(
                    "send Matrix message seq {} to {room_id}: {err}",
                    event.seq
                ))
            })
    }
}

/// Execute one bridge iteration using the HTTP backend and an SDK-facing Matrix
/// client transport.
pub fn run_matrix_client_bridge_once<C>(
    config: &MatrixClientBridgeOnceConfig,
    client: C,
) -> Result<BridgeOnceReport, BridgeError>
where
    C: MatrixClientPort,
{
    let puppet_account_provisioning = config
        .puppet_accounts
        .as_ref()
        .map(run_bridge_once_puppet_account_provisioning)
        .transpose()?;
    let mut state = BridgeState::load_json(&config.state_path)?;
    let mut backend = AgentdHttpBackend::new(&config.bridge_config)?;
    backend.require_native_runtime()?;
    if let Some(cursor) = backend.outbox_cursor()? {
        state.next_from_seq = state.next_from_seq.max(cursor);
    }
    let mut transport = MatrixClientBridgeTransport::new(client, config.transport_config.clone());
    let command_count = transport.bot_command_plans()?.len();
    let bot_command_replies_sent = if command_count == 0 {
        0
    } else {
        let snapshot = backend.bot_command_snapshot()?;
        transport
            .execute_bot_command_replies_with_effects(&snapshot, &mut backend)?
            .len()
    };
    let mut runtime = BridgeRuntime::new(backend, transport, state);
    let mut report = runtime.run_once()?;
    report.bot_command_replies_sent += bot_command_replies_sent;
    let next_from_seq = runtime.state().next_from_seq();
    runtime.state().save_json(&config.state_path)?;
    Ok(BridgeOnceReport {
        run: report,
        next_from_seq,
        puppet_account_provisioning,
    })
}

/// Execute one bridge iteration using the HTTP backend and file Matrix
/// transport.
pub fn run_bridge_once(config: &BridgeOnceConfig) -> Result<BridgeOnceReport, BridgeError> {
    let puppet_account_provisioning = config
        .puppet_accounts
        .as_ref()
        .map(run_bridge_once_puppet_account_provisioning)
        .transpose()?;
    let backend = AgentdHttpBackend::new(&config.bridge_config)?;
    // File-backed transport is also an execution ingress. Keep it behind the
    // same native-runtime contract as the SDK transport so replay/testing
    // cannot silently become a legacy production path.
    backend.require_native_runtime()?;
    let state = BridgeState::load_json(&config.state_path)?;
    let transport = FileMatrixTransport::from_files(
        &config.rooms_json_path,
        &config.inbound_json_path,
        &config.sent_log_jsonl_path,
    )?;
    let mut runtime = BridgeRuntime::new(backend, transport, state);
    let report = runtime.run_once()?;
    let next_from_seq = runtime.state().next_from_seq();
    runtime.state().save_json(&config.state_path)?;
    Ok(BridgeOnceReport {
        run: report,
        next_from_seq,
        puppet_account_provisioning,
    })
}

fn run_bridge_once_puppet_account_provisioning(
    config: &BridgeOncePuppetAccountConfig,
) -> Result<MatrixPuppetAccountProvisioningReport, BridgeError> {
    let provisioner = MatrixPuppetHttpAccountProvisioner::new(&config.http_account_config)?;
    let mut token_store = MatrixPuppetTokenFileStore::new(&config.token_state_path);
    let token_state = token_store.load_token_state()?;
    Ok(provisioner.provision(
        &config.directory,
        &config.provisioning_config,
        &token_state,
        &mut token_store,
    ))
}

/// Synchronous HTTP implementation of the agentd Matrix bridge backend.
#[derive(Debug, Clone)]
pub struct AgentdHttpBackend {
    endpoint: HttpEndpoint,
    operator_token: Option<String>,
}

impl AgentdHttpBackend {
    /// Build an HTTP backend from bridge configuration.
    pub fn new(config: &BridgeConfig) -> Result<Self, BridgeError> {
        Ok(Self {
            endpoint: HttpEndpoint::parse(config.agentd_api())?,
            operator_token: config.operator_token().map(ToOwned::to_owned),
        })
    }

    /// Read the daemon-owned runtime contract used by Matrix/Robrix adapters.
    /// Callers can gate native session operations without inspecting legacy
    /// agent records or tmux targets.
    pub fn runtime_capabilities(&self) -> Result<Value, BridgeError> {
        self.request_json("GET", "/api/runtime/capabilities", None)
    }

    /// Gate the production Matrix bridge on the daemon-native runtime contract.
    /// This keeps compatibility APIs available while preventing a live bridge
    /// from silently routing commands to the legacy runtime.
    pub fn require_native_runtime(&self) -> Result<(), BridgeError> {
        let capabilities = self.runtime_capabilities()?;
        let native = capabilities.get("runtime").and_then(Value::as_str) == Some("native");
        let resumable = capabilities.get("sessionResume").and_then(Value::as_bool) == Some(true);
        let acknowledged = capabilities
            .get("artifactAcknowledgement")
            .and_then(Value::as_bool)
            == Some(true);
        if native && resumable && acknowledged {
            return Ok(());
        }
        Err(BridgeError::backend(
            "agentd runtime capabilities do not satisfy the native bridge contract",
        ))
    }

    /// Read durable per-project cutover state for Robrix/project views.
    pub fn cutover_project_state(&self, project_id: &str) -> Result<Option<Value>, BridgeError> {
        self.request_optional_json(
            "GET",
            &format!("/api/cutover/projects/{}", encode_path_segment(project_id)),
            None,
        )
    }

    /// Advance a daemon-owned project through the cutover state machine.
    pub fn transition_cutover_project(
        &self,
        project_id: &str,
        phase: &str,
        authority_revision: &str,
        matrix_cursor: i64,
        lease_epoch: i64,
    ) -> Result<Value, BridgeError> {
        self.request_json(
            "POST",
            &format!(
                "/api/cutover/projects/{}/transition",
                encode_path_segment(project_id)
            ),
            Some(json!({
                "phase": phase,
                "authorityRevision": authority_revision,
                "matrixCursor": matrix_cursor,
                "leaseEpoch": lease_epoch,
            })),
        )
    }

    /// Roll a project back using a strictly newer lease epoch.
    pub fn rollback_cutover_project(
        &self,
        project_id: &str,
        lease_epoch: i64,
    ) -> Result<Value, BridgeError> {
        self.request_json(
            "POST",
            &format!(
                "/api/cutover/projects/{}/rollback",
                encode_path_segment(project_id)
            ),
            Some(json!({ "leaseEpoch": lease_epoch })),
        )
    }

    /// Read native execution artifacts for a Robrix run detail view.
    pub fn runtime_run_artifacts(&self, run_id: &str) -> Result<Value, BridgeError> {
        self.request_json(
            "GET",
            &format!(
                "/api/runtime/runs/{}/artifacts",
                encode_path_segment(run_id)
            ),
            None,
        )
    }

    pub fn acknowledge_matrix_outbox_cursor(&self, last_seq: i64) -> Result<(), BridgeError> {
        self.request_json(
            "POST",
            "/api/matrix/outbox/ack",
            Some(json!({ "bridgeId": "matrix-bridge", "lastSeq": last_seq })),
        )?;
        Ok(())
    }

    pub fn matrix_outbox_cursor(&self) -> Result<i64, BridgeError> {
        let value = self.request_json(
            "GET",
            "/api/matrix/outbox/cursor?bridgeId=matrix-bridge",
            None,
        )?;
        value
            .get("lastSeq")
            .and_then(Value::as_i64)
            .ok_or_else(|| BridgeError::backend("matrix cursor response missing lastSeq"))
    }

    fn request_json(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, BridgeError> {
        let path = self.endpoint.path(path);
        let body = match body {
            Some(value) => serde_json::to_string(&value)
                .map_err(|err| BridgeError::backend(format!("encode JSON body: {err}")))?,
            None => String::new(),
        };
        let response = self.http_request(method, &path, &body)?;
        if !(200..300).contains(&response.status) {
            return Err(BridgeError::backend(format!(
                "{method} {path} returned status {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            )));
        }
        serde_json::from_slice(&response.body)
            .map_err(|err| BridgeError::backend(format!("decode JSON from {method} {path}: {err}")))
    }

    fn http_request(
        &self,
        method: &str,
        path: &str,
        body: &str,
    ) -> Result<HttpResponse, BridgeError> {
        let address = self.endpoint.address();
        let mut stream = TcpStream::connect(&address)
            .map_err(|err| BridgeError::backend(format!("connect {address}: {err}")))?;
        let timeout = Some(Duration::from_secs(5));
        stream
            .set_read_timeout(timeout)
            .map_err(|err| BridgeError::backend(format!("set read timeout: {err}")))?;
        stream
            .set_write_timeout(timeout)
            .map_err(|err| BridgeError::backend(format!("set write timeout: {err}")))?;

        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n",
            self.endpoint.host_header
        );
        if let Some(token) = &self.operator_token {
            let _ = write!(request, "Authorization: Bearer {token}\r\n");
        }
        if !body.is_empty() {
            request.push_str("Content-Type: application/json\r\n");
            let _ = write!(request, "Content-Length: {}\r\n", body.len());
        }
        request.push_str("\r\n");
        request.push_str(body);

        stream
            .write_all(request.as_bytes())
            .map_err(|err| BridgeError::backend(format!("write HTTP request: {err}")))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|err| BridgeError::backend(format!("read HTTP response: {err}")))?;
        HttpResponse::parse(&response)
    }

    /// Read the agentd state needed by p257 Matrix bot read-only replies.
    pub fn bot_command_snapshot(&self) -> Result<MatrixBotCommandSnapshot, BridgeError> {
        let agents = self.request_json("GET", "/api/agents", None)?;
        let groups = self.request_json("GET", "/api/groups", None)?;
        Ok(MatrixBotCommandSnapshot {
            agents: decode_bot_agent_summaries(&agents)?,
            groups: decode_bot_group_summaries(&groups)?,
            tmux_sessions: None,
            bridge_running: true,
        })
    }

    fn request_optional_json(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<Value>, BridgeError> {
        let path = self.endpoint.path(path);
        let body = match body {
            Some(value) => serde_json::to_string(&value)
                .map_err(|err| BridgeError::backend(format!("encode JSON body: {err}")))?,
            None => String::new(),
        };
        let response = self.http_request(method, &path, &body)?;
        if response.status == 404 {
            return Ok(None);
        }
        if !(200..300).contains(&response.status) {
            return Err(BridgeError::backend(format!(
                "{method} {path} returned status {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            )));
        }
        let value = serde_json::from_slice(&response.body).map_err(|err| {
            BridgeError::backend(format!("decode JSON from {method} {path}: {err}"))
        })?;
        Ok(Some(value))
    }
}

impl MatrixBotCommandBackendEffectPort for AgentdHttpBackend {
    fn lookup_bot_agent(
        &mut self,
        agent_name: &str,
    ) -> Result<Option<MatrixBotAgentSummary>, BridgeError> {
        let Some(agent) = self.request_optional_json(
            "GET",
            &format!("/api/agents/{}", encode_path_segment(agent_name)),
            None,
        )?
        else {
            return Ok(None);
        };
        decode_bot_agent_summary(&agent).map(Some)
    }

    fn update_bot_agent_identity(
        &mut self,
        agent_name: &str,
        identity: &str,
    ) -> Result<MatrixBotCommandMutationResult, BridgeError> {
        let value = self.request_json(
            "PATCH",
            &format!("/api/agents/{}", encode_path_segment(agent_name)),
            Some(json!({ "identity": identity })),
        )?;
        Ok(MatrixBotCommandMutationResult {
            error: first_string(&value, &["error"]),
        })
    }

    fn create_bot_group(
        &mut self,
        name: &str,
        members: &[String],
    ) -> Result<MatrixBotCommandMutationResult, BridgeError> {
        let value = self.request_json(
            "POST",
            "/api/groups",
            Some(json!({ "name": name, "members": members })),
        )?;
        Ok(MatrixBotCommandMutationResult {
            error: first_string(&value, &["error"]),
        })
    }

    fn lookup_bot_group(
        &mut self,
        group_name: &str,
    ) -> Result<Option<MatrixBotGroupSummary>, BridgeError> {
        let Some(group) = self.request_optional_json(
            "GET",
            &format!("/api/groups/{}", encode_path_segment(group_name)),
            None,
        )?
        else {
            return Ok(None);
        };
        decode_bot_group_summary(&group).map(Some)
    }

    fn update_bot_group_members(
        &mut self,
        group_name: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<MatrixBotCommandMutationResult, BridgeError> {
        let value = self.request_json(
            "POST",
            &format!("/api/groups/{}/members", encode_path_segment(group_name)),
            Some(json!({ "add": add, "remove": remove })),
        )?;
        Ok(MatrixBotCommandMutationResult {
            error: first_string(&value, &["error"]),
        })
    }

    fn delete_bot_group(
        &mut self,
        group_name: &str,
    ) -> Result<MatrixBotCommandMutationResult, BridgeError> {
        let value = self.request_json(
            "DELETE",
            &format!("/api/groups/{}", encode_path_segment(group_name)),
            None,
        )?;
        Ok(MatrixBotCommandMutationResult {
            error: first_string(&value, &["error"]),
        })
    }
}

impl AgentdBridgeBackend for AgentdHttpBackend {
    fn register_room(&mut self, room: MatrixRoomRegistration) -> Result<(), BridgeError> {
        self.request_json(
            "POST",
            "/api/matrix/rooms",
            Some(json!({
                "roomId": room.room_id,
                "group": room.group_name,
                "agent": room.agent_name,
                "trusted": room.trusted,
                "trustReason": room.trust_reason,
                "inviterMxid": room.inviter_mxid,
                "members": room.members,
            })),
        )?;
        Ok(())
    }

    fn post_inbound(&mut self, event: MatrixInboundEvent) -> Result<(), BridgeError> {
        self.request_json(
            "POST",
            "/api/matrix/inbound",
            Some(json!({
                "eventId": event.event_id,
                "roomId": event.room_id,
                "senderMxid": event.sender_mxid,
                "body": event.body,
                "mentions": event.mentions,
                "replyTo": event.reply_to,
            })),
        )?;
        Ok(())
    }

    fn poll_outbox(&mut self, from_seq: i64) -> Result<Vec<MatrixOutboundEvent>, BridgeError> {
        let value = self.request_json(
            "GET",
            &format!("/api/matrix/outbox?from_seq={from_seq}"),
            None,
        )?;
        let events = value
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| BridgeError::backend("matrix outbox response missing events array"))?;

        events.iter().map(decode_outbox_event).collect()
    }

    fn acknowledge_outbox_cursor(&mut self, last_seq: i64) -> Result<(), BridgeError> {
        self.acknowledge_matrix_outbox_cursor(last_seq)
    }

    fn outbox_cursor(&mut self) -> Result<Option<i64>, BridgeError> {
        self.matrix_outbox_cursor().map(Some)
    }
}

/// Error type for bridge runtime and adapter failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// Invalid bridge configuration.
    #[error("invalid bridge config: {0}")]
    InvalidConfig(String),
    /// Agentd-side backend failed.
    #[error("agentd backend error: {0}")]
    Backend(String),
    /// Matrix-side transport failed.
    #[error("matrix transport error: {0}")]
    Transport(String),
    /// Bridge state persistence failed.
    #[error("bridge state error: {0}")]
    State(String),
}

impl BridgeError {
    /// Build an invalid-config error.
    #[must_use]
    pub fn invalid_config(message: impl Into<String>) -> Self {
        Self::InvalidConfig(message.into())
    }

    /// Build an agentd backend error.
    #[must_use]
    pub fn backend(message: impl Into<String>) -> Self {
        Self::Backend(message.into())
    }

    /// Build a Matrix transport error.
    #[must_use]
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport(message.into())
    }

    /// Build a bridge state error.
    #[must_use]
    pub fn state(message: impl Into<String>) -> Self {
        Self::State(message.into())
    }
}

#[derive(Debug, Clone)]
struct HttpEndpoint {
    host: String,
    port: u16,
    host_header: String,
    base_path: String,
}

impl HttpEndpoint {
    fn parse(url: &str) -> Result<Self, BridgeError> {
        Self::parse_labeled(url, "agentd_api")
    }

    fn parse_labeled(url: &str, label: &str) -> Result<Self, BridgeError> {
        let rest = url.strip_prefix("http://").ok_or_else(|| {
            BridgeError::invalid_config(format!(
                "{label} must use http:// for the standard-library HTTP adapter"
            ))
        })?;
        let (host_port, path) = rest
            .split_once('/')
            .map_or((rest, ""), |(host, path)| (host, path));
        if host_port.is_empty() {
            return Err(BridgeError::invalid_config(format!(
                "{label} host is required"
            )));
        }
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() => {
                let port = port.parse::<u16>().map_err(|err| {
                    BridgeError::invalid_config(format!("{label} port is invalid: {err}"))
                })?;
                (host.to_owned(), port)
            }
            _ => (host_port.to_owned(), 80),
        };
        let path = path.trim_matches('/');
        let base_path = if path.is_empty() {
            String::new()
        } else {
            format!("/{path}")
        };
        Ok(Self {
            host,
            port,
            host_header: host_port.to_owned(),
            base_path,
        })
    }

    fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    fn path(&self, path: &str) -> String {
        format!("{}{}", self.base_path, path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatrixClientSyncCache {
    registrations: Vec<MatrixRoomRegistration>,
    inbound: Vec<MatrixInboundEvent>,
    bot_commands: Vec<MatrixBotCommand>,
}

impl MatrixClientInvite {
    fn into_room(self, trusted: bool, trust_reason: &str) -> MatrixRoomRegistration {
        MatrixRoomRegistration {
            room_id: self.room_id,
            group_name: self.group_name,
            agent_name: self.agent_name,
            trusted,
            trust_reason: trust_reason.to_owned(),
            inviter_mxid: self.inviter_mxid,
            members: self.members,
        }
    }
}

impl MatrixClientRoom {
    fn into_room(self) -> MatrixRoomRegistration {
        MatrixRoomRegistration {
            room_id: self.room_id,
            group_name: self.group_name,
            agent_name: self.agent_name,
            trusted: self.trusted,
            trust_reason: self.trust_reason,
            inviter_mxid: self.inviter_mxid,
            members: self.members,
        }
    }
}

impl MatrixClientTextMessage {
    fn into_inbound(self) -> MatrixInboundEvent {
        MatrixInboundEvent {
            event_id: self.event_id,
            room_id: self.room_id,
            sender_mxid: self.sender_mxid,
            body: self.body,
            mentions: self.mentions,
            reply_to: self.reply_to,
        }
    }
}

impl HttpResponse {
    fn parse(bytes: &[u8]) -> Result<Self, BridgeError> {
        let header_end = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| BridgeError::backend("HTTP response missing header terminator"))?;
        let headers = std::str::from_utf8(&bytes[..header_end])
            .map_err(|err| BridgeError::backend(format!("decode HTTP headers: {err}")))?;
        let status = headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .ok_or_else(|| BridgeError::backend("HTTP response missing status"))?
            .parse::<u16>()
            .map_err(|err| BridgeError::backend(format!("parse HTTP status: {err}")))?;
        Ok(Self {
            status,
            body: bytes[(header_end + 4)..].to_vec(),
        })
    }
}

fn decode_outbox_event(event: &Value) -> Result<MatrixOutboundEvent, BridgeError> {
    let seq = event
        .get("seq")
        .and_then(Value::as_i64)
        .ok_or_else(|| BridgeError::backend("matrix outbox event missing seq"))?;
    let payload = event.get("payload").cloned().unwrap_or_else(|| json!({}));
    let body = first_string(&payload, &["full", "summary", "body"])
        .ok_or_else(|| BridgeError::backend(format!("matrix outbox event {seq} missing body")))?;

    Ok(MatrixOutboundEvent {
        seq,
        room_id: first_string(
            &payload,
            &["roomId", "room_id", "sourceRoom", "source_room"],
        ),
        target: first_string(&payload, &["target", "agent"]),
        body,
        message_id: first_string(&payload, &["messageId", "message_id"]),
        source: first_string(&payload, &["source"]),
        payload,
    })
}

fn decode_bot_agent_summaries(value: &Value) -> Result<Vec<MatrixBotAgentSummary>, BridgeError> {
    let agents = value
        .as_array()
        .ok_or_else(|| BridgeError::backend("agent list response is not an array"))?;
    agents.iter().map(decode_bot_agent_summary).collect()
}

fn decode_bot_agent_summary(agent: &Value) -> Result<MatrixBotAgentSummary, BridgeError> {
    let name = first_string(agent, &["name"])
        .ok_or_else(|| BridgeError::backend("agent list entry missing name"))?;
    Ok(MatrixBotAgentSummary {
        name,
        status: first_string(agent, &["status"]).unwrap_or_else(|| "unknown".to_owned()),
        role: first_string(agent, &["role"]),
        capability: first_string(agent, &["capability"]),
        runtime: first_string(agent, &["runtime"]),
    })
}

fn decode_bot_group_summaries(value: &Value) -> Result<Vec<MatrixBotGroupSummary>, BridgeError> {
    let groups = value
        .as_array()
        .ok_or_else(|| BridgeError::backend("group list response is not an array"))?;
    groups.iter().map(decode_bot_group_summary).collect()
}

fn decode_bot_group_summary(value: &Value) -> Result<MatrixBotGroupSummary, BridgeError> {
    let name = first_string(value, &["name"])
        .ok_or_else(|| BridgeError::backend("group list entry missing name"))?;
    let members = value
        .get("members")
        .and_then(Value::as_array)
        .map(|members| {
            members
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(MatrixBotGroupSummary { name, members })
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn encode_path_segment(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            byte => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    out
}

fn read_json_file<T: DeserializeOwned>(path: &Path, label: &str) -> Result<T, BridgeError> {
    let contents = fs::read_to_string(path)
        .map_err(|err| BridgeError::transport(format!("read {label} {}: {err}", path.display())))?;
    serde_json::from_str(&contents).map_err(|err| {
        BridgeError::transport(format!("decode {label} JSON {}: {err}", path.display()))
    })
}

fn ensure_parent_dir(path: &Path, label: &str) -> Result<(), BridgeError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| {
            BridgeError::transport(format!("create {label} dir {}: {err}", parent.display()))
        })?;
    }
    Ok(())
}

fn default_matrix_bridge_state() -> Value {
    json!({
        "botToken": null,
        "agentTokens": {},
        "roomGroupMap": {},
        "groupRoomMap": {},
    })
}

fn ensure_agent_tokens_object(value: &mut Value) -> Result<&mut Map<String, Value>, BridgeError> {
    let object = value.as_object_mut().ok_or_else(|| {
        BridgeError::state("Matrix puppet token state root must be a JSON object")
    })?;
    let agent_tokens = object.entry("agentTokens").or_insert_with(|| json!({}));
    agent_tokens.as_object_mut().ok_or_else(|| {
        BridgeError::state("Matrix puppet token state agentTokens must be a JSON object")
    })
}

fn normalize_matrix_server_name(server_name: &str) -> Result<String, BridgeError> {
    let trimmed = server_name.trim();
    if trimmed.is_empty() {
        return Err(BridgeError::invalid_config(
            "Matrix server_name is required",
        ));
    }
    if trimmed.starts_with('@')
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.contains('/')
        || trimmed.chars().any(char::is_whitespace)
    {
        return Err(BridgeError::invalid_config(format!(
            "Matrix server_name {trimmed:?} is invalid"
        )));
    }
    Ok(trimmed.to_owned())
}

fn normalize_agent_user_prefix(agent_user_prefix: &str) -> Result<String, BridgeError> {
    let trimmed = agent_user_prefix.trim();
    if trimmed.is_empty() {
        return Err(BridgeError::invalid_config(
            "Matrix agent_user_prefix is required",
        ));
    }
    if trimmed.starts_with('@')
        || trimmed.contains(':')
        || trimmed.contains('/')
        || trimmed.chars().any(char::is_whitespace)
    {
        return Err(BridgeError::invalid_config(format!(
            "Matrix agent_user_prefix {trimmed:?} is invalid"
        )));
    }
    Ok(trimmed.to_owned())
}

fn normalize_matrix_agent_name(agent_name: &str) -> Result<String, BridgeError> {
    let trimmed = agent_name.trim();
    if trimmed.is_empty() {
        return Err(BridgeError::invalid_config(
            "Matrix puppet agent name is required",
        ));
    }
    if trimmed.starts_with('@')
        || trimmed.contains(':')
        || trimmed.contains('/')
        || trimmed.chars().any(char::is_whitespace)
    {
        return Err(BridgeError::invalid_config(format!(
            "Matrix puppet agent name {trimmed:?} is invalid"
        )));
    }
    Ok(trimmed.to_owned())
}

fn name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    let trimmed = value?.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.contains(&value) {
        values.push(value);
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn matrix_user_parts(mxid: &str) -> Option<(&str, &str)> {
    let (localpart, server_name) = mxid.strip_prefix('@')?.split_once(':')?;
    (!localpart.is_empty() && !server_name.is_empty()).then_some((localpart, server_name))
}

fn matrix_localpart(mxid: &str) -> Option<&str> {
    matrix_user_parts(mxid).map(|(localpart, _)| localpart)
}

fn normalize_agentd_api(agentd_api: &str) -> Result<String, BridgeError> {
    let trimmed = agentd_api.trim();
    if trimmed.is_empty() {
        return Err(BridgeError::invalid_config("agentd_api is required"));
    }

    let normalized = trimmed.trim_end_matches('/');
    if normalized.is_empty() {
        return Err(BridgeError::invalid_config("agentd_api is required"));
    }

    Ok(normalized.to_owned())
}
