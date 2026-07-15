use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn script_path() -> PathBuf {
    repo_root()
        .join("scripts")
        .join("agentd_real_security_sandbox_smoke.sh")
}

#[test]
fn real_oci_sandbox_denies_host_cross_tenant_cache_and_egress() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let dry_run = Command::new("bash")
        .arg(script_path())
        .args(["--dry-run", "--state-dir"])
        .arg(&state_dir)
        .env_remove("AGENTD_REAL_SECURITY_SANDBOX_SMOKE")
        .output()
        .expect("run sandbox dry-run");
    assert!(
        dry_run.status.success(),
        "dry-run stderr: {}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let plan = String::from_utf8_lossy(&dry_run.stdout);
    assert!(plan.contains("no host credentials"), "{plan}");
    assert!(plan.contains("network none"), "{plan}");
    assert!(!state_dir.exists(), "dry-run creates no state");

    let guarded = Command::new("bash")
        .arg(script_path())
        .args([
            "--execute",
            "--runtime",
            "docker",
            "--image",
            "test.invalid/security@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--state-dir",
        ])
        .arg(&state_dir)
        .env_remove("AGENTD_REAL_SECURITY_SANDBOX_SMOKE")
        .output()
        .expect("run guarded sandbox execute");
    assert!(!guarded.status.success(), "execute without opt-in fails");
    assert!(!state_dir.exists(), "guarded execute starts no container");

    if std::env::var("AGENTD_REAL_SECURITY_SANDBOX_SMOKE").as_deref() == Ok("1") {
        let image = std::env::var("AGENTD_SECURITY_SANDBOX_IMAGE")
            .expect("opt-in real smoke requires AGENTD_SECURITY_SANDBOX_IMAGE");
        let runtime =
            std::env::var("AGENTD_SECURITY_SANDBOX_RUNTIME").unwrap_or_else(|_| "auto".to_string());
        let executed = Command::new("bash")
            .arg(script_path())
            .args([
                "--execute",
                "--runtime",
                &runtime,
                "--image",
                &image,
                "--state-dir",
            ])
            .arg(&state_dir)
            .env("AGENTD_REAL_SECURITY_SANDBOX_SMOKE", "1")
            .output()
            .expect("run real sandbox smoke");
        assert!(
            executed.status.success(),
            "real smoke stderr: {}",
            String::from_utf8_lossy(&executed.stderr)
        );
        assert!(state_dir.join("summary.txt").is_file());
        assert!(state_dir.join("retained-output.txt").is_file());
        assert!(!state_dir.join("runtime").exists());
    }
}
