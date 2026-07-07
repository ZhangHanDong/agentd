use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn read_repo_file(path: &str) -> String {
    std::fs::read_to_string(repo_root().join(path)).unwrap_or_else(|err| {
        panic!("read {path}: {err}");
    })
}

fn task_assignment_gap_line(checklist: &str) -> &str {
    checklist
        .lines()
        .find(|line| line.contains("**TaskAssignment**"))
        .expect("TaskAssignment known-gap line")
}

fn initial_context_gap_line(checklist: &str) -> &str {
    checklist
        .lines()
        .find(|line| line.contains("**Initial run context**"))
        .expect("Initial run context known-gap line")
}

fn checkpoint_atomicity_gap_line(checklist: &str) -> &str {
    checklist
        .lines()
        .find(|line| line.contains("**Checkpoint/outcome atomicity**"))
        .expect("Checkpoint/outcome atomicity known-gap line")
}

fn sse_sanitization_gap_line(checklist: &str) -> &str {
    checklist
        .lines()
        .find(|line| line.contains("**SSE field sanitization**"))
        .expect("SSE field sanitization known-gap line")
}

fn workflow_change_resume_line(checklist: &str) -> &str {
    checklist
        .lines()
        .find(|line| line.contains("--accept-workflow-change"))
        .expect("workflow-change resume checklist line")
}

#[test]
fn deployment_checklist_marks_p121_agent_id_gap_resolved() {
    let checklist = read_repo_file("docs/p0.9-deployment-checklist.md");
    let p121 = read_repo_file("specs/e2e/p121-production-assign-task-agent-ownership.spec.md");
    let line = task_assignment_gap_line(&checklist);

    assert!(
        p121.contains("ProductionRunHost::open_task")
            && p121.contains("TaskAssignment.agent_id")
            && p121.contains("legacy rows with a null agent id"),
        "P121 spec should document the resolved production ownership behavior"
    );
    assert!(
        line.contains("P121") && line.contains("agent_id"),
        "TaskAssignment gap line should explicitly mention P121 resolved agent_id: {line}"
    );
    assert!(
        !line.contains("agent_id`/`spec_path`/`plan_path` are populated from the spawn context"),
        "line still describes agent_id as an unresolved spawn-context gap: {line}"
    );
}

#[test]
fn deployment_checklist_marks_p136_task_assignment_metadata_resolved() {
    let checklist = read_repo_file("docs/p0.9-deployment-checklist.md");
    let p136 = read_repo_file("specs/e2e/p136-task-assignment-runtime-metadata.spec.md");
    let line = task_assignment_gap_line(&checklist);

    assert!(
        p136.contains("ProductionRunHost::open_task")
            && p136.contains("spec_path")
            && p136.contains("plan_path"),
        "P136 spec should document the runtime metadata bridge"
    );
    assert!(
        line.contains("P136") && line.contains("runtime metadata"),
        "TaskAssignment gap line should name P136 runtime metadata resolution: {line}"
    );
    assert!(
        !line.contains("remaining gaps are `spec_path`/`plan_path`"),
        "TaskAssignment gap line still lists spec_path/plan_path as remaining: {line}"
    );
}

#[test]
fn deployment_checklist_marks_p137_initial_context_resolved() {
    let checklist = read_repo_file("docs/p0.9-deployment-checklist.md");
    let p137 = read_repo_file("specs/e2e/p137-initial-run-context-seeding.spec.md");
    let line = initial_context_gap_line(&checklist);

    assert!(
        p137.contains("ProductionRunHost::start_workflow") && p137.contains("RunContext"),
        "P137 spec should document the production initial-context bridge"
    );
    assert!(
        line.contains("P137") && line.contains("seed"),
        "Initial run context gap line should name P137 seeding resolution: {line}"
    );
    assert!(
        !line.contains("accepts but does not seed `context`"),
        "Initial run context line still says production discards context: {line}"
    );
}

#[test]
fn deployment_checklist_marks_p138_checkpoint_atomicity_resolved() {
    let checklist = read_repo_file("docs/p0.9-deployment-checklist.md");
    let p138 = read_repo_file("specs/e2e/p138-outcome-checkpoint-atomicity.spec.md");
    let line = checkpoint_atomicity_gap_line(&checklist);

    assert!(
        p138.contains("Store") && p138.contains("atomically"),
        "P138 spec should document the outcome/checkpoint atomic commit"
    );
    assert!(
        line.contains("P138") && line.contains("atomic"),
        "Checkpoint/outcome line should name P138 atomic resolution: {line}"
    );
    assert!(
        !line.contains("crash between the outcome insert and the checkpoint write")
            && !line.contains("duplicate-able node"),
        "Checkpoint/outcome line still describes the duplicate-able node gap: {line}"
    );
}

#[test]
fn deployment_checklist_marks_p139_sse_sanitization_resolved() {
    let checklist = read_repo_file("docs/p0.9-deployment-checklist.md");
    let p139 = read_repo_file("specs/e2e/p139-sse-field-sanitization.spec.md");
    let line = sse_sanitization_gap_line(&checklist);

    assert!(
        p139.contains("SSE frame builder") && p139.contains("CR/LF"),
        "P139 spec should document the SSE boundary sanitizer"
    );
    assert!(
        line.contains("P139") && line.contains("boundary"),
        "SSE sanitization line should name P139 boundary sanitizer: {line}"
    );
    assert!(
        !line.contains("sanitize at the SSE boundary"),
        "SSE line still describes boundary sanitization as future work: {line}"
    );
}

#[test]
fn deployment_checklist_marks_p140_workflow_change_gate_wired() {
    let checklist = read_repo_file("docs/p0.9-deployment-checklist.md");
    let p140 = read_repo_file("specs/e2e/p140-resume-workflow-change-gate.spec.md");
    let line = workflow_change_resume_line(&checklist);

    assert!(
        p140.contains("resume_guard") && p140.contains("--accept-workflow-change"),
        "P140 spec should document the production workflow-change resume gate"
    );
    assert!(
        line.contains("P140") && line.contains("operator"),
        "workflow-change checklist line should name P140 as the wired operator gate: {line}"
    );
    assert!(
        !line.contains("Wire `--accept-workflow-change` into the resume path"),
        "workflow-change line still describes the flag as future wiring: {line}"
    );
}
