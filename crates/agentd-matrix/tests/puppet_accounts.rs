use std::collections::BTreeMap;

use agentd_matrix::{
    BridgeError, MatrixPuppetAccountExecutor, MatrixPuppetAccountOutcome, MatrixPuppetAccountPort,
    MatrixPuppetAccountSession, MatrixPuppetAccountStep, MatrixPuppetDirectory,
    MatrixPuppetProvisioningConfig, MatrixPuppetTokenSink, MatrixPuppetTokenState,
    MatrixPuppetWhoami,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum AccountCall {
    Whoami(String),
    Login { localpart: String, password: String },
    Register { localpart: String, password: String },
}

#[derive(Default)]
struct FakeAccountPort {
    calls: Vec<AccountCall>,
    whoami: BTreeMap<String, Result<MatrixPuppetWhoami, BridgeError>>,
    logins: BTreeMap<(String, String), Result<MatrixPuppetAccountSession, BridgeError>>,
    registrations: BTreeMap<(String, String), Result<MatrixPuppetAccountSession, BridgeError>>,
}

impl MatrixPuppetAccountPort for FakeAccountPort {
    fn whoami(&mut self, access_token: &str) -> Result<MatrixPuppetWhoami, BridgeError> {
        self.calls
            .push(AccountCall::Whoami(access_token.to_owned()));
        self.whoami
            .get(access_token)
            .cloned()
            .unwrap_or_else(|| Err(BridgeError::transport("unexpected whoami token")))
    }

    fn login(
        &mut self,
        localpart: &str,
        password: &str,
    ) -> Result<MatrixPuppetAccountSession, BridgeError> {
        self.calls.push(AccountCall::Login {
            localpart: localpart.to_owned(),
            password: password.to_owned(),
        });
        self.logins
            .get(&(localpart.to_owned(), password.to_owned()))
            .cloned()
            .unwrap_or_else(|| Err(BridgeError::transport("unexpected login candidate")))
    }

    fn register(
        &mut self,
        localpart: &str,
        password: &str,
    ) -> Result<MatrixPuppetAccountSession, BridgeError> {
        self.calls.push(AccountCall::Register {
            localpart: localpart.to_owned(),
            password: password.to_owned(),
        });
        self.registrations
            .get(&(localpart.to_owned(), password.to_owned()))
            .cloned()
            .unwrap_or_else(|| Err(BridgeError::transport("unexpected register candidate")))
    }
}

#[derive(Default)]
struct FakeTokenSink {
    saved: Vec<(String, String)>,
    deleted: Vec<String>,
}

impl MatrixPuppetTokenSink for FakeTokenSink {
    fn save_agent_token(
        &mut self,
        agent_name: &str,
        access_token: &str,
    ) -> Result<(), BridgeError> {
        self.saved
            .push((agent_name.to_owned(), access_token.to_owned()));
        Ok(())
    }

    fn delete_agent_token(&mut self, token_name: &str) -> Result<(), BridgeError> {
        self.deleted.push(token_name.to_owned());
        Ok(())
    }
}

fn directory(agent_names: &[&str]) -> MatrixPuppetDirectory {
    MatrixPuppetDirectory::new("matrix.test", "ac_", agent_names, Vec::<&str>::new())
        .expect("puppet directory")
}

fn secret_config() -> MatrixPuppetProvisioningConfig {
    MatrixPuppetProvisioningConfig {
        password_secret: Some("top-secret".to_owned()),
        ..MatrixPuppetProvisioningConfig::default()
    }
}

fn session(user_id: &str, access_token: &str) -> MatrixPuppetAccountSession {
    MatrixPuppetAccountSession {
        user_id: user_id.to_owned(),
        access_token: access_token.to_owned(),
    }
}

#[test]
fn matrix_puppet_account_executor_reuses_valid_tokens_and_prunes_stale_entries() {
    let directory = MatrixPuppetDirectory::new(
        "matrix.test",
        "ac_",
        ["codex-worker", "openfab-bridge"],
        ["openfab-bridge"],
    )
    .expect("puppet directory");
    let token_state = MatrixPuppetTokenState::from_agent_tokens([
        ("Codex-Worker", "worker-token"),
        ("old-agent", "stale-token"),
    ]);
    let mut port = FakeAccountPort::default();
    port.whoami.insert(
        "worker-token".to_owned(),
        Ok(MatrixPuppetWhoami {
            user_id: "@ac_codex-worker:matrix.test".to_owned(),
        }),
    );
    let mut sink = FakeTokenSink::default();

    let report = MatrixPuppetAccountExecutor.provision(
        &directory,
        &MatrixPuppetProvisioningConfig::default(),
        &token_state,
        &mut port,
        &mut sink,
    );

    assert_eq!(
        port.calls,
        vec![AccountCall::Whoami("worker-token".to_owned())]
    );
    assert_eq!(
        report.outcomes(),
        &[MatrixPuppetAccountOutcome::ReusedToken {
            agent_name: "codex-worker".to_owned(),
            localpart: "ac_codex-worker".to_owned(),
            mxid: "@ac_codex-worker:matrix.test".to_owned(),
            token_name: "Codex-Worker".to_owned(),
            user_id: "@ac_codex-worker:matrix.test".to_owned(),
        }]
    );
    assert_eq!(report.pruned_token_names(), &["old-agent".to_owned()]);
    assert!(report.prune_failures().is_empty());
    assert!(sink.saved.is_empty());
    assert_eq!(sink.deleted, vec!["old-agent".to_owned()]);
}

#[test]
fn matrix_puppet_account_executor_logs_in_then_persists_agent_token() {
    let directory = directory(&["codex-worker"]);
    let token_state = MatrixPuppetTokenState::default();
    let config = secret_config();
    let password = config
        .password_candidates("codex-worker")
        .into_iter()
        .next()
        .expect("derived password");
    let mut port = FakeAccountPort::default();
    port.logins.insert(
        ("ac_codex-worker".to_owned(), password.clone()),
        Ok(session("@ac_codex-worker:matrix.test", "login-token")),
    );
    let mut sink = FakeTokenSink::default();

    let report = MatrixPuppetAccountExecutor.provision(
        &directory,
        &config,
        &token_state,
        &mut port,
        &mut sink,
    );

    assert_eq!(
        port.calls,
        vec![AccountCall::Login {
            localpart: "ac_codex-worker".to_owned(),
            password,
        }]
    );
    assert_eq!(
        report.outcomes(),
        &[MatrixPuppetAccountOutcome::LoggedIn {
            agent_name: "codex-worker".to_owned(),
            localpart: "ac_codex-worker".to_owned(),
            mxid: "@ac_codex-worker:matrix.test".to_owned(),
            user_id: "@ac_codex-worker:matrix.test".to_owned(),
        }]
    );
    assert_eq!(
        sink.saved,
        vec![("codex-worker".to_owned(), "login-token".to_owned())]
    );
    assert!(sink.deleted.is_empty());
}

#[test]
fn matrix_puppet_account_executor_reports_missing_password_without_registering() {
    let directory = directory(&["codex-worker"]);
    let token_state = MatrixPuppetTokenState::default();
    let mut port = FakeAccountPort::default();
    let mut sink = FakeTokenSink::default();

    let report = MatrixPuppetAccountExecutor.provision(
        &directory,
        &MatrixPuppetProvisioningConfig::default(),
        &token_state,
        &mut port,
        &mut sink,
    );

    assert!(port.calls.is_empty());
    assert_eq!(
        report.outcomes(),
        &[MatrixPuppetAccountOutcome::MissingPassword {
            agent_name: "codex-worker".to_owned(),
            localpart: "ac_codex-worker".to_owned(),
            mxid: "@ac_codex-worker:matrix.test".to_owned(),
        }]
    );
    assert!(sink.saved.is_empty());
    assert!(sink.deleted.is_empty());
}

#[test]
fn matrix_puppet_account_executor_registers_after_login_candidates_fail() {
    let directory = directory(&["codex-worker"]);
    let token_state = MatrixPuppetTokenState::default();
    let config = MatrixPuppetProvisioningConfig {
        password_secret: Some("top-secret".to_owned()),
        legacy_password_template: Some("legacy-{name}".to_owned()),
        allow_legacy_password: true,
        registration_token: None,
    };
    let passwords = config.password_candidates("codex-worker");
    assert_eq!(passwords.len(), 2);
    let mut port = FakeAccountPort::default();
    port.registrations.insert(
        ("ac_codex-worker".to_owned(), passwords[0].clone()),
        Ok(session("@ac_codex-worker:matrix.test", "register-token")),
    );
    let mut sink = FakeTokenSink::default();

    let report = MatrixPuppetAccountExecutor.provision(
        &directory,
        &config,
        &token_state,
        &mut port,
        &mut sink,
    );

    assert_eq!(
        port.calls,
        vec![
            AccountCall::Login {
                localpart: "ac_codex-worker".to_owned(),
                password: passwords[0].clone(),
            },
            AccountCall::Login {
                localpart: "ac_codex-worker".to_owned(),
                password: passwords[1].clone(),
            },
            AccountCall::Register {
                localpart: "ac_codex-worker".to_owned(),
                password: passwords[0].clone(),
            },
        ]
    );
    assert_eq!(
        report.outcomes(),
        &[MatrixPuppetAccountOutcome::Registered {
            agent_name: "codex-worker".to_owned(),
            localpart: "ac_codex-worker".to_owned(),
            mxid: "@ac_codex-worker:matrix.test".to_owned(),
            user_id: "@ac_codex-worker:matrix.test".to_owned(),
        }]
    );
    assert_eq!(
        sink.saved,
        vec![("codex-worker".to_owned(), "register-token".to_owned())]
    );
}

#[test]
fn matrix_puppet_account_executor_reports_failures_without_stopping_other_agents() {
    let directory = directory(&["codex-fail", "codex-ok"]);
    let token_state = MatrixPuppetTokenState::default();
    let config = secret_config();
    let ok_password = config
        .password_candidates("codex-ok")
        .into_iter()
        .next()
        .expect("derived password");
    let mut port = FakeAccountPort::default();
    port.logins.insert(
        ("ac_codex-ok".to_owned(), ok_password),
        Ok(session("@ac_codex-ok:matrix.test", "ok-token")),
    );
    let mut sink = FakeTokenSink::default();

    let report = MatrixPuppetAccountExecutor.provision(
        &directory,
        &config,
        &token_state,
        &mut port,
        &mut sink,
    );

    assert_eq!(report.outcomes().len(), 2);
    match &report.outcomes()[0] {
        MatrixPuppetAccountOutcome::Failed {
            agent_name,
            localpart,
            mxid,
            step,
            error,
        } => {
            assert_eq!(agent_name, "codex-fail");
            assert_eq!(localpart, "ac_codex-fail");
            assert_eq!(mxid, "@ac_codex-fail:matrix.test");
            assert_eq!(*step, MatrixPuppetAccountStep::Register);
            assert!(error.contains("unexpected register candidate"));
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
    assert_eq!(
        report.outcomes()[1],
        MatrixPuppetAccountOutcome::LoggedIn {
            agent_name: "codex-ok".to_owned(),
            localpart: "ac_codex-ok".to_owned(),
            mxid: "@ac_codex-ok:matrix.test".to_owned(),
            user_id: "@ac_codex-ok:matrix.test".to_owned(),
        }
    );
    assert_eq!(
        sink.saved,
        vec![("codex-ok".to_owned(), "ok-token".to_owned())]
    );
}
