use agentd_core::types::NativeExecutionSpec;

fn valid_spec() -> NativeExecutionSpec {
    NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: "codex".into(),
        args: vec!["exec".into()],
        cwd: None,
        env: Vec::new(),
    }
}

#[test]
fn native_execution_spec_accepts_versioned_codex_input() {
    let spec = valid_spec();
    assert!(spec.validate().is_ok());
    assert!(spec.provider_matches_program());
}

#[test]
fn native_execution_spec_rejects_invalid_version_and_nul() {
    let mut spec = valid_spec();
    spec.version = 0;
    assert!(spec.validate().is_err());
    let mut spec = valid_spec();
    spec.version = 2;
    assert!(spec.validate().is_err());
    let mut spec = valid_spec();
    spec.args = vec!["bad\0arg".into()];
    assert!(spec.validate().is_err());
    let mut spec = valid_spec();
    spec.program = "other".into();
    assert!(!spec.provider_matches_program());
    assert!(spec.validate().is_err());
    let mut spec = valid_spec();
    spec.env = vec![("BAD KEY".into(), "value".into())];
    assert!(spec.validate().is_err());
    let mut spec = valid_spec();
    spec.cwd = Some("/tmp/bad\0cwd".into());
    assert!(spec.validate().is_err());
    let mut spec = valid_spec();
    spec.cwd = Some(String::new());
    assert!(spec.validate().is_err());
}
