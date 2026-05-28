//! Scaffold-level tests that exercise the workspace itself.
//! These don't test agentd-core directly; they live here so `cargo test -p agentd-core`
//! is the canonical way to run them and they live in `tests/` (compiled per-crate).

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/agentd-core. Two parents up is the repo root.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // repo root
    p
}

#[test]
fn scaffold_workspace_builds() {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--workspace", "--quiet"])
        .current_dir(repo_root())
        .status()
        .expect("failed to spawn cargo");
    assert!(status.success(), "cargo build --workspace failed");
}

#[test]
fn scaffold_workspace_lints_inherited() {
    let crates_dir = repo_root().join("crates");
    let entries = std::fs::read_dir(&crates_dir).expect("crates dir missing");
    let mut checked = 0_u32;
    for entry in entries {
        let entry = entry.expect("readdir");
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let body = std::fs::read_to_string(&manifest).expect("read manifest");
        let normalized = body.replace([' ', '\t'], "");
        assert!(
            normalized.contains("[lints]\nworkspace=true"),
            "{} does not inherit workspace lints\n--- body ---\n{}",
            manifest.display(),
            body,
        );
        checked += 1;
    }
    assert!(checked >= 9, "expected >= 9 crates, found {checked}");
}

#[test]
fn scaffold_workspace_deps_pinned() {
    let manifest = repo_root().join("Cargo.toml");
    let body = std::fs::read_to_string(&manifest).expect("read root Cargo.toml");
    for name in ["tokio", "sqlx", "axum", "matrix-sdk", "octocrab", "rmcp"] {
        assert!(
            body.contains(&format!("{name} = ")) || body.contains(&format!("{name} = {{")),
            "workspace.dependencies missing {name}",
        );
    }
    assert!(
        body.contains("tokio = { version = \"1.49\""),
        "tokio is not pinned to 1.49",
    );
}

#[test]
fn scaffold_local_check_script_runs() {
    // GATED: this is an opt-in local smoke test, run only when
    // AGENTD_RUN_SCRIPT_SMOKE=1. It is NOT run in CI (no job sets the var) and
    // is skipped by default. scripts/check.sh itself runs
    // `cargo nextest run --workspace`, which re-enters this very test — so to
    // avoid infinite recursion we `env_remove` the trigger variable when
    // spawning check.sh. The nested test invocation then sees the var unset
    // and early-returns, breaking the recursion at depth 1.
    if std::env::var("AGENTD_RUN_SCRIPT_SMOKE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set AGENTD_RUN_SCRIPT_SMOKE=1 to enable");
        return;
    }
    let script = repo_root().join("scripts").join("check.sh");
    assert!(script.exists(), "scripts/check.sh not found");
    let status = Command::new("bash")
        .arg(&script)
        .current_dir(repo_root())
        .env_remove("AGENTD_RUN_SCRIPT_SMOKE") // break recursion in the nested run
        .status()
        .expect("failed to run check.sh");
    assert!(status.success(), "scripts/check.sh exited non-zero");
}

// The boundary tests below verify the GATE PATTERN, not the gate's real
// invocation against this repo's crates/. They build a tempdir mirroring the
// production glob shape (crates/<n>/src/), so the production glob
// `crates/*/src/**` is what's exercised. They never touch the real tree.

fn rg_finds(pattern: &str, root: &Path) -> bool {
    // Mirror the production gate EXACTLY: it runs from the repo root and searches
    // `.` with `--glob 'crates/*/src/**'`. ripgrep globs are matched against paths
    // relative to the search root, so we must cd into `root` and search `.` — NOT
    // pass an absolute path (which would prepend a prefix the glob can't match).
    let out = Command::new("rg")
        .args(["--quiet", "--glob", "crates/*/src/**", pattern, "."])
        .current_dir(root)
        .output()
        .expect("rg must be installed (CONTRIBUTING.md lists it as a dev dep)");
    out.status.success()
}

#[test]
fn scaffold_palace_db_reference_fails_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let leak = dir
        .path()
        .join("crates")
        .join("fake")
        .join("src")
        .join("lib.rs");
    std::fs::create_dir_all(leak.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &leak,
        "// uses palace.db which must not appear in production",
    )
    .expect("write leak file");
    assert!(
        rg_finds(r"palace\.db", dir.path()),
        "gate pattern should detect 'palace.db' under crates/*/src/** but did not",
    );
}

#[test]
fn scaffold_send_keys_dash_l_fails_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let leak = dir
        .path()
        .join("crates")
        .join("fake")
        .join("src")
        .join("lib.rs");
    std::fs::create_dir_all(leak.parent().expect("parent")).expect("mkdir");
    // Build the forbidden literal via concat so this test source stays clean.
    let bad = format!("fn bad() {{ let _ = \"send-keys{} \"; }}", " -l");
    std::fs::write(&leak, bad).expect("write leak file");
    assert!(
        rg_finds("send-keys -l", dir.path()),
        "gate pattern should detect the forbidden literal under crates/*/src/** but did not",
    );
}

#[test]
fn scaffold_gate_does_not_flag_tests_directory() {
    // Positive control: the same string under tests/ must NOT match, because
    // the gate scopes to crates/*/src/**.
    let dir = tempfile::tempdir().expect("tempdir");
    let leak = dir
        .path()
        .join("crates")
        .join("fake")
        .join("tests")
        .join("fixtures.rs");
    std::fs::create_dir_all(leak.parent().expect("parent")).expect("mkdir");
    std::fs::write(&leak, "// fixture referencing palace.db").expect("write");
    assert!(
        !rg_finds(r"palace\.db", dir.path()),
        "gate must scope to crates/*/src/** and ignore tests/",
    );
}
