fn repo_root() -> std::path::PathBuf {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo_root().join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

#[test]
fn p2_plan_records_worktree_activation_as_built() {
    let plan = read("docs/plans/2026-06-06-agentd-p2-plan.md");
    let daemon = read("crates/agentd-bin/src/daemon.rs");
    let execute = read("workflows/execute.dot");

    assert!(
        daemon.contains(".with_worktree_allocator(Some(Box::new(worktree_pool)))"),
        "daemon source should prove the WorktreePool is injected"
    );
    assert!(
        execute.contains("--code ${worktree}"),
        "execute.dot should prove verify_lifecycle uses the staged worktree"
    );
    assert!(
        execute.contains("agentd_publish_worktree.sh ${worktree} ${task_run_id}"),
        "execute.dot should prove PR publication reads the staged worktree"
    );

    assert!(
        plan.contains("Status: as-built plus remaining real-env gates"),
        "P2 plan should no longer present itself as uncommitted planning"
    );
    assert!(
        plan.contains("Worktree activation is delivered"),
        "P2 plan should state that the worktree activation path is delivered"
    );
    assert!(
        plan.contains("daemon injects `WorktreePool`"),
        "P2 plan should name daemon WorktreePool injection"
    );
    assert!(
        plan.contains("`execute.dot` consumes `${worktree}`"),
        "P2 plan should name execute.dot's staged worktree use"
    );
}

#[test]
fn p2_plan_keeps_real_execute_smoke_as_remaining_gate() {
    let plan = read("docs/plans/2026-06-06-agentd-p2-plan.md");

    assert!(
        plan.contains("remaining real execute smoke gate"),
        "P2 plan should make the remaining real execute smoke gate explicit"
    );
    assert!(
        plan.contains("AGENTD_REAL_EXECUTE_SMOKE=1"),
        "P2 plan should name the opt-in gate instead of implying it has run"
    );
    assert!(
        !plan.contains("real execute smoke finished"),
        "P2 plan must not claim the operator-gated real execute smoke already ran"
    );
}

#[test]
fn p11_spec_no_longer_claims_r3a_is_unimplemented() {
    let spec = read("specs/core/p11-per-task-run-worktree.spec.md");
    let daemon = read("crates/agentd-bin/src/daemon.rs");
    let execute = read("workflows/execute.dot");

    assert!(
        daemon.contains(".with_worktree_allocator(Some(Box::new(worktree_pool)))"),
        "source sanity: daemon now injects the allocator"
    );
    assert!(
        execute.contains("--code ${worktree}"),
        "source sanity: execute.dot now consumes the staged worktree"
    );

    for stale in [
        "STATUS: DRAFT",
        "design pending an advisor review",
        "INERT BY DESIGN",
        "daemon keeps passing `None`",
        "execute.dot stays UNMIGRATED",
        "Do NOT inject the allocator in the daemon",
        "migrate any workflow",
    ] {
        assert!(
            !spec.contains(stale),
            "p11 spec still contains stale pre-activation wording: {stale}"
        );
    }
    assert!(
        spec.contains("R3a is complete"),
        "p11 spec should state the mechanism contract is complete"
    );
    assert!(
        spec.contains("Activation is covered by P99-P104"),
        "p11 spec should point current activation readers to the follow-on specs"
    );
}
